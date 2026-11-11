//! The activity feed.

use llmtodo_api_types::*;
use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::store::{activity, issues};

/// One page of events. `reload` tells a client that its seq was pruned.
pub async fn page(c: &mut SqliteConnection, q: &ActivityQuery) -> ApiResult<ActivityPage> {
    let issue_id = match &q.issue {
        Some(r) => Some(issues::resolve_id(c, r).await?),
        None => None,
    };
    let after = q.after_seq.unwrap_or(0);
    let first = activity::first_seq(c).await?;
    let reload = q.after_seq.is_some() && first > 0 && after < first - 1;
    let limit = q.limit.unwrap_or(500).clamp(1, 5000) as i64;
    let events = activity::after(c, after, issue_id.as_deref(), limit).await?;
    let last_seq = activity::last_seq(c).await?;
    Ok(ActivityPage {
        events,
        last_seq,
        reload,
    })
}
