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

use crate::model::backend::{Backend, Event, Target, Write, WriteKind};
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
        Backend::start(Target {
            data_dir: self.dir.path().to_path_buf(),
            spawn: None,
            read_only: false,
        })
    }

    fn read_only_backend(&self) -> (Backend, UnboundedReceiver<Event>) {
        Backend::start(Target {
            data_dir: self.dir.path().to_path_buf(),
            spawn: None,
            read_only: true,
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
        Event::Issue(i) => (**i).clone(),
        Event::Snapshot(s) => s
            .issues
            .iter()
            .find(|i| i.key.to_string() == "LLM-1")?
            .clone(),
        _ => return None,
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

fn issue_event(event: &Event) -> Option<llmtodo_client::types::Issue> {
    match event {
        Event::Issue(issue) => Some((**issue).clone()),
        _ => None,
    }
}

/// LLM-137: a read-only backend loads a real server's data, and no write
/// reaches that server.
#[test]
fn a_read_only_backend_reads_but_cannot_write() {
    use crate::model::backend::Request;
    let live = Live::new();
    let (backend, mut events) = live.read_only_backend();
    let snapshot = live.wait(&mut events, "snapshot", |e| match e {
        Event::Snapshot(s) => Some(s.clone()),
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
        Event::Snapshot(s) => Some(s.clone()),
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
        Event::Issue(i) => Some(Ok(i.claim.clone())),
        Event::Snapshot(_) => Some(Err(())),
        _ => None,
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
        Event::Issue(i) => i.claim.clone(),
        Event::Snapshot(s) => s.issues[0].claim.clone(),
        _ => None,
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
        Event::Git { git, .. } => Some((**git).clone()),
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
    use crate::model::backend::Request;
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
        Event::Snapshot(s) => Some(s.clone()),
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

#[test]
fn backend_creates_an_issue_and_names_it() {
    use crate::model::backend::Request;
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
    let issue = live.wait(&mut events, "the new issue", |e| match e {
        Event::Issue(i) if i.title == "From the app" => Some((**i).clone()),
        _ => None,
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
    use crate::model::backend::Request;

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
    let md = crate::task::history::activity_markdown(&history);
    assert!(md.contains("**You** created the issue"), "{md}");
}

#[test]
fn backend_sends_the_agent_sessions_with_the_history() {
    use crate::model::backend::Request;

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
    backend.request(Request::History {
        key: "LLM-1".into(),
    });
    let sessions = live.wait(&mut events, "the history", |e| match e {
        Event::History { sessions, .. } => Some(sessions.clone()),
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
    use crate::model::backend::Request;
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
        Event::Snapshot(s) => Some(s.catalog.documents.clone()),
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
        Event::Snapshot(s) => Some(s.clone()),
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
/// the closed issue, then its review rounds, then the write's result.
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
    live.wait(&mut events, "snapshot", |e| {
        matches!(e, Event::Snapshot(_)).then_some(())
    });
    backend.write(
        "LLM-1",
        Write::Review(llmtodo_api_types::SubmitReview {
            verdict: llmtodo_api_types::ReviewVerdict::Approved,
            feedback: "Good.".into(),
        }),
        WriteKind::Review,
    );
    let issue = live.wait(&mut events, "the closed issue", |e| {
        issue_event(e).filter(|i| i.status.name == "Done")
    });
    assert_eq!(issue.waiting_on, None);
    let reviews = live.wait(&mut events, "the rounds", |e| match e {
        Event::Reviews { reviews, .. } => Some(reviews.clone()),
        _ => None,
    });
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].feedback, "Good.");
    live.wait(&mut events, "the write's result", |e| match e {
        Event::WriteDone { kind, .. } => (*kind == WriteKind::Review).then_some(()),
        _ => None,
    });
}
