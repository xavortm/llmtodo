//! Prune: the only permanent delete. Only the owner may run it.

use llmtodo_api_types::*;
use serde_json::json;
use sqlx::SqliteConnection;

use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::{Ctx, Write};
use crate::store;

/// Refuses a prune by anyone but the owner, or one with nothing to select.
pub fn check(ctx: &Ctx, req: &PruneRequest) -> ApiResult<()> {
    if !ctx.actor.is_owner {
        return Err(ApiError::forbidden(
            "only the owner can prune; agents may trash and restore, but never delete for good",
        ));
    }
    if req.before.is_none() && req.revisions_before.is_none() {
        return Err(ApiError::bad_request(
            "prune needs --before or --revisions-before",
        ));
    }
    Ok(())
}

/// What a prune would delete, without deleting it.
pub async fn dry_run(c: &mut SqliteConnection, req: &PruneRequest) -> ApiResult<PruneReport> {
    let (issue_ids, revisions) = targets(c, req).await?;
    Ok(PruneReport {
        dry_run: true,
        issues: issue_keys(&issue_ids),
        revisions,
        backup: None,
    })
}

/// Deletes the targets inside the write, after the caller made a backup.
pub async fn apply(w: &mut Write<'_>, req: &PruneRequest) -> ApiResult<(Vec<IssueKey>, i64)> {
    // Select inside the write transaction, after the backup, so an issue
    // restored meanwhile is never deleted.
    let (issue_ids, revisions) = targets(w.c(), req).await?;
    let keys = issue_keys(&issue_ids);
    if let Some(confirmed) = &req.confirm_issues {
        let mut want: Vec<String> = confirmed
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        want.sort();
        let mut have: Vec<String> = keys.iter().map(std::string::ToString::to_string).collect();
        have.sort();
        if want != have {
            return Err(ApiError::new(
                ErrorCode::StaleVersion,
                "the trash changed since you confirmed the prune; nothing was deleted, run prune again",
            )
            .with_details(json!({ "confirmed": want, "now": have })));
        }
    }
    if let Some(confirmed) = req.confirm_revisions
        && confirmed != revisions
    {
        return Err(ApiError::new(
            ErrorCode::StaleVersion,
            "the revision count changed since you confirmed the prune; nothing was deleted, run prune again",
        )
        .with_details(json!({ "confirmed": confirmed, "now": revisions })));
    }
    let before = req.before.map(db::ts);
    for (id, key) in &issue_ids {
        store::revisions::delete_of_issue(w.c(), id).await?;
        store::revisions::delete_of_comments(w.c(), id).await?;
        // Children keep living; they lose their parent.
        store::issues::clear_parent(w.c(), id).await?;
        // Check the trash again, so only a trashed issue can go.
        store::issues::delete_if_trashed_before(w.c(), id, before.as_deref()).await?;
        // Keep the key reserved, so it is never reused.
        w.event(
            EntityType::Issue,
            id,
            None,
            ActivityField::Pruned,
            Some(json!(key)),
            None,
        )
        .await?;
    }
    if let Some(t) = req.revisions_before {
        store::revisions::delete_old(w.c(), db::ts(t)).await?;
    }

    Ok((keys, revisions))
}

/// The trashed issues and the revision count a prune would delete.
async fn targets(
    c: &mut sqlx::SqliteConnection,
    req: &PruneRequest,
) -> ApiResult<(Vec<(String, String)>, i64)> {
    let issue_ids: Vec<(String, String)> = match req.before {
        Some(t) => store::issues::trashed_before(&mut *c, db::ts(t)).await?,
        None => Vec::new(),
    };
    let revisions: i64 = match req.revisions_before {
        Some(t) => store::revisions::count_old(&mut *c, db::ts(t)).await?,
        None => 0,
    };
    Ok((issue_ids, revisions))
}

fn issue_keys(issue_ids: &[(String, String)]) -> Vec<IssueKey> {
    issue_ids
        .iter()
        .filter_map(|(_, k)| k.parse().ok())
        .collect()
}
