//! Markdown helpers: `+++` collapsible sections, heading sections and
//! description patches.
//!
//! The parser is line based. It knows about fenced code blocks, so a `#` or a
//! `+++` inside a fence never counts as a section boundary.

use serde::{Deserialize, Serialize};

/// What kind of section a [`Section`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// `## Title`, up to the next heading of the same or a higher level, or
    /// the next `+++` open or close line, whichever comes first.
    Heading { level: u8 },
    /// `+++ Title` up to a line that holds only `+++`.
    Collapsible,
}

/// One section of a Markdown document. All ranges are byte offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Section {
    pub kind: SectionKind,
    pub title: String,
    /// The whole section, from its first line to its last, including the
    /// closing `+++` line and its newline.
    pub start: usize,
    pub end: usize,
    /// The body only: after the opening line, before the closing line.
    pub body_start: usize,
    pub body_end: usize,
}

struct Line<'a> {
    start: usize,
    /// End of the line, after its newline if it has one.
    end: usize,
    text: &'a str,
    in_fence: bool,
}

fn lines(src: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut fence: Option<(u8, usize)> = None;
    for raw in src.split_inclusive('\n') {
        let text = raw.trim_end_matches(['\n', '\r']);
        let trimmed = text.trim_start();
        let indent = text.len() - trimmed.len();
        let fence_char = trimmed
            .as_bytes()
            .first()
            .copied()
            .filter(|c| *c == b'`' || *c == b'~');
        let mut is_fence_line = false;
        if indent < 4
            && let Some(c) = fence_char
        {
            let run = trimmed.bytes().take_while(|b| *b == c).count();
            if run >= 3 {
                match fence {
                    None => {
                        fence = Some((c, run));
                        is_fence_line = true;
                    }
                    Some((fc, fr)) if fc == c && run >= fr && trimmed[run..].trim().is_empty() => {
                        fence = None;
                        is_fence_line = true;
                    }
                    _ => {}
                }
            }
        }
        out.push(Line {
            start: pos,
            end: pos + raw.len(),
            text,
            in_fence: fence.is_some() || is_fence_line,
        });
        pos += raw.len();
    }
    out
}

fn heading(text: &str) -> Option<(u8, String)> {
    let t = text.trim_start();
    if text.len() - t.len() > 3 {
        return None;
    }
    let level = t.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim().to_string();
    Some((level as u8, title))
}

fn collapsible_open(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("+++")?;
    if rest.starts_with('+') {
        return None;
    }
    let title = rest.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn is_collapsible_close(text: &str) -> bool {
    text.trim() == "+++"
}

/// Lists every heading section and `+++` section, in document order.
///
/// An unclosed `+++` section runs to the end of the document, the way Linear
/// shows it.
pub fn sections(src: &str) -> Vec<Section> {
    let ls = lines(src);
    let mut out = Vec::new();
    for (i, l) in ls.iter().enumerate() {
        if l.in_fence {
            continue;
        }
        if let Some((level, title)) = heading(l.text) {
            // A `+++` line ends the heading section too. Otherwise a patch on
            // the heading would swallow the `+++` sections after it, or run
            // past the close of the `+++` section that holds it.
            let end = ls[i + 1..]
                .iter()
                .find(|n| {
                    !n.in_fence
                        && (heading(n.text).is_some_and(|(lv, _)| lv <= level)
                            || collapsible_open(n.text).is_some()
                            || is_collapsible_close(n.text))
                })
                .map_or(src.len(), |n| n.start);
            out.push(Section {
                kind: SectionKind::Heading { level },
                title,
                start: l.start,
                end,
                body_start: l.end,
                body_end: end,
            });
        } else if let Some(title) = collapsible_open(l.text) {
            let close = ls[i + 1..]
                .iter()
                .find(|n| !n.in_fence && is_collapsible_close(n.text));
            let (body_end, end) = close.map_or((src.len(), src.len()), |c| (c.start, c.end));
            out.push(Section {
                kind: SectionKind::Collapsible,
                title,
                start: l.start,
                end,
                body_start: l.end,
                body_end,
            });
        }
    }
    out
}

/// A change to a description or document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Patch {
    /// Replace the body of the one section whose title matches. The title
    /// match ignores case and surrounding spaces. The heading or `+++` lines
    /// stay.
    ReplaceSection { section: String, body: String },
    /// Add a new `+++ Title` section at the end when no section has the
    /// title, or replace its body when one does.
    UpsertCollapsible { section: String, body: String },
    /// Replace one exact string, which must occur exactly once.
    ReplaceExact { old: String, new: String },
    /// Add a question to the `+++ Owner questions` section. See
    /// [`crate::questions`] for the format.
    AskQuestion {
        question: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        context: String,
        /// Empty for a free-text question.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<crate::questions::QuestionOption>,
    },
    /// Write the answer to one question, such as `Q1`. A single option
    /// letter expands to the option. An empty answer opens it again.
    AnswerQuestion { id: String, answer: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("patch target {target:?} matched {count} times; it must match exactly once")]
    MatchCount { target: String, count: usize },
    #[error("patch target is empty")]
    EmptyTarget,
    #[error("{0}")]
    Question(String),
}

fn ensure_newline(s: &str) -> String {
    if s.is_empty() || s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

/// Applies a patch and returns the new text.
pub fn apply_patch(src: &str, patch: &Patch) -> Result<String, PatchError> {
    match patch {
        Patch::ReplaceExact { old, new } => {
            if old.is_empty() {
                return Err(PatchError::EmptyTarget);
            }
            let count = src.matches(old.as_str()).count();
            if count != 1 {
                return Err(PatchError::MatchCount {
                    target: old.clone(),
                    count,
                });
            }
            Ok(src.replacen(old.as_str(), new, 1))
        }
        Patch::AskQuestion {
            question,
            context,
            options,
        } => crate::questions::ask(src, question, context, options).map(|(out, _)| out),
        Patch::AnswerQuestion { id, answer } => crate::questions::answer(src, id, answer),
        Patch::ReplaceSection { section, body } => {
            let s = find_one(src, section, None)?;
            Ok(splice(src, &s, body))
        }
        Patch::UpsertCollapsible { section, body } => {
            match find_one(src, section, Some(SectionKind::Collapsible)) {
                Ok(s) => Ok(splice(src, &s, body)),
                Err(PatchError::MatchCount { count: 0, .. }) => {
                    let mut out = src.trim_end().to_string();
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&format!(
                        "+++ {}\n{}+++\n",
                        section.trim(),
                        ensure_newline(body)
                    ));
                    Ok(out)
                }
                Err(e) => Err(e),
            }
        }
    }
}

fn find_one(src: &str, title: &str, kind: Option<SectionKind>) -> Result<Section, PatchError> {
    let want = title.trim();
    if want.is_empty() {
        return Err(PatchError::EmptyTarget);
    }
    let mut hits: Vec<Section> = sections(src)
        .into_iter()
        .filter(|s| s.title.eq_ignore_ascii_case(want))
        .filter(|s| kind.is_none_or(|k| k == s.kind))
        .collect();
    if hits.len() != 1 {
        return Err(PatchError::MatchCount {
            target: want.to_string(),
            count: hits.len(),
        });
    }
    Ok(hits.remove(0))
}

fn splice(src: &str, s: &Section, body: &str) -> String {
    let mut body = ensure_newline(body);
    // Keep one blank line before the next heading or `+++` line, as it was
    // written.
    if matches!(s.kind, SectionKind::Heading { .. })
        && s.body_end < src.len()
        && src[s.body_start..s.body_end].ends_with("\n\n")
        && !body.ends_with("\n\n")
    {
        body.push('\n');
    }
    // A closing `+++` line needs a newline before it.
    if s.kind == SectionKind::Collapsible && s.body_end == src.len() {
        return format!("{}{}+++\n", &src[..s.body_start], body);
    }
    format!("{}{}{}", &src[..s.body_start], body, &src[s.body_end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Title\n\nIntro.\n\n## Plan\n\nStep one.\n\n### Detail\n\nDeep.\n\n## Notes\n\n```md\n## Not a heading\n+++ Not a section\n```\n\n+++ Agent notes\nold notes\n+++\n";

    #[test]
    fn lists_sections() {
        let titles: Vec<_> = sections(DOC)
            .into_iter()
            .map(|s| (s.title, s.kind))
            .collect();
        assert_eq!(
            titles,
            [
                ("Title".into(), SectionKind::Heading { level: 1 }),
                ("Plan".into(), SectionKind::Heading { level: 2 }),
                ("Detail".into(), SectionKind::Heading { level: 3 }),
                ("Notes".into(), SectionKind::Heading { level: 2 }),
                ("Agent notes".into(), SectionKind::Collapsible),
            ]
        );
    }

    #[test]
    fn heading_section_stops_at_same_level() {
        let plan = sections(DOC)
            .into_iter()
            .find(|s| s.title == "Plan")
            .unwrap();
        assert_eq!(
            &DOC[plan.body_start..plan.body_end],
            "\nStep one.\n\n### Detail\n\nDeep.\n\n"
        );
    }

    #[test]
    fn replaces_heading_section() {
        let out = apply_patch(
            DOC,
            &Patch::ReplaceSection {
                section: "plan".into(),
                body: "\nNew step.\n".into(),
            },
        )
        .unwrap();
        assert!(out.contains("## Plan\n\nNew step.\n\n## Notes"), "{out}");
        assert!(!out.contains("Deep."));
    }

    #[test]
    fn replaces_collapsible_section() {
        let out = apply_patch(
            DOC,
            &Patch::ReplaceSection {
                section: "Agent notes".into(),
                body: "new".into(),
            },
        )
        .unwrap();
        assert!(out.ends_with("+++ Agent notes\nnew\n+++\n"), "{out}");
    }

    #[test]
    fn upsert_appends_missing_section() {
        let out = apply_patch(
            "Body.",
            &Patch::UpsertCollapsible {
                section: "Agent notes".into(),
                body: "hi".into(),
            },
        )
        .unwrap();
        assert_eq!(out, "Body.\n\n+++ Agent notes\nhi\n+++\n");
        let again = apply_patch(
            &out,
            &Patch::UpsertCollapsible {
                section: "Agent notes".into(),
                body: "bye\n".into(),
            },
        )
        .unwrap();
        assert_eq!(again, "Body.\n\n+++ Agent notes\nbye\n+++\n");
    }

    #[test]
    fn unclosed_collapsible_runs_to_end() {
        let out = apply_patch(
            "+++ Notes\nopen",
            &Patch::ReplaceSection {
                section: "Notes".into(),
                body: "x".into(),
            },
        )
        .unwrap();
        assert_eq!(out, "+++ Notes\nx\n+++\n");
    }

    #[test]
    fn zero_or_two_matches_fail() {
        let none = apply_patch(
            DOC,
            &Patch::ReplaceSection {
                section: "Missing".into(),
                body: String::new(),
            },
        );
        assert_eq!(
            none,
            Err(PatchError::MatchCount {
                target: "Missing".into(),
                count: 0
            })
        );
        let twice = apply_patch(
            "a a",
            &Patch::ReplaceExact {
                old: "a".into(),
                new: "b".into(),
            },
        );
        assert_eq!(
            twice,
            Err(PatchError::MatchCount {
                target: "a".into(),
                count: 2
            })
        );
        let dup = "## A\nx\n## A\ny\n";
        assert!(matches!(
            apply_patch(
                dup,
                &Patch::ReplaceSection {
                    section: "A".into(),
                    body: String::new()
                }
            ),
            Err(PatchError::MatchCount { count: 2, .. })
        ));
    }

    #[test]
    fn exact_replace() {
        assert_eq!(
            apply_patch(
                "one two",
                &Patch::ReplaceExact {
                    old: "two".into(),
                    new: "2".into()
                }
            )
            .unwrap(),
            "one 2"
        );
        assert_eq!(
            apply_patch(
                "x",
                &Patch::ReplaceExact {
                    old: String::new(),
                    new: "y".into()
                }
            ),
            Err(PatchError::EmptyTarget)
        );
    }

    #[test]
    fn fenced_markers_are_ignored() {
        assert!(
            sections(DOC)
                .iter()
                .all(|s| s.title != "Not a heading" && s.title != "Not a section")
        );
    }

    #[test]
    fn heading_patch_keeps_following_collapsible() {
        let out = apply_patch(
            "## Plan\n\nStep one.\n\n+++ Agent notes\nkeep me\n+++\n",
            &Patch::ReplaceSection {
                section: "Plan".into(),
                body: "\nStep two.\n".into(),
            },
        )
        .unwrap();
        assert_eq!(
            out,
            "## Plan\n\nStep two.\n\n+++ Agent notes\nkeep me\n+++\n"
        );
    }

    #[test]
    fn heading_inside_collapsible_stops_at_close() {
        let out = apply_patch(
            "+++ Agent notes\n## Log\nentry\n+++\n\nTail text.\n",
            &Patch::ReplaceSection {
                section: "Log".into(),
                body: "new entry".into(),
            },
        )
        .unwrap();
        assert_eq!(
            out,
            "+++ Agent notes\n## Log\nnew entry\n+++\n\nTail text.\n"
        );
    }
}
