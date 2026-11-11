//! GitHub sync status and a manual poll.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::github;
use crate::state::AppState;

pub async fn sync_now(State(st): State<AppState>) -> ApiResult<Json<GithubReport>> {
    Ok(Json(github::poll_with_gh(&st).await?))
}

pub async fn status() -> Json<GithubReport> {
    Json(github::LAST.lock().await.clone().unwrap_or_default())
}
