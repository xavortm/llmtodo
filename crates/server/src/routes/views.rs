//! Saved views.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;
use serde_json::json;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::views;

pub async fn list_views(State(st): State<AppState>) -> ApiResult<Json<Vec<SavedView>>> {
    let mut c = st.pool().acquire().await?;
    let rows = views::list(&mut c).await?;
    Ok(Json(
        rows.into_iter()
            .map(views::ViewRow::to_api)
            .collect::<ApiResult<_>>()?,
    ))
}

pub async fn create_view(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateView>,
) -> ApiResult<Json<SavedView>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::views::create(&mut w, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(views::row(&mut c, &id).await?.to_api()?))
}
pub async fn update_view(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<UpdateView>,
) -> ApiResult<Json<SavedView>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::views::update(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(views::row(&mut c, &id).await?.to_api()?))
}
pub async fn delete_view(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::views::delete(&mut w, &r).await?;
    w.commit().await?;
    Ok(Json(json!({ "deleted": id })))
}
