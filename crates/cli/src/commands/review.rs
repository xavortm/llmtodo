//! `issue review`: the review section and the owner's answers.

use clap::Subcommand;
use llmtodo_api_types::*;
use serde_json::{Value, json};

use crate::{App, CliError, CliResult};

#[derive(Subcommand)]
pub enum ReviewCmd {
    /// Show the review section and every answer of an issue.
    Show { key: String },
    /// Write the `+++ Review` section. It replaces the section if there is
    /// one. Use short steps, one action per line, and no reasoning.
    Set {
        key: String,
        /// One step to reproduce. Repeat it for each step, in order.
        #[arg(long = "step", required = true)]
        steps: Vec<String>,
        /// One thing to check. Repeat it for each check.
        #[arg(long = "check", required = true)]
        checks: Vec<String>,
    },
    /// Answer the review of an issue in User review. `approve` moves it to
    /// Done, or, with an open PR, leaves it waiting on an agent to merge.
    /// `request-changes` needs --feedback and moves it to Todo for an agent.
    /// Several keys send every answer in one write, and print one result
    /// per key. A refused answer leaves the others in place, and the exit
    /// code is then 6.
    Answer {
        #[arg(required = true, num_args = 1..)]
        keys: Vec<String>,
        /// `approve` or `request-changes`.
        verdict: ReviewVerdict,
        /// What to change, or a note on the approval.
        #[arg(long, short = 'm')]
        feedback: Option<String>,
    },
    /// Wait for the next review answer and print it as JSON. It listens on
    /// the live events, so it does not poll.
    Wait {
        /// Only answers on this issue.
        key: Option<String>,
        /// Answers after this activity seq. Pass the `seq` of the last
        /// answer you handled, so you miss none between two waits.
        #[arg(long)]
        after_seq: Option<i64>,
        /// Give up after this many seconds, with exit code 1.
        #[arg(long)]
        timeout: Option<u64>,
    },
}

pub async fn run(app: &App, c: &llmtodo_client::Client, cmd: ReviewCmd) -> CliResult {
    match cmd {
        ReviewCmd::Show { key } => show(app, &c.review(&key).await?),
        ReviewCmd::Set { key, steps, checks } => {
            let req = UpdateIssue {
                description: BodyChange {
                    patches: vec![Patch::UpsertCollapsible {
                        section: llmtodo_core::review::REVIEW_SECTION.to_string(),
                        body: llmtodo_core::render_review(&steps, &checks),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            };
            c.update_issue(&key, &req).await?;
            show(app, &c.review(&key).await?);
        }
        ReviewCmd::Answer {
            mut keys,
            verdict,
            feedback,
        } => {
            let req = SubmitReview {
                verdict,
                feedback: feedback.unwrap_or_default(),
            };
            if keys.len() > 1 {
                return answer_many(app, c, keys, req).await;
            }
            let key = keys.remove(0);
            let d = c.submit_review(&key, &req).await?;
            let i = &d.issue;
            app.out.ser(&json!({
                "key": i.key,
                "status": i.status.name,
                "waiting_on": i.waiting_on,
                "answer": d.reviews.last(),
            }));
        }
        ReviewCmd::Wait {
            key,
            after_seq,
            timeout,
        } => wait(app, c, key, after_seq, timeout).await?,
    }
    Ok(())
}

/// Answers the reviews of several issues in one write (LLM-518).
async fn answer_many(
    app: &App,
    c: &llmtodo_client::Client,
    keys: Vec<String>,
    review: SubmitReview,
) -> CliResult {
    let req = SubmitReviews {
        answers: keys
            .into_iter()
            .map(|issue| ReviewItem {
                issue,
                review: review.clone(),
            })
            .collect(),
    };
    let outcomes = c.submit_reviews(&req).await?;
    let refused = outcomes.iter().filter(|o| o.error.is_some()).count();
    let rows: Vec<Value> = outcomes
        .iter()
        .map(|o| match (&o.result, &o.error) {
            (Some(i), _) => json!({
                "key": i.key,
                "status": i.status.name,
                "waiting_on": i.waiting_on,
            }),
            (None, e) => json!({ "key": o.issue, "error": e }),
        })
        .collect();
    app.out.ser(&rows);
    if refused > 0 {
        return Err(CliError::Validation(format!(
            "{refused} of {} answers were refused; each row says why",
            outcomes.len()
        )));
    }
    Ok(())
}

fn show(app: &App, r: &ReviewState) {
    if app.out.json {
        app.out.ser(r);
        return;
    }
    println!("{} {} ({})", r.key, r.title, r.status);
    match &r.section {
        None => println!("No review section."),
        Some(s) => {
            println!("Steps to reproduce:");
            for (ix, step) in s.steps.iter().enumerate() {
                println!("  {}. {step}", ix + 1);
            }
            println!("What to check:");
            for check in &s.checks {
                println!("  - {check}");
            }
        }
    }
    for a in &r.answers {
        let who = a.author.as_ref().map_or("someone", |a| a.name.as_str());
        println!("Round {}: {} by {who}", a.round, a.verdict.as_str());
        if !a.feedback.is_empty() {
            println!("  {}", a.feedback);
        }
    }
}

/// One answer, built from its activity event and the issue as it is now.
async fn answer_json(c: &llmtodo_client::Client, e: &ActivityEvent) -> Value {
    let new = e.new.clone().unwrap_or(Value::Null);
    let mut out = json!({
        "seq": e.seq,
        "key": e.issue,
        "round": new["round"],
        "verdict": new["verdict"],
        "feedback": new["feedback"],
        "author": e.actor.as_ref().map(|a| a.name.clone()),
        "created_at": e.created_at,
    });
    if let Some(key) = &e.issue
        && let Ok(d) = c.issue(&key.to_string()).await
    {
        out["title"] = json!(d.issue.title);
        out["status"] = json!(d.issue.status.name);
        out["waiting_on"] = json!(d.issue.waiting_on);
    }
    out
}

async fn wait(
    app: &App,
    c: &llmtodo_client::Client,
    key: Option<String>,
    after_seq: Option<i64>,
    timeout: Option<u64>,
) -> CliResult {
    let q = ActivityQuery {
        after_seq,
        issue: key,
        limit: None,
    };
    let mut found: Option<ActivityEvent> = None;
    let listen = super::activity::follow(app, q, |e| {
        let hit = e.field == ActivityField::Review;
        if hit {
            found = Some(e.clone());
        }
        hit
    });
    match timeout {
        Some(secs) => tokio::time::timeout(std::time::Duration::from_secs(secs), listen)
            .await
            .map_err(|_| CliError::Other(format!("no review answer within {secs} seconds")))??,
        None => listen.await?,
    }
    if let Some(e) = found {
        app.out.value(answer_json(c, &e).await);
    }
    Ok(())
}
