//! Issue create, read, query, update, trash and restore.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use llmtodo_api_types::*;

use super::issue_detail;
use crate::error::{ApiError, ApiResult};
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::issues;

pub async fn view(
    State(st): State<AppState>,
    Path(r): Path<String>,
) -> ApiResult<Json<IssueDetail>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::issues::view(&mut c, &r).await?))
}

pub async fn deps(
    State(st): State<AppState>,
    Path(r): Path<String>,
    Query(q): Query<DepsQuery>,
) -> ApiResult<Json<DepNode>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::issues::deps(&mut c, &r, q.open).await?))
}

pub async fn query(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(q): Json<IssueQuery>,
) -> ApiResult<Json<Page<Issue>>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::issues::query(&mut c, &ctx.actor, q).await?))
}

pub async fn frontier(
    State(st): State<AppState>,
    Json(q): Json<FrontierQuery>,
) -> ApiResult<Json<Vec<Issue>>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::issues::frontier(&mut c, &q).await?))
}

pub async fn history(
    State(st): State<AppState>,
    Path(r): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<History>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::issues::history(&mut c, &r, q.bodies).await?))
}

pub async fn create(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateIssue>,
) -> ApiResult<Json<IssueDetail>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::issues::create(&mut w, req, None).await?;
    w.commit().await?;
    issue_detail(&st, &id).await
}

pub async fn update(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    req: Result<Json<UpdateIssue>, JsonRejection>,
) -> ApiResult<Json<IssueDetail>> {
    // A field with a value it cannot take, such as `waiting_on: robot`, is a
    // validation error, as it was before the fields had types.
    let Json(req) = req.map_err(|e| match e {
        JsonRejection::JsonDataError(e) => ApiError::validation(e.body_text()),
        e => ApiError::bad_request(e.body_text()),
    })?;
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::issues::update(&mut w, &r, req).await?;
    w.commit().await?;
    issue_detail(&st, &id).await
}

pub async fn delete(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
) -> ApiResult<Json<Issue>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::issues::delete(&mut w, &r).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(issues::load_one(&mut c, &id, false).await?))
}

pub async fn restore(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<RestoreRequest>,
) -> ApiResult<Json<IssueDetail>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::issues::restore(&mut w, &r, req).await?;
    w.commit().await?;
    issue_detail(&st, &id).await
}
