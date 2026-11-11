//! Prune.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;

use crate::db;
use crate::error::ApiResult;
use crate::ops;
use crate::state::{AppState, Ctx, Write};

/// The only permanent delete.
pub async fn prune(
    State(st): State<AppState>,
    ctx: Ctx,
    Json(req): Json<PruneRequest>,
) -> ApiResult<Json<PruneReport>> {
    ops::prune::check(&ctx, &req)?;
    if req.dry_run {
        let mut c = st.pool().acquire().await?;
        return Ok(Json(ops::prune::dry_run(&mut c, &req).await?));
    }
    let backup = db::backup(&st.pool(), &st.data_dir, "pre-prune").await?;
    let mut w = Write::begin(&st, &ctx).await?;
    let (issues, revisions) = ops::prune::apply(&mut w, &req).await?;
    w.commit().await?;
    Ok(Json(PruneReport {
        dry_run: false,
        issues,
        revisions,
        backup: Some(backup.display().to_string()),
    }))
}
