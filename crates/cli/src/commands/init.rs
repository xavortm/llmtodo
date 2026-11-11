//! `llmtodo init`: writes `.llmtodo.toml` at the git root, and installs the
//! agent skill.

use std::io::Write as _;
use std::path::PathBuf;

use clap::Args;
use llmtodo_api_types::*;
use serde::Serialize;
use serde_json::json;

use crate::commands::skill;
use crate::context::{RepoConfig, git_root};
use crate::{App, CliError, CliResult};

#[derive(Args)]
pub struct InitArgs {
    /// Team key, such as `ABC`. Asks on a terminal when left out.
    #[arg(long)]
    pub team: Option<String>,
    /// Name for a new team. Defaults to the key.
    #[arg(long)]
    pub team_name: Option<String>,
    /// Project slug or name. Asks on a terminal when left out. Without a
    /// terminal, the file names no project.
    #[arg(long)]
    pub project: Option<String>,
    /// Name for a new project. Defaults to the --project value.
    #[arg(long)]
    pub project_name: Option<String>,
    /// Create the team or the project when it does not exist.
    #[arg(long)]
    pub create: bool,
    /// Replace an existing .llmtodo.toml.
    #[arg(long)]
    pub force: bool,
    /// Do not install the skill in ~/.claude/skills/llmtodo.
    #[arg(long)]
    pub no_skill: bool,
}

#[derive(Serialize)]
struct FileConfig<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    team: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<&'a str>,
}

/// Reads one line from the terminal, after a prompt on stderr.
fn ask(question: &str) -> CliResult<String> {
    eprint!("{question} ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn yes(question: &str) -> CliResult<bool> {
    let a = ask(&format!("{question} [y/N]"))?;
    Ok(matches!(a.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// A choice from a numbered list, or free text. Empty means `default`.
fn pick(title: &str, items: &[String], default: Option<&str>) -> CliResult<Option<String>> {
    eprintln!("{title}");
    for (i, item) in items.iter().enumerate() {
        eprintln!("  {}. {item}", i + 1);
    }
    let hint = match default {
        Some(d) => format!("Number or new value [{d}]:"),
        None => "Number or new value, empty to skip:".to_string(),
    };
    let a = ask(&hint)?;
    if a.is_empty() {
        return Ok(default.map(str::to_string));
    }
    if let Ok(n) = a.parse::<usize>()
        && (1..=items.len()).contains(&n)
    {
        return Ok(Some(items[n - 1].clone()));
    }
    Ok(Some(a))
}

pub async fn run(app: &App, a: InitArgs) -> CliResult {
    let cwd = std::env::current_dir()?;
    let root = git_root(&cwd).unwrap_or(cwd);
    let path = root.join(".llmtodo.toml");
    let interactive = app.interactive();

    let existing: Option<RepoConfig> = if path.is_file() {
        let text = std::fs::read_to_string(&path)?;
        Some(
            toml::from_str(&text)
                .map_err(|e| CliError::Usage(format!("cannot read {}: {e}", path.display())))?,
        )
    } else {
        None
    };

    // An existing file stays, unless --force or a yes on the terminal.
    if let Some(old) = &existing
        && !a.force
    {
        let same = |flag: &Option<String>, have: &Option<String>| {
            flag.as_ref()
                .is_none_or(|f| have.as_ref().is_some_and(|h| h.eq_ignore_ascii_case(f)))
        };
        let matches = same(&a.team, &old.team) && same(&a.project, &old.project);
        let replace = !matches || (interactive && a.team.is_none() && a.project.is_none());
        let replace = replace
            && if interactive {
                yes(&format!(
                    "{} already names team {} and project {}. Replace it?",
                    path.display(),
                    old.team.as_deref().unwrap_or("(none)"),
                    old.project.as_deref().unwrap_or("(none)")
                ))?
            } else {
                return Err(CliError::Usage(format!(
                    "{} already names team {} and project {}; pass --force to replace it",
                    path.display(),
                    old.team.as_deref().unwrap_or("(none)"),
                    old.project.as_deref().unwrap_or("(none)")
                )));
            };
        if !replace {
            let skill = install_skill(&a)?;
            app.out.value(json!({
                "path": path,
                "changed": false,
                "team": old.team,
                "project": old.project,
                "created_team": false,
                "created_project": false,
                "skill": skill,
            }));
            return Ok(());
        }
    }

    let c = app.client().await?;

    // Team.
    let teams = c.teams().await?;
    let team_key = match a.team.clone() {
        Some(t) => t,
        None if interactive => {
            let keys: Vec<String> = teams
                .iter()
                .map(|t| format!("{} ({})", t.key, t.name))
                .collect();
            let first = teams.first().map(|t| t.key.as_str());
            pick("Teams:", &keys, first)?
                .map(|s| s.split(' ').next().unwrap_or_default().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CliError::Usage("a team key is needed".into()))?
        }
        None if teams.len() == 1 => teams[0].key.clone(),
        None => {
            let keys: Vec<&str> = teams.iter().map(|t| t.key.as_str()).collect();
            return Err(CliError::InputNeeded(if keys.is_empty() {
                "no team exists; pass --team <KEY> --create".into()
            } else {
                format!("pass --team; teams: {}", keys.join(", "))
            }));
        }
    };
    let found = teams
        .iter()
        .find(|t| t.key.eq_ignore_ascii_case(&team_key) || t.name.eq_ignore_ascii_case(&team_key));
    let mut created_team = false;
    let team = match found {
        Some(t) => t.key.clone(),
        None => {
            let allowed = a.create || (interactive && yes(&format!("Create team {team_key}?"))?);
            if !allowed {
                return Err(CliError::Usage(format!(
                    "team {team_key} does not exist; pass --create to create it"
                )));
            }
            let name = match a.team_name.clone() {
                Some(n) => n,
                None if interactive => {
                    let n = ask(&format!("Team name [{team_key}]:"))?;
                    if n.is_empty() { team_key.clone() } else { n }
                }
                None => team_key.clone(),
            };
            created_team = true;
            c.create_team(&CreateTeam {
                key: team_key.to_ascii_uppercase(),
                name,
            })
            .await?
            .key
        }
    };

    // Project.
    let projects = c.projects(false).await?;
    let project_ref = match a.project.clone() {
        Some(p) => Some(p),
        None if interactive => {
            let ours: Vec<&Project> = projects
                .iter()
                .filter(|p| p.teams.iter().any(|t| t.eq_ignore_ascii_case(&team)))
                .collect();
            let shown: Vec<&Project> = if ours.is_empty() {
                projects.iter().collect()
            } else {
                ours
            };
            let slugs: Vec<String> = shown
                .iter()
                .map(|p| format!("{} ({})", p.slug, p.name))
                .collect();
            let default = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| shown.iter().any(|p| p.slug == *n));
            pick("Projects:", &slugs, default.as_deref())?
                .map(|s| s.split(" (").next().unwrap_or_default().to_string())
                .filter(|s| !s.is_empty())
        }
        None => None,
    };
    let mut created_project = false;
    let project = match project_ref {
        None => None,
        Some(r) => {
            let found = projects
                .iter()
                .find(|p| p.slug.eq_ignore_ascii_case(&r) || p.name.eq_ignore_ascii_case(&r));
            match found {
                Some(p) => Some(p.slug.clone()),
                None => {
                    let allowed =
                        a.create || (interactive && yes(&format!("Create project {r}?"))?);
                    if !allowed {
                        return Err(CliError::Usage(format!(
                            "project {r} does not exist; pass --create to create it"
                        )));
                    }
                    let name = match a.project_name.clone() {
                        Some(n) => n,
                        None if interactive => {
                            let n = ask(&format!("Project name [{r}]:"))?;
                            if n.is_empty() { r.clone() } else { n }
                        }
                        None => r.clone(),
                    };
                    created_project = true;
                    let slug_like = r
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
                    Some(
                        c.create_project(&CreateProject {
                            name,
                            slug: slug_like.then(|| r.to_ascii_lowercase()),
                            description: String::new(),
                            state: None,
                            teams: vec![team.clone()],
                        })
                        .await?
                        .slug,
                    )
                }
            }
        }
    };

    // Keep the actor setting of an old file.
    let actor = existing.as_ref().and_then(|e| e.actor.clone());
    let body = toml::to_string(&FileConfig {
        team: Some(&team),
        project: project.as_deref(),
        actor: actor.as_deref(),
    })
    .map_err(|e| CliError::Other(e.to_string()))?;
    let text = format!("# llmtodo settings for this repo, written by `llmtodo init`.\n{body}");
    let changed = std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str());
    if changed {
        std::fs::write(&path, &text)?;
    }

    let skill = install_skill(&a)?;
    app.out.value(json!({
        "path": path,
        "changed": changed,
        "team": team,
        "project": project,
        "created_team": created_team,
        "created_project": created_project,
        "skill": skill,
    }));
    Ok(())
}

/// Installs the skill, unless `--no-skill`. Kept files do not fail the
/// init, but print a note on stderr.
fn install_skill(a: &InitArgs) -> CliResult<serde_json::Value> {
    if a.no_skill {
        return Ok(serde_json::Value::Null);
    }
    let dir: PathBuf = skill::skill_dir()?;
    let report = skill::install(&dir, false)?;
    if report.kept() > 0 {
        eprintln!(
            "note: {}. Run `llmtodo skill install --force`.",
            report.kept_message()
        );
    }
    Ok(serde_json::to_value(&report).unwrap_or_default())
}
