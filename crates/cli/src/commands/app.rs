//! `llmtodo app`: opens the installed desktop app from any folder.

use llmtodo_core::bundle;

use crate::{App, CliError, CliResult};

/// Opens the bundle that `scripts/publish` installed. It builds nothing.
pub fn run(app: &App) -> CliResult {
    let path = bundle::installed_bundle();
    if !path.join("Contents/Info.plist").is_file() {
        return Err(CliError::Other(format!(
            "no app is installed at {}. Run scripts/publish in the llmtodo repo first",
            path.display()
        )));
    }
    // Tests swap `open` for another command, so no window opens.
    let open = std::env::var("LLMTODO_OPEN_COMMAND").unwrap_or_else(|_| "open".into());
    let status = std::process::Command::new(&open)
        .arg(&path)
        // Our stdout carries the JSON answer only.
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| CliError::Other(format!("could not run {open}: {e}")))?;
    if !status.success() {
        return Err(CliError::Other(format!(
            "{open} {} failed with {status}",
            path.display()
        )));
    }
    app.out.value(serde_json::json!({
        "opened": path,
        "commit": bundle::bundle_commit(&path),
    }));
    Ok(())
}
