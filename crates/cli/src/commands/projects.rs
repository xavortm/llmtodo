//! Projects.

use clap::Subcommand;
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult};

#[derive(Subcommand)]
pub enum ProjectCmd {
    List {
        #[arg(long)]
        trashed: bool,
    },
    View {
        project: String,
    },
    Create {
        name: String,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long, short = 'd')]
        description: Option<String>,
        #[arg(long)]
        state: Option<ProjectState>,
        #[arg(long)]
        team: Vec<String>,
    },
    Update {
        project: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long, short = 'd')]
        description: Option<String>,
        #[arg(long)]
        state: Option<ProjectState>,
        #[arg(long)]
        add_team: Vec<String>,
        #[arg(long)]
        remove_team: Vec<String>,
    },
    /// Move a project to the trash.
    Delete {
        project: String,
    },
    /// Link a GitHub repo (`owner/name`) to a project.
    LinkRepo {
        repo: String,
        #[arg(long)]
        project: Option<String>,
    },
    UnlinkRepo {
        repo: String,
        #[arg(long)]
        project: Option<String>,
    },
}

pub async fn run(app: &App, cmd: ProjectCmd) -> CliResult {
    let c = app.client().await?;
    let need = |p: Option<String>| {
        app.project(p)
            .ok_or_else(|| CliError::Usage("pass --project or set project in .llmtodo.toml".into()))
    };
    match cmd {
        ProjectCmd::List { trashed } => app.out.ser(&c.projects(trashed).await?),
        ProjectCmd::View { project } => app.out.ser(&c.project(&project).await?),
        ProjectCmd::Create {
            name,
            slug,
            description,
            state,
            team,
        } => app.out.ser(
            &c.create_project(&CreateProject {
                name,
                slug,
                description: description.unwrap_or_default(),
                state,
                teams: team,
            })
            .await?,
        ),
        ProjectCmd::Update {
            project,
            name,
            slug,
            description,
            state,
            add_team,
            remove_team,
        } => {
            let u = UpdateProject {
                name,
                slug,
                description,
                state,
                add_teams: (!add_team.is_empty()).then_some(add_team),
                remove_teams: (!remove_team.is_empty()).then_some(remove_team),
            };
            app.out.ser(&c.update_project(&project, &u).await?)
        }
        ProjectCmd::Delete { project } => {
            app.confirm(&format!("Move project {project} to the trash?"))?;
            app.out.ser(&c.delete_project(&project).await?)
        }
        ProjectCmd::LinkRepo { repo, project } => {
            app.out.ser(&c.link_repo(&need(project)?, &repo).await?)
        }
        ProjectCmd::UnlinkRepo { repo, project } => {
            app.out.ser(&c.unlink_repo(&need(project)?, &repo).await?)
        }
    }
    Ok(())
}
