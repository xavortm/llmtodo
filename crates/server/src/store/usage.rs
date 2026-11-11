//! Token usage: attribution on write, totals and prices.

use std::collections::{BTreeMap, HashMap};

use llmtodo_api_types::{ModelPrice, TokenUsage, UsageSummary};
use sqlx::SqliteConnection;

use crate::error::ApiResult;
use crate::state::Ctx;

/// Anthropic list prices in USD per million tokens, from 2026-11. Cache
/// writes are 1.25x input for the 5-minute TTL and 2x for 1 hour. Fast mode
/// on Opus 5.5 costs twice the standard rate. `settings set --model-price`
/// overrides any of them.
pub fn default_prices() -> BTreeMap<String, ModelPrice> {
    let p = |input: f64, output: f64, read: f64| ModelPrice {
        input,
        output,
        cache_write_5m: input * 1.25,
        cache_write_1h: input * 2.0,
        cache_read: read,
    };
    BTreeMap::from([
        ("claude-fable-5-2".to_string(), p(8.0, 40.0, 0.20)),
        ("claude-fable-5-2:fast".to_string(), p(16.0, 80.0, 0.40)),
        ("claude-opus-5-5".to_string(), p(4.0, 20.0, 0.20)),
        ("claude-opus-5-5:fast".to_string(), p(8.0, 40.0, 0.40)),
        ("claude-sonnet-5-5".to_string(), p(2.0, 10.0, 0.10)),
        // Haiku 5.5 doubles past 100K prompt tokens; this is the lower tier.
        ("claude-haiku-5-5".to_string(), p(0.10, 0.50, 0.01)),
    ])
}

pub async fn prices(conn: &mut SqliteConnection) -> ApiResult<BTreeMap<String, ModelPrice>> {
    let mut all = default_prices();
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'model_prices'")
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(raw) = raw
        && let Ok(over) = serde_json::from_str::<BTreeMap<String, ModelPrice>>(&raw)
    {
        all.extend(over);
    }
    Ok(all)
}

pub type WorkSessionRow = (String, String, String, i64);

type Row = (String, i64, i64, i64, i64, i64, i64);

fn to_usage(r: &Row) -> TokenUsage {
    TokenUsage {
        requests: r.1 as u64,
        input: r.2 as u64,
        output: r.3 as u64,
        cache_write_5m: r.4 as u64,
        cache_write_1h: r.5 as u64,
        cache_read: r.6 as u64,
    }
}

/// Adds what is new in the caller's cumulative usage to `issue_id`'s work
/// session, and marks it as seen.
pub async fn attribute(
    conn: &mut SqliteConnection,
    ctx: &Ctx,
    session: &str,
    issue_id: &str,
) -> ApiResult<()> {
    let (Some(cumulative), Some(transcript)) = (&ctx.client.usage, &ctx.client.transcript_path)
    else {
        return Ok(());
    };
    let ws_id: String =
        sqlx::query_scalar("SELECT id FROM work_sessions WHERE session_id = ? AND issue_id = ?")
            .bind(session)
            .bind(issue_id)
            .fetch_one(&mut *conn)
            .await?;
    for (model, now) in cumulative {
        let seen: Option<Row> = sqlx::query_as("SELECT model, requests, input, output, cache_write_5m, cache_write_1h, cache_read FROM transcript_usage_seen WHERE transcript = ? AND model = ?")
            .bind(transcript)
            .bind(model)
            .fetch_optional(&mut *conn)
            .await?;
        // Counts that went down mean a new transcript under the same path:
        // start over from it.
        let delta = match &seen {
            None => *now,
            Some(s) => now.since(&to_usage(s)).unwrap_or(*now),
        };
        if !delta.is_zero() {
            sqlx::query(
                "INSERT INTO work_session_usage (work_session_id, model, requests, input, output, cache_write_5m, cache_write_1h, cache_read) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (work_session_id, model) DO UPDATE SET requests = requests + excluded.requests, input = input + excluded.input, output = output + excluded.output,
                 cache_write_5m = cache_write_5m + excluded.cache_write_5m, cache_write_1h = cache_write_1h + excluded.cache_write_1h, cache_read = cache_read + excluded.cache_read",
            )
            .bind(&ws_id)
            .bind(model)
            .bind(delta.requests as i64)
            .bind(delta.input as i64)
            .bind(delta.output as i64)
            .bind(delta.cache_write_5m as i64)
            .bind(delta.cache_write_1h as i64)
            .bind(delta.cache_read as i64)
            .execute(&mut *conn)
            .await?;
        }
        sqlx::query(
            "INSERT INTO transcript_usage_seen (transcript, model, requests, input, output, cache_write_5m, cache_write_1h, cache_read) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (transcript, model) DO UPDATE SET requests = excluded.requests, input = excluded.input, output = excluded.output,
             cache_write_5m = excluded.cache_write_5m, cache_write_1h = excluded.cache_write_1h, cache_read = excluded.cache_read",
        )
        .bind(transcript)
        .bind(model)
        .bind(now.requests as i64)
        .bind(now.input as i64)
        .bind(now.output as i64)
        .bind(now.cache_write_5m as i64)
        .bind(now.cache_write_1h as i64)
        .bind(now.cache_read as i64)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Adds what is new in the caller's cumulative active time to `issue_id`'s
/// work session. A transcript that already reported usage before time
/// tracking existed starts from what it reports now, so past work gets no
/// invented time. Run it before [`attribute`], which records that usage.
pub async fn attribute_time(
    conn: &mut SqliteConnection,
    ctx: &Ctx,
    session: &str,
    issue_id: &str,
) -> ApiResult<()> {
    let (Some(now), Some(transcript)) = (ctx.client.active_seconds, &ctx.client.transcript_path)
    else {
        return Ok(());
    };
    let seen: Option<i64> =
        sqlx::query_scalar("SELECT active_seconds FROM transcript_time_seen WHERE transcript = ?")
            .bind(transcript)
            .fetch_optional(&mut *conn)
            .await?;
    let delta = match seen {
        // A lower total adds nothing: never invent time.
        Some(s) => now.saturating_sub(s as u64),
        None => {
            let older: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM transcript_usage_seen WHERE transcript = ?)",
            )
            .bind(transcript)
            .fetch_one(&mut *conn)
            .await?;
            if older { 0 } else { now }
        }
    };
    if delta > 0 {
        sqlx::query(
            "UPDATE work_sessions SET active_seconds = active_seconds + ? WHERE session_id = ? AND issue_id = ?",
        )
        .bind(delta as i64)
        .bind(session)
        .bind(issue_id)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "INSERT INTO transcript_time_seen (transcript, active_seconds) VALUES (?, ?)
         ON CONFLICT (transcript) DO UPDATE SET active_seconds = excluded.active_seconds",
    )
    .bind(transcript)
    .bind(now as i64)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Usage by model for each work session ID.
pub async fn by_work_session(
    conn: &mut SqliteConnection,
    ids: &[String],
) -> ApiResult<BTreeMap<String, BTreeMap<String, TokenUsage>>> {
    type WsRow = (String, String, i64, i64, i64, i64, i64, i64);
    let rows: Vec<WsRow> = sqlx::query_as(
        "SELECT work_session_id, model, requests, input, output, cache_write_5m, cache_write_1h, cache_read FROM work_session_usage
         WHERE work_session_id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(ids)?)
    .fetch_all(&mut *conn)
    .await?;
    let mut out: BTreeMap<String, BTreeMap<String, TokenUsage>> = BTreeMap::new();
    for (ws, model, a, b, c, d, e, f) in rows {
        out.entry(ws)
            .or_default()
            .insert(model.clone(), to_usage(&(model, a, b, c, d, e, f)));
    }
    Ok(out)
}

/// The cost of each issue in USD, in one query for the whole page. An issue
/// is left out when it has no usage, or when none of its models has a price.
pub async fn cost_by_issue(
    conn: &mut SqliteConnection,
    ids: &[String],
) -> ApiResult<HashMap<String, f64>> {
    type CostRow = (String, String, i64, i64, i64, i64, i64, i64);
    let rows: Vec<CostRow> = sqlx::query_as(
        "SELECT ws.issue_id, u.model, SUM(u.requests), SUM(u.input), SUM(u.output), SUM(u.cache_write_5m), SUM(u.cache_write_1h), SUM(u.cache_read)
         FROM json_each(?) page
         CROSS JOIN work_sessions ws ON ws.issue_id = page.value
         CROSS JOIN work_session_usage u ON u.work_session_id = ws.id
         GROUP BY ws.issue_id, u.model",
    )
    .bind(serde_json::to_string(ids)?)
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let prices = prices(conn).await?;
    let mut by_issue: HashMap<String, BTreeMap<String, TokenUsage>> = HashMap::new();
    for (issue, model, a, b, c, d, e, f) in rows {
        by_issue
            .entry(issue)
            .or_default()
            .insert(model.clone(), to_usage(&(model, a, b, c, d, e, f)));
    }
    Ok(by_issue
        .into_iter()
        .filter_map(|(issue, by_model)| {
            let cost = summary(by_model, &prices).cost_usd?;
            (cost > 0.0).then_some((issue, cost))
        })
        .collect())
}

/// The active time of each issue over all its sessions, in one query for
/// the whole page. An issue with no time data is left out.
pub async fn active_seconds_by_issue(
    conn: &mut SqliteConnection,
    ids: &[String],
) -> ApiResult<HashMap<String, u64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT ws.issue_id, SUM(ws.active_seconds)
         FROM json_each(?) page
         CROSS JOIN work_sessions ws ON ws.issue_id = page.value
         GROUP BY ws.issue_id
         HAVING SUM(ws.active_seconds) > 0",
    )
    .bind(serde_json::to_string(ids)?)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|(i, s)| (i, s as u64)).collect())
}

pub fn merge(into: &mut BTreeMap<String, TokenUsage>, from: &BTreeMap<String, TokenUsage>) {
    for (m, u) in from {
        into.entry(m.clone()).or_default().add(u);
    }
}

pub fn summary(
    by_model: BTreeMap<String, TokenUsage>,
    prices: &BTreeMap<String, ModelPrice>,
) -> UsageSummary {
    UsageSummary::build(by_model, prices)
}

pub async fn work_session_rows(
    c: &mut SqliteConnection,
    project: Option<&str>,
    issue: Option<&str>,
    session: Option<&str>,
) -> ApiResult<Vec<WorkSessionRow>> {
    Ok(sqlx::query_as(
        "SELECT w.id, i.key, i.title, w.active_seconds FROM work_sessions w JOIN issues i ON i.id = w.issue_id
         WHERE (? IS NULL OR i.project_id = ?) AND (? IS NULL OR i.id = ?) AND (? IS NULL OR w.session_id = ?)",
    )
        .bind(project)
        .bind(project)
        .bind(issue)
        .bind(issue)
        .bind(session)
        .bind(session)
        .fetch_all(&mut *c)
        .await?)
}
