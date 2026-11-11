//! Laptop sync.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum SyncCmd {
    /// Show the local generation, unpushed writes and the remote manifest.
    Status,
    /// Copy the database to the sync folder.
    Push {
        /// Clear the lock so another machine can pull. This machine becomes
        /// read-only until it pulls again.
        #[arg(long)]
        release: bool,
        #[arg(long)]
        folder: Option<String>,
    },
    /// Replace the local database with the sync copy and become the primary.
    /// A backup is written first.
    Pull {
        /// Pull while another machine holds the lock.
        #[arg(long)]
        take: bool,
        /// Drop this machine's unpushed writes.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        folder: Option<String>,
    },
}

pub async fn run(app: &App, cmd: SyncCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        SyncCmd::Status => app.out.ser(&c.sync_status().await?),
        SyncCmd::Push { release, folder } => app
            .out
            .ser(&c.sync_push(&SyncPushRequest { release, folder }).await?),
        SyncCmd::Pull {
            take,
            force,
            folder,
        } => app.out.ser(
            &c.sync_pull(&SyncPullRequest {
                take,
                force,
                folder,
            })
            .await?,
        ),
    }
    Ok(())
}
