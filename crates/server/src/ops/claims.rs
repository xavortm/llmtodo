//! Claims: one session holds an issue while it works.

use std::collections::HashMap;

use llmtodo_api_types::*;
use serde_json::json;

use super::analyzing::end_analyzing;
use super::issues::{
    is_duplicate_status, set_assignee, set_status, set_status_fresh, touch_updated,
};
use super::j;
use crate::db::new_id;
use crate::error::{ApiError, ApiResult};
use crate::state::{Ctx, Write};
use crate::store::backend::Pool;
use crate::store::issues::{self, IssueRow};
use crate::store::{self, actors, deps, teams};

/// What a claim did, for the response.
pub struct Claimed {
    pub id: String,
    pub key: String,
    pub changed: bool,
    pub warnings: Vec<String>,
    pub stolen_from: Option<ClaimSummary>,
}

pub(crate) async fn live_claim(
    w: &mut Write<'_>,
    issue_id: &str,
) -> ApiResult<Option<(String, String, String)>> {
    store::claims::live(w.c(), issue_id).await
}

pub(crate) async fn end_claim(
    w: &mut Write<'_>,
    issue_id: &str,
    claim_id: &str,
    reason: &str,
) -> ApiResult<()> {
    store::claims::end(w.c(), reason, claim_id).await?;
    w.event(
        EntityType::Claim,
        claim_id,
        Some(issue_id),
        ActivityField::Ended,
        None,
        j(reason),
    )
    .await?;
    Ok(())
}

/// The reason a claim ends when its issue is in a review status.
pub(crate) const REVIEW_END_REASON: &str = "review";

/// Ends the live claims on issues in a review status, or in one status with
/// `status_id`. A ticket that waits for review is never locked. Returns how
/// many claims ended.
pub(crate) async fn end_review_claims(
    w: &mut Write<'_>,
    status_id: Option<&str>,
) -> ApiResult<usize> {
    let claims = store::claims::live_in_review(w.c(), status_id).await?;
    for (claim_id, issue_id) in &claims {
        end_claim(w, issue_id, claim_id, REVIEW_END_REASON).await?;
    }
    Ok(claims.len())
}

/// Ends the claims on issues in a review status once, after migration 0012
/// set the review flag. It runs at server start as the `migration` system
/// actor, so the activity log records each ended claim. A copy that is not
/// the primary skips it and keeps the mark.
pub async fn sweep_review_claims(st: &crate::state::AppState) -> ApiResult<usize> {
    if !st.primary() {
        return Ok(0);
    }
    let pool = st.pool();
    {
        let mut c = pool.acquire().await?;
        if !store::claims::review_sweep_pending(&mut c).await? {
            return Ok(0);
        }
        // Nothing to end, as in a new database: drop the mark without a
        // write, so the sync copy does not count a change.
        if store::claims::live_in_review(&mut c, None)
            .await?
            .is_empty()
        {
            store::claims::finish_review_sweep(&mut c).await?;
            return Ok(0);
        }
    }
    let ctx = Ctx {
        client: ClientContext::default(),
        actor: actors::system(&pool, "migration").await?,
    };
    let mut w = Write::begin(st, &ctx).await?;
    let ended = end_review_claims(&mut w, None).await?;
    store::claims::finish_review_sweep(w.c()).await?;
    w.commit().await?;
    tracing::info!(ended, "claims on review tickets ended");
    Ok(ended)
}

/// What the current holder of an issue means for this claim.
struct ClaimCheck {
    new_claim: bool,
    changed: bool,
    /// The ended claim and its holder, when this claim took the issue over.
    stolen_from: Option<(String, ClaimSummary)>,
}

pub async fn claim(w: &mut Write<'_>, r: &str, req: ClaimRequest) -> ApiResult<Claimed> {
    let ctx = w.ctx;
    let Some(session) = ctx.session().map(str::to_string) else {
        return Err(ApiError::bad_request(
            "a claim needs a session ID; set LLMTODO_SESSION",
        ));
    };

    let row = issues::resolve_live(w.c(), r).await?;
    let status = w.status(&row.status_id).await?;
    if !status.kind().is_open() {
        return Err(ApiError::validation(format!(
            "{} is {} ({}); it cannot be claimed",
            row.key,
            status.name,
            status.kind()
        )));
    }
    let mut warnings = Vec::new();
    if ctx.client.session_generated {
        warnings.push("the session ID was generated for this one call, so a later release cannot match it; set LLMTODO_SESSION".to_string());
    }
    let catalog = w.actors(&[]).await?;
    let all_actors = &catalog.lookup.actors;
    let (live, marked) = store::claims::live_and_marked(w.c(), &row.id).await?;
    let check = check_holder(w, ctx, &row, live, &session, req.steal, all_actors).await?;
    // Tasks that a session already holds are not affected.
    if check.new_claim && check.stolen_from.is_none() {
        require_estimates(&row)?;
    }
    // A claim ends any Analyzing mark: Locked replaces it.
    if marked {
        end_analyzing(w, &row.id, "claimed").await?;
    }
    if check.new_claim {
        let stolen = check.stolen_from.as_ref().map(|(id, _)| id.as_str());
        add_claim(w, ctx, &row, &session, &status, stolen, all_actors).await?;
    }
    Ok(Claimed {
        id: row.id,
        key: row.key,
        changed: check.changed,
        warnings,
        stolen_from: check.stolen_from.map(|(_, s)| s),
    })
}

/// Refuses a task that lacks a time or a cost estimate. Agents meet this
/// error without knowing the rule, so it says what to run.
fn require_estimates(row: &IssueRow) -> ApiResult<()> {
    let (no_time, no_cost) = (
        row.estimate_minutes.is_none(),
        row.estimate_cost_usd.is_none(),
    );
    if !no_time && !no_cost {
        return Ok(());
    }
    let (what, flags, missing) = match (no_time, no_cost) {
        (true, true) => (
            "a time estimate and a cost estimate",
            "--estimate-minutes <minutes> --estimate-cost <usd>",
            json!(["estimate_minutes", "estimate_cost_usd"]),
        ),
        (true, false) => (
            "a time estimate",
            "--estimate-minutes <minutes>",
            json!(["estimate_minutes"]),
        ),
        _ => (
            "a cost estimate",
            "--estimate-cost <usd>",
            json!(["estimate_cost_usd"]),
        ),
    };
    Err(ApiError::validation(format!(
        "{key} is missing {what}. Every task needs a time estimate and a cost estimate before work starts. Estimate the work, run `llmtodo issue update {key} {flags}`, then claim again.",
        key = row.key,
    ))
    .with_details(json!({ "reason": "estimate_missing", "missing": missing })))
}

/// Decides what a claim does about the current holder. It refuses an issue
/// another session or assignee holds, unless the caller steals it.
#[allow(clippy::too_many_arguments)]
async fn check_holder(
    w: &mut Write<'_>,
    ctx: &Ctx,
    row: &IssueRow,
    live: Option<(String, String, String)>,
    session: &str,
    steal: bool,
    all_actors: &HashMap<String, Actor>,
) -> ApiResult<ClaimCheck> {
    let actor_id = ctx.actor_id();
    let mut check = ClaimCheck {
        new_claim: true,
        changed: true,
        stolen_from: None,
    };
    if let Some((claim_id, holder, holder_session)) = live {
        if holder_session == session {
            check.new_claim = false;
            check.changed =
                refresh_claim_actor(w, ctx, row, &claim_id, &holder, all_actors).await?;
        } else {
            check.stolen_from = steal_claim(w, row, claim_id, steal, all_actors).await?;
        }
    } else if let Some(holder) = row.assignee_id.as_deref()
        && holder != actor_id
        && !steal
    {
        // An assignee holds the issue even without a claim, such as when
        // the owner assigns it in the app.
        let name = all_actors
            .get(holder)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| holder.to_string());
        return Err(ApiError::new(
            ErrorCode::ClaimHeld,
            format!(
                "{} is assigned to {name}; pass --steal to take it over",
                row.key
            ),
        )
        .with_details(json!({ "assignee": all_actors.get(holder) })));
    }
    Ok(check)
}

/// The same session holds the claim. Its model or effort may have changed,
/// which gives it a new actor, so the claim follows. Returns whether it did.
async fn refresh_claim_actor(
    w: &mut Write<'_>,
    ctx: &Ctx,
    row: &IssueRow,
    claim_id: &str,
    holder: &str,
    all_actors: &HashMap<String, Actor>,
) -> ApiResult<bool> {
    let actor_id = ctx.actor_id();
    if holder == actor_id {
        return Ok(false);
    }
    store::claims::set_actor(w.c(), &actor_id, claim_id).await?;
    let old = all_actors.get(holder).map(|a| a.name.clone());
    w.event(
        EntityType::Claim,
        claim_id,
        Some(&row.id),
        ActivityField::Actor,
        j(old),
        j(&ctx.actor.name),
    )
    .await?;
    set_assignee(w, row, Some(actor_id.clone())).await?;
    touch_updated(w, &row.id).await?;
    Ok(true)
}

/// Another session holds the claim. Ends it when the caller steals, and
/// refuses otherwise.
async fn steal_claim(
    w: &mut Write<'_>,
    row: &IssueRow,
    claim_id: String,
    steal: bool,
    all_actors: &HashMap<String, Actor>,
) -> ApiResult<Option<(String, ClaimSummary)>> {
    let summary = issues::live_claims(w.c(), std::slice::from_ref(&row.id), all_actors)
        .await?
        .remove(&row.id);
    if !steal {
        let held = summary
            .as_ref()
            .map(|s| format!("{} in session {}", s.actor.name, s.session_id))
            .unwrap_or_default();
        return Err(ApiError::new(
            ErrorCode::ClaimHeld,
            format!(
                "{} is claimed by {held}; pass --steal to take it over",
                row.key
            ),
        )
        .with_details(json!({ "claim": summary })));
    }
    end_claim(w, &row.id, &claim_id, "stolen").await?;
    Ok(summary.map(|s| (claim_id, s)))
}

/// Records a new claim: the row, the git details, the assignee and the
/// started status.
async fn add_claim(
    w: &mut Write<'_>,
    ctx: &Ctx,
    row: &IssueRow,
    session: &str,
    status: &teams::StatusRow,
    stolen_from: Option<&str>,
    all_actors: &HashMap<String, Actor>,
) -> ApiResult<()> {
    let actor_id = ctx.actor_id();
    let c = &ctx.client;
    let claim_id = new_id();
    store::claims::insert(
        w.c(),
        &claim_id,
        &row.id,
        &actor_id,
        session,
        c.host.as_deref(),
        c.worktree.as_deref(),
        c.branch.as_deref(),
        c.herdr_pane.as_deref(),
        stolen_from,
    )
    .await?;
    w.event(
        EntityType::Claim,
        &claim_id,
        Some(&row.id),
        ActivityField::Claimed,
        None,
        j(json!({ "actor": ctx.actor.name, "session": session, "stolen": stolen_from.is_some() })),
    )
    .await?;
    // The issue keeps the claim's git details after the claim ends.
    if c.branch.is_some() || c.worktree.is_some() {
        store::issues::merge_claim_git(
            w.c(),
            c.branch.as_deref(),
            c.worktree.as_deref(),
            c.host.as_deref(),
            &row.id,
        )
        .await?;
    }
    // Set the assignee without ending the claim we just made, and take a
    // ticket back from review into work, so it never stays locked in a
    // review status. The row was read in this write, and nothing since
    // changed its assignee or status, so one statement does both (LLM-469).
    let cur: Option<String> = row.assignee_id.clone();
    let assignee = (cur.as_deref() != Some(actor_id.as_str())).then_some(actor_id.as_str());
    let started = if status.kind() != StatusType::Started || status.review {
        w.first_work_status(&row.team_id)
            .await?
            .filter(|s| s.id != row.status_id)
    } else {
        None
    };
    // A status with a rule of its own takes the full path.
    let plain = started.as_ref().is_none_or(|s| {
        s.team_id == row.team_id && s.kind().is_open() && !s.review && !is_duplicate_status(s)
    });
    let one_step = started.as_ref().filter(|_| plain);
    if assignee.is_some() || one_step.is_some() {
        store::issues::set_assignee_and_status(
            w.c(),
            assignee,
            one_step.map(|s| s.id.as_str()),
            &row.id,
        )
        .await?;
    }
    if assignee.is_some() {
        let name = |id: &Option<String>| {
            id.as_ref()
                .and_then(|i| all_actors.get(i))
                .map(|a| a.name.clone())
        };
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::Assignee,
            j(name(&cur)),
            j(&ctx.actor.name),
        )
        .await?;
    }
    if let Some(s) = one_step {
        w.event(
            EntityType::Issue,
            &row.id,
            Some(&row.id),
            ActivityField::Status,
            j(&status.name),
            j(&s.name),
        )
        .await?;
    } else if let Some(s) = &started {
        set_status_fresh(w, row, s).await?;
    }
    touch_updated(w, &row.id).await?;
    Ok(())
}

/// The response to a claim, read after it commits: the issue, and a
/// warning when the issue has open blockers. Reading after the commit
/// keeps the write short (LLM-469). On Postgres the issue and the longest
/// chain of blockers load at once, on their own connections.
pub async fn response(
    pool: &Pool,
    claimed: Claimed,
    lookup: Option<&issues::Lookup>,
) -> ApiResult<ClaimResponse> {
    let mut c = pool.acquire().await?;
    let owned;
    let lookup = match lookup {
        Some(l) => l,
        None => {
            owned = issues::Lookup::load(&mut c).await?;
            &owned
        }
    };
    let (issue, chain) = match c.read_pool() {
        Some(p) => tokio::try_join!(
            async { issues::load_one_with(&mut c, &claimed.id, false, lookup).await },
            async {
                deps::longest_chain_of(&mut p.acquire().await?, &claimed.id, &lookup.statuses).await
            },
        )?,
        None => (
            issues::load_one_with(&mut c, &claimed.id, false, lookup).await?,
            deps::longest_chain_of(&mut c, &claimed.id, &lookup.statuses).await?,
        ),
    };
    let mut warnings = claimed.warnings;
    if !issue.open_blockers.is_empty() {
        let keys: Vec<String> = issue
            .open_blockers
            .iter()
            .map(|b| b.key.to_string())
            .collect();
        let chain: Vec<String> = chain.iter().map(ToString::to_string).collect();
        warnings.push(format!(
            "{} has open blockers: {}. Longest chain: {}",
            claimed.key,
            keys.join(", "),
            chain.join(" waits for ")
        ));
    }
    Ok(ClaimResponse {
        open_blockers: issue.open_blockers.clone(),
        issue,
        changed: claimed.changed,
        warnings,
        stolen_from: claimed.stolen_from,
    })
}

pub async fn release(w: &mut Write<'_>, r: &str, req: ReleaseRequest) -> ApiResult<String> {
    let ctx = w.ctx;

    let row = issues::resolve_live(w.c(), r).await?;
    let Some((claim_id, _holder, holder_session)) = live_claim(w, &row.id).await? else {
        return Err(ApiError::validation(format!(
            "{} has no claim to release",
            row.key
        )));
    };
    // The session holds the claim, even when its actor changed since.
    let mine = ctx.session() == Some(holder_session.as_str());
    if !mine && !req.force {
        return Err(ApiError::new(ErrorCode::ClaimHeld, format!("{} is claimed by another session ({holder_session}); pass --force to release it anyway", row.key))
            .with_details(json!({ "session_id": holder_session })));
    }
    end_claim(w, &row.id, &claim_id, "released").await?;
    set_assignee(w, &row, None).await?;
    let status = match &req.status {
        Some(s) => Some(w.status_by_name(&row.team_id, s).await?),
        None => {
            let cur = w.status(&row.status_id).await?;
            if cur.kind() == StatusType::Started {
                w.first_of_type(&row.team_id, StatusType::Unstarted).await?
            } else {
                None
            }
        }
    };
    if let Some(s) = status {
        set_status(w, &row, &s).await?;
    }
    touch_updated(w, &row.id).await?;

    Ok(row.id)
}
