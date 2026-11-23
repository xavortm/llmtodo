//! Line deltas for stored revision text (LLM-346).
//!
//! A delta rebuilds one text, the target, from another, its base. It is a
//! JSON array: `[start, len]` copies `len` bytes of the base from byte
//! `start`, and a string inserts itself. The diff cuts lines after each
//! `\n`, so every copy starts and ends on a character boundary.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Op {
    Copy(usize, usize),
    Insert(String),
}

/// The byte offset where each line starts, plus the text's length.
fn line_starts(text: &str) -> (Vec<&str>, Vec<usize>) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut at = 0;
    starts.push(at);
    for l in &lines {
        at += l.len();
        starts.push(at);
    }
    (lines, starts)
}

/// A delta that rebuilds `target` from `base`, or `None` when it would not
/// be shorter than `target` itself. The delta is checked by applying it, so
/// a `Some` always gives back `target` byte for byte.
pub fn encode(base: &str, target: &str) -> Option<String> {
    let (old, old_at) = line_starts(base);
    let (new, new_at) = line_starts(target);
    // A slow diff still gives a valid, if longer, delta.
    let deadline = Instant::now() + Duration::from_millis(200);
    let diff = capture_diff_slices_deadline(Algorithm::Myers, &old, &new, Some(deadline));
    let mut ops: Vec<Op> = Vec::new();
    for d in diff {
        let (tag, o, n) = d.as_tag_tuple();
        match tag {
            DiffTag::Equal => {
                let (start, end) = (old_at[o.start], old_at[o.end]);
                match ops.last_mut() {
                    Some(Op::Copy(s, len)) if *s + *len == start => *len += end - start,
                    _ => ops.push(Op::Copy(start, end - start)),
                }
            }
            DiffTag::Delete => {}
            DiffTag::Insert | DiffTag::Replace => {
                let text = &target[new_at[n.start]..new_at[n.end]];
                match ops.last_mut() {
                    Some(Op::Insert(s)) => s.push_str(text),
                    _ => ops.push(Op::Insert(text.to_string())),
                }
            }
        }
    }
    let delta = serde_json::to_string(&ops).ok()?;
    (delta.len() < target.len() && apply(base, &delta).ok()? == target).then_some(delta)
}

/// Rebuilds the target text from its base and a delta from `encode`.
pub fn apply(base: &str, delta: &str) -> Result<String, String> {
    let ops: Vec<Op> = serde_json::from_str(delta).map_err(|e| format!("bad delta: {e}"))?;
    let mut out = String::with_capacity(base.len());
    for op in ops {
        match op {
            Op::Copy(start, len) => out.push_str(
                start
                    .checked_add(len)
                    .and_then(|end| base.get(start..end))
                    .ok_or("delta copies outside its base")?,
            ),
            Op::Insert(s) => out.push_str(&s),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(base: &str, target: &str) {
        if let Some(d) = encode(base, target) {
            assert_eq!(apply(base, &d).unwrap(), target);
            assert!(d.len() < target.len());
        }
    }

    #[test]
    fn deltas_give_back_the_text() {
        let long: String = (0..200).map(|i| format!("line {i} ünïcode ✓\n")).collect();
        let edited = long
            .replace("line 100 ", "changed line\r\n")
            .replace("line 5 ", "");
        round_trip(&long, &edited);
        round_trip(&edited, &long);
        round_trip(&long, "");
        round_trip("", &long);
        round_trip(&long, long.trim_end());
        round_trip(&format!("{long}tail with no newline"), &long);
        assert!(
            encode(&long, &edited).is_some(),
            "a small edit stores a delta"
        );
        assert!(encode("a", "b").is_none(), "a short text stays whole");
    }

    #[test]
    fn a_bad_delta_is_an_error() {
        assert!(apply("abc", "[[1,5]]").is_err());
        assert!(apply("é", "[[1,1]]").is_err());
        assert!(apply("abc", "nope").is_err());
    }
}
