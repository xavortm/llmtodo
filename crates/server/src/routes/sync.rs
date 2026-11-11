//! Laptop sync through a shared folder.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::AppState;

pub async fn status(State(st): State<AppState>) -> ApiResult<Json<SyncStatus>> {
    Ok(Json(ops::sync::status(&st).await?))
}

pub async fn push(
    State(st): State<AppState>,
    Json(req): Json<SyncPushRequest>,
) -> ApiResult<Json<SyncReport>> {
    Ok(Json(ops::sync::push(&st, req).await?))
}

pub async fn pull(
    State(st): State<AppState>,
    Json(req): Json<SyncPullRequest>,
) -> ApiResult<Json<SyncReport>> {
    Ok(Json(ops::sync::pull(&st, req).await?))
}
