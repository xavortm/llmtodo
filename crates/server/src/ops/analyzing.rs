//! The Analyzing mark: an agent reads a ticket without holding it.

use llmtodo_api_types::*;
use serde_json::json;

use super::claims::live_claim;
use super::j;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, issues};

/// Ends the Analyzing mark on an issue, whoever set it. Expired marks go
/// too, without an event.
pub(crate) async fn end_analyzing(
    w: &mut Write<'_>,
    issue_id: &str,
    reason: &str,
) -> ApiResult<()> {
    let session: Option<(String, String)> = store::analyzing::remove(w.c(), issue_id).await?;
    if let Some((session, seen)) = session
        && seen >= issues::analyzing_cutoff()
    {
        w.event(
            EntityType::Issue,
            issue_id,
            Some(issue_id),
            ActivityField::Analyzing,
            j(json!({ "session": session })),
            j(json!({ "ended": reason })),
        )
        .await?;
    }
    Ok(())
}

pub async fn analyze(
    w: &mut Write<'_>,
    r: &str,
    req: AnalyzeRequest,
) -> ApiResult<(String, bool, Vec<String>)> {
    let ctx = w.ctx;
    let Some(session) = ctx.session().map(str::to_string) else {
        return Err(ApiError::bad_request(
            "analyzing needs a session ID; set LLMTODO_SESSION",
        ));
    };

    let row = issues::resolve_live(w.c(), r).await?;
    let mut warnings = Vec::new();
    if ctx.client.session_generated {
        warnings.push("the session ID was generated for this one call, so a later --done cannot match it; set LLMTODO_SESSION".to_string());
    }
    let current: Option<(String, String)> = store::analyzing::current(w.c(), &row.id).await?;
    let live_session = current
        .filter(|(_, seen)| *seen >= issues::analyzing_cutoff())
        .map(|(s, _)| s);
    let changed = if req.done {
        if live_session.as_deref() == Some(session.as_str()) {
            end_analyzing(w, &row.id, "done").await?;
            true
        } else {
            false
        }
    } else {
        if let Some((_, _, holder_session)) = live_claim(w, &row.id).await?
            && holder_session != session
        {
            warnings.push(format!(
                "{} is claimed by session {holder_session}; analyzing does not block it",
                row.key
            ));
        }
        let t = now();
        let fresh = live_session.as_deref() != Some(session.as_str());
        store::analyzing::upsert(
            w.c(),
            &row.id,
            ctx.actor_id(),
            &session,
            &t,
            issues::analyzing_cutoff(),
        )
        .await?;
        if fresh {
            w.event(
                EntityType::Issue,
                &row.id,
                Some(&row.id),
                ActivityField::Analyzing,
                live_session.map(|s| json!({ "session": s })),
                j(json!({ "actor": ctx.actor.name, "session": session })),
            )
            .await?;
        } else {
            w.touch(&row.id);
        }
        fresh
    };

    Ok((row.id, changed, warnings))
}
