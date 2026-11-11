//! Saved views.

use llmtodo_api_types::*;
use serde_json::json;

use crate::db::{new_id, now};
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, views};

fn clean_view_query(mut q: IssueQuery) -> IssueQuery {
    q.cursor = None;
    q.view = None;
    q
}

pub async fn create(w: &mut Write<'_>, req: CreateView) -> ApiResult<String> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::validation("view name is empty"));
    }

    let id = new_id();
    let t = now();
    store::views::insert(
        w.c(),
        &id,
        name,
        serde_json::to_string(&clean_view_query(req.query))?,
        &t,
    )
    .await?;
    w.event(
        EntityType::View,
        &id,
        None,
        ActivityField::Created,
        None,
        Some(json!(name)),
    )
    .await?;

    Ok(id)
}

pub async fn update(w: &mut Write<'_>, r: &str, req: UpdateView) -> ApiResult<String> {
    let v = views::row(w.c(), r).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::validation("view name is empty"));
        }
        store::views::set_name(w.c(), name, &v.id).await?;
    }
    if let Some(q) = req.query {
        store::views::set_query(w.c(), serde_json::to_string(&clean_view_query(q))?, &v.id).await?;
    }
    store::views::touch(w.c(), &v.id).await?;
    w.event(
        EntityType::View,
        &v.id,
        None,
        ActivityField::Updated,
        None,
        None,
    )
    .await?;

    Ok(v.id)
}

pub async fn delete(w: &mut Write<'_>, r: &str) -> ApiResult<String> {
    let v = views::row(w.c(), r).await?;
    store::views::delete(w.c(), &v.id).await?;
    w.event(
        EntityType::View,
        &v.id,
        None,
        ActivityField::Deleted,
        Some(json!(v.name)),
        None,
    )
    .await?;

    Ok(v.id)
}
