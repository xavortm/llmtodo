//! `issue order`: the order of an epic's sub-issues, in steps.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Subcommand)]
pub enum OrderCmd {
    /// Write the whole order of an epic's sub-issues, first step first. A
    /// step is one key, or a comma list of keys that can be done in any
    /// order: `issue order set LLM-1 18 28 11,23,4 94`. Bare numbers take
    /// the parent's team. Sub-issues left out lose their step.
    Set {
        /// The epic.
        parent: String,
        /// The steps, in order.
        #[arg(required = true, value_name = "STEP")]
        steps: Vec<String>,
    },
    /// Show the steps of an epic, then its sub-issues with no step.
    Show { parent: String },
    /// Take every sub-issue of an epic out of the order.
    Clear { parent: String },
}

pub async fn run(app: &App, c: &llmtodo_client::Client, cmd: OrderCmd) -> CliResult {
    match cmd {
        OrderCmd::Set { parent, steps } => {
            let parent = full_key(app, &parent)?;
            let steps = parse_steps(c, &parent, &steps).await?;
            show(app, &c.set_order(&parent, &SetOrder { steps }).await?);
        }
        OrderCmd::Show { parent } => {
            let parent = full_key(app, &parent)?;
            show(app, &c.order(&parent).await?);
        }
        OrderCmd::Clear { parent } => {
            let parent = full_key(app, &parent)?;
            show(app, &c.set_order(&parent, &SetOrder::default()).await?);
        }
    }
    Ok(())
}

/// A bare number for the parent takes the repo's default team.
fn full_key(app: &App, r: &str) -> CliResult<String> {
    let r = r.trim();
    if !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()) {
        let team = app.team(None).ok_or_else(|| {
            CliError::Usage(format!(
                "{r} has no team; pass a full key such as ABC-{r}, or set a default team in .llmtodo.toml"
            ))
        })?;
        return Ok(format!("{team}-{r}"));
    }
    Ok(r.to_string())
}

/// Reads each step: one key or a comma list. Bare numbers take the parent's
/// current team, which the server names.
async fn parse_steps(
    c: &llmtodo_client::Client,
    parent: &str,
    args: &[String],
) -> CliResult<Vec<Vec<IssueKey>>> {
    let bare = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut team: Option<String> = None;
    let mut steps = Vec::new();
    for arg in args {
        let mut step = Vec::new();
        for part in arg.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let text = if bare(part) {
                if team.is_none() {
                    team = Some(c.issue(parent).await?.issue.team);
                }
                format!("{}-{part}", team.as_deref().unwrap_or_default())
            } else {
                part.to_string()
            };
            let key: IssueKey = text
                .parse()
                .map_err(|_| CliError::Usage(format!("{part:?} is not an issue key or number")))?;
            step.push(key);
        }
        if step.is_empty() {
            return Err(CliError::Usage(format!("step {arg:?} names no issue")));
        }
        steps.push(step);
    }
    Ok(steps)
}

fn show(app: &App, v: &OrderView) {
    if app.out.json {
        return app.out.ser(v);
    }
    if v.steps.is_empty() {
        println!("{} has no order.", v.parent);
    } else {
        println!("Order of {}:", v.parent);
        for s in &v.steps {
            for (i, l) in s.issues.iter().enumerate() {
                let dot = match (i, s.done) {
                    (0, true) => format!("{:>3} ●", s.step),
                    (0, false) => format!("{:>3} ○", s.step),
                    _ => "    │".to_string(),
                };
                println!("{dot} {}  [{}]  {}", l.key, l.status.name, l.title);
            }
        }
    }
    if !v.unordered.is_empty() {
        println!("\nNo step:");
        for l in &v.unordered {
            println!("      {}  [{}]  {}", l.key, l.status.name, l.title);
        }
    }
}
