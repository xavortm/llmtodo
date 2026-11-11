//! The Analyzing mark on an issue.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::issues;

pub async fn analyze(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<AnalyzeRequest>,
) -> ApiResult<Json<AnalyzeResponse>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let (id, changed, warnings) = ops::analyzing::analyze(&mut w, &r, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    let issue = issues::load_one(&mut c, &id, false).await?;
    Ok(Json(AnalyzeResponse {
        issue,
        changed,
        warnings,
    }))
}
