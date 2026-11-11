//! Shared workloads for the GPU frame benchmarks in `benches/`.
//!
//! The issues copy the demo set, so the rows hold the same mix of statuses,
//! labels, assignees and claims as the app shows. The shell perf tests in
//! `llmtodo-ui` build the same workload.

use llmtodo_core::IssueId;
use llmtodo_ui::model::data::{Catalog, Issue};
use llmtodo_ui::model::demo;

/// The list sizes every task list benchmark runs at.
pub const SIZES: [usize; 3] = [1_000, 10_000, 50_000];

/// A description with the shape agents write: prose, then a `+++` section.
fn description(n: usize) -> String {
    let prose = "The list drops frames while an agent writes to many tasks. ".repeat(20);
    format!("## Goal {n}\n\n{prose}\n\n+++ Agent notes\n\n{prose}\n\n+++\n")
}

/// `count` issues built from the demo set. One in twenty is claimed.
pub fn workload(count: usize) -> (Catalog, Vec<Issue>) {
    let catalog = demo::catalog();
    let base = demo::issues(&catalog);
    let issues = (0..count)
        .map(|n| {
            let mut issue = base[n % base.len()].clone();
            issue.id = IssueId::from_u128(n as u128 + 1);
            issue.key = format!("LLM-{}", n + 1).into();
            issue.title = format!("{} number {n}", issue.title).into();
            issue.description = description(n).into();
            if !n.is_multiple_of(20) {
                issue.claim = None;
            }
            issue
        })
        .collect();
    (catalog, issues)
}
