//! Prune.

use clap::Args;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Args)]
pub struct PruneArgs {
    /// Delete trashed issues that went to the trash before this date.
    #[arg(long)]
    pub before: Option<String>,
    /// Trim revisions older than this date. The newest one of each body stays.
    #[arg(long)]
    pub revisions_before: Option<String>,
    /// Show what would go, and delete nothing.
    #[arg(long)]
    pub dry_run: bool,
}

/// `2026-11-01` means local midnight. A full RFC 3339 time works too.
pub fn parse_when(s: &str) -> CliResult<jiff::Timestamp> {
    if let Ok(t) = s.parse::<jiff::Timestamp>() {
        return Ok(t);
    }
    let d: jiff::civil::Date = s
        .parse()
        .map_err(|_| CliError::Usage(format!("date must be YYYY-MM-DD or RFC 3339, got {s:?}")))?;
    d.to_zoned(jiff::tz::TimeZone::system())
        .map(|z| z.timestamp())
        .map_err(|e| CliError::Usage(e.to_string()))
}

pub async fn run(app: &App, a: PruneArgs) -> CliResult {
    let c = app.client().await?;
    let req = PruneRequest {
        before: a.before.as_deref().map(parse_when).transpose()?,
        revisions_before: a.revisions_before.as_deref().map(parse_when).transpose()?,
        dry_run: true,
        ..Default::default()
    };
    let preview = c.prune(&req).await?;
    if a.dry_run {
        app.out.ser(&preview);
        return Ok(());
    }
    if preview.issues.is_empty() && preview.revisions == 0 {
        app.out.ser(&preview);
        return Ok(());
    }
    let keys: Vec<String> = preview
        .issues
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    eprintln!(
        "This deletes for good: {} trashed issues ({}) and {} revisions.",
        keys.len(),
        keys.join(", "),
        preview.revisions
    );
    app.confirm("Continue? A backup is written first.")?;
    app.out.ser(
        &c.prune(&PruneRequest {
            dry_run: false,
            // The server deletes only what was shown here, or fails.
            confirm_issues: Some(preview.issues.clone()),
            confirm_revisions: Some(preview.revisions),
            ..req
        })
        .await?,
    );
    Ok(())
}
