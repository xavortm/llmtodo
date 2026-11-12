//! Claims: one session holds an issue while it works.

use std::collections::HashMap;

use llmtodo_api_types::*;
use serde_json::json;

use super::analyzing::end_analyzing;
use super::issues::{set_assignee, set_status, touch_updated};
use super::j;
use crate::db::new_id;
use crate::error::{ApiError, ApiResult};
use crate::state::{Ctx, Write};
use crate::store::issues::{self, IssueRow};
use crate::store::{self, actors, deps, teams};

/// What a claim did, for the response.
pub struct Claimed {
    pub id: String,
    pub changed: bool,
    pub open_blockers: Vec<IssueLink>,
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
    let status = teams::status_by_id(w.c(), &row.status_id).await?;
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
    let all_actors = actors::all(w.c()).await?;
    let check = check_holder(w, ctx, &row, &session, req.steal, &all_actors).await?;
    // Tasks that a session already holds are not affected.
    if check.new_claim && check.stolen_from.is_none() {
        require_estimates(&row)?;
    }
    // A claim ends any Analyzing mark: Locked replaces it.
    end_analyzing(w, &row.id, "claimed").await?;
    if check.new_claim {
        let stolen = check.stolen_from.as_ref().map(|(id, _)| id.as_str());
        add_claim(w, ctx, &row, &session, &status, stolen, &all_actors).await?;
    }
    let open_blockers = open_blockers_warning(w, &row, &mut warnings).await?;

    Ok(Claimed {
        id: row.id,
        changed: check.changed,
        open_blockers,
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
async fn check_holder(
    w: &mut Write<'_>,
    ctx: &Ctx,
    row: &IssueRow,
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
    if let Some((claim_id, holder, holder_session)) = live_claim(w, &row.id).await? {
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
    // Set the assignee without ending the claim we just made.
    let cur: Option<String> = store::issues::assignee(w.c(), &row.id).await?;
    if cur.as_deref() != Some(actor_id.as_str()) {
        store::issues::set_assignee(w.c(), Some(actor_id.as_str()), &row.id).await?;
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
    // A claim takes a ticket back from review into work, so it never stays
    // locked in a review status.
    if (status.kind() != StatusType::Started || status.review)
        && let Some(started) = teams::first_work_status(w.c(), &row.team_id).await?
    {
        set_status(w, row, &started).await?;
    }
    touch_updated(w, &row.id).await?;
    Ok(())
}

/// Loads the open blockers, and adds a warning when there are any.
async fn open_blockers_warning(
    w: &mut Write<'_>,
    row: &IssueRow,
    warnings: &mut Vec<String>,
) -> ApiResult<Vec<IssueLink>> {
    let statuses: HashMap<String, teams::StatusRow> = teams::status_map(w.c()).await?;
    let blockers = deps::Blockers::load(w.c(), std::slice::from_ref(&row.id), &statuses).await?;
    let open_blockers = blockers.open_blockers(&row.id);
    if !open_blockers.is_empty() {
        let keys: Vec<String> = open_blockers.iter().map(|b| b.key.to_string()).collect();
        let chain: Vec<String> = blockers
            .longest_chain(&row.id)
            .iter()
            .map(ToString::to_string)
            .collect();
        warnings.push(format!(
            "{} has open blockers: {}. Longest chain: {}",
            row.key,
            keys.join(", "),
            chain.join(" waits for ")
        ));
    }
    Ok(open_blockers)
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
        Some(s) => Some(teams::status_by_name(w.c(), &row.team_id, s).await?),
        None => {
            let cur = teams::status_by_id(w.c(), &row.status_id).await?;
            if cur.kind() == StatusType::Started {
                teams::first_of_type(w.c(), &row.team_id, StatusType::Unstarted).await?
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
