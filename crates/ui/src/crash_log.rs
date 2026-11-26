//! Writes a panic to `logs/app-panic.log` in the data folder. Started from
//! Finder, the app has no terminal, so without this a panic leaves no
//! trace: it is not a signal, so macOS writes no crash report either.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Installs the hook. Each panic appends the time, the thread, the
/// message, the place and a backtrace, then runs the default hook, which
/// still prints to stderr.
pub fn install() {
    let path = llmtodo_client::data_dir::resolve(None)
        .join("logs")
        .join("app-panic.log");
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = append(&path, &report(info));
        default(info);
    }));
}

/// The log file for the data folder `data_dir`.
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("app-panic.log")
}

fn report(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let place = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    format!(
        "--- {} llmtodo-app {} panicked on thread {}\n{message}\nat {place}\n{}\n",
        jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S %Z"),
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("unnamed"),
        std::backtrace::Backtrace::force_capture(),
    )
}

fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_panic_lands_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = super::path_in(dir.path());
        super::append(&path, "first\n").unwrap();
        super::append(&path, "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
    }
}
