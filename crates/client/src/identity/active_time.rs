//! Active working time, from the timestamps in a Claude Code transcript.
//! [`super::ClaudeCode`] uses it.
//!
//! The active time is the sum of the gaps between consecutive user and
//! assistant lines. Two kinds of gap do not count:
//!
//! - A gap that ends at a prompt the owner typed. The agent was waiting.
//! - A gap over [`IDLE_GAP_MS`]. Nothing runs that long inside one step.
//!
//! The lines of the main transcript and its subagent transcripts go on one
//! timeline, so an agent that runs subagents in parallel counts that wall
//! time once. Separate sessions report separately, and the server adds them.

use serde_json::Value;

/// The longest gap that still counts as work: 10 minutes. It equals the
/// longest foreground tool call Claude Code allows, so a slow build counts
/// and a longer silence does not.
pub const IDLE_GAP_MS: i64 = 10 * 60 * 1000;

/// When a line was written, in Unix milliseconds, and whether it is a
/// prompt from the owner.
pub type Mark = (i64, bool);

/// The mark of one transcript line. None for lines that are not user or
/// assistant lines, because those also appear while nothing is running.
pub fn mark(line: &str) -> Option<Mark> {
    if !line.contains("\"type\":\"user\"") && !line.contains("\"type\":\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if !matches!(v["type"].as_str(), Some("user" | "assistant")) {
        return None;
    }
    let at: jiff::Timestamp = v["timestamp"].as_str()?.parse().ok()?;
    Some((at.as_millisecond(), is_owner_prompt(&v)))
}

fn is_owner_prompt(v: &Value) -> bool {
    if v["type"] != "user" || v["isSidechain"] == true {
        return false;
    }
    v["origin"]["kind"] == "human"
        || v["message"]["content"]
            .as_str()
            .is_some_and(|c| c.starts_with("<command-name>"))
}

/// The active seconds of a set of marks, in any order.
pub fn active_seconds(marks: &[Mark]) -> u64 {
    let mut sorted = marks.to_vec();
    sorted.sort_unstable_by_key(|m| m.0);
    let ms: i64 = sorted
        .windows(2)
        .filter(|w| !w[1].1)
        .map(|w| w[1].0 - w[0].0)
        .filter(|gap| *gap <= IDLE_GAP_MS)
        .sum();
    (ms / 1000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn gaps_between_steps_add_up() {
        assert_eq!(
            active_seconds(&[(0, false), (MIN, false), (3 * MIN, false)]),
            180
        );
    }

    #[test]
    fn marks_from_several_files_count_once_in_time_order() {
        assert_eq!(
            active_seconds(&[(2 * MIN, false), (0, false), (MIN, false), (MIN, false)]),
            120
        );
    }

    #[test]
    fn a_gap_that_ends_at_an_owner_prompt_does_not_count() {
        assert_eq!(
            active_seconds(&[(0, false), (MIN, true), (2 * MIN, false)]),
            60
        );
    }

    #[test]
    fn the_idle_threshold_is_ten_minutes() {
        assert_eq!(active_seconds(&[(0, false), (10 * MIN, false)]), 600);
        assert_eq!(active_seconds(&[(0, false), (10 * MIN + 1, false)]), 0);
    }

    #[test]
    fn times_read_short() {
        use llmtodo_api_types::format_active_time as f;
        assert_eq!(f(45), "45s");
        assert_eq!(f(720), "12m");
        assert_eq!(f(3_900), "1h 05m");
    }

    #[test]
    fn only_user_and_assistant_lines_are_marks() {
        let at = "2026-11-11T07:00:00.000Z";
        assert!(mark(&format!(r#"{{"type":"system","timestamp":"{at}"}}"#)).is_none());
        let (when, prompt) = mark(&format!(
            r#"{{"type":"assistant","timestamp":"{at}","message":{{}}}}"#
        ))
        .unwrap();
        assert!(when > 0);
        assert!(!prompt);
    }

    #[test]
    fn owner_prompts_are_told_apart_from_tool_results_and_subagents() {
        let at = "2026-11-11T07:00:00.000Z";
        let line = |rest: &str| format!(r#"{{"type":"user","timestamp":"{at}",{rest}}}"#);
        let is_prompt = |l: String| mark(&l).unwrap().1;
        assert!(is_prompt(line(
            r#""origin":{"kind":"human"},"message":{"content":"go"}"#
        )));
        assert!(is_prompt(line(
            r#""message":{"content":"<command-name>/clear</command-name>"}"#
        )));
        assert!(!is_prompt(line(
            r#""message":{"content":[{"type":"tool_result"}]}"#
        )));
        assert!(!is_prompt(line(
            r#""isSidechain":true,"origin":{"kind":"human"},"message":{}"#
        )));
    }
}
