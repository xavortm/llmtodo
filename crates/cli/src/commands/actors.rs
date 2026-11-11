//! Actors.

use clap::Subcommand;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum ActorCmd {
    List,
    /// The actor this CLI acts as, as the server sees it.
    Me,
}

pub async fn run(app: &App, cmd: ActorCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        ActorCmd::List => app.out.ser(&c.actors().await?.actors),
        ActorCmd::Me => {
            let me = c.me().await?;
            app.out.value(
                serde_json::json!({ "actor": me, "context": app.detected.identity.context }),
            );
        }
    }
    Ok(())
}
