//! The activity feed.

use axum::Json;
use axum::extract::{Query, State};
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::AppState;

pub async fn list_activity(
    State(st): State<AppState>,
    Query(q): Query<ActivityQuery>,
) -> ApiResult<Json<ActivityPage>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::activity::page(&mut c, &q).await?))
}
