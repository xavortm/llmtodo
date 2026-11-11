//! Who is calling and from where: the actor, the session, git details and
//! the agent's token usage. Every client builds its [`ClientContext`] here.
//!
//! A [`Harness`] is the program that runs an agent, such as Claude Code.
//! It knows how to find the agent's session, its transcript and its usage.
//! [`ClaudeCode`] is the first one. An agent from a harness with no
//! implementation still works: it names itself with `LLMTODO_ACTOR`, and
//! the client sends no usage for it.

mod active_time;
mod claude_code;

use std::path::{Path, PathBuf};
use std::process::Command;

use llmtodo_api_types::{ActorInput, ActorKind, ClientContext, TokenUsage, VERSION};

pub use claude_code::ClaudeCode;

use crate::{TranscriptTotals, UsageFn};

/// Token usage by model key, such as `claude-opus-5-5`.
pub type UsageMap = std::collections::BTreeMap<String, TokenUsage>;

/// The program that runs an agent. Each method reads only what this
/// harness owns: its environment variables and its transcript files.
pub trait Harness: Send + Sync {
    /// The harness name an actor records, such as `claude-code`.
    fn name(&self) -> &'static str;

    /// True when this process runs inside the harness.
    fn detected(&self) -> bool;

    /// The harness's own session ID, which names its transcript.
    fn session_id(&self) -> Option<String>;

    /// The harness version, such as `2.1.322`.
    fn version(&self) -> Option<String>;

    /// The reasoning effort the agent runs at, when the harness says.
    fn effort(&self) -> Option<String>;

    /// The transcript of `session`, for an agent that works in `dir`.
    fn transcript(&self, dir: &Path, session: &str) -> Option<PathBuf>;

    /// The short model name the transcript's latest turn used, such as
    /// `opus-5.5`.
    fn model(&self, transcript: &Path) -> Option<String>;

    /// The usage and active time so far. `cache_dir` may keep state
    /// between calls, so each call reads only what is new.
    fn totals(&self, transcript: &Path, cache_dir: &Path) -> TranscriptTotals;
}

/// Every harness with an implementation, in the order detection tries them.
pub fn harnesses() -> Vec<&'static dyn Harness> {
    vec![&ClaudeCode]
}

/// The harness this process runs in, if any.
pub fn detect_harness() -> Option<&'static dyn Harness> {
    harnesses().into_iter().find(|h| h.detected())
}

/// The caller as [`detect`] found it.
pub struct Identity {
    pub context: ClientContext,
    /// The harness that reads the transcript: the detected one, else one
    /// that names its own session.
    pub harness: Option<&'static dyn Harness>,
    /// The agent's transcript, when usage should be sent.
    pub usage_transcript: Option<PathBuf>,
}

impl Identity {
    /// Reads usage from the transcript on demand, with the cache in
    /// `<data_dir>/usage-cache`. None when there is nothing to send.
    pub fn usage_source(&self, data_dir: &Path) -> Option<UsageFn> {
        let harness = self.harness?;
        let transcript = self.usage_transcript.clone()?;
        let cache = data_dir.join("usage-cache");
        Some(std::sync::Arc::new(move || {
            Some(harness.totals(&transcript, &cache))
        }))
    }
}

fn env(k: &str) -> Option<String> {
    std::env::var(k)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Finds who is calling from `cwd`. `root` is the git root of `cwd`, from
/// [`crate::git::git_root`], or None outside a repo. `repo_actor` is the
/// `actor` line of the repo's `.llmtodo.toml`, in the format of
/// `LLMTODO_ACTOR`.
///
/// Only `LLMTODO_ACTOR` overrides agent detection. A repo file is shared by
/// everyone in the repo, so a human actor there must not turn a detected
/// agent into the owner. It may still set agent details.
pub fn detect(
    cwd: &Path,
    root: Option<&Path>,
    repo_actor: Option<&str>,
) -> Result<Identity, String> {
    // `AI_AGENT` looks like `claude-code_2-1-322_agent`. Any harness that
    // sets it marks an agent, even one with no implementation here.
    let ai_agent = env("AI_AGENT");
    let harness = detect_harness();
    let detected_agent = ai_agent.is_some() || harness.is_some();
    let harness_name = harness.map(|h| h.name().to_string()).or_else(|| {
        ai_agent
            .as_deref()
            .map(|a| a.split('_').next().unwrap_or(a).to_string())
    });

    let repo_actor = repo_actor.map(parse_actor).transpose()?;
    let mut actor = if let Some(a) = env("LLMTODO_ACTOR") {
        parse_actor(&a)?
    } else if let Some(a) = repo_actor.filter(|a| !detected_agent || a.kind == ActorKind::Agent) {
        a
    } else if detected_agent {
        ActorInput {
            kind: ActorKind::Agent,
            harness: harness_name,
            ..ActorInput::default()
        }
    } else {
        ActorInput::default()
    };

    let harness_version = harness.and_then(|h| h.version()).or_else(|| {
        ai_agent
            .as_deref()
            .and_then(|a| a.split('_').nth(1))
            .map(|v| v.replace('-', "."))
    });
    // The harness that reads the transcript: the detected one, else one
    // that names its own session, such as a test or a script that sets
    // `CLAUDE_CODE_SESSION_ID` with its own `LLMTODO_ACTOR`.
    let reader = harness.or_else(|| harnesses().into_iter().find(|h| h.session_id().is_some()));
    let own_session = reader.and_then(|h| h.session_id());
    let session = env("LLMTODO_SESSION").or_else(|| own_session.clone());
    let generated = session.is_none() && actor.kind == ActorKind::Agent;
    let session_id = session
        .clone()
        .or_else(|| generated.then(|| format!("gen-{}", uuid::Uuid::now_v7())));

    // The transcript is named after the harness's own session ID, even when
    // LLMTODO_SESSION names the work session differently.
    let transcript = match (reader, own_session.or(session), actor.kind) {
        (Some(h), Some(s), ActorKind::Agent) => h
            .transcript(root.unwrap_or(cwd), &s)
            .or_else(|| h.transcript(cwd, &s)),
        _ => None,
    };
    if actor.kind == ActorKind::Agent {
        if let Some(m) = env("LLMTODO_MODEL") {
            actor.model = Some(m);
        } else if actor.model.is_none() {
            actor.model = reader
                .zip(transcript.as_deref())
                .and_then(|(h, t)| h.model(t));
        }
        if actor.effort.is_none() {
            actor.effort = reader.and_then(|h| h.effort());
        }
    }

    let (worktree, branch, head) = match root {
        Some(r) => {
            let (branch, head) = crate::git::branch_and_head(r);
            (Some(r.display().to_string()), branch, head)
        }
        None => (None, None, None),
    };
    let host = env("LLMTODO_HOST").or_else(|| {
        Command::new("hostname")
            .arg("-s")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|h| !h.is_empty())
    });
    let context = ClientContext {
        actor,
        session_id,
        session_generated: generated,
        host,
        worktree,
        branch,
        head,
        transcript_path: transcript.as_ref().map(|p| p.display().to_string()),
        herdr_pane: env("HERDR_PANE_ID"),
        harness_version,
        client_version: Some(VERSION.to_string()),
        usage: None,
        active_seconds: None,
    };
    // Usage is read later, and only for writes: see `usage_source`.
    let usage_transcript = transcript
        .filter(|_| context.actor.kind == ActorKind::Agent && env("LLMTODO_NO_USAGE").is_none());
    Ok(Identity {
        context,
        harness: reader,
        usage_transcript,
    })
}

/// Parses `human`, `owner`, `human:Name`, or
/// `agent:harness/model/effort` (any part may be empty).
pub fn parse_actor(s: &str) -> Result<ActorInput, String> {
    let s = s.trim();
    let (kind, rest) = s.split_once(':').unwrap_or((s, ""));
    match kind.to_ascii_lowercase().as_str() {
        "human" | "owner" => Ok(ActorInput {
            kind: ActorKind::Human,
            name: (!rest.is_empty()).then(|| rest.to_string()),
            ..ActorInput::default()
        }),
        "agent" => {
            let mut parts = rest
                .split('/')
                .map(|p| (!p.trim().is_empty()).then(|| p.trim().to_string()));
            Ok(ActorInput {
                kind: ActorKind::Agent,
                name: None,
                harness: parts.next().flatten(),
                model: parts.next().flatten(),
                effort: parts.next().flatten(),
            })
        }
        _ => Err(format!(
            "actor must be `human`, `human:<name>` or `agent:<harness>/<model>/<effort>`, got {s:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actors() {
        let a = parse_actor("agent:ollama/qwen3-coder").unwrap();
        assert_eq!(
            (a.kind, a.harness.as_deref(), a.model.as_deref(), a.effort),
            (ActorKind::Agent, Some("ollama"), Some("qwen3-coder"), None)
        );
        assert_eq!(parse_actor("human").unwrap().kind, ActorKind::Human);
        assert_eq!(
            parse_actor("human:Sam").unwrap().name.as_deref(),
            Some("Sam")
        );
        assert!(parse_actor("robot").is_err());
    }

    #[test]
    fn claude_code_is_a_known_harness() {
        assert!(harnesses().iter().any(|h| h.name() == "claude-code"));
    }
}
