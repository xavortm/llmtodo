//! Labels.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum LabelCmd {
    List,
    /// Create a label, or a group with --group-label.
    Create {
        name: String,
        /// The group this label belongs to.
        #[arg(long)]
        group: Option<String>,
        /// Create a label group instead of a label.
        #[arg(long)]
        group_label: bool,
        #[arg(long)]
        color: Option<String>,
        #[arg(long, short = 'd')]
        description: Option<String>,
    },
    Update {
        label: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a label and remove it from every issue.
    Delete {
        label: String,
    },
}

pub async fn run(app: &App, cmd: LabelCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        LabelCmd::List => app.out.ser(&c.labels().await?),
        LabelCmd::Create {
            name,
            group,
            group_label,
            color,
            description,
        } => app.out.ser(
            &c.create_label(&CreateLabel {
                name,
                group,
                color,
                description,
                is_group: group_label,
            })
            .await?,
        ),
        LabelCmd::Update {
            label,
            name,
            color,
            description,
        } => app.out.ser(
            &c.update_label(
                &label,
                &UpdateLabel {
                    name,
                    color,
                    description,
                },
            )
            .await?,
        ),
        LabelCmd::Delete { label } => {
            app.confirm(&format!(
                "Delete label {label} and remove it from every issue?"
            ))?;
            app.out.ser(&c.delete_label(&label).await?)
        }
    }
    Ok(())
}
