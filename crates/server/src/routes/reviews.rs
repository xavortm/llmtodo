//! Review rounds.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use super::issue_detail;
use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store;

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

/// Answers the reviews of several issues in one write (LLM-518).
pub async fn submit_many(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SubmitReviews>,
) -> ApiResult<Json<Vec<ReviewOutcome>>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let done = ops::reviews::submit_many(&mut w, req.answers).await?;
    let catalog = w.catalog_for_response().await;
    w.commit().await?;
    let ids: Vec<String> = done
        .iter()
        .filter_map(|(_, r)| r.as_ref().ok().cloned())
        .collect();
    let mut c = st.pool().acquire().await?;
    let copies = match &catalog {
        Some(cat) => store::issues::load_with(&mut c, &ids, false, &cat.lookup).await?,
        None => store::issues::load(&mut c, &ids, false).await?,
    };
    let mut copies = copies.into_iter();
    let out = done
        .into_iter()
        .map(|(issue, r)| match r {
            Ok(_) => ReviewOutcome {
                issue,
                result: copies.next(),
                error: None,
            },
            Err(e) => ReviewOutcome {
                issue,
                result: None,
                error: Some(ErrorDetail {
                    code: e.code,
                    message: e.message,
                    details: e.details,
                }),
            },
        })
        .collect();
    Ok(Json(out))
}
