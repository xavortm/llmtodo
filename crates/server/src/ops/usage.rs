//! Token usage reports.

use llmtodo_api_types::*;
use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::store::{self, usage};

/// Usage by issue, the most expensive first.
pub async fn report(c: &mut SqliteConnection, q: &UsageQuery) -> ApiResult<UsageReport> {
    let project = match &q.project {
        Some(p) => Some(store::projects::resolve(c, p).await?.id),
        None => None,
    };
    let issue = match &q.issue {
        Some(i) => Some(store::issues::resolve_id(c, i).await?),
        None => None,
    };
    let rows = store::usage::work_session_rows(
        &mut *c,
        project.as_deref(),
        issue.as_deref(),
        q.session.as_deref(),
    )
    .await?;
    let ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let mut by_ws = usage::by_work_session(c, &ids).await?;
    let prices = usage::prices(c).await?;
    let mut per_issue: std::collections::BTreeMap<
        String,
        (String, std::collections::BTreeMap<String, TokenUsage>, u64),
    > = Default::default();
    let mut total = std::collections::BTreeMap::new();
    let mut total_seconds = 0;
    for (ws, key, title, seconds) in rows {
        let u = by_ws.remove(&ws).unwrap_or_default();
        usage::merge(&mut total, &u);
        total_seconds += seconds as u64;
        let entry = per_issue
            .entry(key)
            .or_insert_with(|| (title, Default::default(), 0));
        usage::merge(&mut entry.1, &u);
        entry.2 += seconds as u64;
    }
    let mut issues: Vec<IssueUsage> = per_issue
        .into_iter()
        .filter_map(|(key, (title, u, seconds))| {
            Some(IssueUsage {
                key: key.parse().ok()?,
                title,
                usage: usage::summary(u, &prices),
                active_seconds: (seconds > 0).then_some(seconds),
            })
        })
        .filter(|i| i.usage.total_tokens > 0 || i.active_seconds.is_some())
        .collect();
    issues.sort_by(|a, b| {
        b.usage
            .cost_usd
            .unwrap_or(0.0)
            .total_cmp(&a.usage.cost_usd.unwrap_or(0.0))
            .then(b.usage.total_tokens.cmp(&a.usage.total_tokens))
    });
    Ok(UsageReport {
        total: usage::summary(total, &prices),
        total_active_seconds: (total_seconds > 0).then_some(total_seconds),
        issues,
    })
}
