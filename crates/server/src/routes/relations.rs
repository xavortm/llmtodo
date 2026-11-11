//! Relations between issues.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use super::issue_detail;
use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};

pub async fn add_relation(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<CreateRelation>,
) -> ApiResult<Json<IssueDetail>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::relations::add_to_issue(&mut w, &r, req).await?;
    w.commit().await?;
    issue_detail(&st, &id).await
}

pub async fn remove_relation(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<RemoveRelation>,
) -> ApiResult<Json<IssueDetail>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::relations::remove(&mut w, &r, req).await?;
    w.commit().await?;
    issue_detail(&st, &id).await
}
