//! The outbox of changes made offline on a shared database (LLM-471).

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::Outbox;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Outbox>> {
    Ok(Json(crate::offline::outbox(&st).await?))
}

pub async fn drop_row(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Outbox>> {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request(format!("outbox row IDs are numbers, got {id:?}")))?;
    Ok(Json(crate::offline::drop_row(&st, id).await?))
}
