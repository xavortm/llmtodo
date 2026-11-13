//! The order of a parent's sub-issues (LLM-206).
//!
//! An order is a list of steps, and each step holds one task or several.
//! A step number lives on the sub-issue, scoped to its parent, and the steps
//! of one parent stay dense, 1..n. The order works as dependencies: each
//! task of step N blocks each task of step N+1. Those edges are real
//! `blocks` relations with source `order`, so every query that reads
//! `blocks` edges follows the order. Each order change deletes the parent's
//! order edges and writes them again, in the same transaction.

use std::collections::{HashMap, HashSet};

use llmtodo_api_types::*;
use serde_json::json;
use sqlx::SqliteConnection;

use super::issues::touch_updated;
use super::j;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::issues::{self, IssueRow};
use crate::store::order::{self, Child};
use crate::store::{deps, teams};

/// The steps of a parent as issue IDs, first step first. Only live
/// sub-issues count.
type Steps = Vec<Vec<String>>;

fn steps_of(children: &[Child]) -> Steps {
    let mut steps: Vec<(i64, Vec<String>)> = Vec::new();
    for ch in children.iter().filter(|c| !c.deleted) {
        let Some(n) = ch.order_step else { continue };
        match steps.last_mut() {
            Some((last, ids)) if *last == n => ids.push(ch.id.clone()),
            _ => steps.push((n, vec![ch.id.clone()])),
        }
    }
    steps.into_iter().map(|(_, ids)| ids).collect()
}

/// Two orders are the same when each step holds the same tasks. The order
/// inside a step does not matter.
fn same(a: &Steps, b: &Steps) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let x: HashSet<&String> = x.iter().collect();
            let y: HashSet<&String> = y.iter().collect();
            x == y
        })
}

/// The order of a parent's sub-issues.
pub async fn view(c: &mut SqliteConnection, r: &str) -> ApiResult<OrderView> {
    let parent_id = issues::resolve_id(c, r).await?;
    let parent = issues::row(c, &parent_id).await?;
    let children = order::children(c, &parent_id).await?;
    let live: Vec<String> = children
        .iter()
        .filter(|ch| !ch.deleted)
        .map(|ch| ch.id.clone())
        .collect();
    let statuses = teams::status_map(c).await?;
    let links = issues::link_map_with(c, &live, &statuses).await?;
    let steps = steps_of(&children)
        .into_iter()
        .enumerate()
        .map(|(i, ids)| {
            let issues: Vec<IssueLink> =
                ids.iter().filter_map(|id| links.get(id).cloned()).collect();
            OrderStep {
                step: i as u32 + 1,
                done: issues.iter().all(|l| !l.status.kind.is_open()),
                issues,
            }
        })
        .collect();
    let unordered = children
        .iter()
        .filter(|ch| !ch.deleted && ch.order_step.is_none())
        .filter_map(|ch| links.get(&ch.id).cloned())
        .collect();
    Ok(OrderView {
        parent: parse_key(&parent.key)?,
        steps,
        unordered,
    })
}

fn parse_key(k: &str) -> ApiResult<IssueKey> {
    k.parse()
        .map_err(|_| ApiError::internal(format!("bad key {k}")))
}

/// Writes the whole order of a parent. Returns the parent's ID.
pub async fn set(w: &mut Write<'_>, r: &str, req: SetOrder) -> ApiResult<String> {
    let parent = issues::resolve_live(w.c(), r).await?;
    let children = order::children(w.c(), &parent.id).await?;
    let live: HashMap<&str, &Child> = children
        .iter()
        .filter(|ch| !ch.deleted)
        .map(|ch| (ch.id.as_str(), ch))
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut foreign: Vec<String> = Vec::new();
    let mut repeated: Vec<String> = Vec::new();
    let mut steps: Steps = Vec::new();
    for keys in &req.steps {
        let mut ids = Vec::new();
        for key in keys {
            let key = key.to_string();
            let id = match issues::resolve_id(w.c(), &key).await {
                Ok(id) if live.contains_key(id.as_str()) => id,
                Ok(_) | Err(_) => {
                    foreign.push(key);
                    continue;
                }
            };
            if !seen.insert(id.clone()) {
                repeated.push(key);
                continue;
            }
            ids.push(id);
        }
        if !ids.is_empty() {
            steps.push(ids);
        }
    }
    if !foreign.is_empty() {
        return Err(ApiError::validation(format!(
            "{} {} not a live sub-issue of {}. An order holds only the direct sub-issues of its parent.",
            foreign.join(", "),
            if foreign.len() == 1 { "is" } else { "are" },
            parent.key
        ))
        .with_details(json!({ "reason": "not_a_sub_issue", "keys": foreign })));
    }
    if !repeated.is_empty() {
        return Err(ApiError::validation(format!(
            "{} {} in more than one step. A task has one step.",
            repeated.join(", "),
            if repeated.len() == 1 { "is" } else { "are" }
        ))
        .with_details(json!({ "reason": "repeated", "keys": repeated })));
    }
    apply(w, &parent, &children, steps).await?;
    Ok(parent.id)
}

/// Moves one task to step `step` of its parent's order, or out of the order
/// with `None`. `step` names an existing step, or the last step plus one for
/// a new last step. Returns whether the order changed.
pub async fn move_one(w: &mut Write<'_>, row: &IssueRow, step: Option<u32>) -> ApiResult<bool> {
    let Some(parent_id) = row.parent_id.clone() else {
        if step.is_none() {
            return Ok(false);
        }
        return Err(ApiError::validation(format!(
            "{} has no parent, so it has no order to join. Set a parent first.",
            row.key
        ))
        .with_details(json!({ "reason": "no_parent" })));
    };
    let parent = issues::row(w.c(), &parent_id).await?;
    let children = order::children(w.c(), &parent_id).await?;
    let mut steps = steps_of(&children);
    for ids in &mut steps {
        ids.retain(|id| id != &row.id);
    }
    if let Some(n) = step {
        let n = n as usize;
        let count = steps.len();
        if n == 0 || n > count + 1 {
            return Err(ApiError::validation(format!(
                "{} has {count} steps, so the step must be 1 to {}, or none",
                parent.key,
                count + 1
            ))
            .with_details(json!({ "reason": "step_out_of_range", "steps": count })));
        }
        if n <= count {
            steps[n - 1].push(row.id.clone());
        } else {
            steps.push(vec![row.id.clone()]);
        }
    }
    steps.retain(|ids| !ids.is_empty());
    apply(w, &parent, &children, steps).await
}

/// Takes a task out of its parent's order, before it goes to the trash or to
/// another parent.
pub async fn leave(w: &mut Write<'_>, row: &IssueRow) -> ApiResult<bool> {
    if row.order_step.is_none() {
        return Ok(false);
    }
    move_one(w, row, None).await
}

/// Writes `new` as the order of `parent` when it differs from the order in
/// `children`. One activity event on the parent records the change, and the
/// app reloads the parent's sub-issues from it.
async fn apply(
    w: &mut Write<'_>,
    parent: &IssueRow,
    children: &[Child],
    new: Steps,
) -> ApiResult<bool> {
    let old = steps_of(children);
    if same(&old, &new) {
        return Ok(false);
    }
    let before: HashMap<&str, Option<i64>> = children
        .iter()
        .map(|ch| (ch.id.as_str(), ch.order_step))
        .collect();
    let mut after: HashMap<&str, i64> = HashMap::new();
    for (i, ids) in new.iter().enumerate() {
        for id in ids {
            after.insert(id.as_str(), i as i64 + 1);
        }
    }
    order::clear_steps(w.c(), &parent.id).await?;
    for (id, step) in &after {
        order::set_step(w.c(), id, Some(*step)).await?;
    }
    rewrite_edges(w, &parent.id).await?;
    // Each sub-issue whose step changed counts as updated, so a client that
    // reloads by update time sees the new step.
    for ch in children {
        if before.get(ch.id.as_str()).copied().flatten() != after.get(ch.id.as_str()).copied() {
            issues::touch_updated(w.c(), &ch.id).await?;
        }
    }
    let keys: HashMap<&str, &str> = children
        .iter()
        .map(|ch| (ch.id.as_str(), ch.key.as_str()))
        .collect();
    let as_keys = |steps: &Steps| -> Vec<Vec<String>> {
        steps
            .iter()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| keys.get(id.as_str()).map(|k| k.to_string()))
                    .collect()
            })
            .collect()
    };
    w.event(
        EntityType::Issue,
        &parent.id,
        Some(&parent.id),
        ActivityField::Order,
        j(as_keys(&old)),
        j(as_keys(&new)),
    )
    .await?;
    touch_updated(w, &parent.id).await?;
    Ok(true)
}

/// Deletes the order edges of a parent and writes them again from the steps
/// of its live sub-issues: each task of step N blocks each task of step
/// N+1. The chain is transitive, so no edge skips a step. An edge that would
/// close a cycle with the other `blocks` edges is refused, and the whole
/// write rolls back.
pub async fn rewrite_edges(w: &mut Write<'_>, parent_id: &str) -> ApiResult<()> {
    order::delete_edges(w.c(), parent_id).await?;
    let children = order::children(w.c(), parent_id).await?;
    let steps = steps_of(&children);
    let keys: HashMap<&str, &str> = children
        .iter()
        .map(|ch| (ch.id.as_str(), ch.key.as_str()))
        .collect();
    let actor = w.ctx.actor_id();
    for pair in steps.windows(2) {
        for from in &pair[0] {
            for to in &pair[1] {
                if let Some(path) = deps::blocks_path(w.c(), to, from).await? {
                    let from_key = keys.get(from.as_str()).copied().unwrap_or_default();
                    let to_key = keys.get(to.as_str()).copied().unwrap_or_default();
                    let mut cycle = vec![from_key.to_string()];
                    cycle.extend(path);
                    return Err(ApiError::validation(format!(
                        "the order puts {from_key} before {to_key}, but that makes a cycle: {}. Remove the manual relation or change the order.",
                        cycle.join(" blocks ")
                    ))
                    .with_details(json!({ "reason": "order_cycle", "cycle": cycle })));
                }
                order::insert_edge(w.c(), from, to, &actor).await?;
            }
        }
    }
    Ok(())
}
