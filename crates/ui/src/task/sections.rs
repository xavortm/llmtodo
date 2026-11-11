//! Finds `+++ Title` / `+++` sections in Markdown.
//!
//! Linear writes collapsible sections as a line `+++ Title`, the body, and a
//! closing line `+++`. Markdown parsers see those lines as plain paragraphs,
//! so the app finds them itself. The editor uses the line ranges for folding,
//! and the preview uses the segments to draw collapsible blocks.
//!
//! Rules: a marker starts at column 0. Markers inside fenced code blocks do
//! not count. Sections do not nest. A section with no closing line runs to
//! the end of the text.

/// A piece of a Markdown document, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    /// Plain Markdown outside any section.
    Markdown(&'a str),
    /// A `+++` section. `index` counts sections from 0 in source order.
    Section {
        index: usize,
        title: &'a str,
        body: &'a str,
    },
}

/// A section by its source lines, zero-based. `end_line` is the closing
/// `+++` line, or the last line when the section is not closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionLines {
    pub start_line: usize,
    pub end_line: usize,
}

fn fence_marker(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// The title when `line` opens a section.
fn opener_title(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("+++")?;
    let title = rest.strip_prefix([' ', '\t'])?.trim();
    (!title.is_empty()).then_some(title)
}

fn is_closer(line: &str) -> bool {
    line.trim_end() == "+++"
}

struct Line<'a> {
    text: &'a str,
    start: usize,
    end_with_newline: usize,
}

fn lines(source: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for piece in source.split_inclusive('\n') {
        let end_with_newline = start + piece.len();
        let text = piece.trim_end_matches('\n').trim_end_matches('\r');
        out.push(Line {
            text,
            start,
            end_with_newline,
        });
        start = end_with_newline;
    }
    out
}

struct Found {
    title_line: usize,
    close_line: Option<usize>,
}

fn find(lines: &[Line<'_>]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut fence: Option<&'static str> = None;
    let mut open: Option<usize> = None;
    for (ix, line) in lines.iter().enumerate() {
        if let Some(marker) = fence {
            if line.text.trim_start().starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_marker(line.text) {
            fence = Some(marker);
            continue;
        }
        match open {
            None if opener_title(line.text).is_some() => open = Some(ix),
            Some(title_line) if is_closer(line.text) => {
                found.push(Found {
                    title_line,
                    close_line: Some(ix),
                });
                open = None;
            }
            _ => {}
        }
    }
    if let Some(title_line) = open {
        found.push(Found {
            title_line,
            close_line: None,
        });
    }
    found
}

/// Section line ranges, for editor folding.
pub fn section_lines(source: &str) -> Vec<SectionLines> {
    let lines = lines(source);
    let last = lines.len().saturating_sub(1);
    find(&lines)
        .into_iter()
        .map(|f| SectionLines {
            start_line: f.title_line,
            end_line: f.close_line.unwrap_or(last),
        })
        .collect()
}

/// Splits `source` into plain Markdown and sections, for the preview.
pub fn segments(source: &str) -> Vec<Segment<'_>> {
    let lines = lines(source);
    let mut out = Vec::new();
    let mut cursor = 0;
    for (index, f) in find(&lines).into_iter().enumerate() {
        let title_line = &lines[f.title_line];
        let before = &source[cursor..title_line.start];
        if !before.trim().is_empty() {
            out.push(Segment::Markdown(before));
        }
        let body_start = title_line.end_with_newline;
        let (body_end, next) = match f.close_line {
            Some(close) => (lines[close].start, lines[close].end_with_newline),
            None => (source.len(), source.len()),
        };
        out.push(Segment::Section {
            index,
            title: opener_title(title_line.text).unwrap_or_default(),
            body: source[body_start..body_end.max(body_start)].trim_matches('\n'),
        });
        cursor = next;
    }
    let rest = &source[cursor..];
    if !rest.trim().is_empty() {
        out.push(Segment::Markdown(rest));
    }
    out
}

/// A tab in the task view: the text outside any section, then one tab per
/// `+++` section, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub title: String,
    pub body: String,
    /// What a read-only tab shows, such as "field changes and description
    /// versions". The description's own tabs are editable and have none.
    pub read_only: Option<&'static str>,
}

pub const DESCRIPTION_TAB: &str = "Description";

/// Splits `source` into tabs. Text outside sections joins into the first
/// tab, Description. It is left out when it is empty, so a description that
/// is all sections starts on its first section.
pub fn tabs(source: &str) -> Vec<Tab> {
    let mut main = Vec::new();
    let mut sections = Vec::new();
    for segment in segments(source) {
        match segment {
            Segment::Markdown(text) => main.push(text.trim_matches('\n')),
            Segment::Section { title, body, .. } => sections.push(Tab {
                title: title.to_string(),
                body: body.to_string(),
                read_only: None,
            }),
        }
    }
    let main = main.join("\n\n");
    let mut out = Vec::new();
    if !main.trim().is_empty() {
        out.push(Tab {
            title: DESCRIPTION_TAB.to_string(),
            body: main,
            read_only: None,
        });
    }
    out.extend(sections);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "Intro\n\n+++ Agent notes\nplan\nmore\n+++\n\nTail\n";

    #[test]
    fn finds_lines_of_a_closed_section() {
        assert_eq!(
            section_lines(DOC),
            vec![SectionLines {
                start_line: 2,
                end_line: 5
            }]
        );
    }

    #[test]
    fn splits_into_segments() {
        assert_eq!(
            segments(DOC),
            vec![
                Segment::Markdown("Intro\n\n"),
                Segment::Section {
                    index: 0,
                    title: "Agent notes",
                    body: "plan\nmore"
                },
                Segment::Markdown("\nTail\n"),
            ]
        );
    }

    #[test]
    fn ignores_markers_inside_code_fences() {
        let doc = "```\n+++ Not a section\n+++\n```\n";
        assert!(section_lines(doc).is_empty());
    }

    #[test]
    fn unclosed_section_runs_to_the_end() {
        let doc = "+++ Log\na\nb";
        assert_eq!(
            section_lines(doc),
            vec![SectionLines {
                start_line: 0,
                end_line: 2
            }]
        );
        assert_eq!(
            segments(doc),
            vec![Segment::Section {
                index: 0,
                title: "Log",
                body: "a\nb"
            }]
        );
    }

    #[test]
    fn tabs_start_with_description_then_sections() {
        let tabs = tabs(DOC);
        let titles: Vec<_> = tabs.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Description", "Agent notes"]);
        assert_eq!(tabs[0].body, "Intro\n\nTail");
        assert_eq!(tabs[1].body, "plan\nmore");
    }

    #[test]
    fn all_sections_means_no_description_tab() {
        let titles: Vec<_> = tabs("+++ A\na\n+++\n+++ B\nb\n+++\n")
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(titles, ["A", "B"]);
    }

    #[test]
    fn bare_marker_without_title_does_not_open() {
        assert!(section_lines("+++\ntext\n+++\n").is_empty());
    }
}
