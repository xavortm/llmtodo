//! GitHub sync.

use clap::Subcommand;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum GithubCmd {
    /// Poll linked repos now, instead of waiting for the next poll.
    Sync,
    /// The last poll, and why the sync is paused, if it is.
    Status,
}

pub async fn run(app: &App, cmd: GithubCmd) -> CliResult {
    let c = app.client().await?;
    let r = match cmd {
        GithubCmd::Sync => c.github_sync().await?,
        GithubCmd::Status => c.github_status().await?,
    };
    if let Some(p) = &r.paused {
        eprintln!("warning: GitHub sync is paused: {p}");
    }
    app.out.ser(&r);
    Ok(())
}
