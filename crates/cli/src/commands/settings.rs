//! Workspace settings.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Subcommand)]
pub enum SettingsCmd {
    Show,
    Set {
        #[arg(long)]
        workspace_name: Option<String>,
        /// Team key, or `none`.
        #[arg(long)]
        default_team: Option<String>,
        #[arg(long)]
        github_poll_seconds: Option<u32>,
        /// Folder for `sync push` and `sync pull`, such as a Dropbox folder.
        #[arg(long)]
        sync_folder: Option<String>,
        /// USD per million tokens: `model=input,output[,cache_write_5m,cache_write_1h,cache_read]`.
        /// Cache prices default to 1.25x, 2x and 0.1x input. Repeat for several.
        #[arg(long)]
        model_price: Vec<String>,
    },
}

pub async fn run(app: &App, cmd: SettingsCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        SettingsCmd::Show => app.out.ser(&c.settings().await?),
        SettingsCmd::Set {
            workspace_name,
            default_team,
            github_poll_seconds,
            sync_folder,
            model_price,
        } => {
            let mut prices = std::collections::BTreeMap::new();
            for p in &model_price {
                let (model, nums) = p.split_once('=').ok_or_else(|| {
                    CliError::Usage(format!("expected model=input,output, got {p:?}"))
                })?;
                let n: Vec<f64> = nums
                    .split(',')
                    .map(|x| x.trim().parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| CliError::Usage(format!("prices must be numbers: {p:?}")))?;
                if n.len() < 2 || n.len() > 5 {
                    return Err(CliError::Usage(format!(
                        "expected 2 to 5 prices, got {}: {p:?}",
                        n.len()
                    )));
                }
                let at = |i: usize, d: f64| n.get(i).copied().unwrap_or(d);
                prices.insert(
                    model.trim().to_string(),
                    ModelPrice {
                        input: n[0],
                        output: n[1],
                        cache_write_5m: at(2, n[0] * 1.25),
                        cache_write_1h: at(3, n[0] * 2.0),
                        cache_read: at(4, n[0] * 0.1),
                    },
                );
            }
            let model_prices = (!prices.is_empty()).then_some(prices);
            let default_team = default_team.map(|t| {
                if t.eq_ignore_ascii_case("none") {
                    String::new()
                } else {
                    t
                }
            });
            app.out.ser(
                &c.update_settings(&UpdateSettings {
                    workspace_name,
                    default_team,
                    github_poll_seconds,
                    sync_folder,
                    model_prices,
                })
                .await?,
            )
        }
    }
    Ok(())
}
