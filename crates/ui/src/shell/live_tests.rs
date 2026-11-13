//! Tests against a real server started in-process on a temporary data
//! folder, as `crates/cli/tests/cli.rs` does. They drive `backend.rs`, the
//! thread the app uses, and check what it sends back.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use llmtodo_client::Client;
use llmtodo_client::types::{
    ActorInput, ActorKind, BodyChange, ClaimRequest, ClientContext, CreateIssue, RelationInput,
    ReleaseRequest, UpdateIssue,
};
use serde_json::json;

use crate::model::backend::{Backend, Event, Request, Target, Write, WriteKind};
use crate::shell::icons::HasIcon as _;

struct Live {
    rt: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
    server: Option<llmtodo_server::Running>,
}

impl Live {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let server = rt.block_on(start(dir.path()));
        let live = Self {
            rt,
            dir,
            server: Some(server),
        };
        live.seed();
        live
    }

    /// A client for the owner, or for an agent with its own session.
    fn client(&self, agent_session: Option<&str>) -> Client {
        let url = self.server.as_ref().unwrap().url();
        let context = match agent_session {
            None => ClientContext::default(),
            Some(session) => ClientContext {
                actor: ActorInput {
                    kind: ActorKind::Agent,
                    name: None,
                    harness: Some("claude-code".into()),
                    model: Some("opus-5.5".into()),
                    effort: Some("high".into()),
                },
                session_id: Some(session.into()),
                worktree: Some("/work/llmtodo".into()),
                branch: Some("main".into()),
                herdr_pane: Some("w1:p2".into()),
                ..Default::default()
            },
        };
        Client::new(url, &context)
    }

    fn seed(&self) {
        let owner = self.client(None);
        self.rt.block_on(async {
            owner
                .create_team(
                    &serde_json::from_value(json!({"key": "LLM", "name": "llmtodo"})).unwrap(),
                )
                .await
                .unwrap();
            owner
                .create_label(
                    &serde_json::from_value(json!({"name": "Area", "is_group": true})).unwrap(),
                )
                .await
                .unwrap();
            owner
                .create_label(
                    &serde_json::from_value(
                        json!({"name": "app", "group": "Area", "color": "#5e6ad2"}),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            owner
                .create_issue(
                    &serde_json::from_value(json!({
                        "team": "LLM",
                        "title": "App on live data",
                        "description": "First text.\n",
                        "estimate_minutes": 30,
                        "estimate_cost_usd": 1.5,
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
        });
    }

    fn backend(&self) -> (Backend, UnboundedReceiver<Event>) {
        self.backend_with(false, Duration::ZERO)
    }

    fn read_only_backend(&self) -> (Backend, UnboundedReceiver<Event>) {
        self.backend_with(true, Duration::ZERO)
    }

    /// A backend whose live refreshes each wait `slow_flush` first.
    fn backend_with(
        &self,
        read_only: bool,
        slow_flush: Duration,
    ) -> (Backend, UnboundedReceiver<Event>) {
        Backend::start(Target {
            data_dir: self.dir.path().to_path_buf(),
            spawn: None,
            read_only,
            slow_flush,
        })
    }

    /// The next event that `pick` accepts. Other events are skipped.
    fn wait<T>(
        &self,
        events: &mut UnboundedReceiver<Event>,
        what: &str,
        mut pick: impl FnMut(&Event) -> Option<T>,
    ) -> T {
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(event) = events.next().await {
                    if let Some(found) = pick(&event) {
                        return found;
                    }
                }
                panic!("events ended before {what}");
            })
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        })
    }

    fn restart(&mut self) {
        let server = self.server.take().unwrap();
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), server.stop())
                .await
                .expect("server stops")
                .unwrap();
        });
        self.server = Some(self.rt.block_on(start(self.dir.path())));
    }

    /// Stops the server and leaves it down until `start_again`.
    fn stop(&mut self) {
        let server = self.server.take().unwrap();
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), server.stop())
                .await
                .expect("server stops")
                .unwrap();
        });
    }

    fn start_again(&mut self) {
        self.server = Some(self.rt.block_on(start(self.dir.path())));
    }
}

/// A description save of `text` on LLM-1, based on `version`.
fn save(text: &str, version: i64) -> Write {
    Write::Update(Box::new(UpdateIssue {
        description: BodyChange {
            body: Some(text.into()),
            base_version: Some(version),
            ..Default::default()
        },
        ..Default::default()
    }))
}

/// LLM-1's description and version in a snapshot or a fetched copy.
fn llm_1_text(event: &Event) -> Option<(String, i64)> {
    let issue = match event {
        Event::Snapshot(s) => s
            .raw
            .issues
            .iter()
            .find(|i| i.key.to_string() == "LLM-1")?
            .clone(),
        _ => issues_in(event)
            .into_iter()
            .find(|i| i.key.to_string() == "LLM-1")?,
    };
    (issue.key.to_string() == "LLM-1").then(|| {
        (
            issue.description.unwrap_or_default(),
            issue.description_version,
        )
    })
}

/// Starts a server on `dir`. Right after a stop, the lock file can still
/// read as held for a moment while other tests run in parallel, so a start
/// that finds it held tries again for a few seconds.
async fn start(dir: &Path) -> llmtodo_server::Running {
    let config = || llmtodo_server::Config {
        data_dir: dir.to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    };
    for _ in 0..50 {
        match llmtodo_server::start(config()).await {
            Ok(running) => return running,
            Err(e) if e.to_string().contains("already runs") => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("start the server: {e:#}"),
        }
    }
    llmtodo_server::start(config()).await.unwrap()
}

/// The one fresh issue copy an event carries, from a write, a fetch or a
/// live refresh of one issue.
fn issue_event(event: &Event) -> Option<llmtodo_client::types::Issue> {
    let mut issues = issues_in(event);
    (issues.len() == 1).then(|| issues.remove(0))
}

/// Every fresh issue copy an event carries. Snapshots do not count.
fn issues_in(event: &Event) -> Vec<llmtodo_client::types::Issue> {
    match event {
        Event::Issue(issue) => vec![(**issue).clone()],
        Event::Issues(issues) => issues.clone(),
        Event::IssueDetail(parts) => vec![parts.issue.clone()],
        _ => Vec::new(),
    }
}

/// LLM-137: a read-only backend loads a real server's data, and no write
/// reaches that server.
#[test]
fn a_read_only_backend_reads_but_cannot_write() {
    let live = Live::new();
    let (backend, mut events) = live.read_only_backend();
    let snapshot = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.clone()),
        _ => None,
    });
    assert_eq!(snapshot.issues.len(), 1);
    let refused = |message: &str| assert!(message.contains("read-only"), "{message}");

    let update = UpdateIssue {
        status: Some("In Review".into()),
        ..Default::default()
    };
    backend.write("LLM-1", Write::Update(Box::new(update)), WriteKind::Status);
    refused(&live.wait(&mut events, "refused update", |e| match e {
        Event::WriteFailed { message, .. } => Some(message.clone()),
        _ => None,
    }));
    backend.write("LLM-1", Write::Trash, WriteKind::Trash);
    refused(&live.wait(&mut events, "refused trash", |e| match e {
        Event::WriteFailed { message, .. } => Some(message.clone()),
        _ => None,
    }));
    backend.request(Request::Create(Box::new(
        serde_json::from_value(json!({"team": "LLM", "title": "Must not exist"})).unwrap(),
    )));
    refused(&live.wait(&mut events, "refused create", |e| match e {
        Event::CreateFailed { message, .. } => Some(message.clone()),
        _ => None,
    }));
    backend.request(Request::DeleteView { name: "x".into() });
    refused(&live.wait(&mut events, "refused view", |e| match e {
        Event::ViewFailed { message, .. } => Some(message.clone()),
        _ => None,
    }));
    backend.request(Request::SaveDocument {
        id: llmtodo_core::DocumentId::nil(),
        body: "Must not exist.\n".into(),
        base: 1,
        force: true,
    });
    refused(&live.wait(&mut events, "refused document", |e| match e {
        Event::DocumentFailed { message, .. } => Some(message.clone()),
        _ => None,
    }));

    // The server's data did not change.
    let owner = live.client(None);
    let issues = live.rt.block_on(async {
        owner
            .query_all(&llmtodo_client::types::IssueQuery::default())
            .await
            .unwrap()
    });
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].status.name, snapshot.issues[0].status.name);
    assert!(issues[0].deleted_at.is_none());
}

#[test]
fn backend_loads_writes_and_hears_claims() {
    let live = Live::new();
    let (backend, mut events) = live.backend();

    live.wait(&mut events, "connect", |e| {
        matches!(e, Event::Connected { .. }).then_some(())
    });
    let snapshot = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.clone()),
        _ => None,
    });
    assert_eq!(snapshot.issues.len(), 1);
    // The snapshot leaves descriptions out. A fetch brings one.
    assert_eq!(snapshot.issues[0].description, None);
    let names: Vec<_> = snapshot.catalog.teams[0]
        .statuses
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(names.contains(&"In Review"), "{names:?}");

    // A status write comes back as a fresh issue, then a done message.
    let update = UpdateIssue {
        status: Some("In Progress".into()),
        add_labels: vec!["Area/app".into()],
        ..Default::default()
    };
    backend.write("LLM-1", Write::Update(Box::new(update)), WriteKind::Status);
    let issue = live.wait(&mut events, "status write", issue_event);
    assert_eq!(issue.status.name, "In Progress");
    assert_eq!(issue.labels, ["Area/app"]);
    live.wait(&mut events, "write done", |e| {
        matches!(
            e,
            Event::WriteDone {
                kind: WriteKind::Status,
                ..
            }
        )
        .then_some(())
    });

    // An agent claims from elsewhere: the claim arrives live.
    let agent = live.client(Some("agent-1"));
    live.rt
        .block_on(agent.claim("LLM-1", &ClaimRequest::default()))
        .unwrap();
    let claimed = live.wait(&mut events, "claim", |e| {
        issue_event(e).filter(|i| i.claim.is_some())
    });
    let claim = claimed.claim.unwrap();
    assert_eq!(claim.session_id, "agent-1");
    assert_eq!(claim.herdr_pane.as_deref(), Some("w1:p2"));
    let actor = crate::model::data::Assignee::from_actor(&claim.actor);
    assert_eq!(actor.icon(), gpui_kit::assets::IconName::Sparkles);

    live.rt
        .block_on(agent.release("LLM-1", &ReleaseRequest::default()))
        .unwrap();
    live.wait(&mut events, "release", |e| {
        issue_event(e).filter(|i| i.claim.is_none())
    });

    // The agent changes the text after the owner opened version 1, so the
    // owner's save is refused as stale.
    let agent_text = UpdateIssue {
        description: BodyChange {
            body: Some("Agent text.\n".into()),
            base_version: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    live.rt
        .block_on(agent.update_issue("LLM-1", &agent_text))
        .unwrap();
    let owner_text = UpdateIssue {
        description: BodyChange {
            body: Some("Owner text.\n".into()),
            base_version: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    backend.write(
        "LLM-1",
        Write::Update(Box::new(owner_text)),
        WriteKind::Description,
    );
    let stale = live.wait(&mut events, "stale save", |e| match e {
        Event::WriteFailed { stale, kind, .. } => Some((*stale, *kind)),
        _ => None,
    });
    assert_eq!(stale, (true, WriteKind::Description));
    // The server's copy follows, so the preview shows the agent's text.
    let fresh = live.wait(&mut events, "server copy", issue_event);
    assert_eq!(fresh.description.as_deref(), Some("Agent text.\n"));

    // Trash and restore.
    backend.write("LLM-1", Write::Trash, WriteKind::Trash);
    live.wait(&mut events, "trash", |e| {
        issue_event(e).filter(|i| i.deleted_at.is_some())
    });
    backend.write("LLM-1", Write::Restore, WriteKind::Restore);
    live.wait(&mut events, "restore", |e| {
        issue_event(e).filter(|i| i.deleted_at.is_none())
    });
}

#[test]
fn backend_catches_up_after_a_reconnect() {
    let mut live = Live::new();
    let (_backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    live.restart();
    live.wait(&mut events, "offline", |e| {
        matches!(e, Event::Offline { .. }).then_some(())
    });
    // A write while the app is away. The backend waits before it retries,
    // so it misses the live event and must catch up.
    let agent = live.client(Some("agent-2"));
    live.rt
        .block_on(agent.claim("LLM-1", &ClaimRequest::default()))
        .unwrap();

    live.wait(&mut events, "reconnect", |e| {
        matches!(e, Event::Connected { .. }).then_some(())
    });
    let next = live.wait(&mut events, "catch-up", |e| match e {
        Event::Snapshot(_) => Some(Err(())),
        e => issue_event(e).map(|i| Ok(i.claim)),
    });
    let claim = next
        .expect("caught up from the activity log, not a full reload")
        .expect("the claim made while offline");
    assert_eq!(claim.session_id, "agent-2");
}

/// A TCP proxy in front of the server. While frozen, it holds every
/// connection open but passes no bytes, like a server stuck on a lock.
/// While HTTP is down, it refuses new connections and closes every one but
/// the live WebSocket, so requests fail and live events still arrive.
struct Proxy {
    port: u16,
    frozen: Arc<AtomicBool>,
    http_down: tokio::sync::watch::Sender<bool>,
}

impl Proxy {
    async fn start(target: u16) -> Self {
        let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = first.local_addr().unwrap().port();
        let frozen = Arc::new(AtomicBool::new(false));
        let (http_down, mut down) = tokio::sync::watch::channel(false);
        let flag = frozen.clone();
        let pipe_down = down.clone();
        tokio::spawn(async move {
            let mut listener = Some(first);
            loop {
                let l = match listener.take() {
                    Some(l) => l,
                    None => {
                        if down.wait_for(|d| !*d).await.is_err() {
                            return;
                        }
                        tokio::net::TcpListener::bind(("127.0.0.1", port))
                            .await
                            .unwrap()
                    }
                };
                loop {
                    tokio::select! {
                        accepted = l.accept() => {
                            let Ok((inbound, _)) = accepted else { return };
                            tokio::spawn(pipe(inbound, target, flag.clone(), pipe_down.clone()));
                        }
                        changed = down.wait_for(|d| *d) => {
                            if changed.is_err() {
                                return;
                            }
                            break;
                        }
                    }
                }
            }
        });
        Self {
            port,
            frozen,
            http_down,
        }
    }

    fn freeze(&self, on: bool) {
        self.frozen.store(on, Ordering::SeqCst);
    }

    fn http_down(&self, on: bool) {
        self.http_down.send_replace(on);
    }
}

/// Forwards one connection both ways.
async fn pipe(
    mut inbound: tokio::net::TcpStream,
    target: u16,
    frozen: Arc<AtomicBool>,
    down: tokio::sync::watch::Receiver<bool>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut first = vec![0u8; 16 * 1024];
    let n = match inbound.read(&mut first).await {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let is_ws = first[..n].starts_with(b"GET /v1/ws");
    let Ok(mut outbound) = tokio::net::TcpStream::connect(("127.0.0.1", target)).await else {
        return;
    };
    if outbound.write_all(&first[..n]).await.is_err() {
        return;
    }
    let (ri, wi) = inbound.into_split();
    let (ro, wo) = outbound.into_split();
    type Reader = Box<dyn tokio::io::AsyncRead + Send + Unpin>;
    type Writer = Box<dyn tokio::io::AsyncWrite + Send + Unpin>;
    let halves: [(Reader, Writer); 2] =
        [(Box::new(ri), Box::new(wo)), (Box::new(ro), Box::new(wi))];
    for (mut r, mut w) in halves {
        let frozen = frozen.clone();
        let mut down = down.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let n = tokio::select! {
                    read = r.read(&mut buf) => match read {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    },
                    _ = down.wait_for(|d| *d), if !is_ws => return,
                };
                while frozen.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if w.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        });
    }
}

impl Live {
    /// Starts a proxy in front of the server and points `server.json` at it,
    /// so the backend talks to the server through the proxy.
    fn proxy(&self) -> Proxy {
        let url = self.server.as_ref().unwrap().url();
        let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
        let proxy = self.rt.block_on(Proxy::start(port));
        let info_path = self.dir.path().join("server.json");
        let mut info: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&info_path).unwrap()).unwrap();
        info["port"] = json!(proxy.port);
        std::fs::write(&info_path, serde_json::to_vec(&info).unwrap()).unwrap();
        proxy
    }
}

/// The server keeps the socket open but stops answering. The app must not
/// keep showing "Live": the ping finds the silence and the thread reconnects.
#[test]
fn backend_notices_a_silent_server() {
    let live = Live::new();
    let proxy = live.proxy();
    let (_backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    proxy.freeze(true);
    let reason = live.wait(&mut events, "offline", |e| match e {
        Event::Offline { reason } => Some(reason.clone()),
        _ => None,
    });
    assert!(reason.contains("stopped answering"), "{reason}");

    proxy.freeze(false);
    live.wait(&mut events, "reconnect", |e| {
        matches!(e, Event::Connected { .. }).then_some(())
    });
}

/// A claim event arrives, but the fetch it starts fails and the session
/// ends. Catch-up after the reconnect starts past that event, so the thread
/// must still owe the app a fresh copy of the issue.
#[test]
fn backend_keeps_events_that_arrive_before_a_disconnect() {
    let live = Live::new();
    let proxy = live.proxy();
    let (_backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    // Requests fail from now on, but the live connection stays up.
    proxy.http_down(true);
    std::thread::sleep(Duration::from_millis(200));
    let agent = live.client(Some("agent-3"));
    live.rt
        .block_on(agent.claim("LLM-1", &ClaimRequest::default()))
        .unwrap();
    live.wait(&mut events, "offline", |e| {
        matches!(e, Event::Offline { .. }).then_some(())
    });

    proxy.http_down(false);
    live.wait(&mut events, "reconnect", |e| {
        matches!(e, Event::Connected { .. }).then_some(())
    });
    let claim = live.wait(&mut events, "the claim", |e| match e {
        Event::Snapshot(s) => s.raw.issues[0].claim.clone(),
        e => issue_event(e).and_then(|i| i.claim),
    });
    assert_eq!(claim.session_id, "agent-3");
}

/// A new label changes the catalog only. The thread must not load every
/// issue with its description again.
#[test]
fn backend_reloads_only_the_catalog_on_a_label_event() {
    let live = Live::new();
    let (_backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    let agent = live.client(Some("agent-4"));
    live.rt
        .block_on(agent.create_label(&serde_json::from_value(json!({"name": "be"})).unwrap()))
        .unwrap();
    let next = live.wait(&mut events, "catalog", |e| match e {
        Event::Catalog(s) => Some(Ok(s.clone())),
        Event::Snapshot(_) => Some(Err(())),
        _ => None,
    });
    let catalog = next.expect("a catalog reload, not a full snapshot");
    assert!(catalog.labels.iter().any(|l| l.full_name == "be"));
}

/// A fetch sends the issue, then its git panel. The panel keeps the branch
/// and worktree after the agent's claim ends.
#[test]
fn backend_sends_the_git_panel_after_a_claim_ends() {
    let live = Live::new();
    let (backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    let agent = live.client(Some("agent-git"));
    live.rt
        .block_on(agent.claim("LLM-1", &ClaimRequest::default()))
        .unwrap();
    live.rt
        .block_on(agent.release("LLM-1", &ReleaseRequest::default()))
        .unwrap();
    backend.fetch("LLM-1");
    let git = live.wait(&mut events, "git panel", |e| match e {
        Event::IssueDetail(parts) => Some(parts.git.clone()),
        _ => None,
    });
    assert_eq!(git.branch.as_deref(), Some("main"));
    assert_eq!(git.worktree.as_deref(), Some("/work/llmtodo"));
}

/// The owner saves while the server is down. The save fails with an
/// offline message, not as stale, so the app keeps the owner's text. After
/// the reconnect, the thread fetches the issue again, which puts the
/// server's copy back in place of the app's early change.
#[test]
fn backend_fails_a_save_while_the_server_is_down_and_refetches() {
    let mut live = Live::new();
    let (backend, mut events) = live.backend();
    let (_, version) = live.wait(&mut events, "snapshot", llm_1_text);

    live.stop();
    live.wait(&mut events, "offline", |e| {
        matches!(e, Event::Offline { .. }).then_some(())
    });
    backend.write(
        "LLM-1",
        save("Owner text.\n", version),
        WriteKind::Description,
    );
    let (message, stale) = live.wait(&mut events, "the failed save", |e| match e {
        Event::WriteFailed {
            kind: WriteKind::Description,
            message,
            stale,
            ..
        } => Some((message.clone(), *stale)),
        _ => None,
    });
    assert!(!stale);
    assert!(message.contains("offline"), "{message}");

    live.start_again();
    live.wait(&mut events, "reconnect", |e| {
        matches!(e, Event::Connected { .. }).then_some(())
    });
    let (text, _) = live.wait(&mut events, "the server's copy", llm_1_text);
    assert_eq!(text, "First text.\n");
}

/// The live connection stays up, but the save's request fails. The thread
/// reports the failure, reconnects and fetches the issue again.
#[test]
fn backend_fails_a_save_whose_request_is_cut_and_refetches() {
    let live = Live::new();
    let proxy = live.proxy();
    let (backend, mut events) = live.backend();
    let (_, version) = live.wait(&mut events, "snapshot", llm_1_text);

    proxy.http_down(true);
    std::thread::sleep(Duration::from_millis(200));
    backend.write(
        "LLM-1",
        save("Owner text.\n", version),
        WriteKind::Description,
    );
    let stale = live.wait(&mut events, "the failed save", |e| match e {
        Event::WriteFailed {
            kind: WriteKind::Description,
            stale,
            ..
        } => Some(*stale),
        _ => None,
    });
    assert!(!stale);

    proxy.http_down(false);
    live.wait(&mut events, "reconnect", |e| {
        matches!(e, Event::Connected { .. }).then_some(())
    });
    let (text, _) = live.wait(&mut events, "the server's copy", llm_1_text);
    assert_eq!(text, "First text.\n");
}

#[test]
fn backend_loads_the_trash_and_saved_views_and_reads_a_view() {
    use llmtodo_client::types::{CreateView, IssueQuery};

    let live = Live::new();
    let owner = live.client(None);
    live.rt.block_on(async {
        let second = owner
            .create_issue(
                &serde_json::from_value(json!({"team": "LLM", "title": "Dropped idea"})).unwrap(),
            )
            .await
            .unwrap();
        owner
            .delete_issue(&second.issue.key.to_string())
            .await
            .unwrap();
    });
    let (backend, mut events) = live.backend();
    let snapshot = live.wait(&mut events, "a snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.clone()),
        _ => None,
    });
    let trashed: Vec<_> = snapshot
        .issues
        .iter()
        .filter(|i| i.deleted_at.is_some())
        .map(|i| i.key.to_string())
        .collect();
    assert_eq!(trashed, ["LLM-2"], "the snapshot holds the Trash");
    assert!(snapshot.catalog.views.is_empty());

    // A view saved from the app, then read back by name.
    backend.request(Request::CreateView(Box::new(CreateView {
        name: "Live work".into(),
        query: IssueQuery {
            text: Some("live".into()),
            ..Default::default()
        },
    })));
    let name = live.wait(&mut events, "the view saved", |e| match e {
        Event::ViewSaved { name } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(name, "Live work");
    backend.request(Request::ViewIssues {
        name: "Live work".into(),
    });
    let ids = live.wait(&mut events, "the view's issues", |e| match e {
        Event::ViewIssues { ids, .. } => Some(ids.clone()),
        _ => None,
    });
    let llm_1 = snapshot
        .issues
        .iter()
        .find(|i| i.key.to_string() == "LLM-1")
        .unwrap()
        .id;
    assert_eq!(ids, [llm_1]);

    // A view the server does not know fails with a message.
    backend.request(Request::ViewIssues {
        name: "Nope".into(),
    });
    let name = live.wait(&mut events, "the failure", |e| match e {
        Event::ViewFailed { name, .. } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(name, "Nope");
    backend.request(Request::DeleteView {
        name: "Live work".into(),
    });
    let name = live.wait(&mut events, "the view deleted", |e| match e {
        Event::ViewDeleted { name } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(name, "Live work");
}

/// The refresh that follows each issue change reads the view's IDs in one
/// request. It walks no pages of full issues, however large the view is.
#[test]
fn a_view_refresh_reads_only_ids_in_one_request() {
    use llmtodo_client::types::{CreateView, IssueQuery};

    let live = Live::new();
    let owner = live.client(None);
    let ids = live.rt.block_on(async {
        for title in ["Second", "Third"] {
            owner
                .create_issue(
                    &serde_json::from_value(json!({"team": "LLM", "title": title})).unwrap(),
                )
                .await
                .unwrap();
        }
        let query = IssueQuery::default();
        owner
            .create_view(&CreateView {
                name: "Everything".into(),
                query: query.clone(),
            })
            .await
            .unwrap();
        let all = owner.query_all(&query).await.unwrap();
        all.iter().map(|i| i.id).collect::<Vec<_>>()
    });
    assert_eq!(ids.len(), 3);
    let (backend, mut events) = live.backend();
    live.wait(&mut events, "a snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    let server = live.server.as_ref().unwrap();
    server.record_routes();

    // The request the workspace sends after an issue change. The test
    // `a_saved_view_shows_the_issues_the_server_sends` checks that one
    // change sends one.
    backend.request(Request::ViewIssues {
        name: "Everything".into(),
    });
    let got = live.wait(&mut events, "the view's issues", |e| match e {
        Event::ViewIssues { ids, .. } => Some(ids.clone()),
        _ => None,
    });
    assert_eq!(got.len(), ids.len());
    assert!(got.iter().all(|id| ids.contains(id)));
    let views: Vec<_> = server
        .take_routes()
        .into_iter()
        .filter(|r| r.starts_with("POST /v1/issues/query"))
        .collect();
    assert_eq!(views, ["POST /v1/issues/query/ids"], "one IDs-only request");
}

#[test]
fn backend_creates_an_issue_and_names_it() {
    use llmtodo_client::types::CreateIssue;

    let live = Live::new();
    let (backend, mut events) = live.backend();
    live.wait(&mut events, "a snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    let create = |title: &str, labels: Vec<String>| CreateIssue {
        team: Some("LLM".into()),
        title: title.into(),
        description: String::new(),
        status: Some("In Progress".into()),
        priority: None,
        assignee: None,
        labels,
        project: None,
        milestone: None,
        parent: None,
        wanted_executor: None,
        waiting_on: None,
        relations: Default::default(),
        estimate_minutes: None,
        estimate_cost_usd: None,
    };
    backend.request(Request::Create(Box::new(create(
        "From the app",
        vec!["Area/app".into()],
    ))));
    let issue = live.wait(&mut events, "the new issue", |e| {
        issue_event(e).filter(|i| i.title == "From the app")
    });
    assert_eq!(issue.key.to_string(), "LLM-2");
    assert_eq!(issue.status.name, "In Progress");
    assert_eq!(issue.labels, ["Area/app"]);
    let (id, key) = live.wait(&mut events, "the created event", |e| match e {
        Event::Created { id, key } => Some((*id, key.clone())),
        _ => None,
    });
    assert_eq!((id, key.as_str()), (issue.id, "LLM-2"));

    // The server refuses an unknown label, and the app hears why.
    backend.request(Request::Create(Box::new(create(
        "Bad label",
        vec!["nope".into()],
    ))));
    let title = live.wait(&mut events, "the failure", |e| match e {
        Event::CreateFailed { title, .. } => Some(title.clone()),
        _ => None,
    });
    assert_eq!(title, "Bad label");
}

#[test]
fn backend_says_when_this_machine_stops_being_the_primary() {
    use llmtodo_client::types::SyncPushRequest;

    let live = Live::new();
    let (backend, mut events) = live.backend();
    let primary = live.wait(&mut events, "the role at connect", |e| match e {
        Event::Primary { primary, .. } => Some(*primary),
        _ => None,
    });
    assert!(primary);

    // The owner hands the database to another laptop.
    let folder = live.dir.path().join("sync");
    std::fs::create_dir_all(&folder).unwrap();
    live.rt
        .block_on(live.client(None).sync_push(&SyncPushRequest {
            release: true,
            folder: Some(folder.display().to_string()),
        }))
        .unwrap();

    // The next write is refused, and the app hears why.
    let update = UpdateIssue {
        title: Some("Renamed".into()),
        ..Default::default()
    };
    backend.write("LLM-1", Write::Update(Box::new(update)), WriteKind::Status);
    let message = live.wait(&mut events, "the refused write", |e| match e {
        Event::WriteFailed { message, .. } => Some(message.clone()),
        _ => None,
    });
    assert!(message.contains("not the primary"), "{message}");
    let primary = live.wait(&mut events, "the new role", |e| match e {
        Event::Primary { primary, .. } => Some(*primary),
        _ => None,
    });
    assert!(!primary);
}

#[test]
fn backend_sends_an_issues_history() {
    let live = Live::new();
    let (backend, mut events) = live.backend();
    live.wait(&mut events, "a snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    backend.request(Request::History {
        key: "LLM-1".into(),
    });
    let history = live.wait(&mut events, "the history", |e| match e {
        Event::History { key, history, .. } if key == "LLM-1" => Some(history.clone()),
        _ => None,
    });
    assert!(
        history
            .activity
            .iter()
            .any(|e| e.field == llmtodo_client::types::ActivityField::Created)
    );
    assert_eq!(history.revisions.len(), 1);
    // LLM-179: the list carries no text for each version.
    assert_eq!(history.revisions[0].body, None);
    let md = crate::task::history::activity_markdown(&history);
    assert!(md.contains("**You** created the issue"), "{md}");
}

/// The sessions come with the task's detail, not with its history.
#[test]
fn backend_sends_the_agent_sessions_with_the_detail() {
    let live = Live::new();
    live.rt
        .block_on(
            live.client(Some("agent-session-1"))
                .claim("LLM-1", &ClaimRequest::default()),
        )
        .unwrap();
    let (backend, mut events) = live.backend();
    live.wait(&mut events, "a snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    backend.fetch("LLM-1");
    let sessions = live.wait(&mut events, "the detail", |e| match e {
        Event::IssueDetail(parts) => Some(parts.sessions.clone()),
        _ => None,
    });
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "agent-session-1");
    let md = crate::task::history::sessions_markdown(&sessions);
    assert!(md.contains("## claude-code opus-5.5 · high"), "{md}");
    assert!(md.contains("herdr pane `w1:p2`"), "{md}");
}

#[test]
fn backend_reads_and_saves_a_document_on_its_version() {
    use llmtodo_client::types::CreateDocument;

    let live = Live::new();
    let owner = live.client(None);
    let doc = live
        .rt
        .block_on(async {
            owner
                .create_project(
                    &serde_json::from_value(
                        json!({"name": "App", "slug": "app", "teams": ["LLM"]}),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            owner
                .create_document(
                    &serde_json::from_value::<CreateDocument>(
                        json!({"project": "app", "title": "Notes", "body": "First.\n"}),
                    )
                    .unwrap(),
                )
                .await
        })
        .unwrap();
    let (backend, mut events) = live.backend();
    let listed = live.wait(&mut events, "a snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.catalog.documents.clone()),
        _ => None,
    });
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Notes");

    backend.request(Request::Document { id: doc.id });
    let read = live.wait(&mut events, "the document", |e| match e {
        Event::Document(d) => Some(d.clone()),
        _ => None,
    });
    assert_eq!(read.body.as_deref(), Some("First.\n"));

    backend.request(Request::SaveDocument {
        id: doc.id,
        body: "Second.\n".into(),
        base: read.version,
        force: false,
    });
    let saved = live.wait(&mut events, "the save", |e| match e {
        Event::DocumentSaved(d) => Some(d.clone()),
        _ => None,
    });
    assert_eq!(saved.version, read.version + 1);

    // A save on the old version is refused as stale.
    backend.request(Request::SaveDocument {
        id: doc.id,
        body: "Third.\n".into(),
        base: read.version,
        force: false,
    });
    let stale = live.wait(&mut events, "the refusal", |e| match e {
        Event::DocumentFailed { stale, .. } => Some(*stale),
        _ => None,
    });
    assert!(stale);
}

/// LLM-99: closing a blocker changes the depth of every task behind it, and
/// those tasks get no activity event of their own.
#[test]
fn closing_a_blocker_updates_the_depth_of_its_dependents_live() {
    let live = Live::new();
    let owner = live.client(None);
    let create = |title: &str, blocked_by: Option<&str>| {
        let mut relations = std::collections::BTreeMap::new();
        if let Some(b) = blocked_by {
            relations.insert(RelationInput::BlockedBy, vec![b.to_string()]);
        }
        live.rt
            .block_on(owner.create_issue(&CreateIssue {
                title: title.into(),
                description: String::new(),
                team: Some("LLM".into()),
                status: None,
                priority: None,
                assignee: None,
                labels: Vec::new(),
                project: None,
                milestone: None,
                parent: None,
                wanted_executor: None,
                waiting_on: None,
                relations,
                estimate_minutes: None,
                estimate_cost_usd: None,
            }))
            .unwrap()
            .issue
            .key
            .to_string()
    };
    let first = create("First", None);
    let second = create("Second", Some(&first));
    let third = create("Third", Some(&second));

    let (backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    let done = UpdateIssue {
        status: Some("Done".into()),
        ..Default::default()
    };
    live.rt.block_on(owner.update_issue(&first, &done)).unwrap();
    let depths = live.wait(&mut events, "dependents", |e| match e {
        Event::Dependents(issues) => Some(
            issues
                .iter()
                .map(|i| (i.key.to_string(), i.depth))
                .collect::<std::collections::BTreeMap<_, _>>(),
        ),
        _ => None,
    });
    assert_eq!(depths[&second], 0);
    assert_eq!(depths[&third], 1);
    drop(backend);
}

/// LLM-214: a closed step stays held while a later step is open. When the
/// last step closes or opens again, the app refreshes the closed steps
/// before it, which get no event of their own.
#[test]
fn closing_the_last_step_refreshes_the_closed_steps_before_it_live() {
    let live = Live::new();
    let owner = live.client(None);
    let create = |title: &str, blocked_by: Option<&str>| {
        let mut relations = std::collections::BTreeMap::new();
        if let Some(b) = blocked_by {
            relations.insert(RelationInput::BlockedBy, vec![b.to_string()]);
        }
        live.rt
            .block_on(owner.create_issue(&CreateIssue {
                title: title.into(),
                description: String::new(),
                team: Some("LLM".into()),
                status: None,
                priority: None,
                assignee: None,
                labels: Vec::new(),
                project: None,
                milestone: None,
                parent: None,
                wanted_executor: None,
                waiting_on: None,
                relations,
                estimate_minutes: None,
                estimate_cost_usd: None,
            }))
            .unwrap()
            .issue
            .key
            .to_string()
    };
    let first = create("First", None);
    let second = create("Second", Some(&first));
    let third = create("Third", Some(&second));
    let set = |key: &str, status: &str| {
        let update = UpdateIssue {
            status: Some(status.into()),
            ..Default::default()
        };
        live.rt.block_on(owner.update_issue(key, &update)).unwrap();
    };
    set(&first, "Done");
    set(&second, "Done");

    let (backend, mut events) = live.backend();
    let held = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => s
            .issues
            .iter()
            .find(|i| i.key.as_ref() == first)
            .map(|i| i.has_open_dependents),
        _ => None,
    });
    assert!(held, "the open third step holds the first");

    let flags = |events: &mut _, label: &str| {
        live.wait(events, label, |e| match e {
            Event::Dependents(issues) => Some(
                issues
                    .iter()
                    .map(|i| (i.key.to_string(), i.has_open_dependents))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            ),
            _ => None,
        })
    };
    set(&third, "Done");
    let closed = flags(&mut events, "dependents after the close");
    assert_eq!(closed.get(&first), Some(&false));
    assert_eq!(closed.get(&second), Some(&false));

    set(&third, "Todo");
    let reopened = flags(&mut events, "dependents after the reopen");
    assert_eq!(reopened.get(&first), Some(&true));
    assert_eq!(reopened.get(&second), Some(&true));
    drop(backend);
}

/// LLM-208: an order change writes one event, on the parent. The app
/// refreshes every sub-issue, so the steps, depths and open blockers show.
#[test]
fn an_order_change_refreshes_the_parents_sub_issues_live() {
    let live = Live::new();
    let owner = live.client(None);
    let create = |title: &str, parent: Option<&str>| {
        live.rt
            .block_on(owner.create_issue(&CreateIssue {
                title: title.into(),
                description: String::new(),
                team: Some("LLM".into()),
                status: None,
                priority: None,
                assignee: None,
                labels: Vec::new(),
                project: None,
                milestone: None,
                parent: parent.map(Into::into),
                wanted_executor: None,
                waiting_on: None,
                relations: Default::default(),
                estimate_minutes: None,
                estimate_cost_usd: None,
            }))
            .unwrap()
            .issue
            .key
            .to_string()
    };
    let epic = create("Epic", None);
    let first = create("First", Some(&epic));
    let second = create("Second", Some(&epic));
    let third = create("Third", Some(&epic));

    let (backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    let key = |k: &str| -> llmtodo_client::types::IssueKey { k.parse().unwrap() };
    let order = llmtodo_api_types::SetOrder {
        steps: vec![vec![key(&first)], vec![key(&second), key(&third)]],
    };
    live.rt.block_on(owner.set_order(&epic, &order)).unwrap();
    let mut seen = std::collections::BTreeMap::new();
    live.wait(&mut events, "the sub-issues", |e| {
        for issue in issues_in(e) {
            seen.insert(
                issue.key.to_string(),
                (issue.order_step, issue.depth, issue.open_blockers.len()),
            );
        }
        [&first, &second, &third]
            .iter()
            .all(|k| seen.contains_key(*k))
            .then_some(())
    });
    assert_eq!(seen[&first], (Some(1), 0, 0));
    assert_eq!(seen[&second], (Some(2), 1, 1));
    assert_eq!(seen[&third], (Some(2), 1, 1));
    drop(backend);
}

/// LLM-209: `:step` sends the whole order as one write on the parent.
#[test]
fn an_order_write_from_the_app_reaches_the_server_and_comes_back() {
    let live = Live::new();
    let owner = live.client(None);
    let create = |title: &str, parent: Option<&str>| {
        live.rt
            .block_on(owner.create_issue(&CreateIssue {
                title: title.into(),
                description: String::new(),
                team: Some("LLM".into()),
                status: None,
                priority: None,
                assignee: None,
                labels: Vec::new(),
                project: None,
                milestone: None,
                parent: parent.map(Into::into),
                wanted_executor: None,
                waiting_on: None,
                relations: Default::default(),
                estimate_minutes: None,
                estimate_cost_usd: None,
            }))
            .unwrap()
            .issue
            .key
            .to_string()
    };
    let epic = create("Epic", None);
    let first = create("First", Some(&epic));
    let second = create("Second", Some(&epic));
    let third = create("Third", Some(&epic));

    let (backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });

    let key = |k: &str| -> llmtodo_client::types::IssueKey { k.parse().unwrap() };
    let order = llmtodo_api_types::SetOrder {
        steps: vec![vec![key(&first)], vec![key(&second), key(&third)]],
    };
    backend.write(&epic, Write::SetOrder(order), WriteKind::Order);
    live.wait(&mut events, "the write", |e| match e {
        Event::WriteDone { key, .. } if *key == epic => Some(()),
        Event::WriteFailed { message, .. } => panic!("{message}"),
        _ => None,
    });
    let mut seen = std::collections::BTreeMap::new();
    live.wait(&mut events, "the sub-issues", |e| {
        for issue in issues_in(e) {
            seen.insert(
                issue.key.to_string(),
                (issue.order_step, issue.depth, issue.open_blockers.len()),
            );
        }
        [&first, &second, &third]
            .iter()
            .all(|k| seen.contains_key(*k))
            .then_some(())
    });
    assert_eq!(seen[&first], (Some(1), 0, 0));
    assert_eq!(seen[&second], (Some(2), 1, 1));
    assert_eq!(seen[&third], (Some(2), 1, 1));
    drop(backend);
}

/// The snapshot carries the open owner questions, so the inbox needs no
/// description.
#[test]
fn backend_sends_open_questions_with_the_snapshot() {
    let live = Live::new();
    let agent = live.client(Some("agent-ask"));
    let ask = UpdateIssue {
        waiting_on: llmtodo_client::types::Change::Set(llmtodo_client::types::WaitingOn::Owner),
        description: llmtodo_client::types::BodyChange {
            patches: vec![llmtodo_core::Patch::AskQuestion {
                question: "Ship it?".into(),
                context: String::new(),
                options: Vec::new(),
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    live.rt.block_on(agent.update_issue("LLM-1", &ask)).unwrap();
    let (_backend, mut events) = live.backend();
    let snapshot = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.clone()),
        _ => None,
    });
    assert_eq!(snapshot.issues[0].description, None);
    assert_eq!(snapshot.issues[0].open_questions, 1);
    let asked: Vec<_> = snapshot
        .questions
        .iter()
        .map(|e| (e.key.to_string(), e.question.text.clone()))
        .collect();
    assert_eq!(asked, [("LLM-1".to_string(), "Ship it?".to_string())]);
}

/// LLM-138: the owner approves a ticket in User review. The thread sends
/// the closed issue with its review rounds, then the write's result.
#[test]
fn backend_sends_a_review_answer_and_the_rounds() {
    let live = Live::new();
    let owner = live.client(None);
    live.rt.block_on(async {
        owner
            .create_status(
                "LLM",
                &serde_json::from_value(json!({"name": "User review", "type": "started", "review": true}))
                    .unwrap(),
            )
            .await
            .unwrap();
        owner
            .update_issue(
                "LLM-1",
                &serde_json::from_value(json!({
                    "status": "User review",
                    "patches": [{ "op": "upsert_collapsible", "section": "Review",
                                  "body": "### Steps to reproduce\n1. Open it.\n\n### What to check\n- It opens.\n" }]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
    });
    let (backend, mut events) = live.backend();
    let id = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.issues[0].id),
        _ => None,
    });
    // The panel shows LLM-1, so the write's answer carries its rounds.
    backend.request(Request::Open { id: Some(id) });
    backend.write(
        "LLM-1",
        Write::Review(llmtodo_api_types::SubmitReview {
            verdict: llmtodo_api_types::ReviewVerdict::Approved,
            feedback: "Good.".into(),
        }),
        WriteKind::Review,
    );
    // One event carries the closed issue and its rounds.
    let parts = live.wait(&mut events, "the closed issue", |e| match e {
        Event::IssueDetail(parts) if parts.issue.status.name == "Done" => Some(parts.clone()),
        _ => None,
    });
    assert_eq!(parts.issue.waiting_on, None);
    let reviews = parts.reviews;
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].feedback, "Good.");
    live.wait(&mut events, "the write's result", |e| match e {
        Event::WriteDone { kind, .. } => (*kind == WriteKind::Review).then_some(()),
        _ => None,
    });
}

/// LLM-171: a live event on a task the panel does not show sends one list
/// copy with its text, and no detail or git panel. Once the panel shows it,
/// the same kind of event brings its full detail in one event.
#[test]
fn only_the_open_task_gets_its_detail_on_live_events() {
    let live = Live::new();
    let agent = live.client(Some("agent-171"));
    let (backend, mut events) = live.backend();
    let id = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.issues[0].id),
        _ => None,
    });
    let set_text = |text: &str| UpdateIssue {
        description: BodyChange {
            body: Some(text.into()),
            force: true,
            ..Default::default()
        },
        ..Default::default()
    };

    live.rt
        .block_on(agent.update_issue("LLM-1", &set_text("Not open.\n")))
        .unwrap();
    let row = live.wait(&mut events, "the list copy", |e| match e {
        Event::IssueDetail(_) => Some(Err("a detail for a task that is not open")),
        Event::Issues(rows) => Some(Ok(rows.clone())),
        _ => None,
    });
    let rows = row.unwrap();
    assert_eq!(rows.len(), 1);
    // The text comes along, so the inbox sees new questions.
    assert_eq!(rows[0].description.as_deref(), Some("Not open.\n"));

    backend.request(Request::Open { id: Some(id) });
    live.rt
        .block_on(agent.update_issue("LLM-1", &set_text("Open now.\n")))
        .unwrap();
    let parts = live.wait(&mut events, "the detail", |e| match e {
        Event::Issues(_) => Some(Err("a list copy for the open task")),
        Event::IssueDetail(parts) => Some(Ok(parts.clone())),
        _ => None,
    });
    let parts = parts.unwrap();
    assert_eq!(parts.issue.description.as_deref(), Some("Open now.\n"));
}

/// LLM-172: the owner's write reaches the server while a slow live refresh
/// still runs, instead of waiting behind it.
#[test]
fn a_write_does_not_wait_behind_a_live_refresh() {
    let live = Live::new();
    let agent = live.client(Some("agent-172"));
    let (backend, mut events) = live.backend_with(false, Duration::from_secs(3));
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    let rename = UpdateIssue {
        title: Some("Renamed by an agent".into()),
        ..Default::default()
    };
    live.rt
        .block_on(agent.update_issue("LLM-1", &rename))
        .unwrap();
    // The refresh starts after the settle wait, then runs for 3 seconds.
    std::thread::sleep(Duration::from_millis(300));
    let started = std::time::Instant::now();
    let status = UpdateIssue {
        status: Some("In Progress".into()),
        ..Default::default()
    };
    backend.write("LLM-1", Write::Update(Box::new(status)), WriteKind::Status);
    let first = live.wait(&mut events, "the write or the refresh", |e| match e {
        Event::WriteDone { .. } => Some("write"),
        Event::Issues(_) => Some("refresh"),
        _ => None,
    });
    assert_eq!(first, "write");
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// LLM-172: events that keep coming less than the settle wait apart still
/// get their refresh, soon after the first one.
#[test]
fn a_steady_event_stream_still_refreshes() {
    let live = Live::new();
    let agent = live.client(Some("agent-172b"));
    let (_backend, mut events) = live.backend();
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = stop.clone();
        live.rt.spawn(async move {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) && n < 400 {
                let rename = UpdateIssue {
                    title: Some(format!("Title {n}")),
                    ..Default::default()
                };
                agent.update_issue("LLM-1", &rename).await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
                n += 1;
            }
            n
        })
    };
    let started = std::time::Instant::now();
    live.wait(&mut events, "a refresh during the stream", |e| {
        matches!(e, Event::Issues(_)).then_some(())
    });
    let waited = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let writes = live.rt.block_on(writer).unwrap();
    assert!(writes < 400, "the stream was still running");
    assert!(waited < Duration::from_secs(1), "waited {waited:?}");
}

/// LLM-179: a live event on the open task brings its detail, then its
/// history, once. The history needs no second detail fetch.
#[test]
fn a_live_event_on_the_open_task_sends_its_history_once() {
    let live = Live::new();
    let agent = live.client(Some("agent-179"));
    let (backend, mut events) = live.backend();
    let id = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.raw.issues[0].id),
        _ => None,
    });
    backend.request(Request::Open { id: Some(id) });
    let rename = UpdateIssue {
        title: Some("Renamed for history".into()),
        ..Default::default()
    };
    live.rt
        .block_on(agent.update_issue("LLM-1", &rename))
        .unwrap();
    live.wait(&mut events, "the detail", |e| {
        matches!(e, Event::IssueDetail(_)).then_some(())
    });
    let history = live.wait(&mut events, "the history", |e| match e {
        Event::History { history, .. } => Some(history.clone()),
        _ => None,
    });
    assert!(
        history
            .activity
            .iter()
            .any(|e| e.field == llmtodo_client::types::ActivityField::Title)
    );
}

/// LLM-177: the snapshot reaches the app with its issues already mapped to
/// the app's catalog, so the main thread does not map them.
#[test]
fn the_snapshot_arrives_mapped() {
    let live = Live::new();
    let (_backend, mut events) = live.backend();
    let loaded = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.clone()),
        _ => None,
    });
    assert_eq!(loaded.issues.len(), loaded.raw.issues.len());
    let issue = &loaded.issues[0];
    assert_eq!(issue.key.as_ref(), "LLM-1");
    assert_eq!(issue.id, loaded.raw.issues[0].id);
    assert_eq!(issue.title.as_ref(), "App on live data");
    assert!(loaded.catalog.teams.iter().any(|t| t.0.as_ref() == "LLM"));
}
