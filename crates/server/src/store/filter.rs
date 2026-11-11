//! The SQL form of the shared issue filter. `IssueFilter` in `core` states
//! the rules, and `IssueFilter::matches` is the same filter for the app.

use llmtodo_api_types::{Actor, IssueFilter};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use crate::error::ApiResult;
use crate::store::{actors, labels};

use super::issues::analyzing_cutoff;

/// Pushes every condition of `f`. The query must name the issue `i` and its
/// status `s`.
pub async fn push(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    me: &Actor,
    f: &IssueFilter,
) -> ApiResult<()> {
    push_status(b, f)?;
    push_labels(b, conn, f).await?;
    push_assignee(b, conn, me, f).await?;
    push_llm_status(b, f);
    Ok(())
}

fn push_status(b: &mut QueryBuilder<Sqlite>, f: &IssueFilter) -> ApiResult<()> {
    if !f.statuses.is_empty() {
        let names: Vec<String> = f.statuses.iter().map(|s| s.trim().to_lowercase()).collect();
        b.push(" AND (lower(s.name) IN (SELECT value FROM json_each(")
            .push_bind(serde_json::to_string(&names)?)
            .push(")) OR s.id IN (SELECT value FROM json_each(")
            .push_bind(serde_json::to_string(&f.statuses)?)
            .push(")))");
    }
    if !f.status_types.is_empty() {
        let types: Vec<&str> = f.status_types.iter().map(|t| t.as_str()).collect();
        b.push(" AND s.type IN (SELECT value FROM json_each(")
            .push_bind(serde_json::to_string(&types)?)
            .push("))");
    }
    if f.open {
        b.push(" AND s.type NOT IN ('completed', 'canceled')");
    }
    Ok(())
}

async fn push_labels(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    f: &IssueFilter,
) -> ApiResult<()> {
    for l in &f.labels {
        let label = labels::resolve(conn, l).await?;
        b.push(" AND EXISTS (SELECT 1 FROM issue_labels il WHERE il.issue_id = i.id AND il.label_id = ").push_bind(label.id).push(")");
    }
    Ok(())
}

async fn push_assignee(
    b: &mut QueryBuilder<Sqlite>,
    conn: &mut SqliteConnection,
    me: &Actor,
    f: &IssueFilter,
) -> ApiResult<()> {
    let Some(a) = &f.assignee else {
        return Ok(());
    };
    if a.trim()
        .eq_ignore_ascii_case(llmtodo_api_types::ASSIGNEE_NONE)
    {
        b.push(" AND i.assignee_id IS NULL");
        return Ok(());
    }
    let ids = match actors::resolve_ref(conn, me, a).await {
        Ok(id) => vec![id],
        Err(not_found) => {
            let ids = actors::ids_with_name_containing(conn, a.trim()).await?;
            if ids.is_empty() {
                return Err(not_found);
            }
            ids
        }
    };
    b.push(" AND i.assignee_id IN (SELECT value FROM json_each(")
        .push_bind(serde_json::to_string(&ids)?)
        .push("))");
    Ok(())
}

/// Keeps the issues an agent is on. The two flags together keep an issue
/// with either state, because a claim ends an Analyzing mark.
fn push_llm_status(b: &mut QueryBuilder<Sqlite>, f: &IssueFilter) {
    if !f.claimed && !f.analyzing {
        return;
    }
    b.push(" AND (");
    if f.claimed {
        b.push(
            "EXISTS (SELECT 1 FROM claims c JOIN actors a ON a.id = c.actor_id \
             WHERE c.issue_id = i.id AND c.ended_at IS NULL AND a.is_owner = 0)",
        );
    }
    if f.claimed && f.analyzing {
        b.push(" OR ");
    }
    if f.analyzing {
        b.push("EXISTS (SELECT 1 FROM analyzing m WHERE m.issue_id = i.id AND m.seen_at >= ")
            .push_bind(analyzing_cutoff())
            .push(")");
    }
    b.push(")");
}
