//! Token usage.

use clap::Args;
use llmtodo_api_types::*;

use crate::{App, CliResult};

#[derive(Args)]
pub struct UsageArgs {
    /// Only this project. Defaults to the project in .llmtodo.toml.
    #[arg(long)]
    pub project: Option<String>,
    /// Every project, even with a default in .llmtodo.toml.
    #[arg(long, conflicts_with = "project")]
    pub all: bool,
    #[arg(long)]
    pub issue: Option<String>,
    #[arg(long)]
    pub session: Option<String>,
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

pub fn fmt_time(seconds: Option<u64>) -> String {
    seconds.map_or_else(|| "-".into(), format_active_time)
}

pub fn fmt_usage(u: &UsageSummary) -> String {
    let cost = u
        .cost_usd
        .map(|c| format!("${c:.2}"))
        .unwrap_or_else(|| "no price".into());
    let out: u64 = u.by_model.values().map(|m| m.output).sum();
    format!(
        "{} tokens ({} out) · {} requests · {cost}",
        fmt_tokens(u.total_tokens),
        fmt_tokens(out),
        u.requests
    )
}

pub async fn run(app: &App, a: UsageArgs) -> CliResult {
    let c = app.client().await?;
    let project = if a.all || a.issue.is_some() || a.session.is_some() {
        a.project
    } else {
        app.project(a.project)
    };
    let r = c
        .usage(&UsageQuery {
            project,
            issue: a.issue,
            session: a.session,
        })
        .await?;
    if app.out.json {
        app.out.ser(&r);
        return Ok(());
    }
    let rows: Vec<[String; 5]> = r
        .issues
        .iter()
        .map(|i| {
            [
                i.key.to_string(),
                i.usage
                    .cost_usd
                    .map(|c| format!("${c:.2}"))
                    .unwrap_or_else(|| "-".into()),
                fmt_time(i.active_seconds),
                fmt_tokens(i.usage.total_tokens),
                i.title.clone(),
            ]
        })
        .collect();
    crate::output::table(&["KEY", "COST", "TIME", "TOKENS", "TITLE"], &rows);
    println!("\nTotal: {}", fmt_usage(&r.total));
    if let Some(s) = r.total_active_seconds {
        println!("Active time: {}", format_active_time(s));
    }
    if !r.total.unpriced_models.is_empty() {
        println!(
            "No price for: {}. Set one with `llmtodo settings set --model-price`.",
            r.total.unpriced_models.join(", ")
        );
    }
    Ok(())
}
