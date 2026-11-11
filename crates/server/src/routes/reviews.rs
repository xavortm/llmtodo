//! Review rounds.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use super::issue_detail;
use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};

/// The review section and the answers of an issue.
pub async fn state(
    State(st): State<AppState>,
    Path(r): Path<String>,
) -> ApiResult<Json<ReviewState>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::reviews::state(&mut c, &r).await?))
}

/// Approves an issue in User review, or asks for changes.
pub async fn submit(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(r): Path<String>,
    Json(req): Json<SubmitReview>,
) -> ApiResult<Json<IssueDetail>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::reviews::submit(&mut w, &r, req).await?;
    w.commit().await?;
    issue_detail(&st, &id).await
}
