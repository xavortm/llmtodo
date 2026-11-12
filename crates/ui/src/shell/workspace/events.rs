//! How the workspace applies what the API thread hears.

use super::*;
use crate::model::pipeline::FailedSave;

impl Workspace {
    /// Applies one message from the API thread.
    pub fn apply_event(&mut self, event: backend::Event, cx: &mut Context<Self>) {
        match event {
            backend::Event::Connected { url } => {
                self.store.update(cx, |store, _| {
                    store.server_url = Some(url.clone().into());
                });
                let url = url.trim_start_matches("http://").to_string();
                self.message = format!("Connected to {url}").into();
                self.set_connection(Connection::Online(url.into()), cx);
                self.probe_trackers(cx);
            }
            backend::Event::Teams { data_dir, teams } => self.on_teams(data_dir, teams, cx),
            backend::Event::Offline { reason } => {
                self.message = format!("Offline: {reason}. Retrying").into();
                self.set_connection(Connection::Offline(reason.into()), cx);
            }
            backend::Event::Snapshot(snapshot) => {
                let catalog = catalog_of(&snapshot.catalog);
                let issues: Vec<Issue> = snapshot
                    .issues
                    .iter()
                    .map(|i| Issue::from_api(i, &catalog))
                    .collect();
                self.load_questions(&issues, &snapshot.questions, cx);
                self.load(catalog, issues, cx);
                self.loaded_tracker(cx);
            }
            backend::Event::Catalog(data) => self.apply_catalog(catalog_of(&data), cx),
            backend::Event::Issue(issue) => self.receive_rows(vec![*issue], cx),
            backend::Event::Issues(issues) => self.receive_rows(issues, cx),
            backend::Event::IssueDetail(parts) => {
                let backend::DetailParts {
                    issue,
                    relations,
                    reviews,
                    git,
                    sessions,
                } = *parts;
                let issue = Issue::from_api(&issue, &self.store.read(cx).catalog);
                let id = issue.id;
                self.store.update(cx, |store, cx| {
                    store.set_detail(id, relations, reviews, git, sessions);
                    cx.notify();
                });
                self.receive_issue(issue, cx);
                // The API thread sends the open task's history itself,
                // after the live events that change it.
                self.show_sessions(cx);
            }
            backend::Event::ViewIssues { name, ids } => self.on_view_issues(name, ids, cx),
            backend::Event::ViewSaved { name } => self.on_view_saved(name, cx),
            backend::Event::ViewDeleted { name } => self.on_view_deleted(name, cx),
            backend::Event::History { key, history } => self.on_history(key, history, cx),
            backend::Event::Document(doc) => {
                self.doc_view.update(cx, |view, cx| view.set_doc(*doc, cx));
            }
            backend::Event::DocumentSaved(doc) => self.on_document_saved(doc, cx),
            backend::Event::DocumentFailed { id, message, stale } => {
                self.on_document_failed(id, message, stale, cx)
            }
            backend::Event::Primary { primary, holder } => {
                let was = self.not_primary.is_some();
                self.not_primary = (!primary).then(|| holder.map(Into::into));
                if was && primary {
                    self.message = "This machine is the primary copy again".into();
                }
            }
            backend::Event::Created { id, key } => {
                self.message = format!("Created {key}").into();
                self.select_created(id, &key, cx);
            }
            backend::Event::CreateFailed { title, message } => {
                self.message = format!("Could not create \"{title}\": {message}").into();
            }
            backend::Event::ViewFailed { name, message } => {
                self.message = format!("View {name}: {message}").into();
            }
            backend::Event::Dependents(issues) => {
                self.store.update(cx, |store, cx| {
                    for api in &issues {
                        store.modify(api.id, |issue| {
                            issue.depth = api.depth;
                            issue.open_blockers = crate::model::data::Blocker::from_api(api);
                        });
                    }
                    cx.notify();
                });
            }
            backend::Event::WriteDone { key, kind, version } => {
                self.on_write_done(key, kind, version, cx)
            }
            backend::Event::WriteFailed {
                key,
                kind,
                message,
                stale,
            } => self.on_write_failed(key, kind, message, stale, cx),
        }
        cx.notify();
    }

    /// Puts in list copies of issues. They carry no detail, so the detail
    /// the store holds for them may be old now.
    fn receive_rows(&mut self, rows: Vec<llmtodo_client::types::Issue>, cx: &mut Context<Self>) {
        let issues: Vec<Issue> = {
            let catalog = &self.store.read(cx).catalog;
            rows.iter().map(|i| Issue::from_api(i, catalog)).collect()
        };
        self.store.update(cx, |store, _| {
            for issue in &issues {
                store.mark_detail_stale(issue.id);
            }
        });
        for issue in issues {
            self.receive_issue(issue, cx);
        }
    }

    /// Shows the open task's agent sessions, once a fetch brought them.
    pub(super) fn show_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.history_for else {
            return;
        };
        let Some(sessions) = self.store.read(cx).sessions(id) else {
            return;
        };
        let costs = crate::task::history::cost_breakdown(sessions);
        let sessions = crate::task::history::sessions_markdown(sessions);
        self.task.update(cx, |task, cx| {
            task.set_sessions(id, sessions, costs, cx);
        });
    }

    /// An answer about another tracker's teams.
    fn on_teams(&mut self, data_dir: PathBuf, teams: Option<Vec<String>>, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            let open = store.tracker;
            for (ix, tracker) in store.trackers.iter_mut().enumerate() {
                // The open tracker's teams come from its snapshot.
                if ix != open && tracker.data_dir == data_dir {
                    tracker.teams = teams
                        .clone()
                        .map(|t| t.into_iter().map(SharedString::from).collect());
                }
            }
            cx.notify();
        });
    }

    /// Stores which issues a saved view holds, when that view is open.
    fn on_view_issues(&mut self, name: String, ids: Vec<IssueId>, cx: &mut Context<Self>) {
        let name: SharedString = name.into();
        if self.list.read(cx).scope == Scope::View(name.clone()) {
            self.store.update(cx, |store, cx| {
                store.view = Some(ViewMembers {
                    name,
                    ids: ids.into_iter().collect(),
                });
                cx.notify();
            });
            if std::mem::take(&mut self.check_view_task) {
                let scope = self.list.read(cx).scope.clone();
                self.close_task_outside(&scope, cx);
            }
            self.list_do(cx, |list, cx| list.refilter(cx));
        }
    }

    /// The server saved a view: list it and show it.
    fn on_view_saved(&mut self, name: String, cx: &mut Context<Self>) {
        let name: SharedString = name.into();
        self.store.update(cx, |store, cx| {
            cx.notify();
            let views = &mut store.catalog.views;
            if !views.iter().any(|v| v.key == name) {
                views.push(crate::model::data::Place {
                    key: name.clone(),
                    name: name.clone(),
                });
                views.sort_by_key(|v| v.name.to_lowercase());
            }
        });
        self.rebuild_commands(cx);
        self.set_scope(Scope::View(name.clone()), cx);
        self.message = format!("Saved the view {name}").into();
    }

    /// The server deleted a view. The list leaves it when it was open.
    fn on_view_deleted(&mut self, name: String, cx: &mut Context<Self>) {
        let name: SharedString = name.into();
        self.store.update(cx, |store, cx| {
            cx.notify();
            store.catalog.views.retain(|v| v.key != name);
        });
        self.rebuild_commands(cx);
        if self.list.read(cx).scope == Scope::View(name.clone()) {
            self.set_scope(Scope::All, cx);
        }
        self.message = format!("Deleted the view {name}").into();
    }

    /// Shows a task's history, when that task is still the open one.
    fn on_history(
        &mut self,
        key: String,
        history: Box<llmtodo_client::types::History>,
        cx: &mut Context<Self>,
    ) {
        let id = self.store.read(cx).by_key(&key).map(|i| i.id);
        if let Some(id) = id
            && Some(id) == self.history_for
        {
            let activity = crate::task::history::activity_markdown(&history);
            self.task
                .update(cx, |task, cx| task.set_activity(id, activity, cx));
        }
    }

    /// The server saved a document.
    fn on_document_saved(
        &mut self,
        doc: Box<llmtodo_client::types::Document>,
        cx: &mut Context<Self>,
    ) {
        let title = doc.title.clone();
        let version = doc.version;
        self.doc_draft = None;
        self.doc_stale = false;
        self.doc_view.update(cx, |view, cx| {
            if view.editing {
                view.base = version;
            }
            view.set_doc(*doc, cx);
        });
        self.message = format!("Saved {title}").into();
    }

    /// The server refused a document save, or the save failed.
    fn on_document_failed(
        &mut self,
        id: llmtodo_core::DocumentId,
        message: String,
        stale: bool,
        cx: &mut Context<Self>,
    ) {
        let open = self.doc_view.read(cx).open == Some(id);
        self.doc_stale |= stale;
        self.message = if stale {
            "The document changed on the server while you edited it, so your text was not saved. \
             It is kept: press e to get it back, then :w! to overwrite the other version"
                .to_string()
        } else {
            format!("Document: {message}")
        }
        .into();
        if open && stale {
            // Show the server's text in the preview; E brings the
            // owner's text back.
            self.request(backend::Request::Document { id });
        }
    }

    /// A write reached the server. A description save moves the versions
    /// that later saves build on, and sends the save queued behind it.
    fn on_write_done(
        &mut self,
        key: String,
        kind: WriteKind,
        version: i64,
        cx: &mut Context<Self>,
    ) {
        let key: SharedString = key.into();
        self.write_returned(&key, cx);
        if kind == WriteKind::UndoAnswer {
            self.undo_answer_done(&key, cx);
        }
        if kind == WriteKind::Review {
            self.review_done(&key, cx);
        }
        if kind == WriteKind::Description {
            let id = self.store.read(cx).by_key(&key).map(|i| i.id);
            let saved = self.pipeline.description_saved(&key, id, version);
            self.external.saved(&key, saved.sent.as_deref(), version);
            if let Some(next) = saved.next
                && let Some(id) = id
            {
                self.send_description(id, next.text, version, next.force, cx);
            }
        }
    }

    /// A write failed. A failed description save keeps the owner's text.
    fn on_write_failed(
        &mut self,
        key: String,
        kind: WriteKind,
        message: String,
        stale: bool,
        cx: &mut Context<Self>,
    ) {
        let key: SharedString = key.into();
        self.write_returned(&key, cx);
        match kind {
            WriteKind::UndoAnswer => {
                self.undoing = None;
                self.message = format!("Could not undo: {message}").into();
                return;
            }
            WriteKind::Answer => self.drop_answer_undo_for(&key, cx),
            WriteKind::Review => {
                self.review_failed(&key, message, cx);
                return;
            }
            _ => {}
        }
        let outcome = if kind == WriteKind::Description {
            self.pipeline.description_failed(&key, stale)
        } else {
            FailedSave::NothingKept
        };
        match outcome {
            FailedSave::NewerTextKept => {
                self.message = format!(
                    "Could not save {key}: {message}. \
                     Your latest text is kept: press e after the claim ends"
                )
                .into();
            }
            FailedSave::Conflict => {
                // The backend fetches the server's copy after a
                // refused write. An open editor on this task turns
                // into the conflict view, with that copy beside it.
                let open_here = self.task.read(cx).editing()
                    && self
                        .detail(cx)
                        .and_then(|d| self.store.read(cx).get(d.issue))
                        .is_some_and(|i| i.key == key);
                self.message = if open_here {
                    self.task.update(cx, |task, cx| task.set_conflict(true, cx));
                    conflict_help(&key)
                } else {
                    format!(
                        "{key} changed on the server while you edited it, so your text was not saved. \
                         It is kept: press e to compare both versions"
                    )
                }
                .into();
            }
            FailedSave::TextKept => {
                self.message = format!(
                    "Could not save {key}: {message}. \
                     Your text is kept: press e to get it back, then :w to try again"
                )
                .into();
            }
            FailedSave::NothingKept => {
                self.message = format!("Could not save {key}: {message}").into();
            }
        }
    }

    /// Puts in the server's open questions for a snapshot of `issues`.
    /// Issues with writes on their way keep the questions the app shows.
    fn load_questions(
        &mut self,
        issues: &[Issue],
        entries: &[llmtodo_client::types::QuestionEntry],
        cx: &mut Context<Self>,
    ) {
        let ids: std::collections::HashMap<String, IssueId> =
            issues.iter().map(|i| (i.key.to_string(), i.id)).collect();
        let mut list: std::collections::HashMap<IssueId, Vec<llmtodo_core::Question>> =
            Default::default();
        for e in entries {
            if let Some(id) = ids.get(&e.key.to_string()) {
                list.entry(*id).or_default().push(e.question.clone());
            }
        }
        let pipeline = &self.pipeline;
        self.store.update(cx, |store, cx| {
            cx.notify();
            store.set_open_questions(list, |id| pipeline.has_writes(id));
        });
    }
}
