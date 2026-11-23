//! Claim and release.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::issues;

pub async fn claim(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<ClaimRequest>,
) -> ApiResult<Json<ClaimResponse>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let claimed = ops::claims::claim(&mut w, &r, req).await?;
    let catalog = w.catalog_for_response().await;
    w.commit().await?;
    let lookup = catalog.as_ref().map(|c| &c.lookup);
    Ok(Json(
        ops::claims::response(&st.pool(), claimed, lookup).await?,
    ))
}

pub async fn release(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<ReleaseRequest>,
) -> ApiResult<Json<Issue>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::claims::release(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(issues::load_one(&mut c, &id, false).await?))
}
