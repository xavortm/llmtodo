//! Stored files.

use std::path::PathBuf;

use clap::Subcommand;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum FileCmd {
    /// Save a stored file. Writes to stdout without --output.
    Get {
        hash: String,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
}

pub async fn run(app: &App, cmd: FileCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        FileCmd::Get { hash, output } => {
            let bytes = c.file(&hash).await?;
            match output {
                Some(p) => std::fs::write(&p, &bytes)?,
                None => std::io::Write::write_all(&mut std::io::stdout(), &bytes)?,
            }
        }
    }
    Ok(())
}
