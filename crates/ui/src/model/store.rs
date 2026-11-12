//! The issues the app holds, keyed by their stable ID, plus the catalog and
//! the connection state. The list, the task view and the workspace all read
//! this one copy. Only the workspace writes it.
//!
//! Nothing outside this file holds a position in the issue list. Rows,
//! menus, drags and the open task hold an `IssueId`, so a snapshot that
//! reorders the issues, or a team move that changes a key, cannot point
//! them at another issue.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_shared_string::SharedString;

use llmtodo_api_types::{IssueGit, Relation, ReviewAnswer};
use llmtodo_core::Question;

use crate::model::data::{Catalog, Issue, IssueId, Renames};
use crate::model::scope::ViewMembers;
use crate::model::trackers::Tracker;

/// Whether the app talks to a server, and how that is going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// No server: tests on demo data. Writes stay in memory.
    #[cfg_attr(not(test), allow(dead_code))]
    Local,
    Connecting,
    Online(SharedString),
    Offline(SharedString),
}

pub struct IssueStore {
    /// In the order the server sent them. The list sorts its own copy of
    /// the IDs, so this order only breaks ties. Each issue is shared, so
    /// `snapshot` is cheap and a background task can read the copy it took
    /// while the store changes.
    issues: Vec<Arc<Issue>>,
    index: HashMap<IssueId, usize>,
    /// Each issue's ID by its key, so `by_key` is one lookup. The preview
    /// asks it once for each issue key in the text.
    keys: HashMap<SharedString, IssueId>,
    /// Counts up whenever the set of keys changes. A view that turned issue
    /// keys into links keeps its text until this changes.
    keys_revision: u64,
    /// The counts the status bar and the sidebar show, kept up to date on
    /// every write, so a render does not scan the issues.
    tally: Tally,
    /// The git panel per issue. Only a single-issue fetch carries it, so a
    /// snapshot keeps what is here.
    git: HashMap<IssueId, IssueGit>,
    /// The relations per issue, for the dependency section. Like the git
    /// panel, only a single-issue fetch carries them.
    relations: HashMap<IssueId, Vec<Relation>>,
    /// The open owner questions per issue, for the inbox. They come from
    /// the server's question list, and from the text whenever the app holds
    /// it. An answer the server has not confirmed yet is already gone.
    questions: HashMap<IssueId, Vec<Question>>,
    /// The review answers per issue. Like the git panel, only a
    /// single-issue fetch carries them.
    reviews: HashMap<IssueId, Vec<ReviewAnswer>>,
    /// Issues whose git panel, relations or reviews may be old: a list copy
    /// came after them. Only the open task gets its detail on live events,
    /// so a task that opens again fetches it when it is here.
    stale_detail: std::collections::HashSet<IssueId>,
    pub catalog: Catalog,
    pub connection: Connection,
    /// The issues of the saved view the list shows, as the server sent
    /// them last.
    pub view: Option<ViewMembers>,
    /// The server's base URL, such as `http://127.0.0.1:7878`, once it
    /// connected. The preview loads stored files from it.
    pub server_url: Option<SharedString>,
    /// The trackers the sidebar lists, and which one is open. The workspace
    /// switches between them.
    pub trackers: Vec<Tracker>,
    pub tracker: usize,
}

/// Counts over the issues that are not in the Trash.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tally {
    /// Issues with a claim.
    claims: usize,
    /// Open questions on issues that are not closed.
    questions: usize,
}

impl Tally {
    fn of(issue: &Issue) -> Self {
        if issue.deleted {
            return Self::default();
        }
        Self {
            claims: usize::from(issue.claim.is_some()),
            questions: if issue.status.is_closed() {
                0
            } else {
                issue.open_questions as usize
            },
        }
    }

    fn add(&mut self, other: Self) {
        self.claims += other.claims;
        self.questions += other.questions;
    }

    fn remove(&mut self, other: Self) {
        self.claims -= other.claims;
        self.questions -= other.questions;
    }
}

impl Default for IssueStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IssueStore {
    /// The open tracker, when the app knows more than one.
    pub fn open_tracker(&self) -> Option<&Tracker> {
        (self.trackers.len() > 1)
            .then(|| self.trackers.get(self.tracker))
            .flatten()
    }

    pub fn new() -> Self {
        Self {
            issues: Vec::new(),
            index: HashMap::new(),
            keys: HashMap::new(),
            keys_revision: 0,
            tally: Tally::default(),
            git: HashMap::new(),
            relations: HashMap::new(),
            questions: HashMap::new(),
            reviews: HashMap::new(),
            stale_detail: Default::default(),
            catalog: Catalog::default(),
            connection: Connection::Connecting,
            view: None,
            server_url: None,
            trackers: Vec::new(),
            tracker: 0,
        }
    }

    pub fn get(&self, id: IssueId) -> Option<&Issue> {
        self.index.get(&id).map(|&ix| &*self.issues[ix])
    }

    /// Changes one issue in place. Returns None when the store lacks it.
    /// New text counts as held text, and its questions replace the old.
    pub fn modify<R>(&mut self, id: IssueId, f: impl FnOnce(&mut Issue) -> R) -> Option<R> {
        let ix = *self.index.get(&id)?;
        let before = self.issues[ix].description.clone();
        let key_before = self.issues[ix].key.clone();
        self.tally.remove(Tally::of(&self.issues[ix]));
        let issue = Arc::make_mut(&mut self.issues[ix]);
        let out = f(issue);
        if issue.description != before {
            issue.description_loaded = true;
            let open = llmtodo_core::open_questions(&issue.description);
            issue.open_questions = open.len() as u32;
            Self::set_questions(&mut self.questions, id, open);
        }
        self.tally.add(Tally::of(&self.issues[ix]));
        let key = self.issues[ix].key.clone();
        if key != key_before {
            self.rekey(&key_before, key, id);
        }
        Some(out)
    }

    /// Moves `id` from its old key to its new one.
    fn rekey(&mut self, old: &SharedString, new: SharedString, id: IssueId) {
        if self.keys.get(old) == Some(&id) {
            self.keys.remove(old);
        }
        self.keys.insert(new, id);
        self.keys_revision += 1;
    }

    fn set_questions(map: &mut HashMap<IssueId, Vec<Question>>, id: IssueId, open: Vec<Question>) {
        if open.is_empty() {
            map.remove(&id);
        } else {
            map.insert(id, open);
        }
    }

    /// Reads the questions from the text, when the issue carries it.
    /// The server's count says when there is nothing to read.
    fn questions_from_text(&mut self, issue: &Issue) {
        if !issue.description_loaded {
            return;
        }
        let open = if issue.open_questions == 0 {
            Vec::new()
        } else {
            llmtodo_core::open_questions(&issue.description)
        };
        Self::set_questions(&mut self.questions, issue.id, open);
    }

    /// The open questions of one issue, in the order the agent asked them.
    pub fn open_questions_of(&self, id: IssueId) -> &[Question] {
        self.questions.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Replaces the open questions with the server's list. Issues for which
    /// `keep` is true keep what the app shows, because writes for them are
    /// on their way.
    pub fn set_open_questions(
        &mut self,
        list: HashMap<IssueId, Vec<Question>>,
        keep: impl Fn(IssueId) -> bool,
    ) {
        let mut next: HashMap<IssueId, Vec<Question>> =
            list.into_iter().filter(|(id, _)| !keep(*id)).collect();
        for (id, qs) in self.questions.drain() {
            if keep(id) {
                next.insert(id, qs);
            }
        }
        self.questions = next;
    }

    /// Takes question `number` of `id` off the open list, for an answer on
    /// its way to the server. Returns how many open questions are left, or
    /// None when the question is not open.
    pub fn take_open_question(&mut self, id: IssueId, number: u32) -> Option<usize> {
        let open = self.questions.get_mut(&id)?;
        let ix = open.iter().position(|q| q.number == number)?;
        open.remove(ix);
        let left = open.len();
        if left == 0 {
            self.questions.remove(&id);
        }
        if let Some(&ix) = self.index.get(&id) {
            self.tally.remove(Tally::of(&self.issues[ix]));
            let issue = Arc::make_mut(&mut self.issues[ix]);
            issue.open_questions = issue.open_questions.saturating_sub(1);
            self.tally.add(Tally::of(&self.issues[ix]));
        }
        Some(left)
    }

    /// Puts in a new catalog and maps every issue to it. Returns the names
    /// that changed.
    pub fn remap(&mut self, catalog: Catalog) -> Renames {
        let renames = catalog.renames_from(&self.catalog);
        for issue in &mut self.issues {
            self.tally.remove(Tally::of(issue));
            Arc::make_mut(issue).remap(&catalog, &renames);
            self.tally.add(Tally::of(issue));
        }
        self.catalog = catalog;
        renames
    }

    /// The git panel of an issue, once a fetch brought it.
    pub fn git(&self, id: IssueId) -> Option<&IssueGit> {
        self.git.get(&id)
    }

    pub fn set_git(&mut self, id: IssueId, git: IssueGit) {
        self.git.insert(id, git);
    }

    /// The relations of an issue, once a fetch brought them.
    pub fn relations(&self, id: IssueId) -> Option<&[Relation]> {
        self.relations.get(&id).map(Vec::as_slice)
    }

    /// Tests put in relations as a fetch would.
    #[cfg(test)]
    pub fn set_relations(&mut self, id: IssueId, relations: Vec<Relation>) {
        self.relations.insert(id, relations);
    }

    /// Marks the detail of `id` as possibly old, when the store holds one.
    pub fn mark_detail_stale(&mut self, id: IssueId) {
        if self.git.contains_key(&id) {
            self.stale_detail.insert(id);
        }
    }

    /// Whether the detail of `id` may be old. Returns true once, and the
    /// caller fetches it.
    pub fn take_stale_detail(&mut self, id: IssueId) -> bool {
        self.stale_detail.remove(&id)
    }

    /// Puts in the relations, reviews and git panel of one fetch.
    pub fn set_detail(
        &mut self,
        id: IssueId,
        relations: Vec<Relation>,
        reviews: Vec<ReviewAnswer>,
        git: IssueGit,
    ) {
        self.relations.insert(id, relations);
        self.reviews.insert(id, reviews);
        self.git.insert(id, git);
        self.stale_detail.remove(&id);
    }

    /// The review answers of an issue, oldest first, once a fetch brought
    /// them.
    pub fn reviews(&self, id: IssueId) -> &[ReviewAnswer] {
        self.reviews.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The open issues in a review status that wait on the owner, oldest first.
    pub fn waiting_for_review(&self) -> Vec<&Issue> {
        let mut out: Vec<&Issue> = self
            .iter()
            .filter(|i| {
                !i.deleted
                    && i.status.review
                    && i.waiting_on != Some(crate::model::data::WaitingOn::Agent)
            })
            .collect();
        out.sort_by(|a, b| a.added.cmp(&b.added).then_with(|| a.key.cmp(&b.key)));
        out
    }

    pub fn by_key(&self, key: &str) -> Option<&Issue> {
        self.keys.get(key).and_then(|&id| self.get(id))
    }

    /// Counts up whenever an issue key starts or stops naming an issue.
    pub fn keys_revision(&self) -> u64 {
        self.keys_revision
    }

    /// How many tasks outside the Trash hold a claim.
    pub fn claim_count(&self) -> usize {
        self.tally.claims
    }

    pub fn iter(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().map(|issue| &**issue)
    }

    /// A shared copy of every issue, in store order. It costs one counter
    /// bump per issue, and later writes to the store leave it alone.
    pub fn snapshot(&self) -> Vec<Arc<Issue>> {
        self.issues.clone()
    }

    pub fn len(&self) -> usize {
        self.issues.len()
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    /// How many open questions the open issues hold.
    pub fn open_question_count(&self) -> usize {
        self.tally.questions
    }

    /// The issues that are neither closed nor in the Trash and have open
    /// questions, with how many they have.
    pub fn with_open_questions(&self) -> impl Iterator<Item = (&Issue, u32)> {
        self.iter()
            .filter(|issue| issue.open_questions > 0 && !issue.deleted && !issue.status.is_closed())
            .map(|issue| (issue, issue.open_questions))
    }

    /// Replaces every issue. A later copy of the same ID wins. A copy that
    /// came without text keeps the text the store holds for its version.
    pub fn replace_all(&mut self, issues: Vec<Issue>) {
        let old = std::mem::take(&mut self.issues);
        let old_index = std::mem::take(&mut self.index);
        let old_keys = std::mem::take(&mut self.keys);
        let revision = self.keys_revision;
        self.tally = Tally::default();
        for mut issue in issues {
            if let Some(&ix) = old_index.get(&issue.id) {
                issue.keep_text_of(&old[ix]);
            }
            self.upsert(issue);
        }
        let index = &self.index;
        self.questions.retain(|id, _| index.contains_key(id));
        // The same keys keep the old revision, so prepared text stays.
        self.keys_revision = if self.keys == old_keys {
            revision
        } else {
            revision + 1
        };
    }

    /// Puts a fresh copy in place by ID, or adds it. A team move changes the
    /// key, and the copy replaces the old one under its new key. Returns the
    /// copy it replaced. A copy without text keeps the text of the copy it
    /// replaces, when the versions match.
    pub fn upsert(&mut self, mut issue: Issue) -> Option<Issue> {
        if let Some(&ix) = self.index.get(&issue.id) {
            issue.keep_text_of(&self.issues[ix]);
        }
        self.questions_from_text(&issue);
        self.tally.add(Tally::of(&issue));
        match self.index.get(&issue.id) {
            Some(&ix) => {
                let old = std::mem::replace(&mut self.issues[ix], Arc::new(issue));
                self.tally.remove(Tally::of(&old));
                if old.key != self.issues[ix].key {
                    let (key, id) = (self.issues[ix].key.clone(), old.id);
                    self.rekey(&old.key, key, id);
                }
                Some(Arc::unwrap_or_clone(old))
            }
            None => {
                self.index.insert(issue.id, self.issues.len());
                self.rekey(&SharedString::default(), issue.key.clone(), issue.id);
                self.issues.push(Arc::new(issue));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> IssueId {
        IssueId::from_u128(n)
    }

    /// LLM-171: a list copy makes a held detail old, so the task fetches it
    /// once when it opens again. A fetch makes it fresh.
    #[test]
    fn a_list_copy_marks_a_held_detail_as_old_once() {
        let mut store = IssueStore::new();
        // No detail held: nothing to refetch.
        store.mark_detail_stale(id(1));
        assert!(!store.take_stale_detail(id(1)));

        store.set_detail(id(1), Vec::new(), Vec::new(), IssueGit::default());
        store.mark_detail_stale(id(1));
        assert!(store.take_stale_detail(id(1)));
        assert!(!store.take_stale_detail(id(1)));

        store.mark_detail_stale(id(1));
        store.set_detail(id(1), Vec::new(), Vec::new(), IssueGit::default());
        assert!(!store.take_stale_detail(id(1)));
    }
}
