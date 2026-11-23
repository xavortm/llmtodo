//! What the app waits for, and when to say so (LLM-470).
//!
//! On a shared database a write takes a second or two, and the first load
//! several. On the local SQLite file both take milliseconds. Every wait
//! shows only after [`SHOW_AFTER`], so a fast one never flashes.
//!
//! Nothing here reads a clock. The caller passes `now`, so tests run the
//! rules with made-up times.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::model::data::IssueId;

/// How long a wait lasts before the app shows it.
pub const SHOW_AFTER: Duration = Duration::from_millis(400);

/// How long without an answer before a shared database may sleep. Neon
/// suspends after 5 minutes, and the server closes idle connections at 4.
pub const MAY_SLEEP_AFTER: Duration = Duration::from_secs(4 * 60);

/// The wait the status bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The first request after a quiet spell on a shared database.
    Waking,
    /// The first load: issues read so far, of how many once counted.
    Loading { loaded: u64, total: Option<u64> },
    /// Writes wait for the server.
    Saving,
}

#[derive(Debug, Clone, Copy)]
struct Load {
    since: Instant,
    loaded: u64,
    total: Option<u64>,
}

#[derive(Debug, Default)]
pub struct Activity {
    /// The tracker uses a shared database, which can sleep.
    shared: bool,
    /// When the server last answered anything.
    last_answer: Option<Instant>,
    /// The waits began after the database may have slept, and no answer
    /// came since.
    may_wake: bool,
    /// When each issue's oldest unfinished write went out.
    writes: HashMap<IssueId, Instant>,
    load: Option<Load>,
    /// While the app connects: since when.
    connecting: Option<Instant>,
}

impl Activity {
    /// Whether the tracker uses a shared database. A connect says so; the
    /// app start guesses from the config.
    pub fn set_shared(&mut self, shared: bool) {
        self.shared = shared;
        if !shared {
            self.may_wake = false;
        }
    }

    fn busy(&self) -> bool {
        !self.writes.is_empty() || self.load.is_some() || self.connecting.is_some()
    }

    /// A new wait begins. When nothing waited and the server was quiet
    /// long enough, the database may have to wake first.
    fn begin(&mut self, now: Instant) {
        if self.busy() {
            return;
        }
        self.may_wake = self.shared
            && self
                .last_answer
                .is_none_or(|t| now.saturating_duration_since(t) >= MAY_SLEEP_AFTER);
    }

    /// The server answered something, so its database is awake.
    pub fn answer(&mut self, now: Instant) {
        self.last_answer = Some(now);
        self.may_wake = false;
        self.connecting = None;
    }

    /// The app looks for its server, as at start or after it went away.
    pub fn connecting(&mut self, now: Instant) {
        self.begin(now);
        self.connecting.get_or_insert(now);
    }

    /// The server or its database is away. Nothing waits for an answer any
    /// more: the writes failed, and the load starts over on reconnect.
    pub fn offline(&mut self) {
        self.connecting = None;
        self.load = None;
        self.writes.clear();
        self.may_wake = false;
    }

    pub fn write_sent(&mut self, id: IssueId, now: Instant) {
        self.begin(now);
        self.writes.entry(id).or_insert(now);
    }

    /// The last write for `id` returned.
    pub fn write_done(&mut self, id: IssueId) {
        self.writes.remove(&id);
    }

    /// Keeps only the writes of issues that still have some on their way.
    pub fn retain_writes(&mut self, mut waiting: impl FnMut(IssueId) -> bool) {
        self.writes.retain(|id, _| waiting(*id));
    }

    /// The first load reports how far it got. The first report starts it.
    pub fn loading(&mut self, loaded: u64, total: Option<u64>, now: Instant) {
        if self.load.is_none() {
            self.begin(now);
        }
        let since = self.load.map_or(now, |l| l.since);
        self.load = Some(Load {
            since,
            loaded,
            total,
        });
    }

    pub fn loaded(&mut self) {
        self.load = None;
    }

    /// The load's progress, at once. The empty list shows it in place of
    /// rows, so it needs no delay.
    pub fn load_progress(&self) -> Option<(u64, Option<u64>)> {
        self.load.map(|l| (l.loaded, l.total))
    }

    /// The issues whose writes waited past [`SHOW_AFTER`]. Their rows show
    /// that they are saving.
    pub fn slow_writes(&self, now: Instant) -> impl Iterator<Item = IssueId> + '_ {
        self.writes
            .iter()
            .filter(move |(_, since)| shown(**since, now))
            .map(|(id, _)| *id)
    }

    /// What the status bar shows now, if anything.
    pub fn shown(&self, now: Instant) -> Option<Wait> {
        let load = self.load.filter(|l| shown(l.since, now));
        let saving = self.slow_writes(now).next().is_some();
        let connecting = self.connecting.is_some_and(|t| shown(t, now));
        if self.may_wake && (load.is_some() || saving || connecting) {
            return Some(Wait::Waking);
        }
        if let Some(l) = load {
            return Some(Wait::Loading {
                loaded: l.loaded,
                total: l.total,
            });
        }
        saving.then_some(Wait::Saving)
    }

    /// How long until a wait that does not show yet will show. The caller
    /// draws again then.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        let starts = self
            .writes
            .values()
            .copied()
            .chain(self.load.map(|l| l.since))
            .chain(self.connecting);
        starts
            .map(|t| (t + SHOW_AFTER).saturating_duration_since(now))
            .filter(|d| !d.is_zero())
            .min()
    }
}

fn shown(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) >= SHOW_AFTER
}

/// A count with a comma every three digits, such as `15,000`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, c) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The words for a load in progress: `Loading issues… 15,000 of 50,000`.
pub fn loading_text(loaded: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if loaded > 0 => {
            format!(
                "Loading issues… {} of {}",
                thousands(loaded.min(total)),
                thousands(total)
            )
        }
        None if loaded > 0 => format!("Loading issues… {}", thousands(loaded)),
        _ => "Loading issues…".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> IssueId {
        IssueId::from_u128(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_fast_write_never_shows() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.write_sent(id(1), t);
        assert_eq!(a.shown(t + ms(5)), None);
        a.write_done(id(1));
        assert_eq!(a.shown(t + ms(500)), None, "done before the delay");
        assert_eq!(a.next_change(t + ms(5)), None);
    }

    #[test]
    fn a_slow_write_shows_after_the_delay_on_its_row_and_in_the_bar() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.write_sent(id(1), t);
        assert_eq!(a.next_change(t + ms(100)), Some(ms(300)));
        assert_eq!(a.shown(t + ms(399)), None);
        assert_eq!(a.slow_writes(t + ms(399)).count(), 0);
        assert_eq!(a.shown(t + SHOW_AFTER), Some(Wait::Saving));
        assert_eq!(a.slow_writes(t + SHOW_AFTER).collect::<Vec<_>>(), [id(1)]);
        a.write_done(id(1));
        assert_eq!(a.shown(t + ms(2000)), None);
    }

    #[test]
    fn a_second_write_on_the_same_issue_keeps_the_first_start() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.write_sent(id(1), t);
        a.write_sent(id(1), t + ms(300));
        assert_eq!(a.shown(t + SHOW_AFTER), Some(Wait::Saving));
    }

    #[test]
    fn the_load_shows_its_progress_after_the_delay() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.loading(0, None, t);
        a.loading(5000, Some(50_000), t + ms(200));
        assert_eq!(a.shown(t + ms(300)), None);
        assert_eq!(a.load_progress(), Some((5000, Some(50_000))));
        assert_eq!(
            a.shown(t + SHOW_AFTER),
            Some(Wait::Loading {
                loaded: 5000,
                total: Some(50_000)
            })
        );
        a.loaded();
        assert_eq!(a.shown(t + ms(900)), None);
        assert_eq!(a.load_progress(), None);
    }

    #[test]
    fn the_local_file_never_wakes() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.connecting(t);
        assert_eq!(a.shown(t + ms(900)), None, "Connecting… shows instead");
        a.answer(t + ms(950));
        a.write_sent(id(1), t + Duration::from_secs(3600));
        assert_eq!(
            a.shown(t + Duration::from_secs(3600) + SHOW_AFTER),
            Some(Wait::Saving)
        );
    }

    #[test]
    fn a_shared_database_may_wake_after_a_quiet_spell() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.set_shared(true);
        a.answer(t);
        // A minute later the database is still awake.
        let soon = t + Duration::from_secs(60);
        a.write_sent(id(1), soon);
        assert_eq!(a.shown(soon + SHOW_AFTER), Some(Wait::Saving));
        a.write_done(id(1));
        a.answer(soon + ms(500));
        // Five quiet minutes later it may sleep.
        let later = soon + Duration::from_secs(5 * 60);
        a.write_sent(id(2), later);
        assert_eq!(a.shown(later + ms(100)), None, "no flash");
        assert_eq!(a.shown(later + SHOW_AFTER), Some(Wait::Waking));
        // The first answer means it woke. The write still waits.
        a.answer(later + ms(900));
        assert_eq!(a.shown(later + ms(901)), Some(Wait::Saving));
    }

    #[test]
    fn the_first_connect_to_a_shared_database_may_wake_it() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.set_shared(true);
        a.connecting(t);
        assert_eq!(a.shown(t + ms(100)), None);
        assert_eq!(a.shown(t + SHOW_AFTER), Some(Wait::Waking));
        a.answer(t + ms(800));
        assert_eq!(a.shown(t + ms(801)), None);
    }

    #[test]
    fn an_outage_drops_every_wait() {
        let t = Instant::now();
        let mut a = Activity::default();
        a.write_sent(id(1), t);
        a.loading(10, None, t);
        a.offline();
        assert_eq!(a.shown(t + ms(900)), None);
        assert_eq!(a.next_change(t), None);
    }

    #[test]
    fn counts_read_with_commas() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(15_000), "15,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(
            loading_text(15_000, Some(50_000)),
            "Loading issues… 15,000 of 50,000"
        );
        assert_eq!(loading_text(15_000, None), "Loading issues… 15,000");
        assert_eq!(loading_text(0, Some(10)), "Loading issues…");
    }
}
