//! Builds the list rows from the issues: the filter, the sort and the
//! groups. It reads no GPUI state, so the list can run it on a background
//! task when it holds many issues. See `IssueList::recompute`.

use std::collections::{HashMap, HashSet};

use gpui_shared_string::SharedString;

use crate::issues::filter::ResolvedFilter;
use crate::issues::list::{GroupBy, Row, SortKey};
use crate::model::data::{Issue, IssueId, Label, LabelColor, Status};
use crate::model::scope::{Scope, ViewMembers};

/// From this many issues in the store, the list builds its rows on a
/// background task. Below it, the build runs on the main thread and fits in
/// a frame, so the common case gets no delay. `row_build_times` in
/// `shell/perf_tests.rs` measures it. In a debug build, grouped by status,
/// 2,000 issues take about 2 ms and 4,000 issues take 5 to 8 ms, against a
/// frame of 8 ms at 120 Hz. LLM-112 holds the full table.
pub const BACKGROUND_FROM: usize = 2_000;

/// Everything the build reads besides the issues.
pub struct RowsInput<'a> {
    pub scope: &'a Scope,
    pub view: Option<&'a ViewMembers>,
    pub query: &'a ResolvedFilter,
    /// Closed tasks closed before this time are hidden.
    pub cutoff: Option<jiff::Timestamp>,
    pub now: jiff::Timestamp,
    pub sort: (SortKey, bool),
    pub group: GroupBy,
    /// Collapsed groups, as `grouping:title`.
    pub collapsed: &'a HashSet<String>,
    /// Every status of the catalog, for the status groups.
    pub statuses: &'a [Status],
    /// Every label of the catalog, for the label groups.
    pub labels: &'a [Label],
}

/// The same input with its own copies, for a background task.
pub struct OwnedRowsInput {
    pub scope: Scope,
    pub view: Option<ViewMembers>,
    pub query: ResolvedFilter,
    pub cutoff: Option<jiff::Timestamp>,
    pub now: jiff::Timestamp,
    pub sort: (SortKey, bool),
    pub group: GroupBy,
    pub collapsed: HashSet<String>,
    pub statuses: Vec<Status>,
    pub labels: Vec<Label>,
}

impl OwnedRowsInput {
    pub fn borrow(&self) -> RowsInput<'_> {
        RowsInput {
            scope: &self.scope,
            view: self.view.as_ref(),
            query: &self.query,
            cutoff: self.cutoff,
            now: self.now,
            sort: self.sort,
            group: self.group,
            collapsed: &self.collapsed,
            statuses: &self.statuses,
            labels: &self.labels,
        }
    }
}

/// What the build gives the list.
#[derive(Debug, Default)]
pub struct BuiltRows {
    /// The issues that pass the filter, in sort order.
    pub visible: Vec<IssueId>,
    /// How many tasks the scope and filter would show but the "Hide old
    /// closed tasks" setting hides.
    pub hidden_closed: usize,
    pub rows: Vec<Row>,
    /// When the next shown task gets old enough for "Hide old closed
    /// tasks" to hide it. The list builds again then.
    pub next_hide: Option<jiff::Timestamp>,
}

/// The key that names a group in the collapsed set, such as `status:Done`.
pub fn group_key(group: GroupBy, title: &str) -> String {
    format!("{}:{title}", group.label().to_lowercase())
}

/// Issue keys sort by team, then by number, so LLM-9 comes before LLM-10.
fn key_order(key: &str) -> (&str, u64) {
    match key.rsplit_once('-') {
        Some((team, n)) => (team, n.parse().unwrap_or(u64::MAX)),
        None => (key, 0),
    }
}

/// Filters, sorts and groups `issues`.
pub fn build<'i>(input: &RowsInput<'_>, issues: impl Iterator<Item = &'i Issue>) -> BuiltRows {
    let mut hidden = 0;
    let mut next_hide: Option<jiff::Timestamp> = None;
    // A closed task hides this long after it closed.
    let hide_age = input.cutoff.map(|c| input.now.duration_since(c));
    let mut kept: Vec<&Issue> = issues
        .filter(|issue| {
            input.scope.matches(issue, input.view) && input.query.matches(issue, input.now)
        })
        .filter(|issue| {
            // The Trash shows what it holds, however old.
            let hide = !issue.deleted && input.cutoff.is_some_and(|c| issue.closed_before(c));
            hidden += usize::from(hide);
            if !hide
                && !issue.deleted
                && issue.status.is_closed()
                && let (Some(age), Some(at)) = (hide_age, issue.closed_at)
                && let Ok(when) = at.checked_add(age)
            {
                next_hide = Some(next_hide.map_or(when, |n| n.min(when)));
            }
            !hide
        })
        .collect();
    sort(&mut kept, input);
    let rows = group(&kept, input);
    BuiltRows {
        visible: kept.iter().map(|i| i.id).collect(),
        hidden_closed: hidden,
        rows,
        next_hide,
    }
}

fn sort(issues: &mut [&Issue], input: &RowsInput<'_>) {
    let (key, reversed) = input.sort;
    // Equal sort values fall back to the key, so the order is always the
    // same. `*i` keeps the borrow of the issue, not of the sort's slot.
    match key {
        SortKey::Key => issues.sort_by_cached_key(|i| {
            let i = *i;
            let key: &str = i.key.as_ref();
            (key_order(key), key)
        }),
        SortKey::Status => issues.sort_by_cached_key(|i| {
            let i = *i;
            (i.status.rank(), i.key.as_ref() as &str)
        }),
        // Newest first reads best for dates.
        SortKey::Added => issues.sort_by_cached_key(|i| {
            let i = *i;
            (std::cmp::Reverse(i.added), i.key.as_ref() as &str)
        }),
        SortKey::Llm => {
            let now = input.now;
            issues.sort_by_cached_key(|i| {
                let i = *i;
                let key: &str = i.key.as_ref();
                (i.llm_rank(now), key_order(key), key)
            });
        }
    }
    if reversed {
        issues.reverse();
    }
}

/// The grouping step alone, for `ab_times`.
#[cfg(test)]
pub fn group_for_bench(issues: &[&Issue], input: &RowsInput<'_>) -> Vec<Row> {
    group(issues, input)
}

/// Builds the rows from the sorted issues. Each group keeps the sort order.
/// Under "group by label", an issue with two labels shows in both groups.
/// A collapsed group keeps its header and hides its issues.
fn group(issues: &[&Issue], input: &RowsInput<'_>) -> Vec<Row> {
    type Group = (SharedString, Option<Label>, Option<Status>, Vec<IssueId>);
    // Each case puts every issue in its groups in one pass. A map finds the
    // group, so the cost does not grow with the number of groups.
    let mut groups: Vec<Group> = match input.group {
        GroupBy::None => return issues.iter().map(|i| Row::Issue(i.id)).collect(),
        GroupBy::Status => {
            // Every status in use, also ones the catalog lacks. Statuses are
            // equal by name in any case, so the map holds each exact name
            // seen, and a new spelling looks for its group once.
            let mut groups: Vec<Group> = input
                .statuses
                .iter()
                .map(|s| (s.name.clone(), None, Some(s.clone()), Vec::new()))
                .collect();
            let mut at: HashMap<&str, usize> = HashMap::new();
            for (ix, s) in input.statuses.iter().enumerate() {
                at.entry(s.name.as_ref()).or_insert(ix);
            }
            for issue in issues {
                let status = &issue.status;
                let ix = *at.entry(status.name.as_ref()).or_insert_with(|| {
                    groups
                        .iter()
                        .position(|g| g.2.as_ref() == Some(status))
                        .unwrap_or_else(|| {
                            groups.push((
                                status.name.clone(),
                                None,
                                Some(status.clone()),
                                Vec::new(),
                            ));
                            groups.len() - 1
                        })
                });
                groups[ix].3.push(issue.id);
            }
            // A stable sort, so equal ranks keep the catalog's order.
            groups.sort_by_key(|g| g.2.as_ref().map(Status::rank));
            groups
        }
        GroupBy::Label => {
            let mut groups: Vec<Group> = input
                .labels
                .iter()
                .map(|l| (l.name.clone(), Some(l.clone()), None, Vec::new()))
                .collect();
            let mut at: HashMap<(&str, LabelColor), usize> = HashMap::new();
            for (ix, l) in input.labels.iter().enumerate() {
                at.entry((l.name.as_ref(), l.color)).or_insert(ix);
            }
            let mut none = Vec::new();
            for issue in issues {
                if issue.labels.is_empty() {
                    none.push(issue.id);
                }
                for l in &issue.labels {
                    let ix = *at.entry((l.name.as_ref(), l.color)).or_insert_with(|| {
                        groups.push((l.name.clone(), Some(l.clone()), None, Vec::new()));
                        groups.len() - 1
                    });
                    // A label twice on one issue still shows it once.
                    if groups[ix].3.last() != Some(&issue.id) {
                        groups[ix].3.push(issue.id);
                    }
                }
            }
            groups.push(("No label".into(), None, None, none));
            groups
        }
        GroupBy::Assignee => {
            let mut groups: Vec<Group> = Vec::new();
            let mut at: HashMap<&str, usize> = HashMap::new();
            let mut none = Vec::new();
            for issue in issues {
                let Some(a) = &issue.assignee else {
                    none.push(issue.id);
                    continue;
                };
                let name = a.name_ref();
                let ix = *at.entry(name.as_ref()).or_insert_with(|| {
                    groups.push((name.clone(), None, None, Vec::new()));
                    groups.len() - 1
                });
                groups[ix].3.push(issue.id);
            }
            groups.push(("Unassigned".into(), None, None, none));
            groups
        }
    };
    groups.retain(|(_, _, _, members)| !members.is_empty());
    let mut rows = Vec::new();
    for (title, label, status, members) in groups {
        let collapsed = input.collapsed.contains(&group_key(input.group, &title));
        rows.push(Row::Header {
            title,
            count: members.len(),
            label,
            status,
            collapsed,
        });
        if !collapsed {
            rows.extend(members.into_iter().map(Row::Issue));
        }
    }
    rows
}
