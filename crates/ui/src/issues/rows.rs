//! Builds the list rows from the issues: the filter, the sort and the
//! groups. It reads no GPUI state, so the list can run it on a background
//! task when it holds many issues. See `IssueList::recompute`.

use std::collections::HashSet;

use gpui_shared_string::SharedString;

use crate::issues::filter::ResolvedFilter;
use crate::issues::list::{GroupBy, Row, SortKey};
use crate::model::data::{Assignee, Issue, IssueId, Label, Status};
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
    let mut kept: Vec<&Issue> = issues
        .filter(|issue| {
            input.scope.matches(issue, input.view) && input.query.matches(issue, input.now)
        })
        .filter(|issue| {
            // The Trash shows what it holds, however old.
            let hide = !issue.deleted && input.cutoff.is_some_and(|c| issue.closed_before(c));
            hidden += usize::from(hide);
            !hide
        })
        .collect();
    sort(&mut kept, input);
    let rows = group(&kept, input);
    BuiltRows {
        visible: kept.iter().map(|i| i.id).collect(),
        hidden_closed: hidden,
        rows,
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

/// Builds the rows from the sorted issues. Each group keeps the sort order.
/// Under "group by label", an issue with two labels shows in both groups.
/// A collapsed group keeps its header and hides its issues.
fn group(issues: &[&Issue], input: &RowsInput<'_>) -> Vec<Row> {
    type Group = (SharedString, Option<Label>, Option<Status>, Vec<IssueId>);
    let members = |keep: &dyn Fn(&Issue) -> bool| -> Vec<IssueId> {
        issues.iter().filter(|i| keep(i)).map(|i| i.id).collect()
    };
    let mut groups: Vec<Group> = match input.group {
        GroupBy::None => return issues.iter().map(|i| Row::Issue(i.id)).collect(),
        GroupBy::Status => {
            // Every status in use, also ones the catalog lacks.
            let mut statuses = input.statuses.to_vec();
            for issue in issues {
                if !statuses.contains(&issue.status) {
                    statuses.push(issue.status.clone());
                }
            }
            statuses.sort_by_key(Status::rank);
            statuses
                .into_iter()
                .map(|status| {
                    let m = members(&|i| i.status == status);
                    (status.name.clone(), None, Some(status), m)
                })
                .collect()
        }
        GroupBy::Label => {
            let mut labels = input.labels.to_vec();
            for issue in issues {
                for l in &issue.labels {
                    if !labels.contains(l) {
                        labels.push(l.clone());
                    }
                }
            }
            let mut groups: Vec<_> = labels
                .into_iter()
                .map(|label| {
                    let m = members(&|i| i.labels.contains(&label));
                    (label.name.clone(), Some(label), None, m)
                })
                .collect();
            groups.push((
                "No label".into(),
                None,
                None,
                members(&|i| i.labels.is_empty()),
            ));
            groups
        }
        GroupBy::Assignee => {
            let mut names: Vec<SharedString> = Vec::new();
            for issue in issues {
                if let Some(a) = &issue.assignee
                    && !names.contains(&a.name())
                {
                    names.push(a.name());
                }
            }
            let mut groups: Vec<_> = names
                .into_iter()
                .map(|name| {
                    let m = members(&|i| {
                        i.assignee.as_ref().map(Assignee::name).as_ref() == Some(&name)
                    });
                    (name, None, None, m)
                })
                .collect();
            groups.push((
                "Unassigned".into(),
                None,
                None,
                members(&|i| i.assignee.is_none()),
            ));
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
