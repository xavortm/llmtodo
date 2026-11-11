//! Work sessions and their notes.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};
use crate::store::sessions;

pub async fn show_session(
    State(st): State<AppState>,
    Path(session): Path<String>,
) -> ApiResult<Json<SessionView>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::sessions::view(&mut c, session).await?))
}

pub async fn add_session_note(
    State(st): State<AppState>,
    ctx: Ctx,
    Path(session): Path<String>,
    Json(req): Json<CreateSessionNote>,
) -> ApiResult<Json<SessionNote>> {
    let mut w = Write::begin(&st, &ctx).await?;
    let id = ops::sessions::add_note(&mut w, &session, req).await?;
    w.commit().await?;
    let mut c = st.pool().acquire().await?;
    Ok(Json(sessions::note_by_id(&mut c, &id).await?))
}
