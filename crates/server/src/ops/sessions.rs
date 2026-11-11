//! Work sessions and their notes.

use llmtodo_api_types::*;
use serde_json::json;
use sqlx::SqliteConnection;

use crate::db::new_id;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, actors, issues, sessions};

/// One session and everything it touched.
pub async fn view(c: &mut SqliteConnection, session: String) -> ApiResult<SessionView> {
    let all = actors::all(c).await?;
    let (issues, notes) = sessions::for_session(c, &session, &all).await?;
    if issues.is_empty() && notes.is_empty() {
        return Err(ApiError::not_found("session", &session));
    }
    let actor = issues.first().map(|w| w.actor.clone());
    Ok(SessionView {
        session_id: session,
        actor,
        issues,
        notes,
    })
}

pub async fn add_note(
    w: &mut Write<'_>,
    session: &str,
    req: CreateSessionNote,
) -> ApiResult<String> {
    let ctx = w.ctx;
    if req.body.trim().is_empty() {
        return Err(ApiError::validation("note body is empty"));
    }

    let issue_id = match &req.issue {
        Some(r) => Some(issues::resolve_live(w.c(), r).await?.id),
        None => None,
    };
    let id = new_id();
    store::sessions::add_note(
        w.c(),
        &id,
        session,
        issue_id.as_deref(),
        &req.body,
        ctx.actor_id(),
    )
    .await?;
    w.event(
        EntityType::SessionNote,
        &id,
        issue_id.as_deref(),
        ActivityField::SessionNote,
        None,
        Some(json!({ "session": session })),
    )
    .await?;

    Ok(id)
}
