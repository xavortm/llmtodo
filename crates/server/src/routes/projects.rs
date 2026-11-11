//! Projects, their linked repositories and their milestones.

use axum::Json;
use axum::extract::{Path, Query, State};
use llmtodo_api_types::*;
use serde_json::json;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::{self, projects};

pub async fn list_projects(
    State(st): State<AppState>,
    Query(p): Query<ProjectQuery>,
) -> ApiResult<Json<Vec<Project>>> {
    let mut c = st.pool().acquire().await?;
    let mut out = Vec::new();
    for row in projects::rows(&mut c, p.trashed).await? {
        out.push(projects::to_api(&mut c, row).await?);
    }
    Ok(Json(out))
}

pub async fn view_project(
    State(st): State<AppState>,
    Path(r): Path<String>,
) -> ApiResult<Json<Project>> {
    let mut c = st.pool().acquire().await?;
    let row = projects::resolve(&mut c, &r).await?;
    Ok(Json(projects::to_api(&mut c, row).await?))
}

pub async fn create_project(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateProject>,
) -> ApiResult<Json<Project>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::projects::create(&mut w, req).await?;
    w.commit().await?;
    view_project(State(st), Path(id)).await
}

pub async fn update_project(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<UpdateProject>,
) -> ApiResult<Json<Project>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::projects::update(&mut w, &r, req).await?;
    w.commit().await?;
    view_project(State(st), Path(id)).await
}

pub async fn delete_project(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
) -> ApiResult<Json<Project>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::projects::delete(&mut w, &r).await?;
    w.commit().await?;
    view_project(State(st), Path(id)).await
}

pub async fn link_repo(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<RepoLink>,
) -> ApiResult<Json<Project>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::projects::link_repo(&mut w, &r, req).await?;
    w.commit().await?;
    view_project(State(st), Path(id)).await
}

pub async fn unlink_repo(
    State(st): State<AppState>,
    ctx: Ctx,
    Path((r, owner, name)): Path<(String, String, String)>,
) -> ApiResult<Json<Project>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::projects::unlink_repo(&mut w, &r, &owner, &name).await?;
    w.commit().await?;
    view_project(State(st), Path(id)).await
}

pub async fn list_milestones(
    State(st): State<AppState>,
    Path(r): Path<String>,
) -> ApiResult<Json<Vec<Milestone>>> {
    let mut c = st.pool().acquire().await?;
    let p = projects::resolve(&mut c, &r).await?;
    Ok(Json(
        projects::milestones(&mut c, &p.id)
            .await?
            .iter()
            .map(store::projects::MilestoneRow::to_api)
            .collect(),
    ))
}

pub async fn create_milestone(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<CreateMilestone>,
) -> ApiResult<Json<Milestone>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::projects::create_milestone(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(projects::milestone_by_id(&mut c, &id).await?.to_api()))
}

pub async fn update_milestone(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Json(req): Json<UpdateMilestone>,
) -> ApiResult<Json<Milestone>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::projects::update_milestone(&mut w, &id, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(projects::milestone_by_id(&mut c, &id).await?.to_api()))
}

pub async fn delete_milestone(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut w = Write::begin(&st, &ctx).await?;
    ops::projects::delete_milestone(&mut w, &id).await?;
    w.commit().await?;
    Ok(Json(json!({ "deleted": id })))
}
