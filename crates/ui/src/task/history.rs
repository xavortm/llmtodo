//! The Activity and Sessions tabs. Activity holds an issue's field changes
//! and description revisions. Sessions is the work log: each agent session
//! that wrote to the issue, with where it ran, its commits, notes and token
//! use. Both are Markdown, so the task view shows them like any other tab.

use llmtodo_api_types::{ActivityEvent, ActivityField, Actor, History, WorkSession};
use serde_json::Value;

use crate::model::data::Assignee;

pub const ACTIVITY_TAB: &str = "Activity";
pub const SESSIONS_TAB: &str = "Sessions";

/// What each read-only tab shows, for its caption line.
pub const ACTIVITY_SHOWS: &str = "field changes and description versions";
pub const SESSIONS_SHOWS: &str = "agent sessions, with their commits, notes and token use";

/// Why the Activity and Sessions tabs cannot be edited, for their tooltip.
pub const READ_ONLY_WHY: &str = "The server writes this tab from the task's history. \
     Edit the description, or another tab, to change the task.";

/// The most commits a session lists.
const SESSION_COMMITS: usize = 10;

/// `Nov 10, 14:03` in local time.
fn when(ts: jiff::Timestamp) -> String {
    ts.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%b %-d, %H:%M")
        .to_string()
}

/// Who made a change: `You` for the owner, else the actor's short name.
fn who(actor: Option<&Actor>) -> String {
    match actor {
        None => "Someone".into(),
        Some(a) if a.is_owner => "You".into(),
        Some(a) if a.name == "github" => "GitHub".into(),
        Some(a) => Assignee::from_actor(a).label().to_string(),
    }
}

/// Backslashes before the characters Markdown would act on.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\`*_[]<>#|~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A field value in words: strings as they are, lists joined, `none` for
/// an empty value.
fn value(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "none".into(),
        Some(Value::String(s)) if s.is_empty() => "none".into(),
        Some(Value::String(s)) => escape(s),
        Some(Value::Array(items)) if items.is_empty() => "none".into(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| value(Some(i)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::Object(map)) => map
            .get("key")
            .or_else(|| map.get("title"))
            .or_else(|| map.get("url"))
            .or_else(|| map.get("name"))
            .map_or_else(
                || escape(&Value::Object(map.clone()).to_string()),
                |v| value(Some(v)),
            ),
        Some(other) => escape(&other.to_string()),
    }
}

/// What one event did, as a short phrase.
fn what(e: &ActivityEvent) -> String {
    let (old, new) = (e.old.as_ref(), e.new.as_ref());
    match &e.field {
        ActivityField::Created => "created the issue".into(),
        ActivityField::Deleted => "moved it to the Trash".into(),
        ActivityField::Restored => "took it out of the Trash".into(),
        ActivityField::Description => match new {
            Some(Value::Number(n)) => format!("changed the description, version {n}"),
            _ => "changed the description".into(),
        },
        ActivityField::CommentAdded => "added a comment".into(),
        ActivityField::Attached => format!("attached {}", value(new)),
        ActivityField::Claimed => "claimed it".into(),
        ActivityField::EstimateReason => {
            format!("gave a reason for the estimate change: {}", value(new))
        }
        field => {
            let field = field.as_str().replace('_', " ");
            format!("{field}: {} → {}", value(old), value(new))
        }
    }
}

/// The tab's Markdown. Empty when the server sent nothing.
pub fn activity_markdown(history: &History) -> String {
    let mut events: Vec<&ActivityEvent> = history.activity.iter().collect();
    events.sort_by_key(|e| std::cmp::Reverse(e.seq));
    let mut out = String::new();
    if !events.is_empty() {
        out.push_str("## Changes\n\n");
        for e in events {
            out.push_str(&format!(
                "- {} · **{}** {}\n",
                when(e.created_at),
                escape(&who(e.actor.as_ref())),
                what(e)
            ));
        }
    }
    let mut revisions: Vec<_> = history.revisions.iter().collect();
    revisions.sort_by_key(|r| std::cmp::Reverse(r.version));
    if !revisions.is_empty() {
        out.push_str("\n## Description versions\n\n");
        for r in revisions {
            out.push_str(&format!(
                "- Version {} · {} · {}\n",
                r.version,
                when(r.created_at),
                escape(&who(r.actor.as_ref()))
            ));
        }
        out.push_str("\nTo bring one back, run `llmtodo issue restore <key> --revision <n>`.\n");
    }
    out
}

/// `1.2M`, `45.3k` or `812` tokens.
fn tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// Text inside backticks. A backtick in it would end the span early.
fn code(text: &str) -> String {
    format!("`{}`", text.replace('`', "'"))
}

/// What one agent session cost on a task.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCost {
    pub cost: f64,
    /// The agent, when it last wrote and how many tokens it used.
    pub detail: String,
}

/// A task's cost per session, most expensive first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CostBreakdown {
    pub sessions: Vec<SessionCost>,
    /// Models with usage but no price. The cost leaves them out.
    pub unpriced: Vec<String>,
}

pub fn cost_breakdown(sessions: &[WorkSession]) -> CostBreakdown {
    let mut unpriced: Vec<String> = sessions
        .iter()
        .flat_map(|s| s.usage.unpriced_models.iter().cloned())
        .collect();
    unpriced.sort_unstable();
    unpriced.dedup();
    let mut rows: Vec<SessionCost> = sessions
        .iter()
        .filter_map(|s| {
            let cost = s.usage.cost_usd.filter(|c| *c > 0.0)?;
            Some(SessionCost {
                cost,
                detail: format!(
                    "{} · {} · {} tokens",
                    s.actor.name,
                    when(s.last_seen_at),
                    tokens(s.usage.total_tokens)
                ),
            })
        })
        .collect();
    rows.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    CostBreakdown {
        sessions: rows,
        unpriced,
    }
}

/// The Sessions tab's Markdown: one section per agent session, the one
/// seen last first. Empty when no agent wrote to the issue.
pub fn sessions_markdown(sessions: &[WorkSession]) -> String {
    let mut sessions: Vec<&WorkSession> = sessions.iter().collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.last_seen_at));
    let mut out = String::new();
    for s in sessions {
        let mut title = who(Some(&s.actor));
        if let Some(effort) = &s.actor.effort {
            title.push_str(&format!(" · {effort}"));
        }
        out.push_str(&format!("## {}\n\n", escape(&title)));
        out.push_str(&format!(
            "- Session {} · started {} · last seen {}\n",
            code(&s.session_id),
            when(s.started_at),
            when(s.last_seen_at)
        ));
        let mut place = Vec::new();
        if let Some(b) = &s.branch {
            place.push(format!("branch {}", code(b)));
        }
        if let Some(w) = &s.worktree {
            place.push(format!(
                "worktree {}",
                code(&crate::model::data::shorten_home(w))
            ));
        }
        if let Some(p) = &s.herdr_pane {
            place.push(format!("herdr pane {}", code(p)));
        }
        if !place.is_empty() {
            out.push_str(&format!("- {}\n", place.join(" · ")));
        }
        if s.usage.total_tokens > 0 {
            let cost = s
                .usage
                .cost_usd
                .map(|c| format!(" · {}", crate::model::data::format_cost(c)))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {} tokens in {} requests{cost}\n",
                tokens(s.usage.total_tokens),
                s.usage.requests
            ));
        }
        if let Some(seconds) = s.active_seconds {
            out.push_str(&format!(
                "- Active time {}\n",
                llmtodo_api_types::format_active_time(seconds)
            ));
        }
        if !s.commits.is_empty() {
            out.push_str("- Commits:\n");
            for c in s.commits.iter().take(SESSION_COMMITS) {
                let (hash, subject) = c.split_once(' ').unwrap_or((c.as_str(), ""));
                out.push_str(&format!("  - {} {}\n", code(hash), escape(subject)));
            }
            if s.commits.len() > SESSION_COMMITS {
                out.push_str(&format!(
                    "  - and {} more\n",
                    s.commits.len() - SESSION_COMMITS
                ));
            }
        }
        for note in &s.notes {
            out.push_str(&format!(
                "- Note, {}: {}\n",
                when(note.created_at),
                escape(note.body.trim())
            ));
        }
        if let Some(t) = &s.transcript_path {
            out.push_str(&format!(
                "- Transcript {}\n",
                code(&crate::model::data::shorten_home(t))
            ));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(seq: i64, field: &str, old: Value, new: Value, actor: Value) -> Value {
        json!({
            "seq": seq, "entity_type": "issue", "entity_id": "01a1c453-1260-7779-be1c-619faeb9d401",
            "issue": "LLM-1", "field": field, "old": old, "new": new, "actor": actor,
            "session_id": null, "created_at": "2026-11-10T12:03:00Z"
        })
    }

    #[test]
    fn changes_read_newest_first_in_words() {
        let owner = json!({"id": "01a1c453-1260-7779-be1c-619faeb9d402", "kind": "human", "name": "owner", "is_owner": true});
        let agent = json!({"id": "01a1c453-1260-7779-be1c-619faeb9d403", "kind": "agent", "name": "claude-code/opus-5.5/high", "harness": "claude-code", "model": "opus-5.5", "effort": "high", "is_owner": false});
        let history: History = serde_json::from_value(json!({
            "revisions": [
                {"version": 1, "body": "", "actor": owner.clone(), "session_id": null, "created_at": "2026-11-10T12:00:00Z"},
                {"version": 2, "body": "x", "actor": agent.clone(), "session_id": "s", "created_at": "2026-11-10T12:05:00Z"}
            ],
            "activity": [
                event(1, "created", Value::Null, json!({"key": "LLM-1", "title": "One"}), owner.clone()),
                event(2, "status", json!("Todo"), json!("In Progress"), agent.clone()),
                event(3, "labels", json!([]), json!(["Area/app", "bug"]), owner.clone()),
                event(4, "title", json!("One"), json!("Use *stars*"), owner.clone()),
                event(5, "assignee", json!("owner"), Value::Null, agent.clone()),
            ]
        }))
        .unwrap();
        let md = activity_markdown(&history);
        let lines: Vec<&str> = md.lines().filter(|l| l.starts_with("- ")).collect();
        let tail = |l: &str| l.split_once(" · ").unwrap().1.to_string();
        assert_eq!(
            lines.iter().take(5).map(|l| tail(l)).collect::<Vec<_>>(),
            [
                "**claude-code opus-5.5** assignee: owner → none",
                "**You** title: One → Use \\*stars\\*",
                "**You** labels: none → Area/app, bug",
                "**claude-code opus-5.5** status: Todo → In Progress",
                "**You** created the issue",
            ]
        );
        assert!(md.contains("- Version 2 · "));
        assert!(md.find("Version 2").unwrap() < md.find("Version 1").unwrap());
    }

    #[test]
    fn nothing_from_the_server_means_no_text() {
        let history: History =
            serde_json::from_value(json!({"revisions": [], "activity": []})).unwrap();
        assert_eq!(activity_markdown(&history), "");
    }

    #[test]
    fn sessions_list_where_they_ran_and_what_they_did() {
        let session = |id: &str, seen: &str, model: &str| {
            json!({
                "id": "01a1c453-1260-7779-be1c-619faeb9d404", "session_id": id, "issue": "LLM-1",
                "actor": {"id": "01a1c453-1260-7779-be1c-619faeb9d403", "kind": "agent",
                    "name": format!("claude-code/{model}/high"), "harness": "claude-code",
                    "model": model, "effort": "high", "is_owner": false},
                "harness_version": "2.1", "transcript_path": "/t/x.jsonl", "herdr_pane": "w1:p2",
                "host": "mac", "worktree": "/work/llmtodo (on another machine: mini)",
                "branch": "fix/x", "first_head": "aaaaaaa", "last_head": "bbbbbbb",
                "commits": ["bbbbbbb feat(app): the *thing*", "aaaaaaa fix: start"],
                "started_at": "2026-11-10T10:00:00Z", "last_seen_at": seen,
                "notes": [{"id": "01a1c453-1260-7779-be1c-619faeb9d405", "session_id": id,
                    "issue": "LLM-1", "body": "Plan: /tmp/p.md", "created_at": "2026-11-10T10:05:00Z"}],
                "usage": {"by_model": {}, "total_tokens": 1_234_567, "requests": 12, "cost_usd": 3.4}
            })
        };
        let sessions: Vec<WorkSession> = serde_json::from_value(json!([
            session("old", "2026-11-10T11:00:00Z", "sonnet-5.5"),
            session("new", "2026-11-10T12:00:00Z", "opus-5.5"),
        ]))
        .unwrap();
        let md = sessions_markdown(&sessions);
        // The session seen last comes first.
        assert!(md.find("opus-5.5").unwrap() < md.find("sonnet-5.5").unwrap());
        assert!(md.contains("## claude-code opus-5.5 · high\n"));
        assert!(md.contains("- Session `new` · started "));
        assert!(md.contains(
            "- branch `fix/x` · worktree `/work/llmtodo (on another machine: mini)` · herdr pane `w1:p2`"
        ));
        assert!(md.contains("- 1.2M tokens in 12 requests · $3.40"));
        assert!(md.contains("  - `bbbbbbb` feat(app): the \\*thing\\*"));
        assert!(md.contains(": Plan: /tmp/p.md"));
        assert!(md.contains("- Transcript `/t/x.jsonl`"));
        assert_eq!(sessions_markdown(&[]), "");
    }
}
