//! Three-way line merge for descriptions changed offline (LLM-471).
//!
//! Each side's change from the base is a list of hunks: a range of base
//! lines and the lines that replace it. Hunks of both sides that overlap or
//! touch form one cluster. A cluster that only one side changed takes that
//! side's lines. A cluster both sides changed merges only when both made
//! the same change. Touching hunks count as a clash, as in git, so two
//! edits of neighbouring lines never interleave by accident.

use similar::{Algorithm, DiffTag, capture_diff_slices};

/// One change of a side: base lines `start..end` become `lines`.
#[derive(Debug, Clone)]
struct Hunk<'a> {
    start: usize,
    end: usize,
    lines: Vec<&'a str>,
    ours: bool,
}

fn hunks<'a>(base: &[&'a str], side: &[&'a str], ours: bool) -> Vec<Hunk<'a>> {
    capture_diff_slices(Algorithm::Myers, base, side)
        .into_iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| {
            let (_, old, new) = op.as_tag_tuple();
            Hunk {
                start: old.start,
                end: old.end,
                lines: side[new].to_vec(),
                ours,
            }
        })
        .collect()
}

/// The lines one side puts in place of base lines `start..end`, from its
/// hunks inside that range.
fn side_text<'a>(base: &[&'a str], start: usize, end: usize, hs: &[&Hunk<'a>]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut at = start;
    for h in hs {
        out.extend_from_slice(&base[at..h.start]);
        out.extend_from_slice(&h.lines);
        at = h.end;
    }
    out.extend_from_slice(&base[at..end]);
    out
}

/// Merges `ours` and `theirs`, two edits of `base`. `None` means both
/// changed the same lines in different ways.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Option<String> {
    if ours == theirs || theirs == base {
        return Some(ours.to_string());
    }
    if ours == base {
        return Some(theirs.to_string());
    }
    let b: Vec<&str> = base.split_inclusive('\n').collect();
    let o: Vec<&str> = ours.split_inclusive('\n').collect();
    let t: Vec<&str> = theirs.split_inclusive('\n').collect();
    let mut all = hunks(&b, &o, true);
    all.extend(hunks(&b, &t, false));
    all.sort_by_key(|h| (h.start, h.end));

    let mut out = String::with_capacity(ours.len().max(theirs.len()));
    let mut at = 0;
    let mut i = 0;
    while i < all.len() {
        // A cluster: hunks that overlap or touch the ones before them.
        let start = all[i].start;
        let mut end = all[i].end;
        let mut j = i + 1;
        while j < all.len() && all[j].start <= end {
            end = end.max(all[j].end);
            j += 1;
        }
        let cluster = &all[i..j];
        for l in &b[at..start] {
            out.push_str(l);
        }
        let mine: Vec<&Hunk> = cluster.iter().filter(|h| h.ours).collect();
        let other: Vec<&Hunk> = cluster.iter().filter(|h| !h.ours).collect();
        let lines = match (mine.is_empty(), other.is_empty()) {
            (false, true) => side_text(&b, start, end, &mine),
            (true, false) => side_text(&b, start, end, &other),
            _ => {
                let a = side_text(&b, start, end, &mine);
                let c = side_text(&b, start, end, &other);
                if a != c {
                    return None;
                }
                a
            }
        };
        for l in lines {
            out.push_str(l);
        }
        at = end;
        i = j;
    }
    for l in &b[at..] {
        out.push_str(l);
    }
    Some(out)
}

/// The shared text with the offline text kept in a `+++ Sync conflict`
/// section at the end. The offline text sits in a code fence, so its own
/// `+++` sections and owner questions stay inert.
pub fn conflict_section(theirs: &str, ours: &str, when: &str, who: &str) -> String {
    let longest = ours
        .lines()
        .map(|l| {
            let t = l.trim_start();
            t.bytes().take_while(|b| *b == b'`').count()
        })
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let mut out = theirs.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&format!(
        "+++ Sync conflict\n{who} changed this description offline at {when}, while it also changed on the shared database. The text above is the shared version. The offline version follows. Merge them by hand, then delete this section.\n\n{fence}markdown\n{}\n{fence}\n+++\n",
        ours.trim_end()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_to_different_lines_merge() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let ours = "ONE\ntwo\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfour\nFIVE\n";
        assert_eq!(
            merge3(base, ours, theirs).as_deref(),
            Some("ONE\ntwo\nthree\nfour\nFIVE\n")
        );
    }

    #[test]
    fn additions_at_both_ends_merge() {
        let base = "a\nb\nc\n";
        let ours = "start\na\nb\nc\n";
        let theirs = "a\nb\nc\nend\n";
        assert_eq!(
            merge3(base, ours, theirs).as_deref(),
            Some("start\na\nb\nc\nend\n")
        );
    }

    #[test]
    fn the_same_change_on_both_sides_merges() {
        let base = "a\nb\nc\nd\n";
        let both = "a\nB\nc\nd\n";
        let theirs = "a\nB\nc\nD\n";
        assert_eq!(merge3(base, both, theirs).as_deref(), Some("a\nB\nc\nD\n"));
    }

    #[test]
    fn changes_to_the_same_line_clash() {
        let base = "a\nb\nc\n";
        assert_eq!(merge3(base, "a\nmine\nc\n", "a\ntheirs\nc\n"), None);
        // Neighbouring lines clash too.
        assert_eq!(merge3("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n"), None);
    }

    #[test]
    fn one_side_unchanged_takes_the_other() {
        assert_eq!(merge3("x\n", "x\n", "y\n").as_deref(), Some("y\n"));
        assert_eq!(merge3("x\n", "y\n", "x\n").as_deref(), Some("y\n"));
    }

    #[test]
    fn a_conflict_section_fences_the_offline_text() {
        let ours = "Mine\n\n+++ Agent notes\n```\ncode\n```\n+++\n";
        let out = conflict_section("Shared\n", ours, "2026-11-23 10:00", "Owner");
        assert!(out.starts_with("Shared\n\n+++ Sync conflict\n"), "{out}");
        assert!(out.contains("````markdown\nMine\n"), "{out}");
        assert!(out.ends_with("````\n+++\n"), "{out}");
        let sections = llmtodo_core::markdown::sections(&out);
        let titles: Vec<_> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Sync conflict"], "{out}");
    }
}
