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
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    let issue = issues::load_one(&mut c, &claimed.id, false).await?;
    Ok(Json(ClaimResponse {
        issue,
        changed: claimed.changed,
        open_blockers: claimed.open_blockers,
        warnings: claimed.warnings,
        stolen_from: claimed.stolen_from,
    }))
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
