//! The route table: every API endpoint, its types, and the CLI command that
//! covers it.
//!
//! Each endpoint is a type in [`ep`] that implements [`Endpoint`]. The
//! [`ROUTES`] list is generated from the same entries. The server mounts its
//! handlers by endpoint type and checks at startup that it serves exactly
//! these routes. The client sends every request through one generic call
//! that takes an endpoint type. A CLI test checks that every `cli` entry is
//! a real command. `llmtodo schema` prints this table with JSON schemas.

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    /// Reads a method name in any case, such as `get` or `PATCH`.
    pub fn parse(s: &str) -> Option<Method> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(Method::Get),
            "POST" => Some(Method::Post),
            "PUT" => Some(Method::Put),
            "PATCH" => Some(Method::Patch),
            "DELETE" => Some(Method::Delete),
            _ => None,
        }
    }

    /// True for the methods that send their request as a query string.
    pub fn has_query(self) -> bool {
        matches!(self, Method::Get | Method::Delete)
    }
}

pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

#[derive(Clone, Copy)]
pub struct Route {
    pub method: Method,
    /// axum path syntax, such as `/v1/issues/{key}`.
    pub path: &'static str,
    pub summary: &'static str,
    /// The JSON body, or the query string for GET and DELETE.
    pub request: Option<SchemaFn>,
    pub response: Option<SchemaFn>,
    /// The CLI command that calls this route, such as `issue view`.
    pub cli: &'static str,
    /// True when the route may change data. Usage goes only with writes,
    /// and a read-only client refuses them.
    pub writes: bool,
    /// True when the route does long work on the server, such as import,
    /// sync, GitHub sync, prune and backup. The client waits longer.
    pub long: bool,
}

/// One API endpoint. Each type in [`ep`] implements it.
pub trait Endpoint {
    const METHOD: Method;
    /// axum path syntax, such as `/v1/issues/{issue}`.
    const PATH: &'static str;
    /// True when the route may change data.
    const WRITES: bool;
    /// True when the route does long work on the server.
    const LONG: bool;
    /// The number of `{name}` segments in [`Endpoint::PATH`].
    const PARAMS: usize = count_params(Self::PATH);
    /// The JSON body, or the query string for GET and DELETE. `()` means
    /// none.
    type Request: Serialize + DeserializeOwned + Send + Sync + 'static;
    /// `serde_json::Value` for a route whose answer has no fixed type.
    type Response: Serialize + DeserializeOwned + Send + 'static;
    /// This endpoint's entry in [`ROUTES`].
    const ROUTE: Route;
}

/// Counts the `{name}` segments of a path.
pub const fn count_params(path: &str) -> usize {
    let b = path.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while i < b.len() {
        if b[i] == b'{' {
            n += 1;
        }
        i += 1;
    }
    n
}

const fn has_flag(flags: &[&str], name: &str) -> bool {
    let mut i = 0;
    while i < flags.len() {
        let (a, b) = (flags[i].as_bytes(), name.as_bytes());
        if a.len() == b.len() {
            let mut j = 0;
            while j < a.len() && a[j] == b[j] {
                j += 1;
            }
            if j == a.len() {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// A write unless the method is GET or the entry says `read`.
const fn writes(method: Method, flags: &[&str]) -> bool {
    !matches!(method, Method::Get) && !has_flag(flags, "read")
}

fn s<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<T>()
}

macro_rules! req_ty {
    (-) => {
        ()
    };
    (($t:ty)) => {
        $t
    };
}
macro_rules! res_ty {
    (-) => {
        serde_json::Value
    };
    (($t:ty)) => {
        $t
    };
}
macro_rules! schema {
    (-) => {
        None
    };
    (($t:ty)) => {
        Some(s::<$t> as SchemaFn)
    };
}

macro_rules! endpoints {
    ($(
        $(#[$doc:meta])*
        $name:ident: $m:ident $path:literal $([$($flag:ident),*])?,
        $summary:literal, $req:tt, $res:tt, $cli:literal;
    )*) => {
        /// One type per API endpoint. Pass one to the client's `call`.
        pub mod ep {
            #[allow(unused_imports)]
            use super::*;
            $(
                $(#[$doc])*
                #[doc = $summary]
                pub struct $name;

                impl Endpoint for $name {
                    const METHOD: Method = Method::$m;
                    const PATH: &'static str = $path;
                    const WRITES: bool =
                        writes(Method::$m, &[$($(stringify!($flag)),*)?]);
                    const LONG: bool = has_flag(&[$($(stringify!($flag)),*)?], "long");
                    type Request = req_ty!($req);
                    type Response = res_ty!($res);
                    const ROUTE: Route = Route {
                        method: Method::$m,
                        path: $path,
                        summary: $summary,
                        request: schema!($req),
                        response: schema!($res),
                        cli: $cli,
                        writes: Self::WRITES,
                        long: Self::LONG,
                    };
                }
            )*
        }

        /// Every route, in the order of the table. Query string parameters
        /// are listed as the request type for GET routes.
        pub const ROUTES: &[Route] = &[$(<ep::$name as Endpoint>::ROUTE),*];
    };
}

// Types are spelled with `t::`, since endpoint names repeat request names.
#[allow(unused_imports)]
use crate as t;

endpoints! {
    Health: Get "/v1/health", "Server version, data folder and primary flag", -, (t::Health), "server status";
    StopServer: Post "/v1/server/stop", "Stop the server after this response", -, -, "server stop";
    Backup: Post "/v1/server/backup" [long], "Write a database backup now", -, (t::BackupReport), "server backup";
    Settings: Get "/v1/settings", "Read workspace settings", -, (t::Settings), "settings show";
    UpdateSettings: Patch "/v1/settings", "Change workspace settings", (t::UpdateSettings), (t::Settings), "settings set";
    Actors: Get "/v1/actors", "List actors", -, (t::ActorList), "actor list";
    Me: Get "/v1/actors/me", "The actor this request comes from", -, (t::Actor), "actor me";
    // teams
    Teams: Get "/v1/teams", "List teams with their statuses", -, (Vec<t::Team>), "team list";
    CreateTeam: Post "/v1/teams", "Create a team with the default statuses", (t::CreateTeam), (t::Team), "team create";
    Team: Get "/v1/teams/{team}", "Read one team", -, (t::Team), "team view";
    UpdateTeam: Patch "/v1/teams/{team}", "Change a team", (t::UpdateTeam), (t::Team), "team update";
    CreateStatus: Post "/v1/teams/{team}/statuses", "Add a status to a team", (t::CreateStatus), (t::Status), "status create";
    UpdateStatus: Patch "/v1/statuses/{id}", "Change a status", (t::UpdateStatus), (t::Status), "status update";
    DeleteStatus: Delete "/v1/statuses/{id}", "Delete a status that no issue uses", -, -, "status delete";
    // labels
    Labels: Get "/v1/labels", "List labels and label groups", -, (Vec<t::Label>), "label list";
    CreateLabel: Post "/v1/labels", "Create a label or a label group", (t::CreateLabel), (t::Label), "label create";
    UpdateLabel: Patch "/v1/labels/{label}", "Change a label", (t::UpdateLabel), (t::Label), "label update";
    DeleteLabel: Delete "/v1/labels/{label}", "Delete a label and remove it from issues", -, -, "label delete";
    // projects
    Projects: Get "/v1/projects", "List projects", (t::ProjectQuery), (Vec<t::Project>), "project list";
    CreateProject: Post "/v1/projects", "Create a project", (t::CreateProject), (t::Project), "project create";
    Project: Get "/v1/projects/{project}", "Read one project", -, (t::Project), "project view";
    UpdateProject: Patch "/v1/projects/{project}", "Change a project", (t::UpdateProject), (t::Project), "project update";
    DeleteProject: Delete "/v1/projects/{project}", "Move a project to the trash", -, (t::Project), "project delete";
    LinkRepo: Post "/v1/projects/{project}/repos", "Link a GitHub repo to a project", (t::RepoLink), (t::Project), "project link-repo";
    UnlinkRepo: Delete "/v1/projects/{project}/repos/{owner}/{name}", "Unlink a GitHub repo", -, (t::Project), "project unlink-repo";
    Milestones: Get "/v1/projects/{project}/milestones", "List a project's milestones", -, (Vec<t::Milestone>), "milestone list";
    CreateMilestone: Post "/v1/projects/{project}/milestones", "Create a milestone", (t::CreateMilestone), (t::Milestone), "milestone create";
    UpdateMilestone: Patch "/v1/milestones/{id}", "Change a milestone", (t::UpdateMilestone), (t::Milestone), "milestone update";
    DeleteMilestone: Delete "/v1/milestones/{id}", "Delete a milestone and clear it from issues", -, -, "milestone delete";
    // documents
    Documents: Get "/v1/documents", "List documents, optionally for one project (?project=)", (t::DocumentQuery), (Vec<t::Document>), "doc list";
    CreateDocument: Post "/v1/documents", "Create a document", (t::CreateDocument), (t::Document), "doc create";
    Document: Get "/v1/documents/{doc}", "Read one document with its body", -, (t::Document), "doc view";
    UpdateDocument: Patch "/v1/documents/{doc}", "Change a document title or body", (t::UpdateDocument), (t::Document), "doc update";
    DeleteDocument: Delete "/v1/documents/{doc}", "Move a document to the trash", -, (t::Document), "doc delete";
    DocumentHistory: Get "/v1/documents/{doc}/history", "List a document's revisions, with their text only on ?bodies=true", (t::HistoryQuery), (t::History), "doc history";
    RestoreDocument: Post "/v1/documents/{doc}/restore", "Restore a document or one of its revisions", (t::RestoreRequest), (t::Document), "doc restore";
    // issues
    QueryIssues: Post "/v1/issues/query" [read], "Query issues", (t::IssueQuery), (t::Page<t::Issue>), "issue query";
    QueryIssueIds: Post "/v1/issues/query/ids" [read], "Query issues for their IDs only, every match in one answer, with no cursor or limit", (t::IssueQuery), (Vec<t::IssueId>), "issue query --ids";
    Frontier: Post "/v1/issues/frontier" [read], "Open, unblocked, unassigned sub-issues of a map issue or project", (t::FrontierQuery), (Vec<t::Issue>), "issue frontier";
    CreateIssue: Post "/v1/issues", "Create an issue", (t::CreateIssue), (t::IssueDetail), "issue create";
    Issue: Get "/v1/issues/{issue}", "Read an issue with comments, relations and work log", -, (t::IssueDetail), "issue view";
    UpdateIssue: Patch "/v1/issues/{issue}", "Change an issue", (t::UpdateIssue), (t::IssueDetail), "issue update";
    DeleteIssue: Delete "/v1/issues/{issue}", "Move an issue to the trash", -, (t::Issue), "issue delete";
    RestoreIssue: Post "/v1/issues/{issue}/restore", "Take an issue out of the trash, or restore a description revision", (t::RestoreRequest), (t::IssueDetail), "issue restore";
    IssueHistory: Get "/v1/issues/{issue}/history", "Description revisions and activity, with revision text only on ?bodies=true", (t::HistoryQuery), (t::History), "issue history";
    Claim: Post "/v1/issues/{issue}/claim", "Claim an issue for this session", (t::ClaimRequest), (t::ClaimResponse), "issue claim";
    Release: Post "/v1/issues/{issue}/release", "Release this session's claim", (t::ReleaseRequest), (t::Issue), "issue release";
    Questions: Get "/v1/questions", "Owner questions, open ones on open issues (?issue=&project=&all=true&cursor=&limit=)", (t::QuestionQuery), (t::Page<t::QuestionEntry>), "issue questions";
    UndoAnswer: Post "/v1/issues/{issue}/questions/{id}/undo-answer", "Reopen an answered owner question, unless someone acted on the answer", -, (t::IssueDetail), "issue undo-answer";
    Review: Get "/v1/issues/{issue}/review", "The review section and every review answer of an issue", -, (t::ReviewState), "issue review show";
    SubmitReview: Post "/v1/issues/{issue}/review", "Approve or request changes on an issue in User review", (t::SubmitReview), (t::IssueDetail), "issue review answer";
    Analyze: Post "/v1/issues/{issue}/analyze", "Mark or unmark this session as analyzing an issue", (t::AnalyzeRequest), (t::AnalyzeResponse), "issue analyze";
    AddComment: Post "/v1/issues/{issue}/comments", "Add a comment", (t::CreateComment), (t::Comment), "issue comment add";
    UpdateComment: Patch "/v1/comments/{id}", "Edit a comment", (t::UpdateComment), (t::Comment), "issue comment update";
    DeleteComment: Delete "/v1/comments/{id}", "Delete a comment", -, -, "issue comment delete";
    Deps: Get "/v1/issues/{issue}/deps", "The dependency tree of an issue, both directions (?open=true hides closed issues)", (t::DepsQuery), (t::DepNode), "issue deps";
    Order: Get "/v1/issues/{issue}/order", "The order of an issue's sub-issues: its steps, then the sub-issues with no step", -, (t::OrderView), "issue order show";
    SetOrder: Put "/v1/issues/{issue}/order", "Write the whole order of an issue's sub-issues, as steps of keys", (t::SetOrder), (t::OrderView), "issue order set";
    AddRelation: Post "/v1/issues/{issue}/relations", "Add a relation", (t::CreateRelation), (t::IssueDetail), "issue relation add";
    RemoveRelation: Post "/v1/issues/{issue}/relations/remove", "Remove a relation", (t::RemoveRelation), (t::IssueDetail), "issue relation remove";
    Attach: Post "/v1/issues/{issue}/attachments", "Attach a link or a local path", (t::CreateAttachment), (t::Attachment), "issue attach";
    Detach: Delete "/v1/attachments/{id}", "Remove an attachment", -, -, "issue detach";
    // sessions
    Session: Get "/v1/sessions/{session}", "Tickets a session touched, with notes", -, (t::SessionView), "session show";
    AddSessionNote: Post "/v1/sessions/{session}/notes", "Add a note to a session", (t::CreateSessionNote), (t::SessionNote), "session note";
    // usage
    Usage: Get "/v1/usage", "Token usage and cost by ticket (?project=&issue=&session=)", (t::UsageQuery), (t::UsageReport), "usage";
    // activity
    Activity: Get "/v1/activity", "Activity events after a sequence number (?after_seq=&issue=&limit=)", (t::ActivityQuery), (t::ActivityPage), "activity";
    Ws: Get "/v1/ws", "WebSocket of live events. Sends WsMessage values", -, (t::WsMessage), "activity --follow";
    // views
    Views: Get "/v1/views", "List saved views", -, (Vec<t::SavedView>), "view list";
    CreateView: Post "/v1/views", "Create a saved view", (t::CreateView), (t::SavedView), "view create";
    UpdateView: Patch "/v1/views/{view}", "Change a saved view", (t::UpdateView), (t::SavedView), "view update";
    DeleteView: Delete "/v1/views/{view}", "Delete a saved view", -, -, "view delete";
    // sync
    SyncStatus: Get "/v1/sync/status", "Local generation, unpushed writes and the remote manifest", -, (t::SyncStatus), "sync status";
    SyncPush: Post "/v1/sync/push" [long], "Copy the database to the sync folder", (t::SyncPushRequest), (t::SyncReport), "sync push";
    SyncPull: Post "/v1/sync/pull" [long], "Replace the local database with the copy in the sync folder", (t::SyncPullRequest), (t::SyncReport), "sync pull";
    // github
    GithubStatus: Get "/v1/github/status", "The last GitHub poll and why it is paused, if it is", -, (t::GithubReport), "github status";
    GithubSync: Post "/v1/github/sync" [long], "Poll linked GitHub repos now", -, (t::GithubReport), "github sync";
    // import and files
    ImportLinear: Post "/v1/import/linear" [long], "Import or update everything from Linear. Idempotent by Linear ID", (t::LinearImportRequest), (t::ImportReport), "import linear";
    File: Get "/v1/files/{hash}", "A stored file, such as an image copied from Linear", -, -, "file get";
    // admin
    Prune: Post "/v1/prune" [long], "Permanently delete trashed issues or old revisions. Owner only", (t::PruneRequest), (t::PruneReport), "prune";
}

/// The route that serves a concrete request path, such as
/// `PATCH /v1/issues/LLM-1`. A literal segment beats a `{name}` segment, so
/// `/v1/issues/query` finds the query route. The query string is ignored.
pub fn find(method: Method, path: &str) -> Option<&'static Route> {
    let path = path.split('?').next().unwrap_or(path);
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    ROUTES
        .iter()
        .filter(|r| r.method == method)
        .filter_map(|r| {
            let tpl: Vec<&str> = r.path.trim_matches('/').split('/').collect();
            if tpl.len() != segs.len() {
                return None;
            }
            let mut literal = 0;
            for (t, s) in tpl.iter().zip(&segs) {
                if t.starts_with('{') {
                    continue;
                }
                if t != s {
                    return None;
                }
                literal += 1;
            }
            Some((literal, r))
        })
        .max_by_key(|(literal, _)| *literal)
        .map(|(_, r)| r)
}

/// True when a request may change data. A path outside the table counts
/// as a write unless its method is GET.
pub fn is_write(method: Method, path: &str) -> bool {
    find(method, path).map_or(method != Method::Get, |r| r.writes)
}

/// Builds a JSON document of every route with its schemas.
pub fn describe() -> serde_json::Value {
    let mut g = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let routes: Vec<serde_json::Value> = ROUTES
        .iter()
        .map(|r| {
            serde_json::json!({
                "method": r.method.as_str(),
                "path": r.path,
                "summary": r.summary,
                "cli": r.cli,
                "writes": r.writes,
                "request": r.request.map(|f| f(&mut g)),
                "response": r.response.map(|f| f(&mut g)),
            })
        })
        .collect();
    let defs = g.take_definitions(true);
    serde_json::json!({ "version": crate::VERSION, "routes": routes, "$defs": defs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for r in ROUTES {
            assert!(
                seen.insert((r.method, r.path)),
                "duplicate route {} {}",
                r.method.as_str(),
                r.path
            );
        }
    }

    #[test]
    fn describe_builds() {
        let d = describe();
        assert!(d["$defs"]["IssueQuery"].is_object());
    }

    #[test]
    fn reads_and_writes() {
        const {
            assert!(!ep::QueryIssues::WRITES);
            assert!(!ep::Frontier::WRITES);
            assert!(!ep::Issue::WRITES);
            assert!(ep::CreateIssue::WRITES);
            assert!(ep::UpdateIssue::WRITES);
            assert!(ep::Prune::LONG && !ep::UpdateIssue::LONG);
            assert!(ep::UnlinkRepo::PARAMS == 3);
            assert!(ep::Health::PARAMS == 0);
        }
    }

    #[test]
    fn find_prefers_literal_segments() {
        let r = find(Method::Post, "/v1/issues/query").unwrap();
        assert_eq!(r.path, "/v1/issues/query");
        let r = find(Method::Patch, "/v1/issues/LLM-1?x=1").unwrap();
        assert_eq!(r.path, "/v1/issues/{issue}");
        assert!(find(Method::Get, "/v1/nope").is_none());
        assert!(!is_write(Method::Post, "/v1/issues/frontier"));
        assert!(is_write(Method::Post, "/v1/issues"));
        assert!(!is_write(Method::Get, "/v1/nope"));
    }
}
