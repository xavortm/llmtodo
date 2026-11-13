//! Labels.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

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
        /// Create the label even when its name looks like a workflow state,
        /// such as `ready-for-agent`.
        #[arg(long)]
        force: bool,
    },
    Update {
        label: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Rename the label even when the new name looks like a workflow
        /// state.
        #[arg(long)]
        force: bool,
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
            force,
        } => {
            if !force {
                refuse_workflow_name(&name)?;
            }
            app.out.ser(
                &c.create_label(&CreateLabel {
                    name,
                    group,
                    color,
                    description,
                    is_group: group_label,
                })
                .await?,
            )
        }
        LabelCmd::Update {
            label,
            name,
            color,
            description,
            force,
        } => {
            if let (Some(name), false) = (&name, force) {
                refuse_workflow_name(name)?;
            }
            app.out.ser(
                &c.update_label(
                    &label,
                    &UpdateLabel {
                        name,
                        color,
                        description,
                    },
                )
                .await?,
            )
        }
        LabelCmd::Delete { label } => {
            app.confirm(&format!(
                "Delete label {label} and remove it from every issue?"
            ))?;
            app.out.ser(&c.delete_label(&label).await?)
        }
    }
    Ok(())
}

/// Words in a label name that name a state the tracker already has a field,
/// a relation or a status for. Agents made labels such as `ready-for-agent`
/// and `needs-owner`, which the app does not show as waiting on (LLM-161).
const WORKFLOW_WORDS: &[(&[&str], &str)] = &[
    (
        &["ready", "agent", "agents"],
        "Mark a ticket ready for an agent with waiting on: `llmtodo issue update <KEY> -w agent`.",
    ),
    (
        &["owner", "human"],
        "Ask the owner with `llmtodo issue ask <KEY> \"Question?\"`, or set waiting on: `llmtodo issue update <KEY> -w owner`.",
    ),
    (
        &["waiting", "wait"],
        "Use waiting on: `llmtodo issue update <KEY> -w agent` or `-w owner`.",
    ),
    (
        &["blocked", "blocker", "blocks", "blocking"],
        "Record a dependency with a relation: `llmtodo issue relation add <KEY> blocked-by <OTHER>`.",
    ),
    (
        &["review"],
        "Use a review status: `llmtodo issue review set <KEY> ...`, then `llmtodo issue update <KEY> --status \"User review\" -w owner`.",
    ),
    (
        &["progress", "wip", "done"],
        "Use a status: `llmtodo issue update <KEY> --status <name>`. `llmtodo status list` shows them.",
    ),
];

/// The advice for each workflow word in `name`, or none.
fn workflow_advice(name: &str) -> Vec<&'static str> {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    WORKFLOW_WORDS
        .iter()
        .filter(|(list, _)| words.iter().any(|w| list.contains(w)))
        .map(|(_, advice)| *advice)
        .collect()
}

fn refuse_workflow_name(name: &str) -> CliResult {
    let advice = workflow_advice(name);
    if advice.is_empty() {
        return Ok(());
    }
    Err(CliError::Validation(format!(
        "the label name {name:?} looks like a workflow state. Labels do not show as waiting on in the app. {} Pass --force to create the label anyway.",
        advice.join(" ")
    )))
}

#[cfg(test)]
mod tests {
    use super::workflow_advice;

    #[test]
    fn flags_workflow_names() {
        for name in [
            "ready-for-agent",
            "needs-owner",
            "Blocked",
            "in review",
            "waiting_on_agent",
            "WIP",
        ] {
            assert!(!workflow_advice(name).is_empty(), "{name}");
        }
        let advice = workflow_advice("ready-for-agent");
        assert_eq!(advice.len(), 1);
        assert!(advice[0].contains("-w agent"));
        assert!(workflow_advice("needs-owner")[0].contains("-w owner"));
    }

    #[test]
    fn keeps_topic_names() {
        for name in [
            "Area",
            "cli",
            "server",
            "design-system",
            "handoff",
            "already",
            "reviewer-tools-x",
        ] {
            // "reviewer" is not "review": only whole words count.
            assert!(workflow_advice(name).is_empty(), "{name}");
        }
    }
}
