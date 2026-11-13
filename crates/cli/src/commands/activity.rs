//! The activity feed.

use clap::Args;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Args)]
pub struct ActivityArgs {
    /// Events after this sequence number.
    #[arg(long)]
    pub after: Option<i64>,
    #[arg(long)]
    pub issue: Option<String>,
    #[arg(long)]
    pub limit: Option<u32>,
    /// Keep printing new events as they happen, one JSON line each. It
    /// listens on the WebSocket and reconnects after a server restart.
    #[arg(long)]
    pub follow: bool,
}

pub async fn run(app: &App, a: ActivityArgs) -> CliResult {
    let c = app.client().await?;
    let q = ActivityQuery {
        after_seq: a.after,
        issue: a.issue,
        limit: a.limit,
    };
    if !a.follow {
        app.out.ser(&c.activity(&q).await?);
        return Ok(());
    }
    follow(app, q, |e| {
        println!("{}", serde_json::to_string(e).unwrap_or_default());
        false
    })
    .await
}

/// `activity --follow`, through the client's live stream. Warnings go to
/// stderr as JSON lines. After a lost connection it connects again, which
/// starts the server when needed.
///
/// `each` gets every event in seq order. It returns true to stop. Without
/// `after_seq`, the events start after the server's newest one.
pub(crate) async fn follow<F>(app: &App, q: ActivityQuery, each: F) -> CliResult
where
    F: FnMut(&ActivityEvent) -> bool,
{
    let first = app.client().await?.clone();
    let connect = || async {
        app.connect().await.map_err(|e| match e {
            CliError::Client(e) => e,
            CliError::Usage(m)
            | CliError::Validation(m)
            | CliError::InputNeeded(m)
            | CliError::Other(m) => llmtodo_client::ClientError::Other(m),
        })
    };
    let warn = |w: llmtodo_client::live::Warning| {
        eprintln!("{}", serde_json::json!({ "warning": w.to_string() }));
    };
    llmtodo_client::live::follow(first, connect, q, Default::default(), warn, each)
        .await
        .map_err(CliError::from)
}
