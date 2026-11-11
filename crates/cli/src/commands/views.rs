//! Saved views.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliResult};

#[derive(Subcommand)]
pub enum ViewCmd {
    List,
    /// Save the filters of `issue query` under a name.
    Create {
        name: String,
        #[command(flatten)]
        query: super::issue::QueryArgs,
    },
    Update {
        #[arg(value_name = "VIEW")]
        target: String,
        #[arg(long)]
        name: Option<String>,
        /// Replace the saved filters with these.
        #[arg(long)]
        replace_query: bool,
        #[command(flatten)]
        query: super::issue::QueryArgs,
    },
    Delete {
        view: String,
    },
}

pub async fn run(app: &App, cmd: ViewCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        ViewCmd::List => app.out.ser(&c.views().await?),
        ViewCmd::Create { name, query } => {
            let mut q = query.into_query(app);
            q.limit = None;
            app.out
                .ser(&c.create_view(&CreateView { name, query: q }).await?)
        }
        ViewCmd::Update {
            target,
            name,
            replace_query,
            query,
        } => {
            let q = replace_query.then(|| query.into_query(app));
            app.out.ser(
                &c.update_view(&target, &UpdateView { name, query: q })
                    .await?,
            )
        }
        ViewCmd::Delete { view } => app.out.ser(&c.delete_view(&view).await?),
    }
    Ok(())
}
