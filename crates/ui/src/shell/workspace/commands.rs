//! The `:` palette: parsing a command and running it, on one task or on
//! every marked task.

use super::*;

impl Workspace {
    pub fn run_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut words = command.trim().trim_start_matches(':').split_whitespace();
        let verb = words.next().unwrap_or_default();
        let arg = words.collect::<Vec<_>>().join(" ");
        // LLM-152: a bare status name, such as `:done`, means `:status done`.
        let whole = command.trim().trim_start_matches(':').trim();
        let (verb, arg) = if self.is_status_word(verb, whole, cx) {
            ("status", whole.to_string())
        } else {
            (verb, arg)
        };
        if let Some(message) = self.run_bulk(verb, &arg, cx) {
            self.message = message.into();
            cx.notify();
            return;
        }
        if let Some(message) = self.run_verb(command, verb, &arg, window, cx) {
            self.message = message.into();
        }
        cx.notify();
    }

    /// Whether `whole`, the full command, names a status of some team, such
    /// as `done` or `in-progress`. A command word always wins.
    fn is_status_word(&self, verb: &str, whole: &str, cx: &App) -> bool {
        let is_command = COMMANDS
            .iter()
            .any(|c| c.split_whitespace().next() == Some(verb));
        if verb.is_empty() || is_command {
            return false;
        }
        let catalog = &self.store.read(cx).catalog;
        catalog
            .teams
            .iter()
            .any(|(team, _)| catalog.find_status(team, whole).is_some())
    }

    /// Runs one palette command. Returns the message to show, or `None`
    /// when the command already set its own. The arms run in order, and
    /// some guards depend on it.
    fn run_verb(
        &mut self,
        command: &str,
        verb: &str,
        arg: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let target = self.target(cx);
        let catalog = self.store.read(cx).catalog.clone();
        let team = target
            .and_then(|id| self.store.read(cx).get(id))
            .map(|i| i.team.clone())
            .unwrap_or_default();
        match (verb, target) {
            ("status", Some(id)) => match catalog.find_status(&team, arg) {
                Some(status) => {
                    self.set_status(id, status, cx);
                    None
                }
                None => Some(format!("Unknown status: {arg}")),
            },
            ("label", Some(id)) => match catalog.find_label(arg) {
                Some(label) => {
                    self.toggle_label(id, label, cx);
                    None
                }
                None => Some(format!("Unknown label: {arg}")),
            },
            ("wait", Some(id)) => match parse_waiting_on(arg) {
                Some(waiting) => {
                    self.set_waiting_on(id, waiting, cx);
                    None
                }
                None => Some(format!("Wait on agent, owner or none, not {arg}")),
            },
            ("copy" | "yank", Some(id)) => {
                self.copy_key(id, cx);
                None
            }
            ("delete" | "trash", Some(id)) => {
                self.delete_issue(id, cx);
                None
            }
            ("tab", Some(_)) => Some(self.command_tab(arg, cx)),
            ("release" | "unlock", Some(id)) => {
                self.release(id, cx);
                None
            }
            ("new" | "create", _) => {
                self.create_issue(arg, "", window, cx);
                None
            }
            ("sync", _) => Some(match &self.backend {
                Some(backend) => {
                    backend.request(backend::Request::SyncStatus);
                    "Checking which machine is the primary copy".into()
                }
                None => "No server to ask".into(),
            }),
            ("sidebar", _) => {
                self.set_sidebar_open(!self.sidebar_shown(), window, cx);
                Some(String::new())
            }
            ("all", _) => {
                self.set_scope(Scope::All, cx);
                None
            }
            ("tracker", _) => self.command_tracker(arg, cx),
            ("team", _) => match Self::find_place(&catalog.team_names, arg) {
                Some(key) => {
                    self.set_scope(Scope::Team(key), cx);
                    None
                }
                None => Some(format!("Unknown team: {arg}")),
            },
            ("view", _) => self.command_view(arg, &catalog, cx),
            ("show", _) if arg == "trash" => {
                self.set_scope(Scope::Trash, cx);
                None
            }
            ("show", _) if arg == "all" => {
                self.set_scope(Scope::All, cx);
                None
            }
            ("project", _) => match Self::find_place(&catalog.projects, arg) {
                Some(slug) => {
                    self.set_scope(Scope::Project(slug), cx);
                    None
                }
                None => Some(format!("Unknown project: {arg}")),
            },
            ("claims", _) => {
                self.show_claims(&ShowClaims, window, cx);
                Some(String::new())
            }
            ("inbox", _) => {
                self.show_inbox(&ShowInbox, window, cx);
                None
            }
            ("settings", _) => {
                self.show_settings(window, cx);
                None
            }
            ("depends" | "blocks" | "undepend" | "unblock", Some(id)) => {
                self.change_dependency(id, verb, arg, cx);
                None
            }
            ("help", _) => {
                self.help_open = !self.help_open;
                Some(String::new())
            }
            ("open" | "goto", _) if !arg.is_empty() => self.command_open(arg, window, cx),
            ("reload", _) => Some(match &self.backend {
                Some(backend) => {
                    backend.reload();
                    "Reloading from the server".into()
                }
                None => "No server to reload from".into(),
            }),
            ("restore", _) => {
                self.restore_issue(cx);
                None
            }
            ("assign" | "unassign", Some(id)) if !self.editable(id, cx) => None,
            ("assign", Some(id)) if !arg.is_empty() => {
                self.assign(id, Some(arg.to_string()), cx);
                None
            }
            ("sort", _) => Some(self.command_sort(arg, cx)),
            ("filter", _) => Some(self.command_filter(arg, window, cx)),
            ("group", _) => Some(match GroupBy::parse(arg) {
                Some(group) => {
                    self.group_by(group, cx);
                    format!("Grouped by {}", group.label().to_lowercase())
                }
                None => format!("Unknown group: {arg}"),
            }),
            ("unassign", Some(id)) => {
                self.assign(id, None, cx);
                None
            }
            ("w", _) => {
                self.save(&Save, window, cx);
                None
            }
            ("w!", _) if self.doc_view.read(cx).editing => {
                self.save_document(true, cx);
                None
            }
            ("w!", _) => {
                if !self.refuse_read_only_tab(cx) {
                    self.save_description(true, cx);
                }
                None
            }
            ("q" | "wq" | "x", _) if self.doc_view.read(cx).editing => {
                self.leave_doc_editor(window, cx);
                None
            }
            ("doc", _) => self.command_doc(arg, &catalog, cx),
            ("mine", _) if self.editing(cx) => {
                self.keep_mine(window, cx);
                None
            }
            ("mine", _) => Some("Open the editor first: press e".into()),
            ("theirs", _) => Some(self.keep_theirs(window, cx)),
            ("external", Some(_)) => {
                self.edit_external(&EditExternal, window, cx);
                None
            }
            ("q" | "wq" | "x", _) if self.editing(cx) => {
                self.save_editor(cx);
                self.focus_list(window, cx);
                Some(String::new())
            }
            ("edit", Some(_)) => {
                self.edit_description(&EditDescription, window, cx);
                Some(String::new())
            }
            ("theme", _) if arg == "dark" || arg == "light" => {
                self.set_dark(arg == "dark", window, cx);
                Some(format!("Theme: {arg}"))
            }
            // Drops the task view width the owner dragged to, so the list
            // and the task view share the room by content again.
            ("split", _) if arg == "auto" => {
                self.layout.split_sizes.0 = None;
                self.layout.auto_split = None;
                self.persist(cx);
                cx.notify();
                Some("The list and the task view now share the width by content".into())
            }
            ("", _) => Some(String::new()),
            _ => Some(format!("Unknown command: {}", command.trim())),
        }
    }

    /// `:tab`: picks a tab of the open task by number, name or name start.
    fn command_tab(&mut self, arg: &str, cx: &mut Context<Self>) -> String {
        let Some((_, tabs)) = self.open_tab(cx) else {
            return "Open a task first".into();
        };
        let found = arg
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&n| n < tabs.len())
            .or_else(|| find_named(&tabs, arg, |t| t.title.to_string()));
        match found {
            Some(ix) => {
                self.set_tab(ix, cx);
                format!("Tab: {}", tabs[ix].title)
            }
            None => format!("No tab named {arg}"),
        }
    }

    /// `:tracker`: opens the tracker named `arg`, or whose name starts with it.
    fn command_tracker(&mut self, arg: &str, cx: &mut Context<Self>) -> Option<String> {
        let wanted = arg.trim().to_lowercase();
        let found = self.store.read(cx).trackers.iter().position(|t| {
            let name = t.name.to_lowercase();
            !wanted.is_empty() && (name == wanted || name.starts_with(&wanted))
        });
        match found {
            Some(ix) => {
                self.switch_tracker(ix, cx);
                None
            }
            None => Some(format!("Unknown tracker: {arg}")),
        }
    }

    /// `:view`: shows a saved view, or saves or deletes one.
    fn command_view(
        &mut self,
        arg: &str,
        catalog: &Catalog,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        match arg.split_once(' ').unwrap_or((arg, "")) {
            ("save" | "delete", _) if self.read_only_reason(cx).is_some() => {
                self.read_only_reason(cx)
            }
            ("save", name) => Some(self.save_view(name.trim(), cx)),
            ("delete", name) => Some(match Self::find_place(&catalog.views, name.trim()) {
                Some(name) if self.backend.is_some() => {
                    self.request(backend::Request::DeleteView {
                        name: name.to_string(),
                    });
                    format!("Deleting the view {name}")
                }
                Some(_) => "No server to delete the view on".into(),
                None => format!("Unknown view: {}", name.trim()),
            }),
            _ => match Self::find_place(&catalog.views, arg) {
                Some(name) => {
                    self.set_scope(Scope::View(name), cx);
                    None
                }
                None => Some(format!("Unknown view: {arg}")),
            },
        }
    }

    /// `:open`: opens a task by key, here or in the tracker that holds it.
    fn command_open(
        &mut self,
        arg: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let found = self
            .store
            .read(cx)
            .iter()
            .find(|i| i.key.eq_ignore_ascii_case(arg.trim()))
            .map(|i| i.id);
        match found {
            Some(id) => {
                self.message = SharedString::default();
                self.open_task(id, window, cx);
                None
            }
            None => Some(
                self.open_elsewhere(arg.trim(), cx)
                    .unwrap_or_else(|| format!("No task {arg}")),
            ),
        }
    }

    /// `:filter`: fills the `/` filter box, so the owner sees what narrows
    /// the list. `llm` is short for `llm:any`. No word clears the box.
    fn command_filter(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) -> String {
        let query = if arg == "llm" { "llm:any" } else { arg };
        let filter = self.list.read(cx).filter.clone();
        filter.update(cx, |state, cx| state.set_value(query, window, cx));
        self.apply_filter(&query.to_lowercase(), cx);
        match query {
            "" => "Cleared the filter".into(),
            "llm:any" => "Showing tasks with an agent on them".into(),
            other => format!("Filtered by {other}"),
        }
    }

    /// `:sort`: sorts the list by status, date added, key or LLM status.
    fn command_sort(&mut self, arg: &str, cx: &mut Context<Self>) -> String {
        let (key, message) = match arg {
            "status" => (SortKey::Status, "Sorted by status"),
            "added" | "date" => (SortKey::Added, "Sorted by date added"),
            "key" => (SortKey::Key, "Sorted by key"),
            "llm" => (SortKey::Llm, "Sorted by LLM status"),
            _ => return format!("Unknown sort: {arg}"),
        };
        self.sort_by(key, cx);
        message.into()
    }

    /// `:doc`: shows the document with this title, or whose title starts
    /// with it.
    fn command_doc(
        &mut self,
        arg: &str,
        catalog: &Catalog,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        match find_named(&catalog.documents, arg, |d| d.title.to_string()) {
            Some(ix) => {
                let id = catalog.documents[ix].id;
                self.set_scope(Scope::Document(id.to_string().into()), cx);
                None
            }
            None => Some(format!("Unknown document: {arg}")),
        }
    }

    /// `:mine` in the conflict view: overwrites the server's version with
    /// the editor text and closes the editor.
    fn keep_mine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_description(true, cx);
        self.task
            .update(cx, |task, cx| task.set_mode(DetailMode::Preview, cx));
        self.pipeline.end_editing();
        let focus = self.task.read(cx).focus.clone();
        window.focus(&focus, cx);
    }

    /// Runs a palette command on every marked task. None when nothing is
    /// marked or the command does not take several tasks; then it runs on
    /// one task as usual. Tasks an agent holds are skipped and counted.
    pub(super) fn run_bulk(
        &mut self,
        verb: &str,
        arg: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let ids = self.marked(cx);
        if ids.is_empty() {
            return None;
        }
        let store = self.store.read(cx);
        let catalog = store.catalog.clone();
        let (free, held): (Vec<IssueId>, Vec<IssueId>) = ids
            .iter()
            .partition(|id| store.get(**id).is_some_and(|i| i.claim.is_none()));
        let message = match verb {
            "status" => match self.bulk_status(&free, arg, &catalog, cx) {
                Ok(message) => message + &skipped(held.len()),
                Err(message) => return Some(message),
            },
            "label" => {
                let Some(label) = catalog.find_label(arg) else {
                    return Some(format!("Unknown label: {arg}"));
                };
                self.bulk_label(&free, label, cx) + &skipped(held.len())
            }
            "assign" if !arg.is_empty() => {
                for id in &free {
                    self.assign(*id, Some(arg.to_string()), cx);
                }
                format!("Assigned {} to {arg}", tasks(free.len())) + &skipped(held.len())
            }
            "unassign" => {
                for id in &free {
                    self.assign(*id, None, cx);
                }
                format!("Unassigned {}", tasks(free.len())) + &skipped(held.len())
            }
            "delete" | "trash" => {
                let live: Vec<IssueId> = free
                    .iter()
                    .copied()
                    .filter(|id| self.store.read(cx).get(*id).is_some_and(|i| !i.deleted))
                    .collect();
                for id in &live {
                    self.delete_issue(*id, cx);
                }
                format!(
                    "Moved {} to Trash. :restore brings the marked ones back",
                    tasks(live.len())
                ) + &skipped(held.len())
            }
            "restore" => {
                let trashed: Vec<IssueId> = ids
                    .iter()
                    .copied()
                    .filter(|id| self.store.read(cx).get(*id).is_some_and(|i| i.deleted))
                    .collect();
                // No marked task is in the Trash: `:restore` brings back the
                // last one trashed, as without marks.
                if trashed.is_empty() {
                    return None;
                }
                for id in &trashed {
                    self.restore_trashed(*id, cx);
                }
                format!("Restored {}", tasks(trashed.len()))
            }
            _ => return None,
        };
        self.list_do(cx, |list, cx| list.refresh_keeping_selection(cx));
        Some(message)
    }

    /// `:status` on several tasks. Each task uses its own team's status.
    /// The error is the message to show when no task has that status.
    fn bulk_status(
        &mut self,
        ids: &[IssueId],
        arg: &str,
        catalog: &Catalog,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let mut done = 0;
        let mut missing = 0;
        let mut name = String::new();
        let mut approved = Vec::new();
        let open = self.panel_open(cx);
        for id in ids {
            let team = self
                .store
                .read(cx)
                .get(*id)
                .map(|i| i.team.clone())
                .unwrap_or_default();
            match catalog.find_status(&team, arg) {
                Some(status) => {
                    name = status.name.to_string();
                    if self.set_status_only(*id, status, cx) {
                        approved.push(*id);
                    }
                    done += 1;
                }
                None => missing += 1,
            }
        }
        if done == 0 {
            return Err(format!("Unknown status: {arg}"));
        }
        if !approved.is_empty() {
            self.open_next_review(ids, open, cx);
        }
        let mut out = format!("{} now {name}", tasks(done));
        if missing > 0 {
            out.push_str(&format!(". {} have no status {arg}", tasks(missing)));
        }
        Ok(out)
    }

    /// `:label` on several tasks. Like Linear: removes the label when every
    /// task has it, else adds it to the tasks that lack it.
    fn bulk_label(&mut self, ids: &[IssueId], label: Label, cx: &mut Context<Self>) -> String {
        let has = |this: &Self, id: &IssueId, cx: &App| {
            this.store
                .read(cx)
                .get(*id)
                .is_some_and(|i| i.labels.contains(&label))
        };
        let remove = ids.iter().all(|id| has(self, id, cx));
        for id in ids {
            if remove || !has(self, id, cx) {
                self.toggle_label(*id, label.clone(), cx);
            }
        }
        let verb = if remove { "Removed" } else { "Added" };
        let to = if remove { "from" } else { "to" };
        format!("{verb} {} {to} {}", label.name, tasks(ids.len()))
    }
}

/// How many tasks, in words.
pub(super) fn tasks(n: usize) -> String {
    if n == 1 {
        "1 task".to_string()
    } else {
        format!("{n} tasks")
    }
}

/// The end of a bulk message: how many tasks an agent's claim kept out.
fn skipped(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => ". Skipped 1 task an agent holds".into(),
        n => format!(". Skipped {n} tasks agents hold"),
    }
}

/// The index of the first item named `wanted`, or else of the first whose
/// name starts with it. Case does not matter.
fn find_named<T>(items: &[T], wanted: &str, name: impl Fn(&T) -> String) -> Option<usize> {
    let wanted = wanted.to_lowercase();
    let lower = |item: &T| name(item).to_lowercase();
    items
        .iter()
        .position(|item| lower(item) == wanted)
        .or_else(|| {
            items
                .iter()
                .position(|item| lower(item).starts_with(&wanted))
        })
}

/// The argument of `:wait`: `agent`, `owner` or `none`. `Some(None)` clears
/// the field; `None` means the word is unknown.
fn parse_waiting_on(arg: &str) -> Option<Option<WaitingOn>> {
    match arg.trim().to_lowercase().as_str() {
        "agent" | "a" => Some(Some(WaitingOn::Agent)),
        "owner" | "me" | "o" => Some(Some(WaitingOn::Owner)),
        "none" | "" => Some(None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(names: &[&str], wanted: &str) -> Option<usize> {
        find_named(names, wanted, |n| n.to_string())
    }

    #[test]
    fn an_exact_name_wins_over_an_earlier_prefix_match() {
        assert_eq!(index(&["Work log", "Work"], "work"), Some(1));
    }

    #[test]
    fn a_name_start_matches_and_case_does_not_matter() {
        assert_eq!(index(&["Description", "Agent notes"], "AGENT"), Some(1));
        assert_eq!(index(&["Description"], "notes"), None);
    }

    #[test]
    fn bulk_messages_count_tasks_in_words() {
        assert_eq!(tasks(1), "1 task");
        assert_eq!(tasks(3), "3 tasks");
        assert_eq!(skipped(0), "");
        assert_eq!(skipped(1), ". Skipped 1 task an agent holds");
        assert_eq!(skipped(2), ". Skipped 2 tasks agents hold");
    }

    #[test]
    fn wait_accepts_its_short_words_and_refuses_others() {
        assert_eq!(parse_waiting_on("a"), Some(Some(WaitingOn::Agent)));
        assert_eq!(parse_waiting_on("me"), Some(Some(WaitingOn::Owner)));
        assert_eq!(parse_waiting_on("none"), Some(None));
        assert_eq!(parse_waiting_on("later"), None);
    }
}
