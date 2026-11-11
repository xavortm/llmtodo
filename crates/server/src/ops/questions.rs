//! Owner questions: the derived table, the list, and taking back an answer.
//!
//! The `+++ Owner questions` section of a description is the record. The
//! `questions` table is a copy that the server can query. Every description
//! write rewrites the issue's rows in its own transaction.
//!
//! An answer hands the issue back to the agents, so an agent may act on it
//! at once. Undo is safe only while nothing has happened since. The rule:
//!
//! - The issue is open, and the question has an answer.
//! - The caller is the actor who wrote that answer.
//! - The question and the answer are the same as in the revision that
//!   answered it.
//! - No other actor wrote to the issue after that revision.
//!
//! Anything else is a conflict whose `details.reason` names the rule. An
//! agent that read the answer and wrote nothing leaves no trace, so undo
//! cannot see it.

use std::collections::HashMap;

use llmtodo_api_types::*;
use llmtodo_core::{Patch, Question};
use serde_json::json;
use sqlx::{SqliteConnection, SqlitePool};

use super::issues::apply_fields;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::issues::{self, IssueRow};
use crate::store::questions::{ListFilter, QuestionRow};
use crate::store::{self, actors, projects, teams};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1000;

fn refuse(reason: &str, message: String) -> ApiError {
    ApiError::conflict(message).with_details(json!({ "reason": reason }))
}

/// The revision being stored, and who wrote it.
#[derive(Clone, Copy)]
struct Author<'a> {
    version: i64,
    actor: Option<&'a str>,
}

/// The revision that took a question from open to answered.
#[derive(Clone)]
struct Answered {
    version: i64,
    actor: Option<String>,
    question: Question,
}

/// One question and what the revisions show about its answer.
#[derive(Clone)]
struct Derived {
    question: Question,
    answered: Option<Answered>,
}

/// The questions of `body`, given those of the revision before it. `None`
/// for `before` means this is the oldest revision, which cannot show an
/// answer being written. `None` for `author` means no revision records this
/// text. A question that stays answered keeps its author.
fn derive(before: Option<&[Derived]>, body: &str, author: Option<Author<'_>>) -> Vec<Derived> {
    // Undo looks up the first question with a number, so only that one
    // carries the history when a hand-edited section repeats a number.
    let mut earlier: HashMap<u32, &Derived> = HashMap::new();
    for d in before.unwrap_or_default() {
        earlier.entry(d.question.number).or_insert(d);
    }
    llmtodo_core::questions(body)
        .into_iter()
        .map(|question| {
            let old = earlier.remove(&question.number);
            let answered = if question.is_open() {
                None
            } else if let Some(d) = old.filter(|d| !d.question.is_open()) {
                d.answered.clone()
            } else {
                before.and(author).map(|a| Answered {
                    version: a.version,
                    actor: a.actor.map(str::to_string),
                    question: question.clone(),
                })
            };
            Derived { question, answered }
        })
        .collect()
}

/// Replays an issue's revisions, oldest first, to find who answered what.
fn replay(revisions: &[(i64, String, Option<String>)], current: &str) -> Vec<Derived> {
    let mut state: Option<Vec<Derived>> = None;
    for (version, body, actor) in revisions {
        let author = Author {
            version: *version,
            actor: actor.as_deref(),
        };
        state = Some(derive(state.as_deref(), body, Some(author)));
    }
    if revisions.last().is_none_or(|(_, body, _)| body != current) {
        state = Some(derive(state.as_deref(), current, None));
    }
    state.unwrap_or_default()
}

fn to_row(position: usize, d: &Derived) -> ApiResult<QuestionRow> {
    let q = &d.question;
    let snapshot = d
        .answered
        .as_ref()
        .map(|a| serde_json::to_string(&a.question))
        .transpose()?;
    Ok(QuestionRow {
        position: position as i64,
        number: i64::from(q.number),
        text: q.text.clone(),
        context: q.context.clone(),
        options: serde_json::to_string(&q.options)?,
        answer: q.answer.clone(),
        answered_version: d.answered.as_ref().map(|a| a.version),
        answered_by: d.answered.as_ref().and_then(|a| a.actor.clone()),
        answered_question: snapshot,
    })
}

fn to_question(
    number: i64,
    text: String,
    context: String,
    options: &str,
    answer: Option<String>,
) -> ApiResult<Question> {
    Ok(Question {
        id: format!("Q{number}"),
        number: number as u32,
        text,
        context,
        options: serde_json::from_str(options)?,
        answer,
    })
}

fn from_row(r: QuestionRow) -> ApiResult<Derived> {
    let answered = match (r.answered_version, &r.answered_question) {
        (Some(version), Some(snapshot)) => Some(Answered {
            version,
            actor: r.answered_by.clone(),
            question: serde_json::from_str(snapshot)?,
        }),
        _ => None,
    };
    Ok(Derived {
        question: to_question(r.number, r.text, r.context, &r.options, r.answer)?,
        answered,
    })
}

async fn load(c: &mut SqliteConnection, issue_id: &str) -> ApiResult<Vec<Derived>> {
    store::questions::of_issue(c, issue_id)
        .await?
        .into_iter()
        .map(from_row)
        .collect()
}

async fn save(c: &mut SqliteConnection, issue_id: &str, derived: &[Derived]) -> ApiResult<()> {
    let rows = derived
        .iter()
        .enumerate()
        .map(|(position, d)| to_row(position, d))
        .collect::<ApiResult<Vec<_>>>()?;
    store::questions::replace(c, issue_id, &rows).await
}

/// Stores the questions of a new issue's first description.
pub async fn record_created(c: &mut SqliteConnection, issue_id: &str, body: &str) -> ApiResult<()> {
    save(c, issue_id, &derive(None, body, None)).await
}

/// Stores the questions after a description change. The caller runs this in
/// the transaction that writes the description.
pub async fn record_changed(
    c: &mut SqliteConnection,
    issue_id: &str,
    body: &str,
    version: i64,
    actor: &str,
) -> ApiResult<()> {
    let before = load(c, issue_id).await?;
    let author = Author {
        version,
        actor: Some(actor),
    };
    save(c, issue_id, &derive(Some(&before), body, Some(author))).await
}

/// Fills the table once, after the migration that creates it. It reads
/// issues and revisions and writes only to `questions`, so a description,
/// its version and `updated_at` stay as they are. One transaction clears
/// the pending mark too, so a crash leaves the work to the next start.
pub async fn backfill(pool: &SqlitePool) -> ApiResult<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if !store::questions::backfill_pending(&mut tx).await? {
        return Ok(());
    }
    let issues = store::questions::issues_to_backfill(&mut tx).await?;
    for (issue_id, description) in &issues {
        let revisions = store::revisions::of_issue_oldest_first(&mut tx, issue_id).await?;
        save(&mut tx, issue_id, &replay(&revisions, description)).await?;
    }
    store::questions::finish_backfill(&mut tx).await?;
    tx.commit().await?;
    tracing::info!(issues = issues.len(), "questions table filled");
    Ok(())
}

fn parse_cursor(cursor: &str) -> ApiResult<(&str, &str, i64)> {
    let bad = || ApiError::bad_request("bad cursor");
    let mut parts = cursor.splitn(3, '|');
    let created = parts.next().ok_or_else(bad)?;
    let issue = parts.next().ok_or_else(bad)?;
    let position = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    Ok((created, issue, position))
}

/// Lists owner questions, oldest issue first. With `issue`, the questions of
/// that issue whatever its status. Without it, those on open issues.
pub async fn list(c: &mut SqliteConnection, q: &QuestionQuery) -> ApiResult<Page<QuestionEntry>> {
    let issue = match &q.issue {
        Some(r) => Some(issues::resolve_id(c, r).await?),
        None => None,
    };
    let project = match (&q.project, &issue) {
        (Some(r), None) => Some(projects::resolve(c, r).await?.id),
        _ => None,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let filter = ListFilter {
        issue: issue.as_deref(),
        project: project.as_deref(),
        include_answered: q.all,
        after: q.cursor.as_deref().map(parse_cursor).transpose()?,
        limit: i64::from(limit) + 1,
    };
    let mut rows = store::questions::list(c, &filter).await?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = rows
        .last()
        .filter(|_| more)
        .map(|r| format!("{}|{}|{}", r.issue_created_at, r.issue_id, r.position));
    let items = rows
        .into_iter()
        .map(|r| {
            Ok(QuestionEntry {
                key: r
                    .key
                    .parse()
                    .map_err(|_| ApiError::internal(format!("bad key {}", r.key)))?,
                title: r.title,
                waiting_on: r.waiting_on.as_deref().and_then(|w| w.parse().ok()),
                question: to_question(r.number, r.text, r.context, &r.options, r.answer)?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Page { items, next_cursor })
}

/// The first write to the issue by another actor after revision `version`,
/// as `(actor name, field)`.
async fn later_write(
    w: &mut Write<'_>,
    issue_id: &str,
    version: i64,
    answerer: &str,
) -> ApiResult<Option<(String, String)>> {
    let answered_at: Option<i64> =
        store::activity::description_seq(w.c(), issue_id, version.to_string()).await?;
    let later: Option<(String, String)> =
        store::activity::later_write(w.c(), issue_id, answered_at.unwrap_or(0), answerer).await?;
    let Some((actor, field)) = later else {
        return Ok(None);
    };
    let all = actors::all(w.c()).await?;
    let name = all.get(&actor).map_or(actor, |a| a.name.clone());
    Ok(Some((name, field)))
}

/// Refuses the undo unless every rule in the module comment holds.
async fn check_undo(w: &mut Write<'_>, row: &IssueRow, id: &str) -> ApiResult<()> {
    let key = &row.key;
    let current = llmtodo_core::questions::find(row.description.as_deref().unwrap_or(""), id)?;
    let qid = &current.id;
    if !teams::status_by_id(w.c(), &row.status_id)
        .await?
        .kind()
        .is_open()
    {
        return Err(refuse(
            "issue_closed",
            format!("{key} is closed, so {qid} keeps its answer"),
        ));
    }
    if current.is_open() {
        return Err(refuse(
            "already_open",
            format!("{key} {qid} has no answer to undo"),
        ));
    }
    let stored = load(w.c(), &row.id).await?;
    let Some(answered) = stored
        .into_iter()
        .find(|d| d.question.number == current.number)
        .and_then(|d| d.answered)
    else {
        return Err(refuse(
            "no_answer_record",
            format!("{key} {qid} has no recorded answer to undo"),
        ));
    };
    let me = w.ctx.actor_id();
    if answered.actor.as_deref() != Some(me.as_str()) {
        return Err(refuse(
            "not_the_answerer",
            format!("{key} {qid} was answered by someone else, so only they can undo it"),
        ));
    }
    if current != answered.question {
        return Err(refuse(
            "question_changed",
            format!("{key} {qid} changed after the answer, so nothing was undone"),
        ));
    }
    if let Some((actor, field)) = later_write(w, &row.id, answered.version, &me).await? {
        return Err(refuse(
            "agent_acted",
            format!(
                "{actor} already acted on {key} after the answer ({field}), so {qid} stays answered"
            ),
        ));
    }
    Ok(())
}

/// Reopens an answered question and sets `waiting_on` back to the owner.
pub async fn undo_answer(w: &mut Write<'_>, r: &str, id: String) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    check_undo(w, &row, &id).await?;
    let update = UpdateIssue {
        description: BodyChange {
            patches: vec![Patch::AnswerQuestion {
                id,
                answer: String::new(),
            }],
            ..Default::default()
        },
        waiting_on: Change::Set(WaitingOn::Owner),
        ..Default::default()
    };
    apply_fields(w, &row, update, false).await?;

    Ok(row.id)
}
