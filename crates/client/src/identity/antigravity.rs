//! Antigravity (agy), Google's agent app and CLI. It sets
//! `ANTIGRAVITY_AGENT` and `ANTIGRAVITY_CONVERSATION_ID` for the shell
//! commands its agent runs (measured with agy 1.5.18). It keeps no
//! transcript that llmtodo reads, so it reports no usage.

use std::path::{Path, PathBuf};

use super::{Harness, env};
use crate::TranscriptTotals;

/// The Antigravity harness.
pub struct Antigravity;

impl Harness for Antigravity {
    fn name(&self) -> &'static str {
        "antigravity"
    }

    fn detected(&self) -> bool {
        env("ANTIGRAVITY_AGENT").is_some()
    }

    fn session_id(&self) -> Option<String> {
        env("ANTIGRAVITY_CONVERSATION_ID")
    }

    fn version(&self) -> Option<String> {
        None
    }

    fn effort(&self) -> Option<String> {
        None
    }

    fn transcript(&self, _dir: &Path, _session: &str) -> Option<PathBuf> {
        None
    }

    fn model(&self, _transcript: &Path) -> Option<String> {
        None
    }

    fn totals(&self, _transcript: &Path, _cache_dir: &Path) -> TranscriptTotals {
        TranscriptTotals {
            usage: Default::default(),
            active_seconds: 0,
        }
    }
}
