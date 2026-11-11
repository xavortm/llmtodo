//! The one issue filter. The server turns it into SQL, and the app runs
//! [`IssueFilter::matches`] over the issues it holds. Both follow the rules
//! on the fields below, so a saved view and the list agree.
//!
//! `matches` is a pure function of the filter and the issue. It reads no
//! clock and no shared state, so it can run on any thread.
//!
//! Free text is not part of this filter. The list looks in what a row shows,
//! and the server searches titles, descriptions and comments with FTS5.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::StatusType;

/// The assignee word for an issue with no assignee.
pub const ASSIGNEE_NONE: &str = "none";
/// The assignee word for the human owner.
pub const ASSIGNEE_OWNER: &str = "owner";

/// The filters that the server query and the app's list share. An empty
/// filter keeps every issue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IssueFilter {
    /// Status names, ignoring case. An issue passes with any one of them.
    /// The server also accepts a status ID here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<String>,
    /// An issue passes with any one of these types. With `statuses` set
    /// too, an issue must pass both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status_types: Vec<StatusType>,
    /// Only open issues: any status type but completed and canceled.
    #[serde(default)]
    pub open: bool,
    /// Each label must be on the issue. A word is a full name, such as
    /// `Area/app`, or the part after the slash. The server refuses a word
    /// that names no label, or several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// `owner`, `none`, or an actor name. The server also takes `me` and an
    /// actor ID. A word that is no actor's whole name matches part of a
    /// name, and the server refuses a word that matches nobody.
    /// [`IssueFilter::matches`] has no caller, so a client turns `me` into
    /// the word it means first.
    pub assignee: Option<String>,
    /// Only issues an agent has claimed, as the app's Locked. A claim by the
    /// owner does not count. With `analyzing`, keeps issues with either one.
    #[serde(default)]
    pub claimed: bool,
    /// Only issues with a live Analyzing mark. With `claimed`, keeps issues
    /// with either one.
    #[serde(default)]
    pub analyzing: bool,
}

/// Who an issue is assigned to, as [`IssueFilter::matches`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssigneeRef<'a> {
    pub name: &'a str,
    pub is_owner: bool,
}

/// What [`IssueFilter::matches`] reads from an issue. Each client implements
/// it for its own issue type.
pub trait FilterSubject {
    fn status_name(&self) -> &str;
    fn status_type(&self) -> StatusType;
    /// The full name of each label, `Group/name` for a label in a group.
    fn label_names(&self) -> impl Iterator<Item = &str>;
    fn assignee(&self) -> Option<AssigneeRef<'_>>;
    /// Whether an agent holds a claim. A claim by the owner does not count.
    fn claimed(&self) -> bool;
    /// Whether a live Analyzing mark is on the issue and no agent claims it.
    fn analyzing(&self) -> bool;
}

fn label_matches(name: &str, word: &str) -> bool {
    let word = word.trim();
    name.eq_ignore_ascii_case(word)
        || name
            .rsplit_once('/')
            .is_some_and(|(_, short)| short.eq_ignore_ascii_case(word))
}

fn assignee_matches(assignee: Option<AssigneeRef<'_>>, word: &str) -> bool {
    let word = word.trim();
    if word.eq_ignore_ascii_case(ASSIGNEE_NONE) {
        return assignee.is_none();
    }
    let Some(assignee) = assignee else {
        return false;
    };
    if word.eq_ignore_ascii_case(ASSIGNEE_OWNER) {
        return assignee.is_owner;
    }
    assignee.name.to_lowercase().contains(&word.to_lowercase())
}

impl IssueFilter {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `issue` passes every filter.
    pub fn matches(&self, issue: &impl FilterSubject) -> bool {
        let name_ok = self.statuses.is_empty()
            || self
                .statuses
                .iter()
                .any(|s| issue.status_name().eq_ignore_ascii_case(s.trim()));
        let type_ok =
            self.status_types.is_empty() || self.status_types.contains(&issue.status_type());
        let open_ok = !self.open || issue.status_type().is_open();
        let labels_ok = self
            .labels
            .iter()
            .all(|w| issue.label_names().any(|l| label_matches(l, w)));
        let assignee_ok = self
            .assignee
            .as_deref()
            .is_none_or(|w| assignee_matches(issue.assignee(), w));
        let llm_ok = (!self.claimed && !self.analyzing)
            || (self.claimed && issue.claimed())
            || (self.analyzing && issue.analyzing());
        name_ok && type_ok && open_ok && labels_ok && assignee_ok && llm_ok
    }

    /// Narrows a saved view's filter with the filters of a query that names
    /// it. Lists add up, flags turn on, and a new assignee replaces the old.
    pub fn narrow(&mut self, extra: IssueFilter) {
        self.statuses.extend(extra.statuses);
        self.status_types.extend(extra.status_types);
        self.labels.extend(extra.labels);
        self.open |= extra.open;
        self.claimed |= extra.claimed;
        self.analyzing |= extra.analyzing;
        if extra.assignee.is_some() {
            self.assignee = extra.assignee;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Row {
        status: &'static str,
        kind: Option<StatusType>,
        labels: Vec<&'static str>,
        assignee: Option<(&'static str, bool)>,
        claimed: bool,
        analyzing: bool,
    }

    impl FilterSubject for Row {
        fn status_name(&self) -> &str {
            self.status
        }
        fn status_type(&self) -> StatusType {
            self.kind.unwrap_or(StatusType::Unstarted)
        }
        fn label_names(&self) -> impl Iterator<Item = &str> {
            self.labels.iter().copied()
        }
        fn assignee(&self) -> Option<AssigneeRef<'_>> {
            self.assignee
                .map(|(name, is_owner)| AssigneeRef { name, is_owner })
        }
        fn claimed(&self) -> bool {
            self.claimed
        }
        fn analyzing(&self) -> bool {
            self.analyzing
        }
    }

    fn todo() -> Row {
        Row {
            status: "In Review",
            kind: Some(StatusType::Started),
            labels: vec!["Area/app", "bug"],
            assignee: Some(("claude-code/opus-5.5/medium", false)),
            ..Default::default()
        }
    }

    #[test]
    fn an_empty_filter_keeps_everything() {
        assert!(IssueFilter::default().is_empty());
        assert!(IssueFilter::default().matches(&Row::default()));
    }

    #[test]
    fn status_names_ignore_case_and_any_one_passes() {
        let by = |names: &[&str]| IssueFilter {
            statuses: names.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert!(by(&["in review"]).matches(&todo()));
        assert!(by(&["Done", " IN REVIEW "]).matches(&todo()));
        assert!(!by(&["Done"]).matches(&todo()));
    }

    #[test]
    fn names_and_types_must_both_pass() {
        let f = IssueFilter {
            statuses: vec!["In Review".into()],
            status_types: vec![StatusType::Completed],
            ..Default::default()
        };
        assert!(!f.matches(&todo()));
    }

    #[test]
    fn open_drops_completed_and_canceled() {
        let f = IssueFilter {
            open: true,
            ..Default::default()
        };
        assert!(f.matches(&todo()));
        let done = Row {
            kind: Some(StatusType::Completed),
            ..todo()
        };
        assert!(!f.matches(&done));
    }

    #[test]
    fn every_label_must_be_on_the_issue() {
        let by = |words: &[&str]| IssueFilter {
            labels: words.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert!(by(&["area/APP"]).matches(&todo()));
        assert!(by(&["app", "bug"]).matches(&todo()));
        assert!(!by(&["app", "docs"]).matches(&todo()));
    }

    #[test]
    fn assignee_words() {
        let by = |w: &str| IssueFilter {
            assignee: Some(w.to_string()),
            ..Default::default()
        };
        let owned = Row {
            assignee: Some(("alex", true)),
            ..todo()
        };
        assert!(by("OWNER").matches(&owned));
        assert!(!by("owner").matches(&todo()));
        assert!(by("OPUS").matches(&todo()));
        assert!(!by("qwen").matches(&todo()));
        assert!(by("none").matches(&Row::default()));
        assert!(!by("none").matches(&todo()));
        assert!(!by("opus").matches(&Row::default()));
    }

    #[test]
    fn claimed_and_analyzing_keep_either_one() {
        let by = |claimed, analyzing| IssueFilter {
            claimed,
            analyzing,
            ..Default::default()
        };
        let locked = Row {
            claimed: true,
            ..Default::default()
        };
        let reading = Row {
            analyzing: true,
            ..Default::default()
        };
        assert!(by(true, false).matches(&locked));
        assert!(!by(true, false).matches(&reading));
        assert!(by(false, true).matches(&reading));
        assert!(by(true, true).matches(&locked) && by(true, true).matches(&reading));
        assert!(!by(true, true).matches(&Row::default()));
    }

    #[test]
    fn narrowing_a_view_adds_lists_and_replaces_the_assignee() {
        let mut base = IssueFilter {
            labels: vec!["a".into()],
            assignee: Some("x".into()),
            ..Default::default()
        };
        base.narrow(IssueFilter {
            labels: vec!["b".into()],
            assignee: Some("y".into()),
            open: true,
            ..Default::default()
        });
        assert_eq!(base.labels, ["a", "b"]);
        assert_eq!(base.assignee.as_deref(), Some("y"));
        assert!(base.open);
    }
}
