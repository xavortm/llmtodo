//! Raw API calls.

use std::path::PathBuf;

use clap::Args;

use crate::{App, CliError, CliResult, read_text};

#[derive(Args)]
pub struct ApiArgs {
    /// GET, POST, PATCH or DELETE.
    pub method: String,
    /// Path such as `/v1/teams`.
    pub path: String,
    /// JSON body.
    #[arg(long)]
    pub body: Option<String>,
    #[arg(long)]
    pub body_file: Option<PathBuf>,
}

pub async fn run(app: &App, a: ApiArgs) -> CliResult {
    let c = app.client().await?;
    let body = match read_text(a.body, a.body_file)? {
        Some(t) => Some(
            serde_json::from_str(&t)
                .map_err(|e| CliError::Usage(format!("body is not JSON: {e}")))?,
        ),
        None => None,
    };
    let path = if a.path.starts_with('/') {
        a.path
    } else {
        format!("/{}", a.path)
    };
    app.out.value(c.raw(&a.method, &path, body).await?);
    Ok(())
}
