//! Workspace settings.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};

pub async fn get_settings(State(st): State<AppState>) -> ApiResult<Json<Settings>> {
    Ok(Json(ops::settings::read(&st).await?))
}

pub async fn update_settings(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<UpdateSettings>,
) -> ApiResult<Json<Settings>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::settings::update(&mut w, req).await?;
    w.commit().await?;
    Ok(Json(ops::settings::read(&st).await?))
}
