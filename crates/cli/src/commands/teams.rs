//! Teams.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum TeamCmd {
    List,
    View {
        team: String,
    },
    /// Create a team with the default statuses.
    Create {
        key: String,
        name: String,
    },
    Update {
        team: String,
        #[arg(long)]
        name: Option<String>,
        /// Status the GitHub sync sets when a PR opens. `none` turns it off.
        #[arg(long)]
        pr_open_status: Option<String>,
        /// Status the GitHub sync sets when a PR merges. `none` turns it off.
        #[arg(long)]
        pr_merged_status: Option<String>,
    },
}

pub async fn run(app: &App, cmd: TeamCmd) -> CliResult {
    let c = app.client().await?;
    let off = |s: Option<String>| {
        s.map(|v| {
            if v.eq_ignore_ascii_case("none") {
                String::new()
            } else {
                v
            }
        })
    };
    match cmd {
        TeamCmd::List => app.out.ser(&c.teams().await?),
        TeamCmd::View { team } => app.out.ser(&c.team(&team).await?),
        TeamCmd::Create { key, name } => app
            .out
            .ser(&c.create_team(&CreateTeam { key, name }).await?),
        TeamCmd::Update {
            team,
            name,
            pr_open_status,
            pr_merged_status,
        } => app.out.ser(
            &c.update_team(
                &team,
                &UpdateTeam {
                    name,
                    pr_open_status: off(pr_open_status),
                    pr_merged_status: off(pr_merged_status),
                },
            )
            .await?,
        ),
    }
    Ok(())
}
