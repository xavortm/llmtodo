//! Sets `LLMTODO_BUILD_COMMIT` to the git commit the app is built from, with
//! `-dirty` when the tree had changes. `scripts/publish` passes the value in
//! the environment, so the bundle and its stamp file always agree.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=LLMTODO_BUILD_COMMIT");
    let commit = match std::env::var("LLMTODO_BUILD_COMMIT") {
        Ok(c) if !c.is_empty() => c,
        _ => from_git(),
    };
    println!("cargo:rustc-env=LLMTODO_BUILD_COMMIT={commit}");
}

fn from_git() -> String {
    // Rerun when HEAD moves, a commit lands or a source file changes. A
    // source change rebuilds the crates anyway, so the dirty check is cheap.
    // A missing path would rerun this script on every build, so only paths
    // that exist count. A packed ref changes `packed-refs`.
    let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
    let names = ["HEAD", "index", "packed-refs"]
        .into_iter()
        .map(String::from);
    for name in names.chain(head_ref) {
        if let Some(path) = git(&["rev-parse", "--git-path", &name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    println!("cargo:rerun-if-changed={}", crates.display());
    let Some(head) = git(&["rev-parse", "HEAD"]) else {
        return "unknown".into();
    };
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    if dirty { format!("{head}-dirty") } else { head }
}
