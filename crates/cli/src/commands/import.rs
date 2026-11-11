//! The Linear import.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Subcommand)]
pub enum ImportCmd {
    /// Copy issues, comments, relations, attachments, labels, projects,
    /// documents and milestones from Linear. Keeps keys such as ALX-123.
    /// Safe to run again: it updates by Linear ID. Reads LINEAR_API_KEY.
    Linear {
        /// Team key to import. Repeat for several. Default: every team.
        #[arg(long)]
        team: Vec<String>,
        /// Print counts and mapping problems, and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Do not copy images from Linear.
        #[arg(long)]
        skip_files: bool,
    },
}

pub async fn run(app: &App, cmd: ImportCmd) -> CliResult {
    match cmd {
        ImportCmd::Linear {
            team,
            dry_run,
            skip_files,
        } => {
            let token = std::env::var("LINEAR_API_KEY").ok().filter(|t| !t.trim().is_empty()).ok_or_else(|| {
                CliError::InputNeeded("set LINEAR_API_KEY to a Linear personal API key (Linear: Settings > Security & access > Personal API keys)".into())
            })?;
            let c = app.client().await?;
            app.out.ser(
                &c.import_linear(&LinearImportRequest {
                    token,
                    teams: team,
                    dry_run,
                    skip_files,
                })
                .await?,
            );
        }
    }
    Ok(())
}
