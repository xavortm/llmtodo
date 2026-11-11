//! Labels and label groups.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;
use serde_json::json;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::{self, labels};

pub async fn list_labels(State(st): State<AppState>) -> ApiResult<Json<Vec<Label>>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(
        labels::all(&mut c)
            .await?
            .iter()
            .map(store::labels::LabelRow::to_api)
            .collect(),
    ))
}

pub async fn create_label(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateLabel>,
) -> ApiResult<Json<Label>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::labels::create(&mut w, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(labels::resolve_any(&mut c, &id).await?.to_api()))
}

pub async fn update_label(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<UpdateLabel>,
) -> ApiResult<Json<Label>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::labels::update(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(labels::resolve_any(&mut c, &id).await?.to_api()))
}

pub async fn delete_label(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::labels::delete(&mut w, &r).await?;
    w.commit().await?;
    Ok(Json(json!({ "deleted": id })))
}
