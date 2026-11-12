//! Request and response types shared by the server and its clients.
//!
//! Every type here derives `JsonSchema`, so `llmtodo schema` and the route
//! table in [`routes`] can describe the whole API to agents.

mod activity;
mod change;
pub mod routes;

pub use activity::{ActivityField, EntityType};
pub use change::Change;
pub use routes::{Endpoint, ep};

use std::collections::BTreeMap;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use llmtodo_core::{
    ASSIGNEE_NONE, ASSIGNEE_OWNER, ActorFields, ActorId, ActorKind, AssigneeRef, AttachmentId,
    AttachmentKind, CommentId, DocumentId, FilterSubject, IssueFilter, IssueId, IssueKey, LabelId,
    MilestoneId, Patch, Priority, ProjectId, ProjectState, RelationId, RelationInput, RelationType,
    SessionNoteId, StatusId, StatusType, TeamId, ViewId, WaitingOn, WantedExecutor, WorkSessionId,
};

/// Version of this build. The server sends it in the `x-llmtodo-version`
/// header on every response.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const VERSION_HEADER: &str = "x-llmtodo-version";
/// Base64url JSON of [`ClientContext`], sent on every request.
pub const CONTEXT_HEADER: &str = "x-llmtodo-context";
pub const DEFAULT_PORT: u16 = 7878;

// ---------------------------------------------------------------- errors

/// A stable error code. The CLI maps each one to an exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Internal,
    BadRequest,
    NotFound,
    ClaimHeld,
    StaleVersion,
    Validation,
    NotPrimary,
    PatchMismatch,
    Forbidden,
    Conflict,
    /// The client could not reach the server and could not start one.
    /// Only clients report it.
    Unreachable,
    /// The command needs an answer, but `--no-input` is set or there is no
    /// terminal. Only the CLI reports it.
    InputNeeded,
}

impl ErrorCode {
    /// Every code, in exit code order.
    pub const ALL: [ErrorCode; 12] = [
        ErrorCode::Internal,
        ErrorCode::BadRequest,
        ErrorCode::NotFound,
        ErrorCode::ClaimHeld,
        ErrorCode::StaleVersion,
        ErrorCode::Validation,
        ErrorCode::Conflict,
        ErrorCode::Unreachable,
        ErrorCode::InputNeeded,
        ErrorCode::NotPrimary,
        ErrorCode::PatchMismatch,
        ErrorCode::Forbidden,
    ];

    /// The snake_case name used in error bodies, such as `claim_held`.
    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Exit codes from the spec. They never change.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Internal => 1,
            ErrorCode::BadRequest => 2,
            ErrorCode::NotFound => 3,
            ErrorCode::ClaimHeld => 4,
            ErrorCode::StaleVersion => 5,
            ErrorCode::Validation | ErrorCode::Conflict => 6,
            ErrorCode::Unreachable => 7,
            ErrorCode::InputNeeded => 8,
            ErrorCode::NotPrimary => 9,
            ErrorCode::PatchMismatch => 10,
            ErrorCode::Forbidden => 11,
        }
    }

    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::Internal => 500,
            ErrorCode::BadRequest | ErrorCode::InputNeeded => 400,
            ErrorCode::NotFound => 404,
            ErrorCode::ClaimHeld | ErrorCode::StaleVersion | ErrorCode::Conflict => 409,
            ErrorCode::Validation | ErrorCode::PatchMismatch => 422,
            ErrorCode::NotPrimary | ErrorCode::Unreachable => 503,
            ErrorCode::Forbidden => 403,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorDetail {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

/// The body of every error response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

// ---------------------------------------------------------------- identity

/// Who is calling and from where. Clients send it on every request in the
/// [`CONTEXT_HEADER`] header. Writes from an agent update its work log.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClientContext {
    pub actor: ActorInput,
    pub session_id: Option<String>,
    /// True when the CLI made up the session ID for this one call.
    #[serde(default)]
    pub session_generated: bool,
    pub host: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    /// The `HEAD` commit at the time of the call.
    pub head: Option<String>,
    pub transcript_path: Option<String>,
    pub herdr_pane: Option<String>,
    pub harness_version: Option<String>,
    pub client_version: Option<String>,
    /// Cumulative token usage of the whole transcript, by model. Agent
    /// clients send it; the server attributes what is new to the ticket
    /// being written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<BTreeMap<String, TokenUsage>>,
    /// Cumulative active working time of the whole transcript, in seconds.
    /// The server gives what is new to the ticket being written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActorInput {
    pub kind: ActorKind,
    /// Only for human actors other than the owner, such as people imported
    /// from Linear. Leave empty for the owner.
    pub name: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl Default for ActorInput {
    fn default() -> Self {
        Self {
            kind: ActorKind::Human,
            name: None,
            harness: None,
            model: None,
            effort: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Actor {
    pub id: ActorId,
    pub kind: ActorKind,
    /// Display name: `owner`, `github`, or `claude-code/opus-5.5/high`.
    pub name: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub is_owner: bool,
}

// ---------------------------------------------------------------- lists

/// The most issues one page of an issue query returns.
pub const MAX_ISSUE_PAGE: u32 = 1000;

/// One page of a list. Pass `next_cursor` back as `cursor` for the next page.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PageParams {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------- server

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    pub version: String,
    pub pid: u32,
    pub data_dir: String,
    pub schema_version: i64,
    pub is_primary: bool,
    pub started_at: Timestamp,
}

/// Contents of `server.json` in the data folder.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ServerInfo {
    pub port: u16,
    pub pid: u32,
    pub version: String,
    pub started_at: Timestamp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    pub workspace_name: String,
    pub default_team: Option<String>,
    pub github_poll_seconds: u32,
    pub sync_folder: Option<String>,
    /// USD per million tokens, by model ID. Seeded with list prices.
    pub model_prices: BTreeMap<String, ModelPrice>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSettings {
    pub workspace_name: Option<String>,
    pub default_team: Option<String>,
    pub github_poll_seconds: Option<u32>,
    pub sync_folder: Option<String>,
    /// Prices to add or replace, by model ID.
    pub model_prices: Option<BTreeMap<String, ModelPrice>>,
}

// ---------------------------------------------------------------- teams

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    pub id: StatusId,
    pub team: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: StatusType,
    pub color: String,
    pub position: i64,
    /// Tickets here wait for the owner's review. A move here needs a review
    /// section and ends the agent's claim.
    #[serde(default)]
    pub review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Team {
    pub id: TeamId,
    pub key: String,
    pub name: String,
    pub statuses: Vec<Status>,
    /// Status the GitHub sync sets when a PR opens. None turns the move off.
    pub pr_open_status: Option<String>,
    pub pr_merged_status: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateTeam {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateTeam {
    pub name: Option<String>,
    /// Status name, or empty string to turn the move off.
    pub pr_open_status: Option<String>,
    pub pr_merged_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateStatus {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: StatusType,
    pub color: Option<String>,
    pub position: Option<i64>,
    /// Marks a review status. False when left out.
    #[serde(default)]
    pub review: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateStatus {
    pub name: Option<String>,
    pub color: Option<String>,
    pub position: Option<i64>,
    /// Sets or clears the review flag.
    #[serde(default)]
    pub review: Option<bool>,
}

// ---------------------------------------------------------------- labels

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Label {
    pub id: LabelId,
    pub name: String,
    pub group: Option<String>,
    /// `Group/name` for grouped labels, `name` otherwise. Accepted wherever a
    /// label is named.
    pub full_name: String,
    pub color: String,
    pub description: Option<String>,
    pub is_group: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateLabel {
    pub name: String,
    /// Name of the group this label belongs to. The group must exist.
    pub group: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
    /// Create a label group instead of a label.
    #[serde(default)]
    pub is_group: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateLabel {
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
}

// ---------------------------------------------------------------- projects

/// Options for the project list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProjectQuery {
    /// List the trash instead of live projects.
    #[serde(default)]
    pub trashed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub id: ProjectId,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub state: ProjectState,
    pub teams: Vec<String>,
    pub repos: Vec<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub deleted_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateProject {
    pub name: String,
    /// Short name used on the command line. Made from the name when empty.
    pub slug: Option<String>,
    #[serde(default)]
    pub description: String,
    pub state: Option<ProjectState>,
    #[serde(default)]
    pub teams: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateProject {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub state: Option<ProjectState>,
    pub add_teams: Option<Vec<String>>,
    pub remove_teams: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoLink {
    /// `owner/name`
    pub repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Milestone {
    pub id: MilestoneId,
    pub project: String,
    pub name: String,
    pub description: String,
    pub target_date: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateMilestone {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `YYYY-MM-DD`
    pub target_date: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateMilestone {
    pub name: Option<String>,
    pub description: Option<String>,
    pub target_date: Option<String>,
    pub position: Option<i64>,
}

// ---------------------------------------------------------------- documents

/// Options for the document list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DocumentQuery {
    /// List the trash instead of live documents.
    #[serde(default)]
    pub trashed: bool,
    pub project: Option<String>,
    /// Text search over titles and bodies, best match first.
    pub search: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    pub id: DocumentId,
    pub project: String,
    pub slug: String,
    pub title: String,
    /// Left out of lists. Present when one document is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub deleted_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateDocument {
    pub project: String,
    pub title: String,
    pub slug: Option<String>,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateDocument {
    pub title: Option<String>,
    #[serde(flatten)]
    pub body: BodyChange,
}

/// A change to a Markdown body: a full replace or a patch.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BodyChange {
    /// New full text. Needs `base_version`, or `force`.
    pub body: Option<String>,
    pub base_version: Option<i64>,
    #[serde(default)]
    pub force: bool,
    /// Patches applied in order. A patch needs no version.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<Patch>,
}

impl BodyChange {
    pub fn is_empty(&self) -> bool {
        self.body.is_none() && self.patches.is_empty()
    }
}

// ---------------------------------------------------------------- issues

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusRef {
    pub id: StatusId,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: StatusType,
    pub color: String,
    /// A review status: the ticket waits for the owner's review.
    #[serde(default)]
    pub review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClaimSummary {
    pub actor: Actor,
    pub session_id: String,
    pub host: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub herdr_pane: Option<String>,
    pub claimed_at: Timestamp,
}

/// An agent reads or fills in an issue without a claim. It never blocks
/// other writers. It ends with `issue analyze --done`, on a claim, or when
/// the session makes no write to the issue for [`ANALYZING_EXPIRY_SECS`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzingSummary {
    pub actor: Actor,
    pub session_id: String,
    pub started_at: Timestamp,
    /// The last write by this session to the issue.
    pub seen_at: Timestamp,
    /// When the mark ends unless the session writes to the issue again.
    pub expires_at: Timestamp,
}

/// How long an Analyzing mark lasts after the session's last write to the
/// issue: 15 minutes.
pub const ANALYZING_EXPIRY_SECS: i64 = 15 * 60;

/// An issue as lists show it. `description` is present only when it was
/// asked for, or when one issue is read.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    pub id: IssueId,
    pub key: IssueKey,
    pub team: String,
    pub title: String,
    pub status: StatusRef,
    pub priority: Priority,
    pub assignee: Option<Actor>,
    pub labels: Vec<String>,
    pub project: Option<String>,
    pub milestone: Option<String>,
    pub parent: Option<IssueKey>,
    pub wanted_executor: WantedExecutor,
    /// `agent` when it is ready for an agent, `owner` when it needs the
    /// owner's input.
    pub waiting_on: Option<WaitingOn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub description_version: i64,
    pub claim: Option<ClaimSummary>,
    /// Present while an agent analyzes the issue without a claim.
    #[serde(default)]
    pub analyzing: Option<AnalyzingSummary>,
    /// The longest chain of open blockers. A closed or trashed issue has 0.
    #[serde(default)]
    pub depth: u32,
    /// The open, live issues that block this one directly.
    #[serde(default)]
    pub open_blockers: Vec<IssueLink>,
    /// The token cost of the issue so far, in USD. Missing when the issue
    /// has no usage, or when no model it used has a price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// How many owner questions in the description have no answer.
    #[serde(default)]
    pub open_questions: u32,
    /// Active agent working time so far, added up over all sessions. Missing
    /// when the issue has no time data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_seconds: Option<u64>,
    /// Estimated agent working time, set before work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_minutes: Option<u32>,
    /// Estimated token cost in USD, set before work starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_cost_usd: Option<f64>,
    pub created_by: Option<Actor>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub deleted_at: Option<Timestamp>,
    /// Search rank, present on text and similar searches. Higher is better.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// Filters of the owner question list. Without `issue`, the list covers open
/// issues, and `project` narrows it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct QuestionQuery {
    pub issue: Option<String>,
    pub project: Option<String>,
    /// Include answered questions.
    #[serde(default)]
    pub all: bool,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// One owner question with the issue it sits on.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QuestionEntry {
    pub key: IssueKey,
    pub title: String,
    pub waiting_on: Option<WaitingOn>,
    #[serde(flatten)]
    pub question: llmtodo_core::Question,
}

/// The owner's answer to one review round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
}

impl ReviewVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewVerdict::Approved => "approved",
            ReviewVerdict::ChangesRequested => "changes_requested",
        }
    }
}

impl std::str::FromStr for ReviewVerdict {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "approved" | "approve" => Ok(ReviewVerdict::Approved),
            "changes_requested" | "request_changes" | "changes" => {
                Ok(ReviewVerdict::ChangesRequested)
            }
            other => Err(format!(
                "unknown verdict {other:?}; use approve or request-changes"
            )),
        }
    }
}

/// One answer to a review round, as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAnswer {
    /// 1 for the first answer on the ticket, then 2, 3 ...
    pub round: i64,
    pub verdict: ReviewVerdict,
    /// Empty when none was given.
    pub feedback: String,
    pub author: Option<Actor>,
    pub created_at: Timestamp,
}

/// The review answer to send. Feedback is required with
/// `changes_requested`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubmitReview {
    pub verdict: ReviewVerdict,
    #[serde(default)]
    pub feedback: String,
}

/// The review state of one ticket.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewState {
    pub key: IssueKey,
    pub title: String,
    pub status: String,
    pub waiting_on: Option<WaitingOn>,
    /// The `+++ Review` section, or None when the description has none.
    pub section: Option<llmtodo_core::Review>,
    /// Every answer, oldest first.
    pub answers: Vec<ReviewAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueLink {
    pub key: IssueKey,
    pub title: String,
    pub status: StatusRef,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Relation {
    pub id: RelationId,
    /// As seen from the issue being read: blocks, blocked-by, related,
    /// duplicate (this issue duplicates the other) or duplicated-by.
    pub direction: String,
    #[serde(rename = "type")]
    pub kind: RelationType,
    pub issue: IssueLink,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    pub id: AttachmentId,
    pub kind: AttachmentKind,
    pub url: String,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    /// `manual`, `github` or `linear`.
    pub source: String,
    /// Extra data, such as PR state.
    pub metadata: serde_json::Value,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Comment {
    pub id: CommentId,
    pub issue: IssueKey,
    pub author: Option<Actor>,
    pub body: String,
    pub version: i64,
    pub parent_id: Option<CommentId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkSession {
    pub id: WorkSessionId,
    pub session_id: String,
    pub issue: IssueKey,
    pub actor: Actor,
    pub harness_version: Option<String>,
    pub transcript_path: Option<String>,
    pub herdr_pane: Option<String>,
    pub host: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub first_head: Option<String>,
    pub last_head: Option<String>,
    /// Filled by the CLI from the local repo: the commits from `first_head`
    /// to `last_head` that the session made, not the ones it pulled in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    pub started_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub notes: Vec<SessionNote>,
    pub usage: UsageSummary,
    /// Active working time of this session on this ticket. Missing when
    /// the session has no time data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionNote {
    pub id: SessionNoteId,
    pub session_id: String,
    pub issue: Option<IssueKey>,
    pub body: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSessionNote {
    pub body: String,
    /// Tie the note to one ticket. Without it, the note goes on every ticket
    /// the session touched.
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionView {
    pub session_id: String,
    pub actor: Option<Actor>,
    pub issues: Vec<WorkSession>,
    pub notes: Vec<SessionNote>,
}

/// One issue with everything an agent needs, in one call.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueDetail {
    #[serde(flatten)]
    pub issue: Issue,
    pub aliases: Vec<String>,
    pub children: Vec<IssueLink>,
    pub relations: Vec<Relation>,
    pub attachments: Vec<Attachment>,
    pub comments: Vec<Comment>,
    pub sessions: Vec<WorkSession>,
    /// Token usage of every agent session on this ticket.
    pub usage: UsageSummary,
    /// The branch, worktree, PR and commits of this issue.
    #[serde(default)]
    pub git: IssueGit,
    /// The owner's review answers, oldest first.
    #[serde(default)]
    pub reviews: Vec<ReviewAnswer>,
}

/// Git details of an issue. They stay after the claim ends.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IssueGit {
    /// A claim sets it, and `issue update --branch` changes it.
    pub branch: Option<String>,
    /// The worktree path on `host`.
    pub worktree: Option<String>,
    pub host: Option<String>,
    /// The PR set with `issue update --pr`, or else the newest PR attachment.
    pub pr: Option<GitPr>,
    /// The latest `HEAD` an agent session sent for this issue.
    pub head: Option<String>,
    /// The issue's commits as `<short hash> <subject>`, newest first. The
    /// client fills them from a local repo; the server leaves them empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    /// The repo's web page, such as `https://github.com/owner/name`, read
    /// from the local repo's `origin` remote. The client fills it; the
    /// server leaves it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GitPr {
    pub url: String,
    pub title: Option<String>,
    pub number: Option<i64>,
    /// `open`, `draft`, `merged` or `closed`, as the GitHub sync last saw it.
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateIssue {
    /// Team key. Falls back to the default team.
    pub team: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Status name. Defaults to the team's first unstarted status, or the
    /// first backlog status when `triage` is set.
    pub status: Option<String>,
    pub priority: Option<Priority>,
    /// `me`, `owner`, an actor name or ID.
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub project: Option<String>,
    pub milestone: Option<String>,
    pub parent: Option<String>,
    pub wanted_executor: Option<WantedExecutor>,
    #[serde(default)]
    pub waiting_on: Option<WaitingOn>,
    /// Relations to add at once, such as `{"blocked-by": ["ALX-1"]}`.
    #[serde(default)]
    pub relations: BTreeMap<RelationInput, Vec<String>>,
    /// Estimated agent working time. A claim needs this and the cost estimate.
    #[serde(default)]
    pub estimate_minutes: Option<u32>,
    /// Estimated token cost in USD.
    #[serde(default)]
    pub estimate_cost_usd: Option<f64>,
}

/// Every field is optional. Only the fields that are set change.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub status: Option<String>,
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    /// `me`, `owner`, an actor name or ID.
    pub assignee: Change<String>,
    /// Replaces the whole label set.
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    pub project: Change<String>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    pub milestone: Change<String>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    pub parent: Change<String>,
    /// Moves the issue to another team. It gets a new key, and the old key
    /// stays as an alias.
    pub team: Option<String>,
    pub wanted_executor: Option<WantedExecutor>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    pub waiting_on: Change<WaitingOn>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    /// The git branch.
    pub branch: Change<String>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    /// The worktree path. The caller's host is stored with it. Clearing
    /// it clears both.
    pub worktree: Change<String>,
    #[serde(default, skip_serializing_if = "Change::is_keep")]
    /// The PR URL. It is also attached, so the GitHub sync follows it.
    /// Clearing it clears the link but keeps the attachment.
    pub pr: Change<String>,
    /// Filling in an empty estimate needs no reason. Changing one needs
    /// `estimate_reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_minutes: Option<u32>,
    /// Same rule as `estimate_minutes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_cost_usd: Option<f64>,
    /// The spec change that justifies changing a set estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_reason: Option<String>,
    #[serde(flatten)]
    pub description: BodyChange,
}

/// Filters for an issue list. Every filter that is set must match.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct IssueQuery {
    /// Only these issues, by key, old key or ID. Unknown ones are skipped,
    /// so one query refreshes many rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// Statuses, labels, the assignee and the LLM status. The app's list
    /// runs the same filter.
    #[serde(flatten)]
    pub filter: IssueFilter,
    /// `none` for top-level issues only.
    pub parent: Option<String>,
    pub milestone: Option<String>,
    pub priority: Option<Priority>,
    /// Issues whose wanted executor this caller satisfies, such as
    /// `kind=agent,model=opus-5.5`.
    pub for_executor: Option<WantedExecutor>,
    /// `agent`, `owner` or `none`.
    pub waiting_on: Option<String>,
    /// Open issues that wait for this issue, directly or through a chain.
    pub depends_on: Option<String>,
    /// Full-text search over titles, descriptions and comments.
    pub text: Option<String>,
    /// Duplicate search: issues whose title and description look like this
    /// text, ranked by score.
    pub similar: Option<String>,
    /// List the trash instead of live issues.
    #[serde(default)]
    pub trashed: bool,
    #[serde(default)]
    pub include_description: bool,
    /// A saved view to start from. The other filters narrow it.
    pub view: Option<String>,
    pub updated_after: Option<Timestamp>,
    /// The result order. Unset means priority, then last update, or score
    /// for a text search.
    pub order: Option<IssueOrder>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// The order of an issue query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueOrder {
    /// Priority, then last update. A text search ranks by score.
    Default,
    /// By ID, which is creation order. A write never moves an issue in this
    /// order, so a walk over every page sees each issue once.
    Id,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FrontierQuery {
    /// The map issue. Its open sub-issues are listed.
    pub issue: Option<String>,
    /// Instead of a map issue, every open issue in this project.
    pub project: Option<String>,
    /// How many levels of sub-issues to include. Default 1.
    pub depth: Option<u32>,
    pub for_executor: Option<WantedExecutor>,
    #[serde(default)]
    pub include_description: bool,
}

/// Options for the dependency tree of an issue.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DepsQuery {
    /// Hide closed issues. The tree then stops at them.
    #[serde(default)]
    pub open: bool,
}

/// One issue in a dependency tree. The root fills both lists. A node below
/// the root fills the list of its own direction.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DepNode {
    #[serde(flatten)]
    pub issue: IssueLink,
    /// The longest chain of open blockers of this issue.
    pub depth: u32,
    /// The issues this one waits for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<DepNode>,
    /// The issues that wait for this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<DepNode>,
    /// The tree leaves out what lies past this node, because the node shows
    /// earlier in the tree or the chain is very long.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClaimRequest {
    /// Take the issue from another session, and record who held it.
    #[serde(default)]
    pub steal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClaimResponse {
    pub issue: Issue,
    /// False when this session already held the claim.
    pub changed: bool,
    pub open_blockers: Vec<IssueLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The claim that was taken over, when `steal` was set.
    pub stolen_from: Option<ClaimSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzeRequest {
    /// End this session's mark instead of setting it.
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AnalyzeResponse {
    pub issue: Issue,
    /// False when nothing changed, such as `done` without a mark from this
    /// session.
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReleaseRequest {
    /// Status to move to after the release, such as Todo.
    pub status: Option<String>,
    /// Release even when another session holds the claim.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateComment {
    pub body: String,
    pub parent_id: Option<CommentId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateComment {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateRelation {
    #[serde(rename = "type")]
    pub kind: RelationInput,
    pub issue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemoveRelation {
    #[serde(rename = "type")]
    pub kind: RelationInput,
    pub issue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateAttachment {
    /// A URL or a local path.
    pub url: String,
    pub title: Option<String>,
    pub kind: Option<AttachmentKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Revision {
    pub version: i64,
    pub body: String,
    pub actor: Option<Actor>,
    pub session_id: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct History {
    pub revisions: Vec<Revision>,
    pub activity: Vec<ActivityEvent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RestoreRequest {
    /// Bring back this description revision. Without it, the issue leaves
    /// the trash.
    pub revision: Option<i64>,
}

// ---------------------------------------------------------------- activity

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActivityEvent {
    /// Grows by one with every event. Clients catch up from it.
    pub seq: i64,
    pub entity_type: EntityType,
    pub entity_id: Uuid,
    pub issue: Option<IssueKey>,
    /// `created`, `deleted`, `restored`, or the field that changed.
    pub field: ActivityField,
    pub old: Option<serde_json::Value>,
    pub new: Option<serde_json::Value>,
    pub actor: Option<Actor>,
    pub session_id: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ActivityQuery {
    pub after_seq: Option<i64>,
    pub issue: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActivityPage {
    pub events: Vec<ActivityEvent>,
    pub last_seq: i64,
    /// True when `after_seq` is older than the oldest kept event. The client
    /// should reload everything.
    pub reload: bool,
}

/// A message on the `/v1/ws` WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    Hello {
        last_seq: i64,
        version: String,
    },
    Event {
        event: Box<ActivityEvent>,
    },
    /// The database was swapped, such as by `sync pull`. Reload everything.
    Reload,
}

// ---------------------------------------------------------------- views

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SavedView {
    pub id: ViewId,
    pub name: String,
    pub query: IssueQuery,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateView {
    pub name: String,
    pub query: IssueQuery,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UpdateView {
    pub name: Option<String>,
    pub query: Option<IssueQuery>,
}

// ---------------------------------------------------------------- admin

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PruneRequest {
    /// Delete trashed issues that went to the trash before this time.
    pub before: Option<Timestamp>,
    /// Trim revisions older than this time. The newest revision of each body
    /// stays.
    pub revisions_before: Option<Timestamp>,
    /// Show what would go, and delete nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// The issues the owner confirmed from a dry run. When set, the prune
    /// fails unless the trashed issues it would delete are exactly these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_issues: Option<Vec<IssueKey>>,
    /// The revision count the owner confirmed from a dry run. When set, the
    /// prune fails unless the count is the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_revisions: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PruneReport {
    pub dry_run: bool,
    pub issues: Vec<IssueKey>,
    pub revisions: i64,
    pub backup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BackupReport {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActorList {
    pub actors: Vec<Actor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(ErrorCode::ClaimHeld.exit_code(), 4);
        assert_eq!(
            serde_json::to_string(&ErrorCode::PatchMismatch).unwrap(),
            "\"patch_mismatch\""
        );
    }

    #[test]
    fn update_issue_flattens_body_change() {
        let u: UpdateIssue = serde_json::from_value(serde_json::json!({
            "title": "x",
            "body": "new",
            "base_version": 3,
            "patches": [{"op": "replace_exact", "old": "a", "new": "b"}]
        }))
        .unwrap();
        assert_eq!(u.description.base_version, Some(3));
        assert_eq!(u.description.patches.len(), 1);
    }
}

// ---------------------------------------------------------------- sync

/// `manifest.json` in the sync folder. It is written last, so a folder
/// with a manifest always holds a complete copy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncManifest {
    /// The machine that holds the lock. None after `push --release`.
    pub owner_host: Option<String>,
    pub pushed_by: String,
    pub schema_version: i64,
    /// Goes up by one with every push.
    pub generation: i64,
    pub pushed_at: Timestamp,
    pub db_bytes: u64,
    /// Hex SHA-256 of the unpacked database. Pull refuses a file that does
    /// not match, because a synced folder can deliver the manifest before
    /// the database. None in manifests from older builds.
    #[serde(default)]
    pub db_sha256: Option<String>,
    pub client_version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SyncPushRequest {
    /// Clear the lock so another machine can pull without `take`. This
    /// machine becomes read-only.
    #[serde(default)]
    pub release: bool,
    /// Overrides the `sync_folder` setting.
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SyncPullRequest {
    /// Pull even while another machine holds the lock.
    #[serde(default)]
    pub take: bool,
    /// Drop this machine's unpushed writes.
    #[serde(default)]
    pub force: bool,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncStatus {
    pub host: String,
    pub folder: Option<String>,
    pub is_primary: bool,
    pub local_generation: i64,
    /// Writes since the last push or pull.
    pub unpushed_writes: bool,
    pub remote: Option<SyncManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncReport {
    pub manifest: SyncManifest,
    pub backup: Option<String>,
    pub files_copied: u64,
}

// ---------------------------------------------------------------- import

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LinearImportRequest {
    /// Linear API key. The CLI reads it from `LINEAR_API_KEY`.
    pub token: String,
    /// Team keys to import. Empty means every team the key can see.
    #[serde(default)]
    pub teams: Vec<String>,
    /// Count everything and report mapping problems, but write nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Skip downloading images from Linear.
    #[serde(default)]
    pub skip_files: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportCounts {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImportReport {
    pub dry_run: bool,
    /// Per kind: teams, statuses, labels, actors, projects, milestones,
    /// documents, issues, relations, attachments, comments, files.
    pub counts: BTreeMap<String, ImportCounts>,
    /// Things that did not map cleanly. The import still went on.
    pub problems: Vec<String>,
    pub backup: Option<String>,
    pub requests: u64,
}

// ---------------------------------------------------------------- github

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GithubReport {
    pub repos: Vec<String>,
    pub pull_requests_seen: u64,
    pub commits_seen: u64,
    pub attached: u64,
    /// Issue status moves, such as `ALX-12: In Progress -> In Review`.
    pub moves: Vec<String>,
    /// Why the sync is paused, such as a missing `gh` token.
    pub paused: Option<String>,
    pub last_poll_at: Option<Timestamp>,
}

// ---------------------------------------------------------------- usage

/// Token counts of one model, summed over API requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsage {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write_5m + self.cache_write_1h + self.cache_read
    }

    pub fn add(&mut self, o: &TokenUsage) {
        self.requests += o.requests;
        self.input += o.input;
        self.output += o.output;
        self.cache_write_5m += o.cache_write_5m;
        self.cache_write_1h += o.cache_write_1h;
        self.cache_read += o.cache_read;
    }

    /// `self - o`, or None when any count went down, such as after a new
    /// transcript started.
    pub fn since(&self, o: &TokenUsage) -> Option<TokenUsage> {
        Some(TokenUsage {
            requests: self.requests.checked_sub(o.requests)?,
            input: self.input.checked_sub(o.input)?,
            output: self.output.checked_sub(o.output)?,
            cache_write_5m: self.cache_write_5m.checked_sub(o.cache_write_5m)?,
            cache_write_1h: self.cache_write_1h.checked_sub(o.cache_write_1h)?,
            cache_read: self.cache_read.checked_sub(o.cache_read)?,
        })
    }

    pub fn is_zero(&self) -> bool {
        *self == TokenUsage::default()
    }
}

/// USD per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

impl ModelPrice {
    pub fn cost(&self, u: &TokenUsage) -> f64 {
        (u.input as f64 * self.input
            + u.output as f64 * self.output
            + u.cache_write_5m as f64 * self.cache_write_5m
            + u.cache_write_1h as f64 * self.cache_write_1h
            + u.cache_read as f64 * self.cache_read)
            / 1_000_000.0
    }
}

/// Usage by model, with totals. Cost is None when no model has a price.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UsageSummary {
    /// Keyed by model ID, such as `claude-opus-5-5`, or `claude-opus-5-5:fast`.
    pub by_model: BTreeMap<String, TokenUsage>,
    pub total_tokens: u64,
    pub requests: u64,
    pub cost_usd: Option<f64>,
    /// Models with usage but no price in settings. Their cost is left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpriced_models: Vec<String>,
}

impl UsageSummary {
    pub fn build(
        by_model: BTreeMap<String, TokenUsage>,
        prices: &BTreeMap<String, ModelPrice>,
    ) -> Self {
        let mut cost = None;
        let mut unpriced = Vec::new();
        for (m, u) in &by_model {
            match prices.get(m) {
                Some(p) => *cost.get_or_insert(0.0) += p.cost(u),
                None if !u.is_zero() => unpriced.push(m.clone()),
                None => {}
            }
        }
        Self {
            total_tokens: by_model.values().map(TokenUsage::total).sum(),
            requests: by_model.values().map(|u| u.requests).sum(),
            cost_usd: cost.map(|c: f64| (c * 10_000.0).round() / 10_000.0),
            unpriced_models: unpriced,
            by_model,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct UsageQuery {
    pub project: Option<String>,
    pub issue: Option<String>,
    pub session: Option<String>,
}

/// Active working time as clients show it, such as `45s`, `12m` or `1h 05m`.
pub fn format_active_time(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IssueUsage {
    pub key: IssueKey,
    pub title: String,
    pub usage: UsageSummary,
    /// Missing when the issue has no time data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UsageReport {
    pub total: UsageSummary,
    /// Missing when no issue in the report has time data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_active_seconds: Option<u64>,
    /// Most expensive first.
    pub issues: Vec<IssueUsage>,
}
