//! Owner questions.

use axum::Json;
use axum::extract::{Path, Query, State};
use llmtodo_api_types::*;

use super::issue_detail;
use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};

/// Lists owner questions from the derived table.
pub async fn list(
    State(st): State<AppState>,
    Query(q): Query<QuestionQuery>,
) -> ApiResult<Json<Page<QuestionEntry>>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::questions::list(&mut c, &q).await?))
}

/// Reopens an answered question.
pub async fn undo_answer(
    State(st): State<AppState>,
    ctx: Ctx,
    Path((r, id)): Path<(String, String)>,
) -> ApiResult<Json<IssueDetail>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let issue = ops::questions::undo_answer(&mut w, &r, id).await?;
    w.commit().await?;
    issue_detail(&st, &issue).await
}
