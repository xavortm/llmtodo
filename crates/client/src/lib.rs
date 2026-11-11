//! Typed HTTP client for the llmtodo API, used by the CLI and the app.

pub mod data_dir;
pub mod git;
pub mod identity;
pub mod live;

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use llmtodo_api_types::routes::{self, Method};
use llmtodo_api_types::*;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use llmtodo_api_types as types;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server answered with an error body.
    #[error("{}", .detail.message)]
    Api { status: u16, detail: ErrorDetail },
    #[error("cannot reach the llmtodo server: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Other(String),
}

impl ClientError {
    pub fn code(&self) -> ErrorCode {
        match self {
            ClientError::Api { detail, .. } => detail.code,
            ClientError::Unreachable(_) => ErrorCode::Unreachable,
            ClientError::Other(_) => ErrorCode::Internal,
        }
    }

    /// The CLI exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            ClientError::Api { detail, .. } => detail.code.exit_code(),
            ClientError::Unreachable(_) => 7,
            ClientError::Other(_) => 1,
        }
    }

    pub fn to_body(&self) -> ErrorBody {
        match self {
            ClientError::Api { detail, .. } => ErrorBody {
                error: detail.clone(),
            },
            ClientError::Unreachable(m) => ErrorBody {
                error: ErrorDetail {
                    code: ErrorCode::Unreachable,
                    message: format!("cannot reach the llmtodo server: {m}"),
                    details: serde_json::json!({ "unreachable": true }),
                },
            },
            ClientError::Other(m) => ErrorBody {
                error: ErrorDetail {
                    code: ErrorCode::Internal,
                    message: m.clone(),
                    details: serde_json::Value::Null,
                },
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// True for requests that may write. The route table owns this fact:
/// query and frontier are reads sent as POST.
pub fn is_write(method: &reqwest::Method, path: &str) -> bool {
    match Method::parse(method.as_str()) {
        Some(m) => routes::is_write(m, path),
        None => true,
    }
}

/// True for requests that carry usage. Usage goes only with writes, since
/// the server counts it against the ticket a write touches.
pub fn sends_usage(method: &reqwest::Method, path: &str) -> bool {
    is_write(method, path)
}

/// The values for the `{name}` segments of an endpoint's path, in order:
/// `()`, `&str`, or a tuple of two or three `&str`.
pub trait PathArgs {
    const COUNT: usize;
    fn values(&self) -> Vec<&str>;
}

impl PathArgs for () {
    const COUNT: usize = 0;
    fn values(&self) -> Vec<&str> {
        Vec::new()
    }
}

impl PathArgs for &str {
    const COUNT: usize = 1;
    fn values(&self) -> Vec<&str> {
        vec![self]
    }
}

impl PathArgs for (&str, &str) {
    const COUNT: usize = 2;
    fn values(&self) -> Vec<&str> {
        vec![self.0, self.1]
    }
}

impl PathArgs for (&str, &str, &str) {
    const COUNT: usize = 3;
    fn values(&self) -> Vec<&str> {
        vec![self.0, self.1, self.2]
    }
}

/// Fills the `{name}` segments of `template` with encoded `values`.
fn fill_path(template: &str, values: &[&str]) -> String {
    let mut values = values.iter();
    template
        .split('/')
        .map(|part| {
            if part.starts_with('{') {
                seg(values.next().expect("one value per path segment"))
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Turns a request into a query string, with a leading `?` when it has any
/// field. Null fields are left out.
fn query_string(req: &serde_json::Value) -> Result<String> {
    let serde_json::Value::Object(fields) = req else {
        return Ok(String::new());
    };
    let mut parts = Vec::new();
    for (k, v) in fields {
        let v = match v {
            serde_json::Value::Null => continue,
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(_) | serde_json::Value::Number(_) => v.to_string(),
            other => {
                return Err(ClientError::Other(format!(
                    "query field {k} cannot be sent: {other}"
                )));
            }
        };
        parts.push(format!("{}={}", seg(k), seg(&v)));
    }
    Ok(if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    })
}

/// The code for an error response without an [`ErrorBody`], such as an
/// axum rejection of a bad JSON body or a 405 from the router. A 4xx is
/// the caller's fault; anything else is the server's.
fn fallback_code(status: u16) -> ErrorCode {
    match status {
        404 => ErrorCode::NotFound,
        400..=499 => ErrorCode::BadRequest,
        _ => ErrorCode::Internal,
    }
}

/// Characters kept as-is in one path segment. Everything else, `/` included,
/// is percent-encoded.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn seg(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}

/// How long one request may take before the client gives up. A server
/// that accepts the connection but never answers would otherwise hang the
/// caller forever.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The limit for routes that do long work on the server: import, sync,
/// GitHub sync, prune and backup.
pub const LONG_REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Computes the caller's cumulative token usage, by model. It can be slow,
/// so the client calls it only for its first write. See [`sends_usage`].
pub type UsageFn = std::sync::Arc<dyn Fn() -> Option<TranscriptTotals> + Send + Sync>;

/// What a transcript has used so far.
pub struct TranscriptTotals {
    pub usage: std::collections::BTreeMap<String, TokenUsage>,
    /// Active working time, in seconds.
    pub active_seconds: u64,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
    /// The encoded context, without usage. GET requests send it.
    context: String,
    raw_context: std::sync::Arc<ClientContext>,
    usage: Option<UsageFn>,
    /// The encoded context with usage, built on the first write.
    write_context: std::sync::Arc<std::sync::OnceLock<String>>,
    timeout: Duration,
    long_timeout: Duration,
    /// Refuse every write before it leaves this process.
    read_only: bool,
    /// Server version from the last response.
    pub server_version: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl Client {
    pub fn new(base: impl Into<String>, context: &ClientContext) -> Self {
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .expect("http client");
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            http,
            context: encode_context(context),
            raw_context: std::sync::Arc::new(context.clone()),
            usage: None,
            write_context: Default::default(),
            timeout: REQUEST_TIMEOUT,
            long_timeout: LONG_REQUEST_TIMEOUT,
            read_only: false,
            server_version: Default::default(),
        }
    }

    /// Refuses every write from now on, with a `Forbidden` error, before it
    /// reaches the server. Reads still work.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Sets the request limits: `timeout` for normal routes, `long` for
    /// import, sync, GitHub sync, prune and backup.
    pub fn with_timeouts(mut self, timeout: Duration, long: Duration) -> Self {
        self.timeout = timeout;
        self.long_timeout = long;
        self
    }

    /// Sends usage from `f` with every write. Reads never send usage, so
    /// they never pay for computing it.
    pub fn with_usage(mut self, f: UsageFn) -> Self {
        self.usage = Some(f);
        self.write_context = Default::default();
        self
    }

    /// The context header for one request.
    fn context_for(&self, method: &reqwest::Method, path: &str) -> &str {
        let Some(usage) = self.usage.as_ref().filter(|_| sends_usage(method, path)) else {
            return &self.context;
        };
        self.write_context.get_or_init(|| {
            let mut c = (*self.raw_context).clone();
            if let Some(t) = usage() {
                c.usage = Some(t.usage).filter(|u| !u.is_empty());
                c.active_seconds = Some(t.active_seconds).filter(|s| *s > 0);
            }
            encode_context(&c)
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Sends one request. `body` is sent as JSON for every method but GET.
    pub async fn request<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let v = self
            .raw(
                method,
                path,
                body.map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| ClientError::Other(e.to_string()))?,
            )
            .await?;
        serde_json::from_value(v).map_err(|e| {
            ClientError::Other(format!("unexpected response from {method} {path}: {e}"))
        })
    }

    /// Sends one request and returns the JSON as it came.
    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.raw_within(method, path, body, self.timeout).await
    }

    async fn raw_within(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        let m = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes())
            .map_err(|e| ClientError::Other(e.to_string()))?;
        if self.read_only && is_write(&m, path) {
            return Err(ClientError::Api {
                status: 403,
                detail: ErrorDetail {
                    code: ErrorCode::Forbidden,
                    message: "this tracker is read-only here, so nothing changed".into(),
                    details: serde_json::Value::Null,
                },
            });
        }
        let url = format!("{}{}", self.base, path);
        let mut req = self
            .http
            .request(m.clone(), &url)
            .timeout(timeout)
            .header(CONTEXT_HEADER, self.context_for(&m, path));
        if m != reqwest::Method::GET {
            req = req.json(&body.unwrap_or(serde_json::Value::Object(Default::default())));
        }
        let res = req
            .send()
            .await
            .map_err(|e| send_error(e, method, path, timeout))?;
        if let Some(v) = res
            .headers()
            .get(VERSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *self.server_version.lock().unwrap() = Some(v.to_string());
        }
        let status = res.status().as_u16();
        let text = res
            .text()
            .await
            .map_err(|e| send_error(e, method, path, timeout))?;
        let value: serde_json::Value = if text.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text))
        };
        if status >= 400 {
            let detail = serde_json::from_value::<ErrorBody>(value.clone())
                .map(|b| b.error)
                .unwrap_or_else(|_| ErrorDetail {
                    code: fallback_code(status),
                    message: format!("HTTP {status}: {value}"),
                    details: serde_json::Value::Null,
                });
            return Err(ClientError::Api { status, detail });
        }
        Ok(value)
    }

    /// Sends one request to endpoint `E`. `args` fills the `{name}` path
    /// segments in order. GET and DELETE send `req` as the query string,
    /// and other methods send it as the JSON body.
    pub async fn call<E: Endpoint, P: PathArgs>(
        &self,
        args: P,
        req: &E::Request,
    ) -> Result<E::Response> {
        const {
            assert!(
                P::COUNT == E::PARAMS,
                "the path arguments do not match the endpoint's path"
            )
        };
        let req = serde_json::to_value(req).map_err(|e| ClientError::Other(e.to_string()))?;
        let mut path = fill_path(E::PATH, &args.values());
        let body = if E::METHOD.has_query() {
            path.push_str(&query_string(&req)?);
            None
        } else {
            Some(req).filter(|v| !v.is_null())
        };
        let timeout = if E::LONG {
            self.long_timeout
        } else {
            self.timeout
        };
        let method = E::METHOD.as_str();
        let v = self.raw_within(method, &path, body, timeout).await?;
        serde_json::from_value(v).map_err(|e| {
            ClientError::Other(format!("unexpected response from {method} {path}: {e}"))
        })
    }

    // ------------------------------------------------------------ server

    pub async fn health(&self) -> Result<Health> {
        self.call::<ep::Health, _>((), &()).await
    }
    pub async fn stop_server(&self) -> Result<serde_json::Value> {
        self.call::<ep::StopServer, _>((), &()).await
    }
    pub async fn backup(&self) -> Result<BackupReport> {
        self.call::<ep::Backup, _>((), &()).await
    }
    pub async fn settings(&self) -> Result<Settings> {
        self.call::<ep::Settings, _>((), &()).await
    }
    pub async fn update_settings(&self, u: &UpdateSettings) -> Result<Settings> {
        self.call::<ep::UpdateSettings, _>((), u).await
    }
    pub async fn actors(&self) -> Result<ActorList> {
        self.call::<ep::Actors, _>((), &()).await
    }
    pub async fn me(&self) -> Result<Actor> {
        self.call::<ep::Me, _>((), &()).await
    }

    // ------------------------------------------------------------ teams

    pub async fn teams(&self) -> Result<Vec<Team>> {
        self.call::<ep::Teams, _>((), &()).await
    }
    pub async fn team(&self, r: &str) -> Result<Team> {
        self.call::<ep::Team, _>(r, &()).await
    }
    pub async fn create_team(&self, c: &CreateTeam) -> Result<Team> {
        self.call::<ep::CreateTeam, _>((), c).await
    }
    pub async fn update_team(&self, r: &str, u: &UpdateTeam) -> Result<Team> {
        self.call::<ep::UpdateTeam, _>(r, u).await
    }
    pub async fn create_status(&self, team: &str, c: &CreateStatus) -> Result<Status> {
        self.call::<ep::CreateStatus, _>(team, c).await
    }
    pub async fn update_status(&self, id: &str, u: &UpdateStatus) -> Result<Status> {
        self.call::<ep::UpdateStatus, _>(id, u).await
    }
    pub async fn delete_status(&self, id: &str) -> Result<serde_json::Value> {
        self.call::<ep::DeleteStatus, _>(id, &()).await
    }

    // ------------------------------------------------------------ labels

    pub async fn labels(&self) -> Result<Vec<Label>> {
        self.call::<ep::Labels, _>((), &()).await
    }
    pub async fn create_label(&self, c: &CreateLabel) -> Result<Label> {
        self.call::<ep::CreateLabel, _>((), c).await
    }
    pub async fn update_label(&self, r: &str, u: &UpdateLabel) -> Result<Label> {
        self.call::<ep::UpdateLabel, _>(r, u).await
    }
    pub async fn delete_label(&self, r: &str) -> Result<serde_json::Value> {
        self.call::<ep::DeleteLabel, _>(r, &()).await
    }

    // ------------------------------------------------------------ projects

    pub async fn projects(&self, trashed: bool) -> Result<Vec<Project>> {
        self.call::<ep::Projects, _>((), &ProjectQuery { trashed })
            .await
    }
    pub async fn project(&self, r: &str) -> Result<Project> {
        self.call::<ep::Project, _>(r, &()).await
    }
    pub async fn create_project(&self, c: &CreateProject) -> Result<Project> {
        self.call::<ep::CreateProject, _>((), c).await
    }
    pub async fn update_project(&self, r: &str, u: &UpdateProject) -> Result<Project> {
        self.call::<ep::UpdateProject, _>(r, u).await
    }
    pub async fn delete_project(&self, r: &str) -> Result<Project> {
        self.call::<ep::DeleteProject, _>(r, &()).await
    }
    pub async fn link_repo(&self, r: &str, repo: &str) -> Result<Project> {
        let link = RepoLink {
            repo: repo.to_string(),
        };
        self.call::<ep::LinkRepo, _>(r, &link).await
    }
    pub async fn unlink_repo(&self, r: &str, repo: &str) -> Result<Project> {
        let (owner, name) = repo.split_once('/').ok_or_else(|| {
            ClientError::Other(format!("repo must look like owner/name, got {repo:?}"))
        })?;
        self.call::<ep::UnlinkRepo, _>((r, owner, name), &()).await
    }
    pub async fn milestones(&self, project: &str) -> Result<Vec<Milestone>> {
        self.call::<ep::Milestones, _>(project, &()).await
    }
    pub async fn create_milestone(&self, project: &str, c: &CreateMilestone) -> Result<Milestone> {
        self.call::<ep::CreateMilestone, _>(project, c).await
    }
    pub async fn update_milestone(&self, id: &str, u: &UpdateMilestone) -> Result<Milestone> {
        self.call::<ep::UpdateMilestone, _>(id, u).await
    }
    pub async fn delete_milestone(&self, id: &str) -> Result<serde_json::Value> {
        self.call::<ep::DeleteMilestone, _>(id, &()).await
    }

    // ------------------------------------------------------------ documents

    /// Lists documents. `search` keeps only those that match the text,
    /// best match first.
    pub async fn documents(
        &self,
        project: Option<&str>,
        trashed: bool,
        search: Option<&str>,
    ) -> Result<Vec<Document>> {
        let q = DocumentQuery {
            trashed,
            project: project.map(str::to_string),
            search: search.map(str::to_string),
        };
        self.call::<ep::Documents, _>((), &q).await
    }
    pub async fn document(&self, r: &str) -> Result<Document> {
        self.call::<ep::Document, _>(r, &()).await
    }
    pub async fn create_document(&self, c: &CreateDocument) -> Result<Document> {
        self.call::<ep::CreateDocument, _>((), c).await
    }
    pub async fn update_document(&self, r: &str, u: &UpdateDocument) -> Result<Document> {
        self.call::<ep::UpdateDocument, _>(r, u).await
    }
    pub async fn delete_document(&self, r: &str) -> Result<Document> {
        self.call::<ep::DeleteDocument, _>(r, &()).await
    }
    pub async fn document_history(&self, r: &str) -> Result<History> {
        self.call::<ep::DocumentHistory, _>(r, &()).await
    }
    pub async fn restore_document(&self, r: &str, req: &RestoreRequest) -> Result<Document> {
        self.call::<ep::RestoreDocument, _>(r, req).await
    }

    // ------------------------------------------------------------ issues

    pub async fn query(&self, q: &IssueQuery) -> Result<Page<Issue>> {
        self.call::<ep::QueryIssues, _>((), q).await
    }

    /// Fetches every page, or stops at `q.limit` issues when it is set.
    pub async fn query_all(&self, q: &IssueQuery) -> Result<Vec<Issue>> {
        let mut q = q.clone();
        let want = q.limit;
        let mut out = Vec::new();
        loop {
            if let Some(w) = want {
                q.limit = Some(w.saturating_sub(out.len() as u32).max(1));
            }
            let page = self.query(&q).await?;
            out.extend(page.items);
            match page.next_cursor {
                Some(c) if want.is_none_or(|w| (out.len() as u32) < w) => q.cursor = Some(c),
                _ => break,
            }
        }
        Ok(out)
    }
    pub async fn frontier(&self, q: &FrontierQuery) -> Result<Vec<Issue>> {
        self.call::<ep::Frontier, _>((), q).await
    }
    pub async fn create_issue(&self, c: &CreateIssue) -> Result<IssueDetail> {
        self.call::<ep::CreateIssue, _>((), c).await
    }
    pub async fn issue(&self, r: &str) -> Result<IssueDetail> {
        self.call::<ep::Issue, _>(r, &()).await
    }
    pub async fn update_issue(&self, r: &str, u: &UpdateIssue) -> Result<IssueDetail> {
        self.call::<ep::UpdateIssue, _>(r, u).await
    }
    pub async fn delete_issue(&self, r: &str) -> Result<Issue> {
        self.call::<ep::DeleteIssue, _>(r, &()).await
    }
    pub async fn restore_issue(&self, r: &str, req: &RestoreRequest) -> Result<IssueDetail> {
        self.call::<ep::RestoreIssue, _>(r, req).await
    }
    pub async fn issue_history(&self, r: &str) -> Result<History> {
        self.call::<ep::IssueHistory, _>(r, &()).await
    }
    pub async fn claim(&self, r: &str, req: &ClaimRequest) -> Result<ClaimResponse> {
        self.call::<ep::Claim, _>(r, req).await
    }
    /// Every owner question the query matches, across all pages.
    pub async fn questions(&self, q: &QuestionQuery) -> Result<Vec<QuestionEntry>> {
        let mut q = q.clone();
        q.cursor = None;
        let mut out = Vec::new();
        loop {
            let page = self.call::<ep::Questions, _>((), &q).await?;
            out.extend(page.items);
            match page.next_cursor {
                Some(c) => q.cursor = Some(c),
                None => return Ok(out),
            }
        }
    }

    /// Reopens an answered owner question. The server refuses with a
    /// conflict when someone acted on the answer or the question changed.
    pub async fn undo_answer(&self, r: &str, id: &str) -> Result<IssueDetail> {
        self.call::<ep::UndoAnswer, _>((r, id), &()).await
    }
    /// The review section and every review answer of an issue.
    pub async fn review(&self, r: &str) -> Result<ReviewState> {
        self.call::<ep::Review, _>(r, &()).await
    }
    /// Approves an issue in User review, or asks for changes.
    pub async fn submit_review(&self, r: &str, req: &SubmitReview) -> Result<IssueDetail> {
        self.call::<ep::SubmitReview, _>(r, req).await
    }
    pub async fn analyze(&self, r: &str, req: &AnalyzeRequest) -> Result<AnalyzeResponse> {
        self.call::<ep::Analyze, _>(r, req).await
    }

    pub async fn release(&self, r: &str, req: &ReleaseRequest) -> Result<Issue> {
        self.call::<ep::Release, _>(r, req).await
    }
    pub async fn deps(&self, r: &str, open: bool) -> Result<DepNode> {
        self.call::<ep::Deps, _>(r, &DepsQuery { open }).await
    }
    pub async fn add_comment(&self, r: &str, c: &CreateComment) -> Result<Comment> {
        self.call::<ep::AddComment, _>(r, c).await
    }
    pub async fn update_comment(&self, id: &str, u: &UpdateComment) -> Result<Comment> {
        self.call::<ep::UpdateComment, _>(id, u).await
    }
    pub async fn delete_comment(&self, id: &str) -> Result<serde_json::Value> {
        self.call::<ep::DeleteComment, _>(id, &()).await
    }
    pub async fn add_relation(&self, r: &str, c: &CreateRelation) -> Result<IssueDetail> {
        self.call::<ep::AddRelation, _>(r, c).await
    }
    pub async fn remove_relation(&self, r: &str, c: &RemoveRelation) -> Result<IssueDetail> {
        self.call::<ep::RemoveRelation, _>(r, c).await
    }
    pub async fn attach(&self, r: &str, c: &CreateAttachment) -> Result<Attachment> {
        self.call::<ep::Attach, _>(r, c).await
    }
    pub async fn detach(&self, id: &str) -> Result<serde_json::Value> {
        self.call::<ep::Detach, _>(id, &()).await
    }

    // ------------------------------------------------------------ sessions

    pub async fn session(&self, id: &str) -> Result<SessionView> {
        self.call::<ep::Session, _>(id, &()).await
    }
    pub async fn add_session_note(&self, id: &str, c: &CreateSessionNote) -> Result<SessionNote> {
        self.call::<ep::AddSessionNote, _>(id, c).await
    }

    // ------------------------------------------------------------ activity

    pub async fn usage(&self, q: &UsageQuery) -> Result<UsageReport> {
        self.call::<ep::Usage, _>((), q).await
    }

    pub async fn activity(&self, q: &ActivityQuery) -> Result<ActivityPage> {
        self.call::<ep::Activity, _>((), q).await
    }

    /// The WebSocket URL for live events.
    pub fn ws_url(&self) -> String {
        let base = self.base.replacen("http://", "ws://", 1);
        format!("{base}{}", fill_path(ep::Ws::PATH, &[]))
    }

    pub fn context_header(&self) -> (&'static str, &str) {
        (CONTEXT_HEADER, &self.context)
    }

    // ------------------------------------------------------------ views

    pub async fn views(&self) -> Result<Vec<SavedView>> {
        self.call::<ep::Views, _>((), &()).await
    }
    pub async fn create_view(&self, c: &CreateView) -> Result<SavedView> {
        self.call::<ep::CreateView, _>((), c).await
    }
    pub async fn update_view(&self, r: &str, u: &UpdateView) -> Result<SavedView> {
        self.call::<ep::UpdateView, _>(r, u).await
    }
    pub async fn delete_view(&self, r: &str) -> Result<serde_json::Value> {
        self.call::<ep::DeleteView, _>(r, &()).await
    }

    // ------------------------------------------------------------ sync

    pub async fn sync_status(&self) -> Result<SyncStatus> {
        self.call::<ep::SyncStatus, _>((), &()).await
    }
    pub async fn sync_push(&self, r: &SyncPushRequest) -> Result<SyncReport> {
        self.call::<ep::SyncPush, _>((), r).await
    }
    pub async fn sync_pull(&self, r: &SyncPullRequest) -> Result<SyncReport> {
        self.call::<ep::SyncPull, _>((), r).await
    }

    // ------------------------------------------------------------ github

    pub async fn github_status(&self) -> Result<GithubReport> {
        self.call::<ep::GithubStatus, _>((), &()).await
    }
    pub async fn github_sync(&self) -> Result<GithubReport> {
        self.call::<ep::GithubSync, _>((), &()).await
    }

    // ------------------------------------------------------------ import

    pub async fn import_linear(&self, r: &LinearImportRequest) -> Result<ImportReport> {
        self.call::<ep::ImportLinear, _>((), r).await
    }

    /// Downloads a stored file, such as an image copied from Linear. The
    /// answer is bytes, not JSON, so it skips [`Client::call`].
    pub async fn file(&self, hash: &str) -> Result<Vec<u8>> {
        let path = fill_path(ep::File::PATH, &[hash]);
        let url = format!("{}{path}", self.base);
        let res = self
            .http
            .get(&url)
            .timeout(self.timeout)
            .header(CONTEXT_HEADER, &self.context)
            .send()
            .await
            .map_err(|e| send_error(e, "GET", ep::File::PATH, self.timeout))?;
        let status = res.status().as_u16();
        let bytes = res
            .bytes()
            .await
            .map_err(|e| send_error(e, "GET", ep::File::PATH, self.timeout))?;
        if status >= 400 {
            let detail = serde_json::from_slice::<ErrorBody>(&bytes)
                .map(|b| b.error)
                .unwrap_or(ErrorDetail {
                    code: ErrorCode::Internal,
                    message: format!("HTTP {status}"),
                    details: serde_json::Value::Null,
                });
            return Err(ClientError::Api { status, detail });
        }
        Ok(bytes.to_vec())
    }

    // ------------------------------------------------------------ admin

    pub async fn prune(&self, req: &PruneRequest) -> Result<PruneReport> {
        self.call::<ep::Prune, _>((), req).await
    }
}

/// Maps a reqwest error from sending a request or reading its answer. A
/// connect failure, a timeout, or a connection that closes before the whole
/// answer arrives means the server cannot be reached, which is exit 7. A
/// reused keep-alive connection that the server or the network dropped
/// fails as a request error, and a cut body as a body or decode error, so
/// those count too. Only a request this client could not build is another
/// kind of failure.
fn send_error(e: reqwest::Error, method: &str, path: &str, timeout: Duration) -> ClientError {
    if e.is_timeout() {
        ClientError::Unreachable(format!(
            "the server did not answer {method} {path} within {} seconds",
            timeout.as_secs_f32()
        ))
    } else if e.is_builder() || e.is_redirect() {
        ClientError::Other(e.to_string())
    } else {
        ClientError::Unreachable(e.to_string())
    }
}

pub fn encode_context(c: &ClientContext) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(c).unwrap_or_default())
}

/// Reads `server.json` from a data folder.
pub fn read_server_info(data_dir: &Path) -> Option<ServerInfo> {
    let raw = std::fs::read(data_dir.join("server.json")).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// How to find, and if needed start, the server.
pub struct Connect {
    pub data_dir: PathBuf,
    pub context: ClientContext,
    /// Binary to run as `<bin> server run` when no server answers. None means
    /// fail instead.
    pub spawn: Option<PathBuf>,
    /// Restart a running server that is older than this client.
    pub upgrade_old_server: bool,
}

/// Compares dotted versions such as `0.1.10` and `0.1.9`.
pub fn version_older(a: &str, b: &str) -> bool {
    let parse = |v: &str| {
        v.split(['.', '-'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    parse(a) < parse(b)
}

impl Connect {
    pub async fn connect(self) -> Result<Client> {
        if let Some(c) = self.try_existing().await {
            let h = c.health().await?;
            if self.upgrade_old_server && self.spawn.is_some() && version_older(&h.version, VERSION)
            {
                let _ = c.stop_server().await;
                wait_gone(&self.data_dir).await;
            } else {
                return Ok(c);
            }
        }
        let Some(bin) = &self.spawn else {
            return Err(ClientError::Unreachable(format!(
                "no server for {}; start one with `llmtodo server run`",
                self.data_dir.display()
            )));
        };
        spawn_server(bin, &self.data_dir)?;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Some(c) = self.try_existing().await {
                return Ok(c);
            }
        }
        let log = self.data_dir.join("server-stderr.log");
        Err(ClientError::Unreachable(format!(
            "started the server, but it did not answer within 5 seconds; see {}",
            log.display()
        )))
    }

    async fn try_existing(&self) -> Option<Client> {
        find_server(&self.data_dir, &self.context)
            .await
            .map(|(c, _)| c)
    }
}

/// Returns a client for the server named in `server.json`, and its health,
/// but only when that server is the one that wrote the file. A crashed server
/// leaves `server.json` behind, and another llmtodo server for another data
/// folder can take its port later.
pub async fn find_server(data_dir: &Path, context: &ClientContext) -> Option<(Client, Health)> {
    let info = read_server_info(data_dir)?;
    let c = Client::new(format!("http://127.0.0.1:{}", info.port), context);
    let h = c.health().await.ok()?;
    (h.pid == info.pid && same_dir(Path::new(&h.data_dir), data_dir)).then_some((c, h))
}

/// Compares two folders after resolving symlinks and relative parts. Falls
/// back to the raw paths when a folder cannot be resolved.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

async fn wait_gone(data_dir: &Path) {
    for _ in 0..60 {
        if read_server_info(data_dir).is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// True when `dir` is `~/Library/Application Support/llmtodo`, the folder the
/// owner's own server uses.
fn is_default_data_dir(dir: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    same_dir(dir, &data_dir::standard_data_dir_in(Path::new(&home)))
}

fn spawn_server(bin: &Path, data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir).map_err(|e| ClientError::Other(e.to_string()))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("server-stderr.log"))
        .map_err(|e| ClientError::Other(e.to_string()))?;
    let mut cmd = std::process::Command::new(bin);
    cmd.args(["server", "run"]);
    // Only the default folder's server uses the default port. Any other
    // folder takes a free port, since clients read it from `server.json`.
    if std::env::var_os("LLMTODO_PORT").is_none() && !is_default_data_dir(data_dir) {
        cmd.args(["--port", "0"]);
    }
    cmd.env("LLMTODO_DATA_DIR", data_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so Ctrl-C in the caller's terminal does not
        // stop the server.
        cmd.process_group(0);
    }
    cmd.spawn()
        .map_err(|e| ClientError::Unreachable(format!("could not start {}: {e}", bin.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_encode_slashes() {
        assert_eq!(seg("notebook/game-rules"), "notebook%2Fgame-rules");
        assert_eq!(seg("ALX-12"), "ALX-12");
    }

    #[test]
    fn paths_and_queries() {
        assert_eq!(
            fill_path(ep::UnlinkRepo::PATH, &["a b", "o", "n/x"]),
            "/v1/projects/a%20b/repos/o/n%2Fx"
        );
        let q = serde_json::to_value(DocumentQuery {
            trashed: false,
            project: Some("p 1".into()),
            search: None,
        })
        .unwrap();
        // The field order depends on serde_json's `preserve_order` feature.
        let qs = query_string(&q).unwrap();
        let mut parts: Vec<&str> = qs.trim_start_matches('?').split('&').collect();
        parts.sort();
        assert_eq!(parts, ["project=p%201", "trashed=false"]);
        assert_eq!(query_string(&serde_json::Value::Null).unwrap(), "");
    }

    #[test]
    fn versions() {
        assert!(version_older("0.1.9", "0.1.10"));
        assert!(!version_older("0.2.0", "0.1.10"));
        assert!(!version_older("0.1.0", "0.1.0"));
    }

    #[test]
    fn reads_send_no_usage() {
        use reqwest::Method;
        assert!(!sends_usage(&Method::GET, "/v1/issues/LLM-1"));
        assert!(!sends_usage(&Method::POST, "/v1/issues/query"));
        assert!(!sends_usage(&Method::POST, "/v1/issues/frontier"));
        assert!(sends_usage(&Method::POST, "/v1/issues"));
        assert!(sends_usage(&Method::PATCH, "/v1/issues/LLM-1"));
    }

    /// A server that reads the request and closes the connection without
    /// an answer. A reused keep-alive connection fails this way when the
    /// server or the network drops it.
    #[tokio::test]
    async fn a_connection_cut_before_the_answer_is_unreachable() {
        use tokio::io::AsyncReadExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
            }
        });
        let client = Client::new(
            format!("http://127.0.0.1:{port}"),
            &ClientContext::default(),
        );
        let err = client.health().await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Unreachable, "{err}");
    }

    /// The server sends its headers, then closes before the body is done.
    #[tokio::test]
    async fn a_body_cut_short_is_unreachable() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{\"ok\":";
                let _ = socket.write_all(head.as_bytes()).await;
            }
        });
        let client = Client::new(
            format!("http://127.0.0.1:{port}"),
            &ClientContext::default(),
        );
        let err = client.health().await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Unreachable, "{err}");
    }

    #[test]
    fn error_codes_match_exit_codes() {
        let e = ClientError::Unreachable("refused".into());
        assert_eq!(e.code(), ErrorCode::Unreachable);
        assert_eq!(e.to_body().error.code.exit_code(), e.exit_code());
        assert_eq!(fallback_code(405), ErrorCode::BadRequest);
        assert_eq!(fallback_code(422), ErrorCode::BadRequest);
        assert_eq!(fallback_code(404), ErrorCode::NotFound);
        assert_eq!(fallback_code(502), ErrorCode::Internal);
    }
}
