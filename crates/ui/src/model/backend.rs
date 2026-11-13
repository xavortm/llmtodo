//! The bridge between GPUI and the API server.
//!
//! `llmtodo_client` is async on tokio, and reqwest needs a tokio reactor.
//! GPUI runs its own executor, so this module runs one tokio runtime on a
//! background thread. The app sends [`Request`]s over a tokio channel, and
//! the thread sends [`Event`]s back over a `futures` channel, which a GPUI
//! task awaits on the main thread. SPIKE.md records why.
//!
//! The thread finds the server like the CLI does, keeps one WebSocket on
//! `/v1/ws`, and turns activity events into fresh issue copies. After a
//! reconnect it catches up from `/v1/activity?after_seq=<last>`, or loads
//! everything again when the server says the gap is too old.

use std::collections::BTreeSet;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::channel::mpsc as fmpsc;
use llmtodo_client::types::{
    ActivityEvent, ActivityQuery, ClientContext, ErrorCode, HistoryQuery, Issue, IssueDetail,
    IssueGit, IssueOrder, IssueQuery, Label, Project, QuestionEntry, QuestionQuery, ReleaseRequest,
    RestoreRequest, Team, UpdateIssue,
};
use llmtodo_client::types::{ActivityField, DocumentId, EntityType, IssueId};
use llmtodo_client::types::{
    BodyChange, CreateIssue, CreateRelation, CreateView, Document, History, Relation,
    RemoveRelation, SavedView, UpdateDocument, WorkSession,
};
use llmtodo_client::{Client, ClientError, Connect, live};
use tokio::sync::mpsc as tmpsc;

/// The teams with their statuses, the labels, projects, saved views and
/// documents.
#[derive(Debug, Clone, Default)]
pub struct CatalogData {
    pub teams: Vec<Team>,
    pub labels: Vec<Label>,
    pub projects: Vec<Project>,
    pub views: Vec<SavedView>,
    /// Project documents, without their bodies.
    pub documents: Vec<Document>,
}

impl CatalogData {
    /// The catalog as the app maps issues to it.
    pub fn catalog(&self) -> crate::model::data::Catalog {
        crate::model::data::Catalog::from_api(
            &self.teams,
            &self.labels,
            &self.projects,
            &self.views,
            &self.documents,
        )
    }
}

/// Everything the list needs, loaded in one go, as the server sent it.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub catalog: CatalogData,
    /// Live issues, then the Trash, without their descriptions. The app
    /// fetches the text when a task opens.
    pub issues: Vec<Issue>,
    /// The open owner questions on open issues, for the inbox.
    pub questions: Vec<QuestionEntry>,
}

/// A snapshot with its issues already mapped to the app's catalog. The
/// API thread does the mapping, so a large tracker does not hold up the
/// main thread.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The catalog as the server sent it.
    pub data: CatalogData,
    pub catalog: crate::model::data::Catalog,
    pub issues: Vec<crate::model::data::Issue>,
    pub questions: Vec<QuestionEntry>,
    /// Tests: the snapshot as the server sent it.
    #[cfg(test)]
    pub raw: Snapshot,
}

impl Snapshot {
    /// Maps every issue to the app's catalog.
    pub fn load(self) -> Loaded {
        #[cfg(test)]
        let raw = self.clone();
        let catalog = self.catalog.catalog();
        let issues = self
            .issues
            .iter()
            .map(|i| crate::model::data::Issue::from_api(i, &catalog))
            .collect();
        Loaded {
            data: self.catalog,
            catalog,
            issues,
            questions: self.questions,
            #[cfg(test)]
            raw,
        }
    }
}

/// What a write changes, so the app can word its message and track saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    Status,
    Labels,
    Assignee,
    WaitingOn,
    Description,
    /// An answer to an owner question: a patch, so it needs no version.
    Answer,
    UndoAnswer,
    Trash,
    Restore,
    Release,
    Dependency,
    /// A new parent, or none.
    Parent,
    /// A step in the parent's order.
    Order,
    /// Approve or Request changes on a ticket in User review.
    Review,
}

#[derive(Debug, Clone)]
pub enum Write {
    Update(Box<UpdateIssue>),
    /// Reopens an answered owner question, such as `Q1`.
    UndoAnswer {
        id: String,
    },
    Trash,
    Restore,
    Release,
    AddRelation(CreateRelation),
    RemoveRelation(RemoveRelation),
    Review(llmtodo_api_types::SubmitReview),
    /// The whole order of the parent's sub-issues. The key is the parent.
    SetOrder(llmtodo_api_types::SetOrder),
}

#[derive(Debug)]
pub enum Request {
    Write {
        key: String,
        write: Write,
        kind: WriteKind,
    },
    /// Load everything again.
    Reload,
    /// Send a fresh copy of one issue, trashed or not, with its detail.
    Fetch {
        key: String,
    },
    /// The task the panel shows, or none. Live refreshes fetch the full
    /// detail and run git only for this one.
    Open {
        id: Option<IssueId>,
    },
    /// Send the IDs of the issues a saved view holds.
    ViewIssues {
        name: String,
    },
    CreateView(Box<CreateView>),
    DeleteView {
        name: String,
    },
    /// Set or clear a project's color, as `#rrggbb`. A failure comes back
    /// as `ProjectFailed`. The new color comes back as a catalog change.
    SetProjectColor {
        slug: String,
        color: Option<String>,
    },
    /// Create an issue. The answer is the new issue, then `Created`.
    Create(Box<CreateIssue>),
    /// Read again whether this machine is the primary copy.
    SyncStatus,
    /// Send one document with its body.
    Document {
        id: DocumentId,
    },
    /// Save a document's body, based on `base`, or over any version.
    SaveDocument {
        id: DocumentId,
        body: String,
        base: i64,
        force: bool,
    },
    /// Send an issue's field changes and description versions.
    History {
        key: String,
    },
    /// Ask the server of another tracker which teams it holds. It never
    /// starts that server. The answer is `Teams`.
    ProbeTeams {
        data_dir: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub enum Event {
    Connected {
        url: String,
    },
    /// The server cannot be reached. The thread keeps retrying.
    Offline {
        reason: String,
    },
    Snapshot(Box<Loaded>),
    /// Statuses, labels, teams, projects, views or documents changed. No
    /// issues come with it: the app maps the issues it holds again.
    Catalog(Box<CatalogData>),
    /// A fresh list copy of one issue, after a write that returns no
    /// detail.
    Issue(Box<Issue>),
    /// Fresh list copies of the issues live events touched, with their
    /// text. The open task comes as [`Event::IssueDetail`] instead.
    Issues(Vec<Issue>),
    /// One issue with everything the task panel shows, in one event.
    IssueDetail(Box<DetailParts>),
    /// Fresh copies of the tasks that wait for a task that changed. They
    /// come without descriptions: only depth and open blockers apply.
    Dependents(Vec<Issue>),
    WriteDone {
        key: String,
        kind: WriteKind,
        /// The description version after the write.
        version: i64,
    },
    /// The issues a saved view holds now.
    ViewIssues {
        name: String,
        ids: Vec<IssueId>,
    },
    ViewSaved {
        name: String,
    },
    ViewDeleted {
        name: String,
    },
    /// An issue's field changes and description versions, without their
    /// text. The sessions come with [`Event::IssueDetail`].
    History {
        key: String,
        history: Box<History>,
    },
    /// One document with its body.
    Document(Box<Document>),
    /// The server saved a document. This is its new copy.
    DocumentSaved(Box<Document>),
    /// A document could not be read or saved.
    DocumentFailed {
        id: DocumentId,
        message: String,
        /// A save based on an old version.
        stale: bool,
    },
    /// Whether this machine is the primary copy. Only the primary takes
    /// writes. `holder` is the host that holds the sync lock, if known.
    Primary {
        primary: bool,
        holder: Option<String>,
    },
    /// The server created this issue. Its copy came just before.
    Created {
        id: IssueId,
        key: String,
    },
    CreateFailed {
        title: String,
        message: String,
    },
    /// A project's color could not be saved.
    ProjectFailed {
        slug: String,
        message: String,
    },
    /// A saved view could not be read, saved or deleted.
    ViewFailed {
        name: String,
        message: String,
    },
    /// The team keys of the tracker in `data_dir`, or `None` when its
    /// server does not answer.
    Teams {
        data_dir: PathBuf,
        teams: Option<Vec<String>>,
    },
    WriteFailed {
        key: String,
        kind: WriteKind,
        message: String,
        /// The description changed on the server since the owner opened it.
        stale: bool,
    },
}

/// The parts of an issue's detail that the app keeps.
#[derive(Debug, Clone)]
pub struct DetailParts {
    pub issue: Issue,
    /// The relations, for the dependency section.
    pub relations: Vec<Relation>,
    /// The review answers, oldest first.
    pub reviews: Vec<llmtodo_api_types::ReviewAnswer>,
    /// The git panel, with its commits read from the local repo.
    pub git: IssueGit,
    /// The agent sessions. Sessions from this machine carry their commits.
    pub sessions: Vec<WorkSession>,
}

/// Where the data folder is, and which binary starts a server.
#[derive(Debug, Clone)]
pub struct Target {
    pub data_dir: PathBuf,
    /// Run as `<bin> server run` when no server answers.
    pub spawn: Option<PathBuf>,
    /// Read only: the client refuses every write, and the app never starts
    /// a server here, because a start can migrate the database.
    pub read_only: bool,
    /// Tests: each live refresh waits this long first, to stand in for a
    /// slow one.
    #[cfg(test)]
    pub slow_flush: Duration,
}

impl Target {
    /// The data folder the CLI opens too, from
    /// [`llmtodo_client::data_dir::resolve`]. The server binary is `llmtodo` next to the app, else
    /// the one on `PATH`.
    pub fn from_env() -> Self {
        let data_dir = llmtodo_client::data_dir::resolve(None);
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join("llmtodo")))
            .filter(|p| p.is_file());
        let on_path = || {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|d| d.join("llmtodo"))
                    .find(|p| p.is_file())
            })
        };
        Self {
            data_dir,
            spawn: beside.or_else(on_path),
            read_only: false,
            #[cfg(test)]
            slow_flush: Duration::ZERO,
        }
    }
}

/// The app's handle on the API thread. Cloning it is cheap.
#[derive(Clone)]
pub struct Backend {
    tx: tmpsc::UnboundedSender<Request>,
}

impl Backend {
    /// Starts the API thread. Events arrive on the returned receiver until
    /// the last `Backend` clone is dropped.
    pub fn start(target: Target) -> (Self, fmpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = tmpsc::unbounded_channel();
        let (etx, erx) = fmpsc::unbounded();
        std::thread::Builder::new()
            .name("llmtodo-api".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime for the API client");
                runtime.block_on(run(target, rx, etx));
            })
            .expect("start the API thread");
        (Self { tx }, erx)
    }

    /// A backend with no API thread. Tests read what the app sends from
    /// the returned receiver.
    #[cfg(test)]
    pub fn detached() -> (Self, tmpsc::UnboundedReceiver<Request>) {
        let (tx, rx) = tmpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn write(&self, key: &str, write: Write, kind: WriteKind) {
        let _ = self.tx.send(Request::Write {
            key: key.to_string(),
            write,
            kind,
        });
    }

    pub fn request(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    pub fn reload(&self) {
        let _ = self.tx.send(Request::Reload);
    }

    pub fn fetch(&self, key: &str) {
        let _ = self.tx.send(Request::Fetch {
            key: key.to_string(),
        });
    }
}

type Events = fmpsc::UnboundedSender<Event>;

fn send(events: &Events, event: Event) {
    let _ = events.unbounded_send(event);
}

/// How long to wait for more events before fetching the issues they touch.
/// One write emits several events, such as a status, an assignee and a
/// claim, so this turns them into one fetch.
const SETTLE: Duration = Duration::from_millis(40);

/// How often the thread pings the live connection. A server that sends
/// nothing back for two of these is treated as gone, even when the socket
/// stays open.
const PING_EVERY: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(5)
};

/// The wait before a retry after the first failure.
const FIRST_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(5);
/// Before the first connect, this many retries stay at [`FIRST_DELAY`], so
/// a server that is still starting is found within about 3 seconds. Later
/// retries back off like after a drop.
const QUICK_TRIES: u32 = 12;

/// How long to wait after starting a server before starting another one.
/// The started server may still be getting ready, and a second one would
/// only fail on the lock file.
const SPAWN_AGAIN_AFTER: Duration = Duration::from_secs(30);

/// The wait between connection tries.
#[derive(Debug)]
struct Backoff {
    delay: Duration,
    /// Quick retries left before the first connect.
    quick_left: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            delay: FIRST_DELAY,
            quick_left: QUICK_TRIES,
        }
    }
}

impl Backoff {
    /// The wait before the next try. A session that connected starts over
    /// at the first delay. Failed tries double the wait up to
    /// [`MAX_DELAY`]. Before the first connect, the first [`QUICK_TRIES`]
    /// stay quick, so a server that is still starting is found soon.
    fn next(&mut self, connected: bool, connected_once: bool) -> Duration {
        if connected {
            self.delay = FIRST_DELAY;
        }
        let wait = self.delay;
        if !connected {
            if !connected_once && self.quick_left > 0 {
                self.quick_left -= 1;
            } else {
                self.delay = (self.delay * 2).min(MAX_DELAY);
            }
        }
        wait
    }
}

async fn run(target: Target, mut requests: tmpsc::UnboundedReceiver<Request>, events: Events) {
    let mut state = State::default();
    let mut backoff = Backoff::default();
    loop {
        state.connected = false;
        let ended = session(&target, &mut requests, &events, &mut state).await;
        // A flush that the session left running is cut. Its work goes back,
        // so the next connect does it.
        if let Some(work) = state.flushing.take() {
            state.pending.merge(work);
        }
        match ended {
            Ok(()) => return,
            Err(reason) => {
                if events.is_closed() {
                    return;
                }
                if let Some(event) = state.offline(reason) {
                    send(&events, event);
                }
            }
        }
        let delay = backoff.next(state.connected, state.connected_once);
        // Wait before the next try. Writes that arrive meanwhile fail at
        // once, so the owner sees it, and the issue is fetched again on
        // reconnect to undo the app's early change.
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                r = requests.recv() => match r {
                    None => return,
                    Some(Request::Write { key, kind, .. }) => {
                        send(&events, Event::WriteFailed {
                            key: key.clone(),
                            kind,
                            message: "the server is offline, so nothing was saved".into(),
                            stale: false,
                        });
                        state.pending.keys.insert(key);
                    }
                    Some(Request::Fetch { key }) => {
                        state.pending.keys.insert(key);
                    }
                    Some(Request::Open { id }) => state.open = id,
                    Some(Request::ViewIssues { name } | Request::DeleteView { name }) => {
                        send(&events, Event::ViewFailed {
                            name,
                            message: "the server is offline".into(),
                        });
                    }
                    Some(Request::SyncStatus | Request::History { .. } | Request::Document { .. }) => {}
                    Some(Request::ProbeTeams { data_dir }) => probe_teams(data_dir, &events),
                    Some(Request::SaveDocument { id, .. }) => {
                        send(&events, Event::DocumentFailed {
                            id,
                            message: "the server is offline, so nothing was saved".into(),
                            stale: false,
                        });
                    }
                    Some(Request::Create(c)) => {
                        send(&events, Event::CreateFailed {
                            title: c.title,
                            message: "the server is offline, so nothing was created".into(),
                        });
                    }
                    Some(Request::CreateView(c)) => {
                        send(&events, Event::ViewFailed {
                            name: c.name,
                            message: "the server is offline, so nothing was saved".into(),
                        });
                    }
                    Some(Request::SetProjectColor { slug, .. }) => {
                        send(&events, Event::ProjectFailed {
                            slug,
                            message: "the server is offline, so nothing was saved".into(),
                        });
                    }
                    Some(Request::Reload) => break,
                },
            }
        }
    }
}

#[derive(Default)]
struct State {
    /// The last activity event seen, for catch-up after a reconnect.
    last_seq: Option<i64>,
    /// Issues to fetch, or a full reload, that the thread owes the app. It
    /// outlives a session: `last_seq` already counts the events behind it,
    /// so catch-up after a reconnect would not find them again.
    pending: Pending,
    connected_once: bool,
    /// The current session reached the server.
    connected: bool,
    /// When this thread last started a server, until a session connects.
    spawned_at: Option<Instant>,
    /// The task the panel shows. Only it gets its full detail and git.
    open: Option<IssueId>,
    /// The reason the app was last told the server is offline, until a
    /// session connects.
    offline_reason: Option<String>,
    /// A copy of the work the running flush took from `pending`, until it
    /// finishes.
    flushing: Option<Pending>,
}

/// A flush running on its own task. Dropping it stops the task.
struct FlushTask(tokio::task::JoinHandle<Result<(), String>>);

impl Drop for FlushTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl State {
    /// The offline event for a failed try, or none when the app already
    /// shows this reason. Each event makes the app redraw.
    fn offline(&mut self, reason: String) -> Option<Event> {
        if self.connected {
            self.offline_reason = None;
        }
        if self.offline_reason.as_ref() == Some(&reason) {
            return None;
        }
        self.offline_reason = Some(reason.clone());
        Some(Event::Offline { reason })
    }

    /// The binary to start a server with on this try. The thread starts at
    /// most one server per [`SPAWN_AGAIN_AFTER`], so retries before the
    /// first connect do not start a new process each time.
    fn spawn(&mut self, target: &Target) -> Option<PathBuf> {
        if target.read_only {
            return None;
        }
        let bin = target.spawn.clone()?;
        if self
            .spawned_at
            .is_some_and(|at| at.elapsed() < SPAWN_AGAIN_AFTER)
        {
            return None;
        }
        self.spawned_at = Some(Instant::now());
        Some(bin)
    }
}

/// One connection. Returns `Ok` when the app is gone, and `Err` with a
/// reason when the connection drops.
async fn session(
    target: &Target,
    requests: &mut tmpsc::UnboundedReceiver<Request>,
    events: &Events,
    state: &mut State,
) -> Result<(), String> {
    let client = Connect {
        data_dir: target.data_dir.clone(),
        // The default context is the human owner.
        context: ClientContext {
            client_version: Some(llmtodo_client::types::VERSION.to_string()),
            ..Default::default()
        },
        spawn: state.spawn(target),
        // The app never stops a running server; the CLI does upgrades.
        upgrade_old_server: false,
    }
    .connect()
    .await
    .map_err(|e| e.to_string())?;
    let client = if target.read_only {
        client.read_only()
    } else {
        client
    };

    let mut live = live::Connection::open(&client, PING_EVERY).await?;
    // The server subscribes before it says hello, so no event between the
    // hello and the snapshot below is lost.
    let hello_seq = live.hello_seq;

    send(
        events,
        Event::Connected {
            url: client.base_url().to_string(),
        },
    );
    state.connected_once = true;
    state.connected = true;
    state.spawned_at = None;
    // A pull or push on this machine reopens the database, which drops the
    // live connection, so a reconnect is the moment the role can change.
    primary(&client, events).await?;

    match state.last_seq {
        Some(after) if after <= hello_seq => {
            catch_up(&client, after, &mut state.pending).await?;
        }
        // First connect, or a new database with fewer events.
        _ => state.pending.reload = true,
    }
    flush(&client, &mut state.pending, state.open, events).await?;
    state.last_seq = Some(hello_seq);

    // Refreshes run on their own task, so the owner's writes never wait
    // behind them. One runs at a time.
    let mut flushing: Option<FlushTask> = None;
    // When the work in `pending` is due. It is set when the first event
    // arrives and never moved, so a steady stream of events still flushes.
    let mut due: Option<tokio::time::Instant> = None;
    loop {
        if state.pending.is_empty() {
            due = None;
        } else if due.is_none() {
            due = Some(tokio::time::Instant::now() + SETTLE);
        }
        let settle = tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now));
        tokio::select! {
            // The owner's requests come first.
            biased;
            r = requests.recv() => match r {
                None => return Ok(()),
                Some(Request::Reload) => snapshot(&client, events).await?,
                Some(Request::Fetch { key }) => {
                    if let Err(e) = refresh(&client, &key, events).await {
                        state.pending.keys.insert(key);
                        return Err(e);
                    }
                }
                Some(Request::Open { id }) => state.open = id,
                Some(Request::Write { key, write, kind }) => {
                    let open = state.open;
                    if let Err(e) = apply_write(&client, &key, write, kind, open, events).await {
                        state.pending.keys.insert(key);
                        return Err(e);
                    }
                }
                Some(Request::ViewIssues { name }) => view_issues(&client, name, events).await?,
                Some(Request::ProbeTeams { data_dir }) => probe_teams(data_dir, events),
                Some(Request::SyncStatus) => primary(&client, events).await?,
                Some(Request::Document { id }) => match client.document(&id.to_string()).await {
                    Ok(doc) => send(events, Event::Document(Box::new(doc))),
                    Err(ClientError::Unreachable(e)) => return Err(e),
                    Err(e) => send(events, Event::DocumentFailed {
                        id,
                        message: e.to_string(),
                        stale: false,
                    }),
                },
                Some(Request::SaveDocument { id, body, base, force }) => {
                    let update = UpdateDocument {
                        title: None,
                        body: BodyChange {
                            body: Some(body),
                            base_version: (!force).then_some(base),
                            force,
                            patches: Vec::new(),
                        },
                    };
                    match client.update_document(&id.to_string(), &update).await {
                        Ok(doc) => send(events, Event::DocumentSaved(Box::new(doc))),
                        Err(ClientError::Unreachable(e)) => {
                            send(events, Event::DocumentFailed {
                                id,
                                message: "the server is offline, so nothing was saved".into(),
                                stale: false,
                            });
                            return Err(e);
                        }
                        Err(e) => send(events, Event::DocumentFailed {
                            id,
                            stale: e.code() == ErrorCode::StaleVersion,
                            message: e.to_string(),
                        }),
                    }
                }
                Some(Request::History { key }) => {
                    let (client, events) = (client.clone(), events.clone());
                    // Its own task, like a flush. A lost connection ends
                    // the session through the live socket.
                    tokio::spawn(async move { history(&client, key, &events).await });
                }
                Some(Request::Create(c)) => match client.create_issue(&c).await {
                    Ok(detail) => {
                        let (id, key) = (detail.issue.id, detail.issue.key.to_string());
                        send_detail(&client, detail, events).await;
                        send(events, Event::Created { id, key });
                    }
                    Err(ClientError::Unreachable(e)) => {
                        send(events, Event::CreateFailed {
                            title: c.title.clone(),
                            message: "the server is offline, so nothing was created".into(),
                        });
                        return Err(e);
                    }
                    Err(e) => {
                        let not_primary = e.code() == ErrorCode::NotPrimary;
                        send(events, Event::CreateFailed {
                            title: c.title.clone(),
                            message: e.to_string(),
                        });
                        if not_primary {
                            primary(&client, events).await?;
                        }
                    }
                },
                Some(Request::CreateView(c)) => {
                    let name = c.name.clone();
                    let event = match client.create_view(&c).await {
                        Ok(_) => Event::ViewSaved { name },
                        Err(ClientError::Unreachable(e)) => return Err(e),
                        Err(e) => Event::ViewFailed { name, message: e.to_string() },
                    };
                    send(events, event);
                }
                Some(Request::DeleteView { name }) => {
                    let event = match client.delete_view(&name).await {
                        Ok(_) => Event::ViewDeleted { name },
                        Err(ClientError::Unreachable(e)) => return Err(e),
                        Err(e) => Event::ViewFailed { name, message: e.to_string() },
                    };
                    send(events, event);
                }
                Some(Request::SetProjectColor { slug, color }) => {
                    let update = llmtodo_client::types::UpdateProject {
                        color: Some(color.unwrap_or_default()),
                        ..Default::default()
                    };
                    match client.update_project(&slug, &update).await {
                        Ok(_) => {}
                        Err(ClientError::Unreachable(e)) => return Err(e),
                        Err(e) => send(events, Event::ProjectFailed { slug, message: e.to_string() }),
                    }
                }
            },
            done = async { (&mut flushing.as_mut().expect("a running flush").0).await },
                if flushing.is_some() =>
            {
                flushing = None;
                match done {
                    // A failed flush leaves its work in `state.flushing`,
                    // and the next connect does it.
                    Ok(Ok(())) => state.flushing = None,
                    Ok(Err(e)) => return Err(e),
                    Err(e) => return Err(format!("refresh task: {e}")),
                }
            }
            signal = live.next() => match signal? {
                live::Signal::Event(event) => {
                    state.last_seq = Some(event.seq);
                    state.pending.add(&event);
                }
                live::Signal::Reload => state.pending.reload = true,
            },
            _ = settle, if due.is_some() && flushing.is_none() => {
                due = None;
                let mut work = std::mem::take(&mut state.pending);
                state.flushing = Some(work.clone());
                let (client, events, open) = (client.clone(), events.clone(), state.open);
                #[cfg(test)]
                let slow = target.slow_flush;
                flushing = Some(FlushTask(tokio::spawn(async move {
                    #[cfg(test)]
                    tokio::time::sleep(slow).await;
                    flush(&client, &mut work, open, &events).await
                })));
            }
        }
    }
}

/// Sends an issue's field changes and description versions, without the
/// text of each version. A trashed or moved issue sends nothing: the next
/// open asks again.
async fn history(client: &Client, key: String, events: &Events) {
    if let Ok(history) = client.issue_history(&key, &HistoryQuery::default()).await {
        send(
            events,
            Event::History {
                key,
                history: Box::new(history),
            },
        );
    }
}

/// Asks the server of the tracker in `data_dir` for its teams, on its own
/// task, so a slow server does not hold up this one.
fn probe_teams(data_dir: PathBuf, events: &Events) {
    let events = events.clone();
    tokio::spawn(async move {
        let context = ClientContext {
            client_version: Some(llmtodo_client::types::VERSION.to_string()),
            ..Default::default()
        };
        let teams = match llmtodo_client::find_server(&data_dir, &context).await {
            Some((client, _)) => client
                .teams()
                .await
                .ok()
                .map(|teams| teams.into_iter().map(|t| t.key).collect()),
            None => None,
        };
        send(&events, Event::Teams { data_dir, teams });
    });
}

/// The activity fields that change what a dependent task shows.
const CHANGES_DEPENDENTS: &[ActivityField] = &[
    ActivityField::Status,
    ActivityField::Title,
    ActivityField::Deleted,
    ActivityField::Restored,
    ActivityField::RelationAdded,
    ActivityField::RelationRemoved,
];

/// Issues touched by events that arrived close together.

#[derive(Default, Clone)]
struct Pending {
    keys: BTreeSet<String>,
    /// Issues whose change can move the depth of the tasks that wait for
    /// them. Those tasks get no event of their own.
    dependents_of: BTreeSet<String>,
    /// Parents whose order changed. Their sub-issues get a new step and
    /// new order edges but no event of their own.
    children_of: BTreeSet<String>,
    /// Statuses, labels, teams, projects or views changed.
    catalog: bool,
    reload: bool,
}

impl Pending {
    fn add(&mut self, event: &ActivityEvent) {
        match (&event.issue, &event.entity_type) {
            (Some(key), _) => {
                self.keys.insert(key.to_string());
                if CHANGES_DEPENDENTS.contains(&event.field) {
                    self.dependents_of.insert(key.to_string());
                }
                if event.field == ActivityField::Order {
                    self.children_of.insert(key.to_string());
                }
            }
            // Statuses, labels and teams change how every issue shows.
            // Projects, saved views and documents change the sidebar.
            (
                None,
                EntityType::Team
                | EntityType::Status
                | EntityType::Label
                | EntityType::Project
                | EntityType::View
                | EntityType::Document,
            ) => self.catalog = true,
            _ => {}
        }
    }

    fn is_empty(&self) -> bool {
        self.keys.is_empty()
            && self.dependents_of.is_empty()
            && self.children_of.is_empty()
            && !self.catalog
            && !self.reload
    }

    /// Adds the work in `other`.
    fn merge(&mut self, other: Pending) {
        self.keys.extend(other.keys);
        self.dependents_of.extend(other.dependents_of);
        self.children_of.extend(other.children_of);
        self.catalog |= other.catalog;
        self.reload |= other.reload;
    }
}

/// Reads the events missed while offline into `pending`. Asks for a full
/// reload when the gap is too old.
async fn catch_up(client: &Client, after: i64, pending: &mut Pending) -> Result<(), String> {
    let mut q = ActivityQuery {
        after_seq: Some(after),
        issue: None,
        limit: None,
    };
    live::catch_up(client, &mut q, |caught| match caught {
        live::Caught::Pruned => {
            pending.reload = true;
            ControlFlow::Break(())
        }
        live::Caught::Event(event) => {
            pending.add(event);
            ControlFlow::Continue(())
        }
    })
    .await
    .map(|_stopped| ())
    .map_err(|e| e.to_string())
}

/// Sends what `pending` asks for. Work is removed only once it is done, so
/// a failure partway leaves the rest for the next connect.
async fn flush(
    client: &Client,
    pending: &mut Pending,
    open: Option<IssueId>,
    events: &Events,
) -> Result<(), String> {
    if pending.reload {
        snapshot(client, events).await?;
        pending.reload = false;
        pending.catalog = false;
        pending.keys.clear();
        pending.dependents_of.clear();
        pending.children_of.clear();
        return Ok(());
    }
    if pending.catalog {
        catalog(client, events).await?;
        pending.catalog = false;
    }
    // An order change gives the parent's sub-issues new steps, depths and
    // open blockers. Each one then refreshes like a touched issue, and the
    // tasks that wait for it follow.
    while let Some(parent) = pending.children_of.first().cloned() {
        for key in children(client, &parent).await? {
            pending.keys.insert(key.clone());
            pending.dependents_of.insert(key);
        }
        pending.children_of.remove(&parent);
    }
    if !pending.keys.is_empty() {
        let keys: Vec<String> = pending.keys.iter().cloned().collect();
        let open_key = refresh_rows(client, keys, open, events).await?;
        pending.keys.clear();
        // Every event on an issue shows in its history, so the open task's
        // history follows each refresh of it, once per batch of events.
        if let Some(key) = open_key {
            history(client, key, events).await;
        }
    }
    while let Some(key) = pending.dependents_of.first().cloned() {
        dependents(client, &key, events).await?;
        pending.dependents_of.remove(&key);
    }
    Ok(())
}

/// The keys of the live sub-issues of `parent`. An unknown key has none.
async fn children(client: &Client, parent: &str) -> Result<Vec<String>, String> {
    let query = IssueQuery {
        parent: Some(parent.to_string()),
        order: Some(IssueOrder::Id),
        ..Default::default()
    };
    match client.query_all(&query).await {
        Ok(issues) => Ok(issues.into_iter().map(|i| i.key.to_string()).collect()),
        Err(ClientError::Unreachable(e)) => Err(e),
        Err(_) => Ok(Vec::new()),
    }
}

/// Sends fresh copies of the tasks that wait for `key`, and of the closed
/// tasks that `key` waits for through closed tasks. A change to `key` can
/// move the depth of the first and the `has_open_dependents` of the second
/// (LLM-214). An unknown key is skipped.
async fn dependents(client: &Client, key: &str, events: &Events) -> Result<(), String> {
    let down = IssueQuery {
        depends_on: Some(key.to_string()),
        ..Default::default()
    };
    let up = IssueQuery {
        closed_blockers_of: Some(key.to_string()),
        ..Default::default()
    };
    let mut issues = Vec::new();
    for query in [down, up] {
        match client.query_all(&query).await {
            Ok(found) => issues.extend(found),
            Err(ClientError::Unreachable(e)) => return Err(e),
            Err(_) => {}
        }
    }
    if !issues.is_empty() {
        send(events, Event::Dependents(issues));
    }
    Ok(())
}

async fn snapshot(client: &Client, events: &Events) -> Result<(), String> {
    // ID order: a write between two pages never moves an issue across the
    // page edge. The app sorts the list itself. Descriptions stay out: they
    // are most of a snapshot's size, and the app fetches one when a task
    // opens.
    let query = IssueQuery {
        order: Some(IssueOrder::Id),
        ..Default::default()
    };
    let trash = IssueQuery {
        trashed: true,
        ..query.clone()
    };
    let open_questions = QuestionQuery::default();
    let (catalog, mut issues, trashed, questions) = tokio::try_join!(
        catalog_data(client),
        client.query_all(&query),
        client.query_all(&trash),
        client.questions(&open_questions)
    )
    .map_err(|e| e.to_string())?;
    issues.extend(trashed);
    let issues = last_copy_of_each(issues, |i| i.key.to_string());
    send(
        events,
        Event::Snapshot(Box::new(
            Snapshot {
                catalog,
                issues,
                questions,
            }
            .load(),
        )),
    );
    Ok(())
}

async fn catalog_data(client: &Client) -> llmtodo_client::Result<CatalogData> {
    let (teams, labels, projects, views, documents) = tokio::try_join!(
        client.teams(),
        client.labels(),
        client.projects(false),
        client.views(),
        client.documents(None, false, None),
    )?;
    Ok(CatalogData {
        teams,
        labels,
        projects,
        views,
        documents,
    })
}

/// Sends the catalog alone. The app maps the issues it holds again.
async fn catalog(client: &Client, events: &Events) -> Result<(), String> {
    let data = catalog_data(client).await.map_err(|e| e.to_string())?;
    send(events, Event::Catalog(Box::new(data)));
    Ok(())
}

/// Sends whether this machine is the primary copy. A server that cannot
/// say, such as one with a broken sync folder, counts as the primary: the
/// writes themselves still fail with the reason.
async fn primary(client: &Client, events: &Events) -> Result<(), String> {
    let event = match client.sync_status().await {
        Ok(status) => Event::Primary {
            primary: status.is_primary,
            holder: status
                .remote
                .and_then(|m| m.owner_host)
                .filter(|h| *h != status.host),
        },
        Err(ClientError::Unreachable(e)) => return Err(e),
        Err(_) => Event::Primary {
            primary: true,
            holder: None,
        },
    };
    send(events, event);
    Ok(())
}

/// Sends the IDs of the issues a saved view holds, or why it failed. One
/// request reads every ID and no full issue, so the refresh after each issue
/// change stays cheap on a large view.
async fn view_issues(client: &Client, name: String, events: &Events) -> Result<(), String> {
    let query = IssueQuery {
        view: Some(name.clone()),
        ..Default::default()
    };
    let event = match client.query_ids(&query).await {
        Ok(ids) => Event::ViewIssues { name, ids },
        Err(ClientError::Unreachable(e)) => return Err(e),
        Err(e) => Event::ViewFailed {
            name,
            message: e.to_string(),
        },
    };
    send(events, event);
    Ok(())
}

/// Keeps one copy of each key, the one fetched last, in the original order.
fn last_copy_of_each<T, K: std::hash::Hash + Eq>(items: Vec<T>, key: impl Fn(&T) -> K) -> Vec<T> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<T> = items
        .into_iter()
        .rev()
        .filter(|i| seen.insert(key(i)))
        .collect();
    out.reverse();
    out
}

/// The host name the CLI stores with claims and work sessions, so the app
/// reads commits only for sessions that ran on this machine.
pub(crate) fn local_host() -> Option<String> {
    static HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        std::env::var("LLMTODO_HOST")
            .ok()
            .filter(|h| !h.trim().is_empty())
            .or_else(|| {
                std::process::Command::new("hostname")
                    .arg("-s")
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|h| !h.is_empty())
            })
    })
    .clone()
}

/// Sends the issue with its relations, reviews and git panel, as one
/// event. The commits come from each session's worktree, or from the
/// project folder once a worktree is gone. Git runs off the async runtime.
async fn send_detail(client: &Client, detail: IssueDetail, events: &Events) {
    let fallback = project_folder(client, &detail).await;
    let detail = with_commits(detail, fallback).await;
    send(
        events,
        Event::IssueDetail(Box::new(DetailParts {
            issue: detail.issue,
            relations: detail.relations,
            reviews: detail.reviews,
            git: detail.git,
            sessions: detail.sessions,
        })),
    );
}

/// The folder the git panel reads once a session's worktree is gone:
/// `LLMTODO_PROJECT_DIR` for a development run, else the local folder the
/// CLI saved on the issue's project. The app's working folder never
/// counts: an app started from the Dock runs in `/`.
async fn project_folder(client: &Client, detail: &IssueDetail) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LLMTODO_PROJECT_DIR") {
        return Some(PathBuf::from(dir));
    }
    let slug = detail.issue.project.as_deref()?;
    let path = client.project(slug).await.ok()?.local_path?;
    Some(PathBuf::from(path))
}

/// Fills in the commits of each session that ran on this machine, from its
/// worktree, or from `fallback` once the worktree is gone. Git runs off
/// the async runtime. A failed git run leaves the commits empty.
async fn with_commits(detail: IssueDetail, fallback: Option<PathBuf>) -> IssueDetail {
    let plain = detail.clone();
    tokio::task::spawn_blocking(move || {
        let mut detail = detail;
        llmtodo_client::git::fill_commits(
            &mut detail,
            local_host().as_deref(),
            fallback.as_deref(),
        );
        detail
    })
    .await
    .unwrap_or(plain)
}

/// Sends a fresh copy of one issue with its detail. An unknown key is
/// skipped.
async fn refresh(client: &Client, key: &str, events: &Events) -> Result<(), String> {
    match client.issue(key).await {
        Ok(detail) => {
            send_detail(client, detail, events).await;
            Ok(())
        }
        Err(ClientError::Unreachable(e)) => Err(e),
        Err(_) => Ok(()),
    }
}

/// Sends fresh list copies of the issues `keys` name, live or trashed, in
/// one query each. They carry their text, so the inbox reads new questions.
/// The open task gets its full detail instead. Unknown keys are skipped.
/// Returns the open task's key when it was among them.
async fn refresh_rows(
    client: &Client,
    keys: Vec<String>,
    open: Option<IssueId>,
    events: &Events,
) -> Result<Option<String>, String> {
    let live = IssueQuery {
        keys,
        include_description: true,
        order: Some(IssueOrder::Id),
        ..Default::default()
    };
    let trash = IssueQuery {
        trashed: true,
        ..live.clone()
    };
    let (mut rows, trashed) =
        match tokio::try_join!(client.query_all(&live), client.query_all(&trash)) {
            Ok(found) => found,
            Err(ClientError::Unreachable(e)) => return Err(e),
            Err(_) => return Ok(None),
        };
    rows.extend(trashed);
    let mut open_key = None;
    rows.retain(|issue| {
        let is_open = Some(issue.id) == open;
        if is_open {
            open_key = Some(issue.key.to_string());
        }
        !is_open
    });
    if !rows.is_empty() {
        send(events, Event::Issues(rows));
    }
    if let Some(key) = &open_key {
        refresh(client, key, events).await?;
    }
    Ok(open_key)
}

async fn apply_write(
    client: &Client,
    key: &str,
    write: Write,
    kind: WriteKind,
    open: Option<IssueId>,
    events: &Events,
) -> Result<(), String> {
    // A failed order write undoes the app's early steps on every task in it.
    let mut undo = vec![key.to_string()];
    if let Write::SetOrder(order) = &write {
        undo.extend(order.steps.iter().flatten().map(ToString::to_string));
    }
    let mut detail = None;
    let mut keep = |d: IssueDetail| {
        let issue = d.issue.clone();
        detail = Some(d);
        issue
    };
    let result = match write {
        Write::Update(update) => client.update_issue(key, &update).await.map(&mut keep),
        Write::UndoAnswer { id } => client.undo_answer(key, &id).await.map(&mut keep),
        Write::Review(review) => client.submit_review(key, &review).await.map(&mut keep),
        Write::AddRelation(relation) => client.add_relation(key, &relation).await.map(&mut keep),
        Write::RemoveRelation(relation) => {
            client.remove_relation(key, &relation).await.map(&mut keep)
        }
        // The live order event refreshes the sub-issues.
        Write::SetOrder(order) => match client.set_order(key, &order).await {
            Ok(_) => client.issue(key).await.map(&mut keep),
            Err(err) => Err(err),
        },
        Write::Trash => client.delete_issue(key).await,
        Write::Restore => client
            .restore_issue(key, &RestoreRequest::default())
            .await
            .map(|d| d.issue),
        // The owner frees a task an agent holds. Claims never time out, so
        // this is how a stale one ends.
        Write::Release => {
            client
                .release(
                    key,
                    &ReleaseRequest {
                        status: None,
                        force: true,
                    },
                )
                .await
        }
    };
    match result {
        Ok(issue) => {
            let version = issue.description_version;
            // Only the open task needs its detail and a git run.
            match detail {
                Some(d) if Some(d.issue.id) == open => send_detail(client, d, events).await,
                _ => send(events, Event::Issue(Box::new(issue))),
            }
            send(
                events,
                Event::WriteDone {
                    key: key.to_string(),
                    kind,
                    version,
                },
            );
            Ok(())
        }
        Err(err) => {
            let unreachable = matches!(err, ClientError::Unreachable(_));
            send(
                events,
                Event::WriteFailed {
                    key: key.to_string(),
                    kind,
                    message: err.to_string(),
                    stale: err.code() == ErrorCode::StaleVersion,
                },
            );
            if unreachable {
                return Err(err.to_string());
            }
            if err.code() == ErrorCode::NotPrimary {
                primary(client, events).await?;
            }
            // Undo the app's early change with the server's copy.
            refresh_rows(client, undo, open, events).await.map(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_keeps_the_last_copy_of_a_repeated_issue() {
        let pages = vec![("LLM-1", 1), ("LLM-2", 1), ("LLM-1", 2), ("LLM-3", 1)];
        let kept = last_copy_of_each(pages, |p| p.0);
        assert_eq!(kept, [("LLM-2", 1), ("LLM-1", 2), ("LLM-3", 1)]);
    }

    #[test]
    fn backoff_stays_quick_for_a_few_tries_and_then_slows_down() {
        let mut b = Backoff::default();
        // A server that is still starting is found quickly.
        for _ in 0..QUICK_TRIES {
            assert_eq!(b.next(false, false), FIRST_DELAY);
        }
        // With no server at all, the wait grows to the longest one and
        // stays there, instead of 4 tries a second forever.
        let waits: Vec<Duration> = (0..8).map(|_| b.next(false, false)).collect();
        assert_eq!(waits[0], FIRST_DELAY);
        assert_eq!(waits[1], FIRST_DELAY * 2);
        assert!(waits.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*waits.last().unwrap(), MAX_DELAY);
        // The first connect still starts over.
        assert_eq!(b.next(true, true), FIRST_DELAY);
    }

    #[test]
    fn the_app_hears_an_offline_reason_once() {
        let mut state = State::default();
        assert!(state.offline("refused".into()).is_some());
        assert!(state.offline("refused".into()).is_none());
        assert!(state.offline("timed out".into()).is_some());
        // After a connect, the same reason counts as news again.
        state.connected = true;
        assert!(state.offline("timed out".into()).is_some());
        state.connected = false;
        assert!(state.offline("timed out".into()).is_none());
    }

    #[test]
    fn backoff_starts_over_after_a_connect() {
        let mut b = Backoff::default();
        // Before the first connect, retries stay quick.
        assert_eq!(b.next(false, false), FIRST_DELAY);
        assert_eq!(b.next(false, false), FIRST_DELAY);
        // Short drops after a connect double the wait.
        assert_eq!(b.next(true, true), FIRST_DELAY);
        for _ in 0..6 {
            b.next(false, true);
        }
        assert_eq!(b.next(false, true), MAX_DELAY);
        // A session that connects again resets it.
        assert_eq!(b.next(true, true), FIRST_DELAY);
        assert_eq!(b.next(false, true), FIRST_DELAY);
    }

    #[test]
    fn retries_start_one_server_until_a_connect() {
        let target = Target {
            data_dir: PathBuf::from("/nowhere"),
            spawn: Some(PathBuf::from("/bin/llmtodo")),
            read_only: false,
            slow_flush: Duration::ZERO,
        };
        let mut state = State::default();
        assert!(state.spawn(&target).is_some());
        assert!(state.spawn(&target).is_none());
        assert!(state.spawn(&target).is_none());
        // After a connect, a later crash may start a server again.
        state.spawned_at = None;
        assert!(state.spawn(&target).is_some());
    }

    #[test]
    fn a_read_only_target_never_starts_a_server() {
        let target = Target {
            data_dir: PathBuf::from("/nowhere"),
            spawn: Some(PathBuf::from("/bin/llmtodo")),
            read_only: true,
            slow_flush: Duration::ZERO,
        };
        assert!(State::default().spawn(&target).is_none());
    }
}
