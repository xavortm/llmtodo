//! Relations between issues.

use llmtodo_api_types::*;
use serde_json::json;

use super::issues::{is_duplicate_status, set_status, touch_updated};
use super::j;
use crate::db::new_id;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, deps, issues, teams};

pub async fn add_to_issue(w: &mut Write<'_>, r: &str, req: CreateRelation) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    add(w, &row.id, req.kind, &req.issue).await?;
    touch_updated(w, &row.id).await?;

    Ok(row.id)
}

pub async fn add(w: &mut Write<'_>, this: &str, kind: RelationInput, other: &str) -> ApiResult<()> {
    let ctx = w.ctx;
    let other_row = issues::resolve_live(w.c(), other).await?;
    if other_row.id == this {
        return Err(ApiError::validation("an issue cannot relate to itself"));
    }
    let (stored, swap) = kind.normalize();
    let (from, to) = if swap {
        (other_row.id.clone(), this.to_string())
    } else {
        (this.to_string(), other_row.id.clone())
    };
    if stored == RelationType::Blocks
        && let Some((rid, source)) = store::relations::blocks_edge(w.c(), &from, &to).await?
    {
        // The order already holds this edge. The manual relation takes it
        // over, so it stays when the order changes.
        if source == RelationSource::Order.as_str() {
            store::relations::set_source(w.c(), &rid, RelationSource::Manual).await?;
            relation_events(w, &rid, stored, &from, &to).await?;
        }
        return Ok(());
    }
    let exists: Option<i64> = match stored {
        RelationType::Related => store::relations::related_exists(w.c(), &from, &to).await?,
        _ => store::relations::exists(w.c(), stored.as_str(), &from, &to).await?,
    };
    if exists.is_some() {
        return Ok(());
    }
    if stored == RelationType::Blocks
        && let Some(path) = deps::blocks_path(w.c(), &to, &from).await?
    {
        // The new edge closes the path: from, to, ..., from.
        let from_key = issues::row(w.c(), &from).await?.key;
        let to_key = issues::row(w.c(), &to).await?.key;
        let mut cycle = vec![from_key.clone()];
        cycle.extend(path);
        return Err(ApiError::validation(format!(
            "{from_key} blocks {to_key} would make a cycle: {}",
            cycle.join(" blocks ")
        ))
        .with_details(json!({ "cycle": cycle })));
    }
    let rid = new_id();
    store::relations::insert(w.c(), &rid, stored.as_str(), &from, &to, ctx.actor_id()).await?;
    relation_events(w, &rid, stored, &from, &to).await?;
    if stored == RelationType::Duplicate {
        let dup = issues::row(w.c(), &from).await?;
        let statuses = teams::statuses(w.c(), Some(&dup.team_id)).await?;
        if let Some(s) = statuses.iter().find(|s| is_duplicate_status(s)) {
            set_status(w, &dup, s).await?;
        }
    }
    Ok(())
}

/// Records a new relation in the logs of both issues.
async fn relation_events(
    w: &mut Write<'_>,
    rid: &str,
    stored: RelationType,
    from: &str,
    to: &str,
) -> ApiResult<()> {
    let to_key = issues::row(w.c(), to).await?.key;
    let from_key = issues::row(w.c(), from).await?.key;
    for side in [from, to] {
        w.event(
            EntityType::Relation,
            rid,
            Some(side),
            ActivityField::RelationAdded,
            None,
            j(json!({ "type": stored, "from": from_key, "to": to_key })),
        )
        .await?;
    }
    Ok(())
}

pub async fn remove(w: &mut Write<'_>, r: &str, req: RemoveRelation) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    let other = issues::resolve_id(w.c(), &req.issue).await?;
    let (stored, swap) = req.kind.normalize();
    let (from, to) = if swap {
        (other.clone(), row.id.clone())
    } else {
        (row.id.clone(), other.clone())
    };
    let found: Option<String> = match stored {
        RelationType::Related => store::relations::related_id(w.c(), &from, &to).await?,
        _ => store::relations::id_of(w.c(), stored.as_str(), &from, &to).await?,
    };
    let rid = found.ok_or_else(|| {
        ApiError::not_found(
            "relation",
            &format!("{} {} {}", row.key, req.kind, req.issue),
        )
    })?;
    if stored == RelationType::Blocks
        && let Some((_, source)) = store::relations::blocks_edge(w.c(), &from, &to).await?
        && source == RelationSource::Order.as_str()
    {
        let from_row = issues::row(w.c(), &from).await?;
        let to_row = issues::row(w.c(), &to).await?;
        let parent = from_row.parent_key.clone().unwrap_or_default();
        return Err(ApiError::validation(format!(
            "{} blocks {} comes from the order of {parent}, so it cannot be removed as a relation. Take a task out of the order with `llmtodo issue update {} --order-step none`, or write a new order with `llmtodo issue order set {parent} ...`.",
            from_row.key, to_row.key, to_row.key
        ))
        .with_details(json!({ "reason": "order_edge", "parent": parent })));
    }
    store::relations::delete(w.c(), &rid).await?;
    // A manual edge can stand in for an order edge. Without it, the order
    // needs its own edge again.
    if stored == RelationType::Blocks {
        let from_row = issues::row(w.c(), &from).await?;
        if let Some(parent) = &from_row.parent_id
            && from_row.order_step.is_some()
        {
            super::order::rewrite_edges(w, parent).await?;
        }
    }
    w.event(
        EntityType::Relation,
        &rid,
        Some(&from),
        ActivityField::RelationRemoved,
        j(json!({ "type": stored })),
        None,
    )
    .await?;
    w.event(
        EntityType::Relation,
        &rid,
        Some(&to),
        ActivityField::RelationRemoved,
        j(json!({ "type": stored })),
        None,
    )
    .await?;
    touch_updated(w, &row.id).await?;

    Ok(row.id)
}
