//! Work sessions.

use std::path::PathBuf;

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult, read_text};

#[derive(Subcommand)]
pub enum SessionCmd {
    /// List the tickets a session touched. Defaults to this session.
    Show { id: Option<String> },
    /// Add a note or a link to a session, such as a plan file.
    Note {
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// Tie the note to one ticket.
        #[arg(long)]
        issue: Option<String>,
        /// Session ID. Defaults to this session.
        #[arg(long)]
        session: Option<String>,
    },
}

fn this_session(app: &App, id: Option<String>) -> CliResult<String> {
    id.or_else(|| {
        app.detected
            .identity
            .context
            .session_id
            .clone()
            .filter(|_| !app.detected.identity.context.session_generated)
    })
    .ok_or_else(|| CliError::Usage("no session ID; pass one or set LLMTODO_SESSION".into()))
}

pub async fn run(app: &App, cmd: SessionCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        SessionCmd::Show { id } => app.out.ser(&c.session(&this_session(app, id)?).await?),
        SessionCmd::Note {
            body,
            body_file,
            issue,
            session,
        } => {
            let body = read_text(body, body_file)?
                .ok_or_else(|| CliError::Usage("pass the note text or --body-file".into()))?;
            let id = this_session(app, session)?;
            app.out.ser(
                &c.add_session_note(&id, &CreateSessionNote { body, issue })
                    .await?,
            );
        }
    }
    Ok(())
}
