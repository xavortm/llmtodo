//! Commit lists read from a local repo, for the CLI and the app. The server
//! stores only the first and latest `HEAD` of each work session.

use std::path::Path;
use std::process::{Command, Stdio};

use llmtodo_api_types::{IssueDetail, IssueGit, PreviewBuild};

/// The most commits the git panel lists.
pub const MAX_ISSUE_COMMITS: usize = 20;

fn is_hash(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Commits from `first` to `last` as `<short hash> <subject>`, newest first,
/// read from the local repo. Returns nothing when either value is not a
/// commit hash, or git fails.
pub fn commits_between(repo: &Path, first: &str, last: &str) -> Vec<String> {
    // Stored heads come from any client, or from a pulled database. Only a
    // commit hash may reach git, and never as an option.
    if first == last || !is_hash(first) || !is_hash(last) {
        return Vec::new();
    }
    let Ok(out) = Command::new("git")
        .args([
            "log",
            "--format=%h %s",
            "--end-of-options",
            &format!("{first}..{last}"),
        ])
        .current_dir(repo)
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// The commits one work session made, from `first` to `last`, newest
/// first. The bare range also holds every commit the session pulled in,
/// such as other agents' work brought in by a fast-forward or a rebase.
/// The ref logs tell the two apart: they record each commit made in the
/// checkout, and each pull or rebase only as a move.
///
/// `own_checkout` is true when `repo` is the session's own worktree, so its
/// `HEAD` log counts. The branch log counts in any checkout of the repo.
/// When no log reaches back to `since`, the session start in Unix seconds,
/// nothing tells the two apart, and the bare range stays.
pub fn session_commits(
    repo: &Path,
    branch: Option<&str>,
    own_checkout: bool,
    first: &str,
    last: &str,
    since: i64,
) -> Vec<String> {
    session_commits_with(
        &mut RefLogs::default(),
        repo,
        branch,
        own_checkout,
        first,
        last,
        since,
    )
}

/// What one ref log says, read once per [`fill_commits`] call: the time
/// of its oldest entry, and the commits its entries made.
struct RefLogSummary {
    oldest: i64,
    made: Vec<String>,
}

/// The ref logs one [`fill_commits`] call has read, by repo and ref. The
/// sessions of one issue often share a repo and a branch.
#[derive(Default)]
struct RefLogs(std::collections::HashMap<(std::path::PathBuf, String), Option<RefLogSummary>>);

impl RefLogs {
    fn get(&mut self, repo: &Path, name: &str) -> Option<&RefLogSummary> {
        self.0
            .entry((repo.to_path_buf(), name.to_string()))
            .or_insert_with(|| {
                let log = ref_log(repo, name)?;
                Some(RefLogSummary {
                    oldest: log.iter().map(|e| e.time).min().unwrap_or(i64::MAX),
                    made: log.iter().flat_map(|e| e.made(repo)).collect(),
                })
            })
            .as_ref()
    }
}

fn session_commits_with(
    logs: &mut RefLogs,
    repo: &Path,
    branch: Option<&str>,
    own_checkout: bool,
    first: &str,
    last: &str,
    since: i64,
) -> Vec<String> {
    let range = commits_between(repo, first, last);
    if range.is_empty() {
        return range;
    }
    let mut refs: Vec<String> = Vec::new();
    if own_checkout {
        refs.push("HEAD".into());
    }
    if let Some(b) = branch.filter(|b| is_branch_name(b)) {
        refs.push(format!("refs/heads/{b}"));
    }
    let mut made: Vec<String> = Vec::new();
    let mut covered = false;
    for r in &refs {
        let Some(log) = logs.get(repo, r) else {
            continue;
        };
        covered |= log.oldest <= since;
        made.extend(log.made.iter().cloned());
    }
    if !covered {
        return range;
    }
    range
        .into_iter()
        .filter(|c| {
            let short = c.split(' ').next().unwrap_or("");
            made.iter().any(|h| h.starts_with(short))
        })
        .collect()
}

/// A branch name safe to pass to git: no option, no range, no spaces.
fn is_branch_name(b: &str) -> bool {
    !b.is_empty()
        && !b.starts_with('-')
        && !b.contains("..")
        && b.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_' | b'.'))
}

/// One entry of a ref log.
struct LogEntry {
    hash: String,
    time: i64,
    /// What moved the ref, such as `commit: fix x` or
    /// `merge origin/main: Fast-forward`.
    action: String,
}

impl LogEntry {
    /// The commits this entry made in the checkout. A commit, an amend, a
    /// cherry-pick, a revert, a merge commit and each replayed rebase step
    /// make one. A finished rebase in the branch log made every commit from
    /// its base to its tip. A fast-forward, a reset or a checkout makes none.
    fn made(&self, repo: &Path) -> Vec<String> {
        let (verb, rest) = self.action.split_once(':').unwrap_or((&self.action, ""));
        let verb = verb.trim();
        if verb.ends_with("(finish)") {
            // `rebase (finish): refs/heads/x onto <base>`.
            let Some(base) = rest.split(" onto ").nth(1).map(str::trim) else {
                return Vec::new();
            };
            if !is_hash(base) {
                return Vec::new();
            }
            return rev_list(repo, &format!("{base}..{}", self.hash));
        }
        let makes = verb.starts_with("commit")
            || verb.starts_with("cherry-pick")
            || verb.starts_with("revert")
            || ["(pick)", "(reword)", "(edit)", "(squash)", "(fixup)"]
                .iter()
                .any(|step| verb.ends_with(step))
            || (verb.starts_with("merge") && !rest.contains("Fast-forward"));
        if makes {
            vec![self.hash.clone()]
        } else {
            Vec::new()
        }
    }
}

/// The log of `name`, newest first, or nothing when the ref has no log.
fn ref_log(repo: &Path, name: &str) -> Option<Vec<LogEntry>> {
    let out = Command::new("git")
        .args([
            "reflog",
            "show",
            "--date=unix",
            "--format=%H%x09%gd%x09%gs",
            "--end-of-options",
            name,
            "--",
        ])
        .current_dir(repo)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let log: Vec<LogEntry> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let hash = parts.next()?.to_string();
            // `HEAD@{1794381374}` with `--date=unix`.
            let time = parts
                .next()?
                .rsplit_once('{')?
                .1
                .trim_end_matches('}')
                .parse()
                .ok()?;
            let action = parts.next().unwrap_or("").to_string();
            Some(LogEntry { hash, time, action })
        })
        .collect();
    (!log.is_empty()).then_some(log)
}

/// Full hashes of the commits in `range`.
fn rev_list(repo: &Path, range: &str) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .args(["rev-list", "--end-of-options", range])
        .current_dir(repo)
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The web page of a repo, from a remote URL or a linked `owner/name`.
/// Takes `git@host:owner/name.git`, `ssh://git@host[:port]/owner/name.git`
/// and `https://host/owner/name[.git]`. A bare `owner/name` means GitHub.
/// Anything else, such as a local path, has no web page.
pub fn repo_web_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let (host, path) = if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))
        .or_else(|| remote.strip_prefix("ssh://"))
        .or_else(|| remote.strip_prefix("git://"))
    {
        let (authority, path) = rest.split_once('/')?;
        // Drop the user and the port: the web page has neither.
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host, path)
    } else if let Some((user_host, path)) = remote.split_once(':') {
        // The scp-like form: `git@github.com:owner/name.git`.
        if user_host.contains('/') || path.starts_with('/') {
            return None;
        }
        (user_host.rsplit('@').next()?, path)
    } else if remote.split('/').count() == 2 && !remote.starts_with('.') {
        ("github.com", remote)
    } else {
        return None;
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    let clean = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'));
    if !host_ok || parts.len() < 2 || !parts.iter().all(|p| clean(p)) {
        return None;
    }
    Some(format!("https://{host}/{}", parts.join("/")))
}

/// The page of one commit on a repo's web page, as git hosts show it.
/// Returns nothing when `hash` is not a commit hash.
pub fn commit_url(repo_web: &str, hash: &str) -> Option<String> {
    is_hash(hash).then(|| format!("{}/commit/{hash}", repo_web.trim_end_matches('/')))
}

/// The `origin` remote URL of the repo at `repo`, when it has one.
pub fn origin_url(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// How long a remembered `origin` URL stays good. The app reads the git
/// panel on many refreshes, and a remote rarely changes.
const ORIGIN_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// [`origin_url`], remembered per repo folder for [`ORIGIN_TTL`], so a
/// burst of refreshes runs git for it once.
pub fn origin_url_cached(repo: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Instant;
    type Cache = HashMap<PathBuf, (Instant, Option<String>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let fresh = |at: &Instant| at.elapsed() < ORIGIN_TTL;
    if let Ok(cache) = CACHE.lock()
        && let Some((at, url)) = cache.as_ref().and_then(|c| c.get(repo))
        && fresh(at)
    {
        return url.clone();
    }
    let url = origin_url(repo);
    if let Ok(mut cache) = CACHE.lock() {
        let cache = cache.get_or_insert_with(HashMap::new);
        cache.retain(|_, (at, _)| fresh(at));
        cache.insert(repo.to_path_buf(), (Instant::now(), url.clone()));
    }
    url
}

/// What a session's commit list depends on. A commit range never changes,
/// so the list stays good until the session's last `HEAD` moves.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CommitsKey {
    repo: std::path::PathBuf,
    branch: Option<String>,
    own_checkout: bool,
    first: String,
    last: String,
    since: i64,
}

/// The most session commit lists [`cached_commits`] keeps.
const COMMITS_CACHE: usize = 512;

/// The commit list for `key`, read with `read` only the first time. An
/// empty list is not kept: the commits may not be fetched yet.
fn cached_commits(key: CommitsKey, read: impl FnOnce(&CommitsKey) -> Vec<String>) -> Vec<String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<CommitsKey, Vec<String>>>> = Mutex::new(None);
    if let Ok(cache) = CACHE.lock()
        && let Some(found) = cache.as_ref().and_then(|c| c.get(&key))
    {
        return found.clone();
    }
    let commits = read(&key);
    if !commits.is_empty()
        && let Ok(mut cache) = CACHE.lock()
    {
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.len() >= COMMITS_CACHE {
            cache.clear();
        }
        cache.insert(key, commits.clone());
    }
    commits
}

/// Fills each work session's commits, and the issue's git commits, from
/// local repos. Only sessions that ran on `host` count. A session reads its
/// own worktree, or `fallback` when that folder is gone, such as after the
/// worktree was removed. A session lists only the commits it made, not the
/// ones it pulled in: see [`session_commits`]. Sessions on another host get a note on their
/// worktree instead. The repo's web page comes from the `origin` remote of
/// the newest repo it read, or of `fallback`.
pub fn fill_commits(d: &mut IssueDetail, host: Option<&str>, fallback: Option<&Path>) {
    let mut all: Vec<String> = Vec::new();
    let mut web: Option<String> = None;
    let mut logs = RefLogs::default();
    for s in d.sessions.iter_mut().rev() {
        if s.host.is_some() && s.host.as_deref() == host {
            let (Some(first), Some(last)) = (&s.first_head, &s.last_head) else {
                continue;
            };
            let own = s.worktree.as_deref().map(Path::new).filter(|p| p.is_dir());
            if let Some(repo) = own.or(fallback) {
                let key = CommitsKey {
                    repo: repo.to_path_buf(),
                    branch: s.branch.clone(),
                    own_checkout: own.is_some(),
                    first: first.clone(),
                    last: last.clone(),
                    since: s.started_at.as_second(),
                };
                s.commits = cached_commits(key, |k| {
                    session_commits_with(
                        &mut logs,
                        &k.repo,
                        k.branch.as_deref(),
                        k.own_checkout,
                        &k.first,
                        &k.last,
                        k.since,
                    )
                });
                if web.is_none() && !s.commits.is_empty() {
                    web = origin_url_cached(repo).as_deref().and_then(repo_web_url);
                }
            }
            for c in &s.commits {
                if !all.contains(c) {
                    all.push(c.clone());
                }
            }
        } else if let (Some(h), Some(wt)) = (&s.host, &s.worktree) {
            s.worktree = Some(format!("{wt} (on another machine: {h})"));
        }
    }
    all.truncate(MAX_ISSUE_COMMITS);
    d.git.commits = all;
    if let Some(b) = d.preview_build.as_mut() {
        check_preview_build(b, &d.git, host, fallback);
    }
    d.git.repo_url = web.or_else(|| {
        fallback
            .and_then(origin_url_cached)
            .as_deref()
            .and_then(repo_web_url)
    });
}

/// Marks the preview build stale or fresh from the local repo, when the
/// build is on `host`: it is stale when the issue's branch no longer points
/// at the commit it was built from. The repo is the issue's worktree, or
/// `fallback`. Without a branch or a repo, the server's answer stays.
fn check_preview_build(
    b: &mut PreviewBuild,
    git: &IssueGit,
    host: Option<&str>,
    fallback: Option<&Path>,
) {
    if host.is_none() || b.host.as_deref() != host {
        return;
    }
    let Some(branch) = git.branch.as_deref() else {
        return;
    };
    let own = git
        .worktree
        .as_deref()
        .filter(|_| git.host.as_deref() == host)
        .map(Path::new)
        .filter(|p| p.is_dir());
    let Some(repo) = own.or(fallback) else {
        return;
    };
    if let Some(head) = branch_head(repo, branch) {
        b.stale = !head.eq_ignore_ascii_case(&b.commit);
    }
}

/// The commit a local branch points at, as a full hash.
pub fn branch_head(repo: &Path, branch: &str) -> Option<String> {
    // The branch name comes from the database. It reaches git only as a
    // full ref name after `--end-of-options`.
    if branch.starts_with('-') || branch.contains("..") {
        return None;
    }
    let refname = format!("refs/heads/{branch}^{{commit}}");
    git_out(
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &refname,
        ],
        repo,
    )
    .filter(|h| matches!(h.len(), 40 | 64) && is_hash(h))
}

/// Runs git in `dir` and returns its trimmed output. None when git fails
/// or prints nothing.
fn git_out(args: &[&str], dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The top folder of the repo that holds `dir`.
pub fn git_root(dir: &Path) -> Option<std::path::PathBuf> {
    git_out(&["rev-parse", "--show-toplevel"], dir).map(std::path::PathBuf::from)
}

/// The top folder of the main checkout of the repo that holds `dir`. In a
/// linked worktree it is the checkout the worktree came from, so the
/// folder stays when the worktree goes. A bare repo has no checkout.
pub fn main_checkout_root(dir: &Path) -> Option<std::path::PathBuf> {
    let common = git_out(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        dir,
    )?;
    let common = std::path::PathBuf::from(common);
    if common.file_name().is_some_and(|n| n == ".git") {
        return common.parent().map(Path::to_path_buf);
    }
    git_root(dir)
}

/// Where a folder sits in git: the repo's top folder, the branch and the
/// commit hash of `HEAD`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    pub root: std::path::PathBuf,
    /// None on a detached `HEAD`, or before the first commit.
    pub branch: Option<String>,
    /// None before the first commit.
    pub head: Option<String>,
}

/// The repo top folder, the branch and `HEAD` of `dir`, in one git call.
/// None outside a repo. `--abbrev-ref` applies to every rev after it, so
/// the plain `HEAD` must come first. Before the first commit git fails on
/// `HEAD` but has already printed the top folder, so stdout is read
/// whatever the exit status.
pub fn checkout(dir: &Path) -> Option<Checkout> {
    let out = Command::new("git")
        .args([
            "rev-parse",
            "--show-toplevel",
            "HEAD",
            "--abbrev-ref",
            "HEAD",
        ])
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    parse_checkout(&String::from_utf8_lossy(&out.stdout))
}

fn parse_checkout(out: &str) -> Option<Checkout> {
    let mut lines = out.lines().map(str::trim);
    let root = lines.next().filter(|r| r.starts_with('/'))?;
    let head = lines.next().filter(|h| is_hash(h)).map(str::to_string);
    let branch = head
        .as_ref()
        .and(lines.next())
        .filter(|b| !b.is_empty() && *b != "HEAD")
        .map(str::to_string);
    Some(Checkout {
        root: root.into(),
        branch,
        head,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_reads_the_root_the_hash_and_the_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(checkout(&root), None, "outside a repo");
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "feat/x"]);
        std::fs::create_dir(root.join("sub")).unwrap();
        let unborn = checkout(&root.join("sub")).unwrap();
        assert_eq!(
            unborn,
            Checkout {
                root: root.clone(),
                branch: None,
                head: None
            }
        );
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "a",
        ]);
        let c = checkout(&root.join("sub")).unwrap();
        assert_eq!(c.root, root);
        assert_eq!(c.branch.as_deref(), Some("feat/x"));
        let head = c.head.unwrap();
        assert_eq!(head.len(), 40, "{head}");
        assert!(head.chars().all(|c| c.is_ascii_hexdigit()));

        run(&["checkout", "-q", "--detach"]);
        let c = checkout(&root).unwrap();
        assert_eq!(c.branch, None);
        assert_eq!(c.head.map(|h| h.len()), Some(40));
    }

    #[test]
    fn repo_web_url_reads_ssh_https_and_owner_name() {
        let gh = Some("https://github.com/xavortm/llmtodo".to_string());
        for remote in [
            "git@github.com:xavortm/llmtodo.git",
            "git@github.com:xavortm/llmtodo",
            "ssh://git@github.com/xavortm/llmtodo.git",
            "ssh://git@github.com:22/xavortm/llmtodo.git",
            "https://github.com/xavortm/llmtodo.git",
            "https://github.com/xavortm/llmtodo",
            "https://github.com/xavortm/llmtodo/",
            "https://user@github.com/xavortm/llmtodo.git",
            "xavortm/llmtodo",
            " git@github.com:xavortm/llmtodo.git\n",
        ] {
            assert_eq!(repo_web_url(remote), gh, "{remote}");
        }
        assert_eq!(
            repo_web_url("git@gitlab.com:group/sub/proj.git").as_deref(),
            Some("https://gitlab.com/group/sub/proj")
        );
        for remote in [
            "",
            "/Users/me/repo",
            "./repo",
            "../a/b",
            "file:///tmp/repo",
            "C:/repos/x",
            "llmtodo",
            "https://github.com/",
            "https://github.com/onlyowner",
            "git@github.com:/abs/path",
            "https://github.com/o/r\"onclick",
        ] {
            assert_eq!(repo_web_url(remote), None, "{remote}");
        }
    }

    #[test]
    fn origin_url_reads_the_remote_and_none_without_one() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        assert_eq!(origin_url(repo.path()), None);
        git(&["remote", "add", "origin", "git@github.com:o/r.git"]);
        let web = origin_url(repo.path()).as_deref().and_then(repo_web_url);
        assert_eq!(web.as_deref(), Some("https://github.com/o/r"));
    }

    #[test]
    fn the_cached_origin_url_runs_git_once_per_repo() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "git@github.com:o/one.git"]);
        let first = origin_url_cached(repo.path());
        assert_eq!(first.as_deref(), Some("git@github.com:o/one.git"));
        // A second read inside the time limit does not run git again.
        git(&["remote", "set-url", "origin", "git@github.com:o/two.git"]);
        assert_eq!(origin_url_cached(repo.path()), first);
        assert_eq!(
            origin_url(repo.path()).as_deref(),
            Some("git@github.com:o/two.git")
        );
    }

    #[test]
    fn commit_url_needs_a_hash() {
        let web = "https://github.com/o/r";
        assert_eq!(
            commit_url(web, "4c87f0c").as_deref(),
            Some("https://github.com/o/r/commit/4c87f0c")
        );
        assert_eq!(commit_url(web, "main"), None);
        assert_eq!(commit_url(web, "--help"), None);
    }

    /// A stored head such as `--output=<path>` must not reach git as an
    /// option, or `issue view` would write the log to that path.
    #[test]
    fn commits_between_ignores_non_hashes() {
        let repo = tempfile::tempdir().unwrap();
        let ok = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        ok(&["init", "-q"]);
        ok(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "one",
        ]);
        let target = repo.path().join("planted");
        let bad = format!("--output={}", target.display());
        assert!(commits_between(repo.path(), &bad, "HEAD").is_empty());
        assert!(commits_between(repo.path(), "HEAD", &bad).is_empty());
        let planted: Vec<_> = std::fs::read_dir(repo.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("planted"))
            .collect();
        assert!(
            planted.is_empty(),
            "git wrote to a planted path: {planted:?}"
        );
    }

    /// A second read of the same session range comes from the cache, and
    /// a moved last `HEAD` reads git again.
    #[test]
    fn session_commits_are_cached_by_range() {
        let r = Repo::new();
        let first = r.head(".");
        let last = r.commit(".", "one");
        let key = |last: &str| CommitsKey {
            repo: r.dir("."),
            branch: Some("main".into()),
            own_checkout: true,
            first: first.clone(),
            last: last.to_string(),
            since: 0,
        };
        let reads = std::cell::Cell::new(0);
        let read = |k: &CommitsKey| {
            reads.set(reads.get() + 1);
            session_commits(
                &k.repo,
                k.branch.as_deref(),
                k.own_checkout,
                &k.first,
                &k.last,
                k.since,
            )
        };
        let a = cached_commits(key(&last), read);
        let b = cached_commits(key(&last), read);
        assert_eq!(a, b);
        assert_eq!(a.len(), 1, "{a:?}");
        assert_eq!(reads.get(), 1);
        let newer = r.commit(".", "two");
        assert_eq!(cached_commits(key(&newer), read).len(), 2);
        assert_eq!(reads.get(), 2);
    }

    struct Repo(tempfile::TempDir);

    impl Repo {
        fn new() -> Self {
            let r = Repo(tempfile::tempdir().unwrap());
            r.git(".", &["init", "-q", "-b", "main"]);
            r.commit(".", "base");
            r
        }
        fn dir(&self, sub: &str) -> std::path::PathBuf {
            self.0.path().join(sub)
        }
        fn git(&self, sub: &str, args: &[&str]) -> String {
            let out = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(self.dir(sub))
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        fn commit(&self, sub: &str, msg: &str) -> String {
            self.git(sub, &["commit", "-q", "--allow-empty", "-m", msg]);
            self.head(sub)
        }
        fn head(&self, sub: &str) -> String {
            self.git(sub, &["rev-parse", "HEAD"])
        }
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn subjects(commits: &[String]) -> Vec<&str> {
        commits
            .iter()
            .map(|c| c.split_once(' ').map_or("", |(_, s)| s))
            .collect()
    }

    /// LLM-140: a session in the main checkout writes once, other agents
    /// land work, the session pulls it in with a fast-forward and writes
    /// again. Only the commit the session made itself belongs to it.
    #[test]
    fn the_main_checkout_root_skips_linked_worktrees() {
        let r = Repo::new();
        r.git(".", &["worktree", "add", "-q", "-b", "side", "wt"]);
        let main = r.dir(".").canonicalize().unwrap();
        let wt = r.dir("wt").canonicalize().unwrap();
        assert_eq!(main_checkout_root(&r.dir(".")), Some(main.clone()));
        assert_eq!(main_checkout_root(&r.dir("wt")), Some(main));
        assert_eq!(git_root(&r.dir("wt")), Some(wt));
    }

    #[test]
    fn a_session_on_main_lists_only_its_own_commits() {
        let r = Repo::new();
        let since = now_secs();
        let first = r.head(".");
        r.git(".", &["worktree", "add", "-q", "-b", "other", "wt"]);
        r.commit("wt", "other agent one");
        r.git(".", &["merge", "-q", "--ff-only", "other"]);
        r.commit(".", "own work");
        r.git("wt", &["rebase", "-q", "main"]);
        r.commit("wt", "other agent two");
        r.git(".", &["merge", "-q", "--ff-only", "other"]);
        let last = r.head(".");

        let plain = commits_between(&r.dir("."), &first, &last);
        assert_eq!(plain.len(), 3, "the bare range holds every commit");
        let own = session_commits(&r.dir("."), Some("main"), true, &first, &last, since);
        assert_eq!(subjects(&own), ["own work"]);
    }

    /// A worktree session rebases onto a main that moved. The commits the
    /// rebase brings in belong to others; the replayed ones are its own.
    /// This holds while the worktree exists, and after it is removed, from
    /// the branch log alone.
    #[test]
    fn a_rebased_worktree_lists_only_its_own_commits() {
        let r = Repo::new();
        let since = now_secs();
        r.git(".", &["worktree", "add", "-q", "-b", "feat", "wt"]);
        let first = r.head("wt");
        r.commit("wt", "feat one");
        r.commit("wt", "feat two");
        r.commit(".", "landed by another agent");
        r.git("wt", &["rebase", "-q", "main"]);
        let last = r.head("wt");

        assert_eq!(commits_between(&r.dir("wt"), &first, &last).len(), 3);
        let own = session_commits(&r.dir("wt"), Some("feat"), true, &first, &last, since);
        assert_eq!(subjects(&own), ["feat two", "feat one"]);

        r.git(".", &["worktree", "remove", "--force", "wt"]);
        let own = session_commits(&r.dir("."), Some("feat"), false, &first, &last, since);
        assert_eq!(subjects(&own), ["feat two", "feat one"]);
    }

    /// With no log of the branch, such as a deleted branch, nothing tells
    /// own commits from pulled ones. The bare range stays, as before.
    #[test]
    fn without_a_ref_log_the_range_stays() {
        let r = Repo::new();
        r.git(".", &["worktree", "add", "-q", "-b", "gone", "wt"]);
        let first = r.head("wt");
        r.commit("wt", "gone one");
        let last = r.head("wt");
        r.git(".", &["worktree", "remove", "--force", "wt"]);
        r.git(".", &["branch", "-q", "-D", "gone"]);
        let own = session_commits(&r.dir("."), Some("gone"), false, &first, &last, 0);
        assert_eq!(subjects(&own), ["gone one"]);
        // A log that starts after the session started cannot speak for it.
        let own = session_commits(&r.dir("."), Some("main"), false, &first, &last, 0);
        assert_eq!(subjects(&own), ["gone one"]);
    }

    /// LLM-590: on the build's own machine, the branch in the repo decides
    /// whether the preview build is stale, over the server's answer.
    #[test]
    fn preview_build_is_stale_once_the_branch_moves_on() {
        let r = Repo::new();
        let built = r.head(".");
        let git = IssueGit {
            branch: Some("main".into()),
            worktree: Some(r.dir(".").to_string_lossy().into_owned()),
            host: Some("me".into()),
            ..Default::default()
        };
        let build = |stale| PreviewBuild {
            path: "/p/llmtodo-app".into(),
            commit: built.clone(),
            host: Some("me".into()),
            built_at: jiff::Timestamp::UNIX_EPOCH,
            stale,
        };
        let mut b = build(true);
        check_preview_build(&mut b, &git, Some("me"), None);
        assert!(!b.stale, "the branch still points at the build");

        r.commit(".", "after the build");
        check_preview_build(&mut b, &git, Some("me"), None);
        assert!(b.stale, "the branch moved on");

        // Another machine's build keeps the server's answer.
        let mut b = build(false);
        check_preview_build(&mut b, &git, Some("other"), None);
        assert!(!b.stale);
        // So does a branch that is not in the repo.
        let gone = IssueGit {
            branch: Some("gone".into()),
            ..git.clone()
        };
        check_preview_build(&mut b, &gone, Some("me"), None);
        assert!(!b.stale);
        assert_eq!(branch_head(&r.dir("."), "--all"), None);
    }
}
