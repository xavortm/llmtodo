//! Teams and their statuses.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;
use serde_json::json;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::teams;

pub async fn list_teams(State(st): State<AppState>) -> ApiResult<Json<Vec<Team>>> {
    let mut c = st.pool().acquire().await?;
    let mut out = Vec::new();
    for t in teams::team_rows(&mut c).await? {
        out.push(teams::to_api(&mut c, t).await?);
    }
    Ok(Json(out))
}

pub async fn view_team(State(st): State<AppState>, Path(r): Path<String>) -> ApiResult<Json<Team>> {
    let mut c = st.pool().acquire().await?;
    let t = teams::team_row(&mut c, &r).await?;
    Ok(Json(teams::to_api(&mut c, t).await?))
}

pub async fn create_team(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateTeam>,
) -> ApiResult<Json<Team>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::teams::create(&mut w, req).await?;
    w.commit().await?;
    view_team(State(st), Path(id)).await
}

pub async fn update_team(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<UpdateTeam>,
) -> ApiResult<Json<Team>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::teams::update(&mut w, &r, req).await?;
    w.commit().await?;
    view_team(State(st), Path(id)).await
}

pub async fn create_status(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<CreateStatus>,
) -> ApiResult<Json<Status>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::teams::create_status(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(teams::status_by_id(&mut c, &id).await?.to_api()))
}

pub async fn update_status(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Json(req): Json<UpdateStatus>,
) -> ApiResult<Json<Status>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::teams::update_status(&mut w, &id, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(teams::status_by_id(&mut c, &id).await?.to_api()))
}

pub async fn delete_status(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::teams::delete_status(&mut w, &id).await?;
    w.commit().await?;
    Ok(Json(json!({ "deleted": id })))
}
