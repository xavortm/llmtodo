//! Comments on issues.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;
use serde_json::json;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::comments;

pub async fn add(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<CreateComment>,
) -> ApiResult<Json<Comment>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::comments::add(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(comments::comment_by_id(&mut c, &id).await?.1))
}

pub async fn update(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Json(req): Json<UpdateComment>,
) -> ApiResult<Json<Comment>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::comments::update(&mut w, &id, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(comments::comment_by_id(&mut c, &id).await?.1))
}

pub async fn delete(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::comments::delete(&mut w, &id).await?;
    w.commit().await?;
    Ok(Json(json!({ "deleted": id })))
}
