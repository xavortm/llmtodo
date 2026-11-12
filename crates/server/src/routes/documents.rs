//! Documents.

use axum::Json;
use axum::extract::{Path, Query, State};
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::documents;

pub async fn view_document(
    State(st): State<AppState>,
    Path(r): Path<String>,
) -> ApiResult<Json<Document>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(documents::resolve(&mut c, &r).await?.to_api(true)))
}

pub async fn create_document(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateDocument>,
) -> ApiResult<Json<Document>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::documents::create(&mut w, req).await?;
    w.commit().await?;
    view_document(State(st), Path(id)).await
}

pub async fn update_document(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<UpdateDocument>,
) -> ApiResult<Json<Document>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::documents::update(&mut w, &r, req).await?;
    w.commit().await?;
    view_document(State(st), Path(id)).await
}

pub async fn delete_document(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
) -> ApiResult<Json<Document>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::documents::delete(&mut w, &r).await?;
    w.commit().await?;
    view_document(State(st), Path(id)).await
}

pub async fn restore_document(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<RestoreRequest>,
) -> ApiResult<Json<Document>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::documents::restore(&mut w, &r, req).await?;
    w.commit().await?;
    view_document(State(st), Path(id)).await
}

pub async fn list_documents(
    State(st): State<AppState>,
    Query(p): Query<DocumentQuery>,
) -> ApiResult<Json<Vec<Document>>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(
        ops::documents::list(&mut c, p.project.as_deref(), p.trashed, p.search.as_deref()).await?,
    ))
}

pub async fn document_history(
    State(st): State<AppState>,
    Path(r): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<History>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::documents::history(&mut c, &r, q.bodies).await?))
}
