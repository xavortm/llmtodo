//! Token usage reports.

use axum::Json;
use axum::extract::{Query, State};
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::AppState;

pub async fn usage(
    State(st): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> ApiResult<Json<UsageReport>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::usage::report(&mut c, &q).await?))
}
