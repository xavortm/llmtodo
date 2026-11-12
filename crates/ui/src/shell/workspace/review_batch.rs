//! LLM-166: Approve in a task's menu approves every marked task that
//! takes it, when the clicked task is one of 2 or more marks.

use super::commands::tasks;
use super::*;
use crate::model::data::NOT_APPROVABLE;

/// The Approve answers that one menu click sent.
pub(super) struct ReviewBatch {
    /// The tasks whose answer the server has not taken or refused yet.
    waiting: Vec<IssueId>,
    approved: Vec<IssueId>,
    /// The key of each refused task and the server's reason.
    failed: Vec<(SharedString, String)>,
    /// The end of the message: the tasks the batch left out, and why.
    skipped: String,
}

impl Workspace {
    /// Sends Approve for each marked task that takes it, when `id` is one
    /// of 2 or more marks. Returns whether it ran that way; otherwise
    /// Approve applies to `id` alone.
    pub(super) fn approve_marked(&mut self, id: IssueId, cx: &mut Context<Self>) -> bool {
        use llmtodo_api_types::{ReviewVerdict, SubmitReview};
        let marked = self.marked(cx);
        if marked.len() < 2 || !marked.contains(&id) {
            return false;
        }
        if self.refuse_read_only_tracker(cx) {
            return true;
        }
        if self.review_batch.is_some() {
            self.show_message("Wait: the last review answers are still sending".into(), cx);
            return true;
        }
        if self.backend.is_none() {
            self.show_message("No server, so the review answers were not sent".into(), cx);
            return true;
        }
        let (send, skipped) = {
            let store = self.store.read(cx);
            let sending = &self.task.read(cx).review_sending;
            let mut send = Vec::new();
            let mut skipped = Vec::new();
            for id in marked {
                let Some(issue) = store.get(id) else { continue };
                let reason = issue
                    .why_not_approvable()
                    .or_else(|| sending.contains_key(&id).then_some(NOT_APPROVABLE[2]));
                match reason {
                    Some(reason) => skipped.push(reason),
                    None => send.push(id),
                }
            }
            (send, skipped_text(&skipped))
        };
        if send.is_empty() {
            self.show_message(format!("Nothing to approve{skipped}"), cx);
            return true;
        }
        self.task.update(cx, |task, cx| {
            for id in &send {
                task.review_sending.insert(*id, ReviewVerdict::Approved);
                task.review_error.remove(id);
            }
            cx.notify();
        });
        for id in &send {
            let feedback = self.answers.read(cx).feedback_text(*id, cx);
            let review = SubmitReview {
                verdict: ReviewVerdict::Approved,
                feedback,
            };
            self.send(*id, Write::Review(review), WriteKind::Review, cx);
        }
        self.message = format!("Approving {}{skipped}", tasks(send.len())).into();
        self.review_batch = Some(ReviewBatch {
            waiting: send,
            approved: Vec::new(),
            failed: Vec::new(),
            skipped,
        });
        cx.notify();
        true
    }

    /// The server took the answer for `id` (`refusal` is `None`) or refused
    /// it. Returns whether `id` belongs to the batch. The last answer of
    /// the batch writes one message and opens the next review ticket.
    pub(super) fn batch_answered(
        &mut self,
        id: IssueId,
        key: &str,
        refusal: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(batch) = &mut self.review_batch else {
            return false;
        };
        let Some(at) = batch.waiting.iter().position(|w| *w == id) else {
            return false;
        };
        batch.waiting.remove(at);
        match refusal {
            None => {
                batch.approved.push(id);
                self.answers.update(cx, |a, _| a.clear_feedback_later(id));
            }
            Some(message) => batch.failed.push((key.to_string().into(), message)),
        }
        if self
            .review_batch
            .as_ref()
            .is_some_and(|b| b.waiting.is_empty())
            && let Some(batch) = self.review_batch.take()
        {
            self.message = batch_message(&batch).into();
            if !batch.approved.is_empty() {
                self.open_next_review(&batch.approved, cx);
            }
        }
        cx.notify();
        true
    }
}

/// The message after the last answer of `batch`.
fn batch_message(batch: &ReviewBatch) -> String {
    let mut parts = Vec::new();
    if !batch.approved.is_empty() {
        parts.push(format!("Approved {}", tasks(batch.approved.len())));
    }
    if let Some((_, first)) = batch.failed.first() {
        let keys: Vec<&str> = batch.failed.iter().map(|(k, _)| k.as_ref()).collect();
        let keys = keys.join(", ");
        if batch.failed.iter().all(|(_, m)| m == first) {
            parts.push(format!("Could not approve {keys}: {first}"));
        } else {
            parts.push(format!("Could not approve {keys}. Each task shows why"));
        }
    }
    parts.join(". ") + &batch.skipped
}

/// The end of a batch message: how many tasks the batch skipped, and why.
/// Empty when it skipped none.
fn skipped_text(reasons: &[&str]) -> String {
    let counts: Vec<(usize, &str)> = NOT_APPROVABLE
        .iter()
        .map(|reason| (reasons.iter().filter(|r| *r == reason).count(), *reason))
        .filter(|(n, _)| *n > 0)
        .collect();
    match counts.as_slice() {
        [] => String::new(),
        [(n, reason)] => format!(". Skipped {} {reason}", tasks(*n)),
        _ => {
            let each: Vec<String> = counts.iter().map(|(n, r)| format!("{n} {r}")).collect();
            format!(". Skipped {}: {}", tasks(reasons.len()), each.join(", "))
        }
    }
}
