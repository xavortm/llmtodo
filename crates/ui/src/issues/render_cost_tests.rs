//! LLM-178: the list, the filter and the sidebar do the same work as
//! before with fewer passes and copies. These tests hold the new code to
//! the old results.

use std::collections::HashSet;

use crate::issues::list::{GroupBy, Row, SortKey};
use crate::issues::rows::{self, RowsInput};
use crate::model::data::{Assignee, Catalog, Issue, IssueId, Label};
use crate::model::scope::{Scope, ViewMembers};
use crate::model::store::IssueStore;
use crate::shell::sidebar::{ScopeCounts, scopes};

/// The demo set, many times over, with labels, assignees and statuses
/// mixed, some of them unknown to the catalog.
fn workload() -> (Catalog, Vec<Issue>) {
    let catalog = crate::model::demo::catalog();
    let demo = crate::model::demo::issues(&catalog);
    let issues = (0..120)
        .map(|n| {
            let mut issue = demo[n % demo.len()].clone();
            issue.id = IssueId::from_u128(n as u128 + 1);
            issue.key = format!("LLM-{}", n + 1).into();
            issue.deleted = n % 11 == 0;
            if n % 7 == 0 {
                issue.status.name = "Odd status".into();
            }
            if n % 5 == 0 {
                issue.labels.push(Label {
                    name: "Unknown/label".into(),
                    color: catalog.labels[0].color,
                });
            }
            if n % 9 == 0 {
                issue.labels.clear();
            }
            issue.assignee = match n % 4 {
                0 => None,
                1 => Some(Assignee::Person("Ana".into())),
                _ => Some(Assignee::Person(format!("Person {}", n % 3).into())),
            };
            issue
        })
        .collect();
    (catalog, issues)
}

/// The grouping as it was before LLM-178: one walk over the issues per
/// group.
fn old_groups(issues: &[&Issue], group: GroupBy, catalog: &Catalog) -> Vec<(String, Vec<IssueId>)> {
    let members = |keep: &dyn Fn(&Issue) -> bool| -> Vec<IssueId> {
        issues.iter().filter(|i| keep(i)).map(|i| i.id).collect()
    };
    let mut groups: Vec<(String, Vec<IssueId>)> = match group {
        // Parent groups came after LLM-178, so they have no old code.
        GroupBy::None | GroupBy::Parent | GroupBy::Project => return Vec::new(),
        GroupBy::Status => {
            let mut statuses = catalog.all_statuses();
            for issue in issues {
                if !statuses.contains(&issue.status) {
                    statuses.push(issue.status.clone());
                }
            }
            statuses.sort_by_key(crate::model::data::Status::rank);
            statuses
                .into_iter()
                .map(|s| (s.name.to_string(), members(&|i| i.status == s)))
                .collect()
        }
        GroupBy::Label => {
            let mut labels = catalog.labels.clone();
            for issue in issues {
                for l in &issue.labels {
                    if !labels.contains(l) {
                        labels.push(l.clone());
                    }
                }
            }
            let mut g: Vec<_> = labels
                .into_iter()
                .map(|l| (l.name.to_string(), members(&|i| i.labels.contains(&l))))
                .collect();
            g.push(("No label".into(), members(&|i| i.labels.is_empty())));
            g
        }
        GroupBy::Assignee => {
            let mut names = Vec::new();
            for issue in issues {
                if let Some(a) = &issue.assignee
                    && !names.contains(&a.name())
                {
                    names.push(a.name());
                }
            }
            let mut g: Vec<_> = names
                .into_iter()
                .map(|n| {
                    let m =
                        members(&|i| i.assignee.as_ref().map(Assignee::name).as_ref() == Some(&n));
                    (n.to_string(), m)
                })
                .collect();
            g.push(("Unassigned".into(), members(&|i| i.assignee.is_none())));
            g
        }
    };
    groups.retain(|(_, m)| !m.is_empty());
    groups
}

fn new_groups(rows: &[Row]) -> Vec<(String, Vec<IssueId>)> {
    let mut out: Vec<(String, Vec<IssueId>)> = Vec::new();
    for row in rows {
        match row {
            Row::Header { title, .. } => out.push((title.to_string(), Vec::new())),
            Row::Issue(id) => out.last_mut().unwrap().1.push(*id),
        }
    }
    out
}

#[test]
fn one_pass_grouping_matches_the_old_walk() {
    let (catalog, issues) = workload();
    let statuses = catalog.all_statuses();
    let collapsed = HashSet::new();
    let query = crate::issues::filter::Filter::parse("").resolve(&catalog);
    for group in [GroupBy::Status, GroupBy::Label, GroupBy::Assignee] {
        for sort in [SortKey::Key, SortKey::Status, SortKey::Added] {
            let input = RowsInput {
                scope: &Scope::All,
                view: None,
                query: &query,
                cutoff: None,
                now: jiff::Timestamp::now(),
                sort: (sort, false),
                group,
                collapsed: &collapsed,
                statuses: &statuses,
                labels: &catalog.labels,
                projects: &catalog.projects,
            };
            let built = rows::build(&input, issues.iter());
            let by_id: std::collections::HashMap<IssueId, &Issue> =
                issues.iter().map(|i| (i.id, i)).collect();
            let sorted: Vec<&Issue> = built.visible.iter().map(|id| by_id[id]).collect();
            assert_eq!(
                new_groups(&built.rows),
                old_groups(&sorted, group, &catalog),
                "{group:?} sorted by {sort:?}"
            );
        }
    }
}

#[test]
fn sidebar_counts_match_a_count_per_scope() {
    let (catalog, issues) = workload();
    let mut store = IssueStore::new();
    store.remap(catalog.clone());
    store.replace_all(issues);
    let view_name = catalog.views[0].key.clone();
    store.view = Some(ViewMembers {
        name: view_name.clone(),
        ids: store.iter().step_by(3).map(|i| i.id).collect(),
    });
    let counts = ScopeCounts::of(&store);
    let mut checked = 0;
    for row in scopes(&catalog)
        .into_iter()
        .map(|r| r.scope)
        .chain([Scope::Team("NONE".into()), Scope::View("Other".into())])
    {
        let expected = match &row {
            Scope::Inbox => store.open_question_count(),
            scope => store
                .iter()
                .filter(|i| *scope == Scope::Trash || !i.status.is_closed())
                .filter(|i| scope.matches(i, store.view.as_ref()))
                .count(),
        };
        assert_eq!(counts.get(&row), expected, "{row:?}");
        checked += usize::from(expected > 0);
    }
    assert!(checked >= 4, "the workload fills several scopes");
}

#[test]
fn labels_that_fit_matches_the_width_rule() {
    use crate::issues::columns::labels_that_fit;
    use gpui_kit::rems;
    // Twelve short labels: "+N" needs two digits.
    let many = [2usize; 12];
    let fit = labels_that_fit(many, rems(10.));
    assert!(fit >= 1 && fit < many.len());
    assert_eq!(labels_that_fit(many, rems(100.)), many.len());
    assert_eq!(labels_that_fit([0usize; 0], rems(1.)), 0);
}

#[test]
fn the_build_says_when_the_next_closed_task_hides() {
    let (catalog, mut issues) = workload();
    let now = jiff::Timestamp::now();
    let day = jiff::SignedDuration::from_hours(24);
    let done = catalog
        .all_statuses()
        .into_iter()
        .find(|s| s.is_done())
        .unwrap();
    for issue in &mut issues {
        issue.status = catalog.all_statuses()[0].clone();
        issue.closed_at = None;
        issue.deleted = false;
    }
    // One closed 8 days ago: hidden. One closed 2 days ago: hides in 5.
    issues[0].status = done.clone();
    issues[0].closed_at = Some(now - day * 8);
    issues[1].status = done;
    issues[1].closed_at = Some(now - day * 2);
    let statuses = catalog.all_statuses();
    let collapsed = HashSet::new();
    let query = crate::issues::filter::Filter::parse("").resolve(&catalog);
    let input = RowsInput {
        scope: &Scope::All,
        view: None,
        query: &query,
        cutoff: Some(crate::model::data::hide_cutoff(7, now)),
        now,
        sort: (SortKey::Key, false),
        group: GroupBy::None,
        collapsed: &collapsed,
        statuses: &statuses,
        labels: &catalog.labels,
        projects: &catalog.projects,
    };
    let built = rows::build(&input, issues.iter());
    assert_eq!(built.hidden_closed, 1);
    assert_eq!(built.next_hide, Some(now - day * 2 + day * 7));
}

#[gpui_kit::test]
fn hiding_old_closed_tasks_refilters_only_when_a_task_crosses(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::AppContext as _;
    cx.update(crate::init);
    let (_, view) = cx.update(|cx| {
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| crate::shell::workspace::Workspace::new(window, cx))
        })
        .expect("open test window")
    });
    cx.update(|cx| {
        let list = view.read(cx).list().clone();
        list.update(cx, |list, _| list.hide_closed_after = Some(7));
    });
    cx.read(|cx| {
        let this = view.read(cx);
        let list = this.list().read(cx);
        let old = this.visible_issue(0, cx).clone();
        let done = this.store().read(cx).catalog.all_statuses();
        let done = done.into_iter().find(|s| s.is_done()).unwrap();
        let now = jiff::Timestamp::now();
        let day = jiff::SignedDuration::from_hours(24);
        let check = |change: &dyn Fn(&mut Issue)| {
            let mut new = old.clone();
            change(&mut new);
            list.needs_refilter(&old, &new)
        };
        assert!(!check(&|i| i.description = "New text".into()));
        assert!(!check(&|i| i.cost = Some(2.0)));
        // Closed long ago: it hides now.
        assert!(check(&|i| {
            i.status = done.clone();
            i.closed_at = Some(now - day * 30);
        }));
    });
}

/// Times the old and the new grouping and text match side by side, round
/// by round, so both see the same machine load. LLM-178 holds the numbers.
///
/// ```sh
/// cargo test -p llmtodo-ui ab_times -- --ignored --nocapture
/// ```
#[test]
#[ignore = "prints timings"]
fn ab_times() {
    use std::time::{Duration, Instant};
    let (catalog, base) = workload();
    let issues: Vec<Issue> = (0..10_000)
        .map(|n| {
            let mut i = base[n % base.len()].clone();
            i.id = IssueId::from_u128(n as u128 + 1);
            i.key = format!("LLM-{}", n + 1).into();
            i.title = format!("{} number {n}", i.title).into();
            i
        })
        .collect();
    let refs: Vec<&Issue> = issues.iter().collect();
    let statuses = catalog.all_statuses();
    let collapsed = HashSet::new();
    let query = crate::issues::filter::Filter::parse("number 1").resolve(&catalog);
    let median = |mut t: Vec<Duration>| {
        t.sort();
        t[t.len() / 2]
    };
    for group in [GroupBy::Status, GroupBy::Label, GroupBy::Assignee] {
        let input = RowsInput {
            scope: &Scope::All,
            view: None,
            query: &crate::issues::filter::Filter::parse("").resolve(&catalog),
            cutoff: None,
            now: jiff::Timestamp::now(),
            sort: (SortKey::Key, false),
            group,
            collapsed: &collapsed,
            statuses: &statuses,
            labels: &catalog.labels,
            projects: &catalog.projects,
        };
        let (mut old, mut new) = (Vec::new(), Vec::new());
        for _ in 0..21 {
            let start = Instant::now();
            std::hint::black_box(old_groups(&refs, group, &catalog));
            old.push(start.elapsed());
            let start = Instant::now();
            std::hint::black_box(rows::group_for_bench(&refs, &input));
            new.push(start.elapsed());
        }
        println!(
            "10000 issues  group by {group:?}: old {:?}, new {:?}",
            median(old),
            median(new)
        );
    }
    let text = query.text.clone();
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for _ in 0..21 {
        let start = Instant::now();
        std::hint::black_box(
            issues
                .iter()
                .filter(|i| {
                    i.title.to_lowercase().contains(&text)
                        || i.key.to_lowercase().contains(&text)
                        || i.labels
                            .iter()
                            .any(|l| l.name.to_lowercase().contains(&text))
                })
                .count(),
        );
        old.push(start.elapsed());
        let start = Instant::now();
        let contains = crate::issues::filter::contains_lowercase;
        std::hint::black_box(
            issues
                .iter()
                .filter(|i| {
                    contains(&i.title, &text)
                        || contains(&i.key, &text)
                        || i.labels.iter().any(|l| contains(&l.name, &text))
                })
                .count(),
        );
        new.push(start.elapsed());
    }
    println!(
        "10000 issues  filter text match: old {:?}, new {:?}",
        median(old),
        median(new)
    );
}
