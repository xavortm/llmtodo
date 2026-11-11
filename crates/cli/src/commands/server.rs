use clap::Subcommand;
use llmtodo_api_types::DEFAULT_PORT;

use crate::{App, CliError, CliResult};

const LAUNCHD_LABEL: &str = "dev.llmtodo.server";

#[derive(Subcommand)]
pub enum ServerCmd {
    /// Run the server in the foreground.
    Run {
        /// Port to bind. 0 picks a free one; `server.json` records it.
        #[arg(long, env = "LLMTODO_PORT", default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Do not poll GitHub. Previews and test copies use this.
        #[arg(long, env = "LLMTODO_NO_GITHUB", value_parser = clap::builder::BoolishValueParser::new(), action = clap::ArgAction::SetTrue)]
        no_github: bool,
    },
    /// Stop the running server.
    Stop,
    /// Show whether the server runs, and its version.
    Status,
    /// Write a database backup now.
    Backup,
    /// Start the server at login with a launchd agent.
    Install,
    /// Remove the launchd agent.
    Uninstall,
}

fn plist_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(format!("Library/LaunchAgents/{LAUNCHD_LABEL}.plist"))
}

fn uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub async fn run(app: &App, cmd: ServerCmd) -> CliResult {
    match cmd {
        ServerCmd::Run { port, no_github } => {
            let _guard = llmtodo_server::init_logging(&app.data_dir);
            llmtodo_server::run(llmtodo_server::Config {
                data_dir: app.data_dir.clone(),
                port,
                host: None,
                github_poll: !no_github,
            })
            .await?;
        }
        ServerCmd::Stop => {
            // Only stop the server that wrote this folder's server.json, never
            // a server for another folder that took a stale port.
            let Some((c, _)) =
                llmtodo_client::find_server(&app.data_dir, &app.detected.identity.context).await
            else {
                app.out.value(serde_json::json!({ "running": false }));
                return Ok(());
            };
            match c.stop_server().await {
                Ok(v) => app.out.value(v),
                Err(llmtodo_client::ClientError::Unreachable(_)) => {
                    app.out.value(serde_json::json!({ "running": false }))
                }
                Err(e) => return Err(e.into()),
            }
        }
        ServerCmd::Status => {
            let info = llmtodo_client::read_server_info(&app.data_dir);
            let health = llmtodo_client::find_server(&app.data_dir, &app.detected.identity.context)
                .await
                .map(|(_, h)| h);
            app.out.value(serde_json::json!({
                "running": health.is_some(),
                "data_dir": app.data_dir,
                "server": info,
                "health": health,
                "cli_version": llmtodo_api_types::VERSION,
                "launchd": plist_path().exists(),
            }));
        }
        ServerCmd::Backup => app.out.ser(&app.client().await?.backup().await?),
        ServerCmd::Install => {
            let exe = std::env::current_exe()?;
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array><string>{exe}</string><string>server</string><string>run</string></array>
    <key>EnvironmentVariables</key>
    <dict><key>LLMTODO_DATA_DIR</key><string>{data}</string></dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
    <key>StandardErrorPath</key><string>{data}/server-stderr.log</string>
</dict>
</plist>
"#,
                exe = exe.display(),
                data = app.data_dir.display()
            );
            let path = plist_path();
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, plist)?;
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &format!("gui/{}/{LAUNCHD_LABEL}", uid())])
                .output();
            let out = std::process::Command::new("launchctl")
                .args([
                    "bootstrap",
                    &format!("gui/{}", uid()),
                    &path.display().to_string(),
                ])
                .output()?;
            if !out.status.success() {
                return Err(CliError::Other(format!(
                    "launchctl bootstrap failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            app.out.value(serde_json::json!({ "installed": path }));
        }
        ServerCmd::Uninstall => {
            let path = plist_path();
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &format!("gui/{}/{LAUNCHD_LABEL}", uid())])
                .output();
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            app.out.value(serde_json::json!({ "uninstalled": path }));
        }
    }
    Ok(())
}
