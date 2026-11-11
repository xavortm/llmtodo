//! The review section: what the owner tries and checks before a ticket
//! closes.
//!
//! A ticket moves to a review status only with this section. The
//! description stays the record. The format:
//!
//! ```text
//! +++ Review
//! ### Steps to reproduce
//! 1. Open the inbox.
//! 2. Hover an option button.
//!
//! ### What to check
//! - The text turns black on the white background.
//! +++
//! ```
//!
//! Each non-empty line under a heading is one item. A leading `1.`, `-` or
//! `*` is not part of the item.

use serde::{Deserialize, Serialize};

use crate::markdown::{SectionKind, sections};

/// The title of the review section.
pub const REVIEW_SECTION: &str = "Review";
/// The heading of the steps.
pub const STEPS_HEADING: &str = "Steps to reproduce";
/// The heading of the checks.
pub const CHECKS_HEADING: &str = "What to check";
/// The status names that get the review flag when the server creates a
/// status itself, for a new team or a Linear import. The flag on the status
/// decides; a name never does after that.
pub const DEFAULT_REVIEW_STATUSES: &[&str] = &["In Review", "User review"];

/// An example section, for error messages and docs.
pub const REVIEW_FORMAT: &str = "+++ Review\n### Steps to reproduce\n1. Open the inbox.\n2. Hover an option button.\n\n### What to check\n- The text turns black on the white background.\n+++\n";

/// Whether a new status of this name gets the review flag. The match
/// ignores case.
pub fn is_default_review_status(name: &str) -> bool {
    DEFAULT_REVIEW_STATUSES
        .iter()
        .any(|n| name.trim().eq_ignore_ascii_case(n))
}

/// The steps and checks of a review section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Review {
    pub steps: Vec<String>,
    pub checks: Vec<String>,
}

impl Review {
    /// The headings that have no items, such as `["What to check"]`.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.steps.is_empty() {
            out.push(STEPS_HEADING);
        }
        if self.checks.is_empty() {
            out.push(CHECKS_HEADING);
        }
        out
    }

    /// Whether both headings have items.
    pub fn is_complete(&self) -> bool {
        self.missing().is_empty()
    }
}

/// The review section of a description, or None when it has none. A
/// section with a missing or empty heading comes back with an empty list.
pub fn review(description: &str) -> Option<Review> {
    let section = sections(description).into_iter().find(|s| {
        s.kind == SectionKind::Collapsible && s.title.eq_ignore_ascii_case(REVIEW_SECTION)
    })?;
    let body = &description[section.body_start..section.body_end];
    let mut out = Review::default();
    for h in sections(body) {
        if !matches!(h.kind, SectionKind::Heading { .. }) {
            continue;
        }
        let list = if h.title.eq_ignore_ascii_case(STEPS_HEADING) {
            &mut out.steps
        } else if h.title.eq_ignore_ascii_case(CHECKS_HEADING) {
            &mut out.checks
        } else {
            continue;
        };
        if list.is_empty() {
            *list = items(&body[h.body_start..h.body_end]);
        }
    }
    Some(out)
}

/// The missing headings of a description's review section. Both are
/// missing when there is no section.
pub fn review_missing(description: &str) -> Vec<&'static str> {
    review(description).map_or_else(|| vec![STEPS_HEADING, CHECKS_HEADING], |r| r.missing())
}

fn items(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| strip_marker(l).to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn strip_marker(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix(['-', '*', '+'])
        && (rest.is_empty() || rest.starts_with(' '))
    {
        return rest.trim();
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(['.', ')'])
            && (rest.is_empty() || rest.starts_with(' '))
        {
            return rest.trim();
        }
    }
    line
}

/// The body of a review section, for `+++ Review`. Steps get numbers and
/// checks get dashes.
pub fn render_review(steps: &[String], checks: &[String]) -> String {
    let mut out = format!("### {STEPS_HEADING}\n");
    for (ix, s) in steps.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", ix + 1, s.trim()));
    }
    out.push_str(&format!("\n### {CHECKS_HEADING}\n"));
    for c in checks {
        out.push_str(&format!("- {}\n", c.trim()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_steps_and_checks() {
        let doc = format!("Intro.\n\n{REVIEW_FORMAT}\n+++ Agent notes\nx\n+++\n");
        let r = review(&doc).unwrap();
        assert_eq!(r.steps, ["Open the inbox.", "Hover an option button."]);
        assert_eq!(r.checks, ["The text turns black on the white background."]);
        assert!(r.is_complete());
    }

    #[test]
    fn reports_missing_headings() {
        assert_eq!(
            review_missing("No section."),
            [STEPS_HEADING, CHECKS_HEADING]
        );
        let doc = "+++ Review\n### Steps to reproduce\n1. Open it.\n\n### What to check\n\n+++\n";
        assert_eq!(review_missing(doc), [CHECKS_HEADING]);
        let doc = "+++ review\n### what to check\n- It works.\n+++\n";
        assert_eq!(review_missing(doc), [STEPS_HEADING]);
    }

    #[test]
    fn render_round_trips() {
        let body = render_review(&["Open it.".into()], &["It opens.".into()]);
        let doc = format!("+++ Review\n{body}+++\n");
        let r = review(&doc).unwrap();
        assert_eq!(r.steps, ["Open it."]);
        assert_eq!(r.checks, ["It opens."]);
    }

    #[test]
    fn markers_strip_but_text_stays() {
        assert_eq!(strip_marker("12) Go"), "Go");
        assert_eq!(strip_marker("- Go"), "Go");
        assert_eq!(strip_marker("2026 is a year"), "2026 is a year");
        assert_eq!(strip_marker("-not a marker"), "-not a marker");
    }

    #[test]
    fn review_status_ignores_case() {
        assert!(is_default_review_status("user Review"));
        assert!(is_default_review_status(" in review"));
        assert!(!is_default_review_status("In Progress"));
    }
}
