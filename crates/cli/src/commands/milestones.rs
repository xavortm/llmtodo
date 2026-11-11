//! Milestones.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Subcommand)]
pub enum MilestoneCmd {
    List {
        #[arg(long)]
        project: Option<String>,
    },
    Create {
        name: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, short = 'd')]
        description: Option<String>,
        /// `YYYY-MM-DD`
        #[arg(long)]
        target_date: Option<String>,
    },
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, short = 'd')]
        description: Option<String>,
        #[arg(long)]
        target_date: Option<String>,
        #[arg(long)]
        position: Option<i64>,
    },
    Delete {
        id: String,
    },
}

pub async fn run(app: &App, cmd: MilestoneCmd) -> CliResult {
    let c = app.client().await?;
    let need = |p: Option<String>| {
        app.project(p)
            .ok_or_else(|| CliError::Usage("pass --project or set project in .llmtodo.toml".into()))
    };
    match cmd {
        MilestoneCmd::List { project } => app.out.ser(&c.milestones(&need(project)?).await?),
        MilestoneCmd::Create {
            name,
            project,
            description,
            target_date,
        } => app.out.ser(
            &c.create_milestone(
                &need(project)?,
                &CreateMilestone {
                    name,
                    description: description.unwrap_or_default(),
                    target_date,
                },
            )
            .await?,
        ),
        MilestoneCmd::Update {
            id,
            name,
            description,
            target_date,
            position,
        } => app.out.ser(
            &c.update_milestone(
                &id,
                &UpdateMilestone {
                    name,
                    description,
                    target_date,
                    position,
                },
            )
            .await?,
        ),
        MilestoneCmd::Delete { id } => app.out.ser(&c.delete_milestone(&id).await?),
    }
    Ok(())
}
