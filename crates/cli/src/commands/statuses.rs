//! Statuses.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum StatusCmd {
    /// List statuses of one team, or of every team.
    List {
        #[arg(long)]
        team: Option<String>,
    },
    Create {
        team: String,
        name: String,
        #[arg(long = "type")]
        kind: StatusType,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        position: Option<i64>,
        /// Mark it a review status: tickets here wait for the owner's
        /// review, and a move here ends the agent's claim.
        #[arg(long)]
        review: bool,
    },
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        position: Option<i64>,
        /// Set or clear the review flag. `true` also ends the claims on
        /// issues in this status.
        #[arg(long)]
        review: Option<bool>,
    },
    /// Delete a status no issue uses.
    Delete { id: String },
}

pub async fn run(app: &App, cmd: StatusCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        StatusCmd::List { team } => {
            let teams = match app.team(team) {
                Some(t) => vec![c.team(&t).await?],
                None => c.teams().await?,
            };
            let all: Vec<Status> = teams.into_iter().flat_map(|t| t.statuses).collect();
            app.out.ser(&all);
        }
        StatusCmd::Create {
            team,
            name,
            kind,
            color,
            position,
            review,
        } => app.out.ser(
            &c.create_status(
                &team,
                &CreateStatus {
                    name,
                    kind,
                    color,
                    position,
                    review: Some(review),
                },
            )
            .await?,
        ),
        StatusCmd::Update {
            id,
            name,
            color,
            position,
            review,
        } => app.out.ser(
            &c.update_status(
                &id,
                &UpdateStatus {
                    name,
                    color,
                    position,
                    review,
                },
            )
            .await?,
        ),
        StatusCmd::Delete { id } => app.out.ser(&c.delete_status(&id).await?),
    }
    Ok(())
}
