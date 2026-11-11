//! Actors.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;

use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx};

pub async fn list_actors(State(st): State<AppState>) -> ApiResult<Json<ActorList>> {
    let mut c = st.pool().acquire().await?;
    Ok(Json(ops::actors::list(&mut c).await?))
}

pub async fn me(ctx: Ctx) -> Json<Actor> {
    Json(ctx.actor)
}
