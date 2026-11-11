//! Who is calling and from where. The client's `identity` module finds the
//! actor, the session and the usage. This module adds the repo's
//! `.llmtodo.toml`, which only the CLI reads.

use std::path::{Path, PathBuf};

use llmtodo_client::identity::{self, Identity};
use serde::Deserialize;

pub use llmtodo_client::git::git_root;

/// `.llmtodo.toml`, found in the current folder or a parent up to the git
/// root.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct RepoConfig {
    pub team: Option<String>,
    pub project: Option<String>,
    /// Same format as `LLMTODO_ACTOR`. A human actor here is ignored when
    /// an agent is detected.
    pub actor: Option<String>,
}

pub fn repo_config(cwd: &Path, root: Option<&Path>) -> (RepoConfig, Option<PathBuf>) {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let p = d.join(".llmtodo.toml");
        if p.is_file() {
            let cfg = std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
                .unwrap_or_default();
            return (cfg, Some(p));
        }
        if root == Some(d) {
            break;
        }
        dir = d.parent();
    }
    (RepoConfig::default(), None)
}

pub struct Detected {
    pub identity: Identity,
    pub repo: RepoConfig,
    pub repo_root: Option<PathBuf>,
}

pub fn detect(cwd: &Path) -> Result<Detected, String> {
    let root = git_root(cwd);
    let (repo, _) = repo_config(cwd, root.as_deref());
    let identity = identity::detect(cwd, root.as_deref(), repo.actor.as_deref())?;
    Ok(Detected {
        identity,
        repo,
        repo_root: root,
    })
}
