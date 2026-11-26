//! Review rounds: the owner approves a ticket in a review status, or asks for
//! changes. A reviewer agent can review first (LLM-588).
//!
//! The steps and checks live in the `+++ Review` section of the
//! description (see `llmtodo_core::review`). The answers live in the
//! `reviews` table, and each one writes an activity event with the field
//! `review`, so `issue review wait` hears it.
//!
//! - Approve, with no open PR: the ticket moves to the team's first
//!   completed status. That clears `waiting_on` and ends any claim.
//! - Approve, with an open PR: the ticket stays in its review status and waits on
//!   an agent, which merges the PR and marks the ticket Done.
//! - Request changes: feedback is required. The ticket moves to the team's
//!   first unstarted status and waits on an agent. Any claim ends and the
//!   assignee clears, so the frontier offers it again.
//!
//! - Pass: only in an agent-review status, such as In Review. The ticket
//!   moves to the team's owner-review status and waits on the owner. The
//!   feedback stays on the ticket as the reviewer's notes.
//!
//! Only the owner approves: an agent's approve is refused. A reviewer agent
//! asks for changes the same way the owner does.
//!
//! One answer per round. A round ends when the ticket enters a review status
//! again.

use crate::store::backend::Conn;
use llmtodo_api_types::*;
use serde_json::json;

use super::claims::{end_claim, live_claim};
use super::issues::{apply_waiting_on, set_assignee, set_status, touch_updated};
use super::j;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::issues::{self, IssueRow};
use crate::store::{self, teams};

fn refuse(reason: &str, message: String) -> ApiError {
    ApiError::validation(message).with_details(json!({ "reason": reason }))
}

/// The error for a move to a review status without a full review section.
pub(crate) fn section_missing(key: &str, description: &str) -> Option<ApiError> {
    let missing = llmtodo_core::review_missing(description);
    if missing.is_empty() {
        return None;
    }
    let format = llmtodo_core::review::REVIEW_FORMAT;
    Some(
        ApiError::validation(format!(
            "{key} cannot move to a review status: its +++ Review section needs {}. Add it to the description in this format, or run `llmtodo issue review set {key} --step ... --check ...`:\n\n{format}",
            missing.join(" and "),
        ))
        .with_details(json!({
            "reason": "review_section_missing",
            "missing": missing,
            "format": format,
        })),
    )
}

/// The review section and the answers of an issue.
pub async fn state(c: &mut Conn, r: &str) -> ApiResult<ReviewState> {
    let row = issues::resolve_live(c, r).await?;
    let status = teams::status_by_id(c, &row.status_id).await?;
    let description = row.description.clone().unwrap_or_default();
    Ok(ReviewState {
        key: row
            .key
            .parse()
            .map_err(|_| ApiError::internal(format!("bad key {}", row.key)))?,
        title: row.title.clone(),
        status: status.name,
        waiting_on: row.waiting_on.as_deref().and_then(|w| w.parse().ok()),
        section: llmtodo_core::review(&description),
        answers: store::reviews::for_issue(c, &row.id).await?,
    })
}

/// Whether the issue has a PR that is still open. A PR the GitHub sync never
/// saw counts as open, so an agent checks it.
async fn has_open_pr(w: &mut Write<'_>, row: &IssueRow) -> ApiResult<bool> {
    let attachments = store::attachments::attachments(w.c(), &row.id).await?;
    Ok(attachments
        .iter()
        .filter(|a| a.kind == AttachmentKind::PullRequest)
        .any(|a| matches!(a.metadata["state"].as_str(), None | Some("open"))))
}

/// Ends any claim and clears the assignee, so the frontier offers the issue.
async fn free_for_agents(w: &mut Write<'_>, row: &IssueRow, reason: &str) -> ApiResult<()> {
    if let Some((claim_id, _, _)) = live_claim(w, &row.id).await? {
        end_claim(w, &row.id, &claim_id, reason).await?;
    }
    set_assignee(w, row, None).await?;
    Ok(())
}

pub async fn submit(w: &mut Write<'_>, r: &str, req: SubmitReview) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    let key = row.key.clone();
    let status = w.status(&row.status_id).await?;
    if !status.review {
        return Err(refuse(
            "not_in_review",
            format!(
                "{key} is {}, not a review status, so it has no review to answer",
                status.name
            ),
        ));
    }
    if req.verdict == ReviewVerdict::Approved && w.ctx.actor.kind == ActorKind::Agent {
        return Err(refuse(
            "owner_only",
            format!(
                "{key}: only the owner can approve. A reviewer agent answers pass or request-changes"
            ),
        ));
    }
    if req.verdict == ReviewVerdict::Passed && !status.agent_review() {
        return Err(refuse(
            "not_agent_review",
            format!(
                "{key} is in {}, where the owner reviews. Pass is the reviewer agent's answer, in a status such as In Review",
                status.name
            ),
        ));
    }
    // The activity table must hold this write's events too.
    w.flush_events().await?;
    let (entered, answered) =
        store::reviews::entry_and_answer_seqs(w.c(), &row.id, &status.name).await?;
    if answered.is_some_and(|a| entered.is_none_or(|e| a > e)) {
        return Err(refuse(
            "already_answered",
            format!("{key} already has an answer for this review round"),
        ));
    }
    let feedback = req.feedback.trim().to_string();
    if req.verdict == ReviewVerdict::ChangesRequested && feedback.is_empty() {
        return Err(refuse(
            "feedback_missing",
            format!("{key}: say what to change. Request changes needs feedback"),
        ));
    }
    let round = store::reviews::last_round(w.c(), &row.id).await? + 1;
    let author = w.ctx.actor_id();
    store::reviews::insert(w.c(), &row.id, round, req.verdict, &feedback, &author).await?;
    w.event(
        EntityType::Issue,
        &row.id,
        Some(&row.id),
        ActivityField::Review,
        None,
        j(json!({ "round": round, "verdict": req.verdict, "feedback": feedback })),
    )
    .await?;

    match req.verdict {
        ReviewVerdict::Passed => {
            let owner = w
                .team_statuses(&row.team_id)
                .await?
                .into_iter()
                .find(|s| s.owner_review())
                .ok_or_else(|| {
                    refuse(
                        "owner_review_missing",
                        format!(
                            "{key}'s team has no status where the owner reviews. Add one with `llmtodo status create --review-role owner`"
                        ),
                    )
                })?;
            set_status(w, &row, &owner).await?;
            apply_waiting_on(w, &row, Some(WaitingOn::Owner), false).await?;
        }
        ReviewVerdict::Approved if has_open_pr(w, &row).await? => {
            free_for_agents(w, &row, "approved").await?;
            apply_waiting_on(w, &row, Some(WaitingOn::Agent), false).await?;
        }
        ReviewVerdict::Approved => {
            let done = w
                .first_of_type(&row.team_id, StatusType::Completed)
                .await?
                .ok_or_else(|| {
                    ApiError::validation(format!("{key}'s team has no completed status"))
                })?;
            set_status(w, &row, &done).await?;
        }
        ReviewVerdict::ChangesRequested => {
            let todo = w
                .first_of_type(&row.team_id, StatusType::Unstarted)
                .await?
                .ok_or_else(|| {
                    ApiError::validation(format!("{key}'s team has no unstarted status"))
                })?;
            free_for_agents(w, &row, "changes_requested").await?;
            set_status(w, &row, &todo).await?;
            apply_waiting_on(w, &row, Some(WaitingOn::Agent), false).await?;
        }
    }
    touch_updated(w, &row.id).await?;
    Ok(row.id)
}

/// Answers the reviews of several issues in one write, in order (LLM-518).
/// Each answer runs in its own savepoint, so a refused one leaves the
/// others in place. It returns the issue ID or the refusal of each. A lost
/// database ends the whole write.
pub async fn submit_many(
    w: &mut Write<'_>,
    answers: Vec<ReviewItem>,
) -> ApiResult<Vec<(String, ApiResult<String>)>> {
    let mut out = Vec::with_capacity(answers.len());
    for item in answers {
        let mark = w.savepoint().await?;
        match submit(w, &item.issue, item.review).await {
            Ok(id) => {
                w.release().await?;
                out.push((item.issue, Ok(id)));
            }
            Err(e) if e.code == ErrorCode::DatabaseUnreachable => return Err(e),
            Err(e) => {
                w.rollback_to(mark).await?;
                out.push((item.issue, Err(e)));
            }
        }
    }
    Ok(out)
}
