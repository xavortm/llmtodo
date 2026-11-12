//! Description text the server does not have yet, kept in the data folder.
//!
//! The text comes from a refused save, or from a claim that closes the
//! editor. Claims never time out, so it can wait for hours, and it has to
//! survive an app quit. Each task keeps its text until the owner saves it
//! or picks the server's version.
//!
//! Each change writes the file on one background thread, with an fsync, so
//! the main thread never waits on the disk. [`flush`] waits for the writes
//! on their way, and [`DraftStore::open`] and the app's quit call it.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use gpui_shared_string::SharedString;

/// The file name inside the data folder.
const FILE: &str = "drafts.json";

/// Kept text by task key. A store with no file holds it in memory only, as
/// the tests and the demo data do.
#[derive(Default)]
pub struct DraftStore {
    path: Option<PathBuf>,
    entries: BTreeMap<String, String>,
    /// Keep changes in memory and never write the file.
    memory_only: bool,
}

impl DraftStore {
    /// Loads the text kept in `data_dir`. A missing file means no text yet.
    /// A corrupt file must not stop the app, so it is logged and the store
    /// starts empty.
    pub fn open(data_dir: &Path) -> Self {
        // A write on its way may be for this file.
        flush();
        let path = data_dir.join(FILE);
        let entries = match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|err| {
                eprintln!("llmtodo: ignoring corrupt {}: {err}", path.display());
                BTreeMap::new()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(err) => {
                eprintln!("llmtodo: could not read {}: {err}", path.display());
                BTreeMap::new()
            }
        };
        Self {
            path: Some(path),
            entries,
            memory_only: false,
        }
    }

    /// This store, for a read-only tracker: it never writes its file.
    pub fn memory_only(mut self) -> Self {
        self.memory_only = true;
        self
    }

    /// The store for another tracker. Text stays in the folder of the
    /// tracker it belongs to. A store with no file starts empty.
    pub fn for_tracker(&self, data_dir: &Path) -> Self {
        match self.path {
            Some(_) => Self::open(data_dir),
            None => Self::default(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.entries.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn insert(&mut self, key: SharedString, text: String) {
        self.entries.insert(key.to_string(), text);
        self.persist();
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        let removed = self.entries.remove(key);
        if removed.is_some() {
            self.persist();
        }
        removed
    }

    /// Sends the file's new text to the writer thread. A failed write is
    /// logged, and the text stays in memory for as long as the app runs.
    fn persist(&self) {
        let Some(path) = self.path.as_ref().filter(|_| !self.memory_only) else {
            return;
        };
        let bytes = match serde_json::to_vec_pretty(&self.entries) {
            Ok(bytes) => bytes,
            Err(err) => return eprintln!("llmtodo: could not encode drafts: {err}"),
        };
        let job = Job::Write {
            path: path.clone(),
            bytes,
        };
        let unsent = match writer().map(Mutex::lock) {
            Some(Ok(tx)) => tx.send(job).err().map(|e| e.0),
            _ => Some(job),
        };
        // No writer thread: write here.
        if let Some(Job::Write { path, bytes }) = unsent {
            write_logged(&path, &bytes);
        }
    }
}

/// Work for the writer thread.
enum Job {
    Write {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    /// Answers once every write sent before it is done.
    Flush(mpsc::Sender<()>),
}

/// The writer thread's queue, started on first use. None when the thread
/// cannot start.
fn writer() -> Option<&'static Mutex<mpsc::Sender<Job>>> {
    static WRITER: OnceLock<Option<Mutex<mpsc::Sender<Job>>>> = OnceLock::new();
    WRITER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("llmtodo-drafts".into())
                .spawn(move || write_jobs(rx))
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
}

/// Runs the writer thread. Writes that wait together keep only the newest
/// text of each file, so a burst of changes writes once.
fn write_jobs(rx: mpsc::Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        let mut due: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        for job in std::iter::once(job).chain(rx.try_iter()) {
            match job {
                Job::Write { path, bytes } => match due.iter_mut().find(|(p, _)| *p == path) {
                    Some(entry) => entry.1 = bytes,
                    None => due.push((path, bytes)),
                },
                Job::Flush(done) => {
                    for (path, bytes) in due.drain(..) {
                        write_logged(&path, &bytes);
                    }
                    let _ = done.send(());
                }
            }
        }
        for (path, bytes) in due {
            write_logged(&path, &bytes);
        }
    }
}

/// Waits until every draft write sent so far is on disk.
pub fn flush() {
    let Some(tx) = writer() else {
        return;
    };
    let (done, wait) = mpsc::channel();
    let sent = tx.lock().is_ok_and(|tx| tx.send(Job::Flush(done)).is_ok());
    if sent {
        let _ = wait.recv();
    }
}

fn write_logged(path: &Path, bytes: &[u8]) {
    if let Err(err) = write_atomic(path, bytes) {
        eprintln!("llmtodo: could not write {}: {err}", path.display());
    }
}

/// Writes a temporary file and renames it over `path`. A crash at any
/// moment leaves either the old file or the new one, never half of one.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn kept_text_is_there_after_a_restart() {
        let dir = folder();
        let mut store = DraftStore::open(dir.path());
        store.insert("LLM-1".into(), "First.\n".into());
        store.insert("LLM-2".into(), "Ünï\n\"quoted\"\n".into());

        let reopened = DraftStore::open(dir.path());
        assert_eq!(reopened.get("LLM-1").unwrap(), "First.\n");
        assert_eq!(reopened.get("LLM-2").unwrap(), "Ünï\n\"quoted\"\n");
        assert!(reopened.contains_key("LLM-1"));
        assert!(reopened.get("LLM-3").is_none());
    }

    #[test]
    fn newer_text_replaces_older_text() {
        let dir = folder();
        let mut store = DraftStore::open(dir.path());
        store.insert("LLM-1".into(), "Old.".into());
        store.insert("LLM-1".into(), "New.".into());
        assert_eq!(DraftStore::open(dir.path()).get("LLM-1").unwrap(), "New.");
    }

    #[test]
    fn a_removed_entry_stays_gone_and_the_others_stay() {
        let dir = folder();
        let mut store = DraftStore::open(dir.path());
        store.insert("LLM-1".into(), "One.".into());
        store.insert("LLM-2".into(), "Two.".into());
        assert_eq!(store.remove("LLM-1").as_deref(), Some("One."));
        assert_eq!(store.remove("LLM-1"), None);

        let reopened = DraftStore::open(dir.path());
        assert!(!reopened.contains_key("LLM-1"));
        assert_eq!(reopened.get("LLM-2").unwrap(), "Two.");
    }

    /// LLM-177: an insert returns before the disk write, and a flush waits
    /// for it. A burst of changes ends with the newest text on disk.
    #[test]
    fn writes_run_off_the_callers_thread_and_flush_waits_for_them() {
        let dir = folder();
        let mut store = DraftStore::open(dir.path());
        for n in 0..50 {
            store.insert("LLM-1".into(), format!("Version {n}."));
        }
        flush();
        let raw = std::fs::read(dir.path().join(FILE)).unwrap();
        let on_disk: BTreeMap<String, String> = serde_json::from_slice(&raw).unwrap();
        assert_eq!(on_disk["LLM-1"], "Version 49.");
    }

    #[test]
    fn a_missing_file_starts_empty() {
        let dir = folder();
        assert!(DraftStore::open(dir.path()).get("LLM-1").is_none());
        assert!(!dir.path().join(FILE).exists(), "opening writes nothing");
    }

    #[test]
    fn a_missing_data_folder_is_created_on_the_first_write() {
        let dir = folder();
        let nested = dir.path().join("not/yet");
        let mut store = DraftStore::open(&nested);
        store.insert("LLM-1".into(), "One.".into());
        assert_eq!(DraftStore::open(&nested).get("LLM-1").unwrap(), "One.");
    }

    #[test]
    fn a_corrupt_file_starts_empty_and_the_next_write_replaces_it() {
        for junk in [
            &b"{ not json"[..],
            &b"\xff\xfe\x00"[..],
            &b"[1, 2]"[..],
            b"",
        ] {
            let dir = folder();
            std::fs::write(dir.path().join(FILE), junk).unwrap();
            let mut store = DraftStore::open(dir.path());
            assert!(store.get("LLM-1").is_none());

            store.insert("LLM-1".into(), "One.".into());
            assert_eq!(DraftStore::open(dir.path()).get("LLM-1").unwrap(), "One.");
        }
    }

    #[test]
    fn a_write_leaves_only_the_finished_file() {
        let dir = folder();
        let mut store = DraftStore::open(dir.path());
        store.insert("LLM-1".into(), "One.".into());
        flush();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE]);
    }

    #[test]
    fn a_failed_write_keeps_the_text_in_memory() {
        let dir = folder();
        // A file where the data folder should be makes every write fail.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "").unwrap();
        let mut store = DraftStore::open(&blocked);
        store.insert("LLM-1".into(), "One.".into());
        assert_eq!(store.get("LLM-1").unwrap(), "One.");
    }

    #[test]
    fn another_tracker_has_its_own_text() {
        let (a, b) = (folder(), folder());
        let mut store = DraftStore::open(a.path());
        store.insert("LLM-1".into(), "In A.".into());

        let store = store.for_tracker(b.path());
        assert!(store.get("LLM-1").is_none());
        let store = store.for_tracker(a.path());
        assert_eq!(store.get("LLM-1").unwrap(), "In A.");
    }

    #[test]
    fn a_store_without_a_file_writes_nothing_and_forgets_on_switch() {
        let dir = folder();
        let mut store = DraftStore::default();
        store.insert("LLM-1".into(), "One.".into());
        assert_eq!(store.get("LLM-1").unwrap(), "One.");

        let store = store.for_tracker(dir.path());
        assert!(store.get("LLM-1").is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
