//! Links and files attached to issues.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;
use serde_json::json;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::attachments;

pub async fn attach(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<CreateAttachment>,
) -> ApiResult<Json<Attachment>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::attachments::add(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(attachments::attachment_by_id(&mut c, &id).await?.1))
}

pub async fn detach(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::attachments::detach(&mut w, &id).await?;
    w.commit().await?;
    Ok(Json(json!({ "deleted": id })))
}
