//! The state behind writes to the server: which writes are on their way,
//! which server copies wait for them, and where the owner's description
//! text is while a save is unfinished.
//!
//! The workspace owns one `WritePipeline` and calls it in one update order.
//! Nothing here sends anything. Each method says what the caller must do,
//! so tests run the whole flow with no window and no server.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gpui_shared_string::SharedString;

use crate::model::data::{Catalog, Issue, IssueId, Renames};
use crate::model::drafts::DraftStore;
use crate::model::store::IssueStore;

/// A description save that waits for the one before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedSave {
    pub text: String,
    pub force: bool,
}

/// What a finished description save left behind.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SavedDescription {
    /// The text that save sent.
    pub sent: Option<String>,
    /// The save that waited for it. The caller sends this one next, on top
    /// of the version the finished save made.
    pub next: Option<QueuedSave>,
}

/// What a failed description save did with the owner's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedSave {
    /// A claim closed the editor after this save went out. The text kept
    /// then is newer than the text that failed, so it stays.
    NewerTextKept,
    /// The server refused the save as stale. The text is in the conflicts.
    Conflict,
    /// The save failed for another reason. The text is in the conflicts.
    TextKept,
    /// There was no text to keep.
    NothingKept,
}

#[derive(Default)]
pub struct WritePipeline {
    /// Writes on their way to the server, per issue. Each answer carries
    /// the whole issue, but not the writes queued behind it, so it would
    /// undo the app's newer early changes.
    in_flight: HashMap<IssueId, usize>,
    /// The newest server copy of an issue with writes on their way. It is
    /// applied when the last of them returns.
    held: HashMap<IssueId, Issue>,
    /// The issue each sent key names. An answer names the key the write
    /// used, and the store may no longer hold that issue, as with a
    /// restore after a snapshot left the Trash out.
    sent_keys: HashMap<SharedString, IssueId>,
    /// Description saves on their way to the server, by task key.
    saving: HashMap<SharedString, String>,
    /// The next description save per task, held while one is on its way.
    /// It goes out when that one is done, on top of the version it made.
    queued: HashMap<SharedString, QueuedSave>,
    /// The owner's text from a save the server refused as stale. The editor
    /// opens with it, and `:w!` overwrites the server's version. It stays
    /// on disk, so it survives an app quit.
    conflicts: DraftStore,
    /// Tasks whose text in `conflicts` is newer than the save on its way.
    /// The answer to that save must not clear or replace the text.
    draft_newer_than_save: HashSet<SharedString>,
    /// The task in the editor and the description version it started from.
    /// A save sends that version, so the server refuses it when an agent
    /// changed the text in between.
    edit_base: Option<(IssueId, i64)>,
}

impl WritePipeline {
    /// Keeps unsaved description text in this store from now on.
    pub fn set_conflicts(&mut self, conflicts: DraftStore) {
        self.conflicts = conflicts;
    }

    #[cfg(test)]
    pub fn conflicts(&self) -> &DraftStore {
        &self.conflicts
    }

    /// Whether any write or description save is unfinished.
    pub fn is_busy(&self) -> bool {
        !self.in_flight.is_empty() || !self.saving.is_empty() || !self.queued.is_empty()
    }

    /// Starts over for another tracker. Its kept text lives in its own
    /// folder, which a read-only tracker never writes. The caller waits
    /// until `is_busy` is false.
    pub fn switch_tracker(&mut self, data_dir: &Path, read_only: bool) {
        self.edit_base = None;
        let conflicts = self.conflicts.for_tracker(data_dir);
        self.conflicts = if read_only {
            conflicts.memory_only()
        } else {
            conflicts
        };
        self.draft_newer_than_save.clear();
        self.held.clear();
        self.sent_keys.clear();
    }

    // Every write.

    /// A write for `id` went out under `key`.
    pub fn note_sent(&mut self, id: IssueId, key: SharedString) {
        *self.in_flight.entry(id).or_default() += 1;
        self.sent_keys.insert(key, id);
    }

    /// Whether writes for `id` are on their way.
    pub fn has_writes(&self, id: IssueId) -> bool {
        self.in_flight.contains_key(&id)
    }

    /// Keeps a server copy until the last write for its issue returns.
    pub fn hold(&mut self, issue: Issue) {
        self.held.insert(issue.id, issue);
    }

    /// Maps the held server copies to a new catalog.
    pub fn remap_held(&mut self, catalog: &Catalog, renames: &Renames) {
        for issue in self.held.values_mut() {
            issue.remap(catalog, renames);
        }
    }

    /// A fresh server copy. It comes back when the caller can apply it, and
    /// waits when writes for the issue are still on their way.
    pub fn receive(&mut self, issue: Issue) -> Option<Issue> {
        if self.has_writes(issue.id) {
            self.hold(issue);
            None
        } else {
            Some(issue)
        }
    }

    /// One write for `key` got its answer, or failed. After the last one,
    /// this returns the newest server copy, for the caller to apply.
    pub fn write_returned(&mut self, key: &str, store: &IssueStore) -> Option<Issue> {
        let id = self
            .sent_keys
            .get(key)
            .copied()
            .or_else(|| store.by_key(key).map(|i| i.id))?;
        let count = self.in_flight.get_mut(&id)?;
        *count -= 1;
        if *count > 0 {
            return None;
        }
        self.in_flight.remove(&id);
        self.held.remove(&id)
    }

    // Description saves.

    /// Whether the owner's kept text exists for `key`.
    pub fn has_conflict(&self, key: &str) -> bool {
        self.conflicts.contains_key(key)
    }

    pub fn conflict_text(&self, key: &str) -> Option<&String> {
        self.conflicts.get(key)
    }

    /// Drops the owner's kept text. Returns it when there was some.
    pub fn drop_conflict(&mut self, key: &str) -> Option<String> {
        self.conflicts.remove(key)
    }

    /// The editor opened on `id`, at description version `version`.
    pub fn begin_editing(&mut self, id: IssueId, version: i64) {
        self.edit_base = Some((id, version));
    }

    pub fn end_editing(&mut self) {
        self.edit_base = None;
    }

    /// The version a save of `id` builds on: the one its editor started
    /// from, else `current`.
    pub fn base_version(&self, id: IssueId, current: i64) -> i64 {
        self.edit_base
            .filter(|(open, _)| *open == id)
            .map_or(current, |(_, version)| version)
    }

    /// Whether a description save for `key` is on its way.
    pub fn is_saving(&self, key: &str) -> bool {
        self.saving.contains_key(key)
    }

    /// Holds this save until the one on its way is done. A newer queued
    /// save replaces an older one.
    pub fn queue(&mut self, key: SharedString, text: String, force: bool) {
        self.queued.insert(key, QueuedSave { text, force });
    }

    /// This save goes out now. Keeps its text until the server answers.
    pub fn note_saving(&mut self, key: SharedString, text: String) {
        self.saving.insert(key, text);
    }

    /// A description save reached the server and made `version`. `id` is
    /// the task that holds `key` now.
    pub fn description_saved(
        &mut self,
        key: &str,
        id: Option<IssueId>,
        version: i64,
    ) -> SavedDescription {
        let sent = self.saving.remove(key);
        if !self.draft_newer_than_save.remove(key) {
            self.conflicts.remove(key);
        }
        // Later saves from the same editor build on this one.
        if let Some((open, base)) = &mut self.edit_base
            && Some(*open) == id
        {
            *base = version;
        }
        SavedDescription {
            sent,
            next: self.queued.remove(key),
        }
    }

    /// A description save failed. A queued save holds newer text than the
    /// one that failed. It is not sent, because it would build on the same
    /// refused base. Every failure keeps the owner's text: a refetch on
    /// reconnect puts the server's copy back, so the draft is the only
    /// place the edit survives.
    pub fn description_failed(&mut self, key: &SharedString, stale: bool) -> FailedSave {
        let newer = self.draft_newer_than_save.remove(key);
        let sent = self.saving.remove(key);
        let text = self.queued.remove(key).map(|q| q.text).or(sent);
        match text {
            None => FailedSave::NothingKept,
            Some(_) if newer => FailedSave::NewerTextKept,
            Some(text) => {
                self.conflicts.insert(key.clone(), text);
                if stale {
                    FailedSave::Conflict
                } else {
                    FailedSave::TextKept
                }
            }
        }
    }

    /// A claim closed the editor on `key`, with the owner's text in it.
    /// Returns whether a save waited in the queue. A queued save would
    /// write to the locked task, so it never goes out.
    pub fn drop_queued(&mut self, key: &str) -> bool {
        self.queued.remove(key).is_some()
    }

    /// Moves every description save that has not reached the server into
    /// the kept text, before the app quits. Returns how many it kept. A
    /// save that lands later finds its text kept and leaves it, so the
    /// owner compares both versions on the next start.
    pub fn keep_unsent(&mut self) -> usize {
        let mut kept = 0;
        let saving: Vec<_> = self.saving.drain().collect();
        for (key, text) in saving {
            if !self.draft_newer_than_save.contains(&key) {
                self.conflicts.insert(key, text);
                kept += 1;
            }
        }
        let queued: Vec<_> = self.queued.drain().collect();
        for (key, save) in queued {
            self.conflicts.insert(key, save.text);
            kept += 1;
        }
        kept
    }

    /// Keeps the text from an editor a claim closed. A save still on its
    /// way must not clear or replace this newer text.
    pub fn keep_after_lock(&mut self, key: SharedString, text: String) {
        if self.saving.contains_key(&key) {
            self.draft_newer_than_save.insert(key.clone());
        }
        self.conflicts.insert(key, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(key: &str) -> (IssueStore, Issue) {
        let catalog = crate::model::demo::catalog();
        let mut issue = crate::model::demo::issues(&catalog).remove(0);
        issue.key = key.to_string().into();
        let mut store = IssueStore::new();
        store.upsert(issue.clone());
        (store, issue)
    }

    #[test]
    fn a_server_copy_waits_for_the_last_write() {
        let (store, issue) = store_with("LLM-1");
        let mut pipeline = WritePipeline::default();
        pipeline.note_sent(issue.id, "LLM-1".into());
        pipeline.note_sent(issue.id, "LLM-1".into());
        assert!(pipeline.is_busy());

        let mut copy = issue.clone();
        copy.version = 9;
        assert!(pipeline.receive(copy).is_none(), "writes are on their way");

        assert!(pipeline.write_returned("LLM-1", &store).is_none());
        let applied = pipeline.write_returned("LLM-1", &store).unwrap();
        assert_eq!(applied.version, 9);
        assert!(!pipeline.has_writes(issue.id));
        assert!(!pipeline.is_busy());
    }

    #[test]
    fn unsent_saves_become_kept_text_before_a_restart() {
        let mut pipeline = WritePipeline::default();
        pipeline.note_saving("LLM-1".into(), "on its way".into());
        pipeline.queue("LLM-1".into(), "newer".into(), false);
        pipeline.note_saving("LLM-2".into(), "other".into());
        assert_eq!(pipeline.keep_unsent(), 3);
        assert!(!pipeline.is_busy());
        assert_eq!(
            pipeline.conflict_text("LLM-1").map(String::as_str),
            Some("newer")
        );
        assert_eq!(
            pipeline.conflict_text("LLM-2").map(String::as_str),
            Some("other")
        );
    }

    #[test]
    fn a_copy_with_no_writes_on_their_way_applies_at_once() {
        let (_, issue) = store_with("LLM-1");
        let mut pipeline = WritePipeline::default();
        assert!(pipeline.receive(issue).is_some());
    }

    #[test]
    fn an_answer_finds_a_task_the_store_no_longer_holds_by_its_sent_key() {
        let (_, issue) = store_with("LLM-1");
        let empty = IssueStore::new();
        let mut pipeline = WritePipeline::default();
        pipeline.note_sent(issue.id, "LLM-1".into());
        pipeline.hold(issue.clone());
        assert_eq!(
            pipeline.write_returned("LLM-1", &empty).map(|i| i.id),
            Some(issue.id)
        );
    }

    #[test]
    fn a_finished_save_moves_the_base_and_hands_over_the_queued_save() {
        let (_, issue) = store_with("LLM-1");
        let mut pipeline = WritePipeline::default();
        pipeline.begin_editing(issue.id, 3);
        assert_eq!(pipeline.base_version(issue.id, 8), 3);

        pipeline.note_saving("LLM-1".into(), "one".into());
        pipeline.queue("LLM-1".into(), "two".into(), false);
        pipeline.queue("LLM-1".into(), "three".into(), true);

        let saved = pipeline.description_saved("LLM-1", Some(issue.id), 4);
        assert_eq!(saved.sent.as_deref(), Some("one"));
        assert_eq!(
            saved.next,
            Some(QueuedSave {
                text: "three".into(),
                force: true
            })
        );
        assert_eq!(pipeline.base_version(issue.id, 8), 4);
        assert!(!pipeline.is_saving("LLM-1"));
    }

    #[test]
    fn a_stale_save_keeps_the_newest_text_as_a_conflict() {
        let mut pipeline = WritePipeline::default();
        let key: SharedString = "LLM-1".into();
        pipeline.note_saving(key.clone(), "sent".into());
        pipeline.queue(key.clone(), "queued".into(), false);

        assert_eq!(
            pipeline.description_failed(&key, true),
            FailedSave::Conflict
        );
        assert_eq!(pipeline.conflict_text("LLM-1").unwrap(), "queued");
        assert!(!pipeline.is_busy());
    }

    #[test]
    fn any_other_failure_keeps_the_sent_text() {
        let mut pipeline = WritePipeline::default();
        let key: SharedString = "LLM-1".into();
        pipeline.note_saving(key.clone(), "sent".into());
        assert_eq!(
            pipeline.description_failed(&key, false),
            FailedSave::TextKept
        );
        assert_eq!(pipeline.conflict_text("LLM-1").unwrap(), "sent");
    }

    #[test]
    fn a_failure_with_nothing_on_its_way_keeps_nothing() {
        let mut pipeline = WritePipeline::default();
        let key: SharedString = "LLM-1".into();
        assert_eq!(
            pipeline.description_failed(&key, true),
            FailedSave::NothingKept
        );
        assert!(!pipeline.has_conflict("LLM-1"));
    }

    #[test]
    fn text_kept_after_a_lock_outlives_the_save_on_its_way() {
        let mut pipeline = WritePipeline::default();
        let key: SharedString = "LLM-1".into();
        pipeline.note_saving(key.clone(), "older".into());
        pipeline.queue(key.clone(), "queued".into(), false);

        assert!(pipeline.drop_queued("LLM-1"));
        pipeline.keep_after_lock(key.clone(), "newest".into());

        pipeline.description_saved("LLM-1", None, 5);
        assert_eq!(pipeline.conflict_text("LLM-1").unwrap(), "newest");
    }

    #[test]
    fn a_failed_save_does_not_replace_text_kept_after_a_lock() {
        let mut pipeline = WritePipeline::default();
        let key: SharedString = "LLM-1".into();
        pipeline.note_saving(key.clone(), "older".into());
        pipeline.keep_after_lock(key.clone(), "newest".into());

        assert_eq!(
            pipeline.description_failed(&key, true),
            FailedSave::NewerTextKept
        );
        assert_eq!(pipeline.conflict_text("LLM-1").unwrap(), "newest");
    }

    #[test]
    fn a_saved_description_clears_its_conflict() {
        let mut pipeline = WritePipeline::default();
        let key: SharedString = "LLM-1".into();
        pipeline.keep_after_lock(key.clone(), "kept".into());
        pipeline.note_saving(key.clone(), "kept".into());
        pipeline.description_saved("LLM-1", None, 2);
        assert!(!pipeline.has_conflict("LLM-1"));
    }

    #[test]
    fn switching_trackers_forgets_held_copies_and_the_edit() {
        let dir = tempfile::tempdir().unwrap();
        let (_, issue) = store_with("LLM-1");
        let mut pipeline = WritePipeline::default();
        pipeline.begin_editing(issue.id, 3);
        pipeline.hold(issue.clone());
        pipeline.switch_tracker(dir.path(), false);
        assert_eq!(pipeline.base_version(issue.id, 8), 8);
        assert!(pipeline.held.is_empty());
    }

    #[test]
    fn a_read_only_tracker_gets_no_drafts_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut pipeline = WritePipeline::default();
        pipeline.set_conflicts(DraftStore::open(dir.path()));
        pipeline.switch_tracker(dir.path(), true);
        pipeline.keep_after_lock("LLM-1".into(), "kept".into());
        assert!(pipeline.has_conflict("LLM-1"));
        assert!(!dir.path().join("drafts.json").exists());
    }
}
