//! `llmtodo`: the CLI for agents and people, and the server binary.

mod commands;
mod context;
mod output;

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use llmtodo_client::ClientError;
use llmtodo_client::types::ErrorCode;

use crate::output::Out;

#[derive(Parser)]
#[command(
    name = "llmtodo",
    version,
    about = "Linear-style task tracker for people and LLM agents",
    propagate_version = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Args, Clone)]
pub struct Global {
    /// Print JSON. The default when stdout is not a terminal.
    #[arg(long, global = true, conflicts_with = "human")]
    pub json: bool,
    /// Print tables and text for a person.
    #[arg(long, global = true)]
    pub human: bool,
    /// Keep only these JSON fields, such as `key,title,status.name`.
    #[arg(long, global = true)]
    pub fields: Option<String>,
    /// Fail instead of asking a question.
    #[arg(long, global = true)]
    pub no_input: bool,
    /// Answer yes to confirmations.
    #[arg(short = 'y', long = "yes", global = true)]
    pub yes: bool,
    /// Data folder. Defaults to LLMTODO_DATA_DIR, then the global config,
    /// then ~/Library/Application Support/llmtodo.
    #[arg(long, global = true, env = "LLMTODO_DATA_DIR", hide_env_values = true)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Set up llmtodo in this repo: write .llmtodo.toml at the git root and
    /// install the agent skill.
    Init(commands::init::InitArgs),
    /// The agent skill in ~/.claude/skills/llmtodo.
    #[command(subcommand)]
    Skill(commands::skill::SkillCmd),
    /// Issues: view, query, create, update, claim and more.
    #[command(subcommand)]
    Issue(commands::issue::IssueCmd),
    /// Agent sessions and their notes.
    #[command(subcommand)]
    Session(commands::sessions::SessionCmd),
    /// Labels and label groups.
    #[command(subcommand)]
    Label(commands::labels::LabelCmd),
    /// Team statuses.
    #[command(subcommand)]
    Status(commands::statuses::StatusCmd),
    /// Projects and their GitHub repos.
    #[command(subcommand)]
    Project(commands::projects::ProjectCmd),
    /// Project milestones.
    #[command(subcommand)]
    Milestone(commands::milestones::MilestoneCmd),
    /// Project documents.
    #[command(subcommand)]
    Doc(commands::documents::DocCmd),
    /// Teams.
    #[command(subcommand)]
    Team(commands::teams::TeamCmd),
    /// Saved issue views.
    #[command(subcommand)]
    View(commands::views::ViewCmd),
    /// Workspace settings.
    #[command(subcommand)]
    Settings(commands::settings::SettingsCmd),
    /// Actors: the owner and the agents.
    #[command(subcommand)]
    Actor(commands::actors::ActorCmd),
    /// Activity events, optionally followed live.
    Activity(commands::activity::ActivityArgs),
    /// Token usage and cost by ticket, from agent transcripts.
    Usage(commands::usage::UsageArgs),
    /// Open the installed desktop app from any folder. It builds nothing.
    App,
    /// Run and manage the local API server.
    #[command(subcommand)]
    Server(commands::server::ServerCmd),
    /// Hand the database to another laptop through a shared folder.
    #[command(subcommand)]
    Sync(commands::sync::SyncCmd),
    /// GitHub sync: PRs and commits on linked repos.
    #[command(subcommand)]
    Github(commands::github::GithubCmd),
    /// Import from Linear.
    #[command(subcommand)]
    Import(commands::import::ImportCmd),
    /// Stored files, such as images copied from Linear.
    #[command(subcommand)]
    File(commands::files::FileCmd),
    /// Permanently delete trashed issues or old revisions. Owner only.
    Prune(commands::prune::PruneArgs),
    /// Print every command, flag and API shape as JSON.
    Schema,
    /// Send a raw API request, such as `llmtodo api GET /v1/teams`.
    Api(commands::api::ApiArgs),
}

/// An error with a stable exit code.
#[derive(Debug)]
pub enum CliError {
    Client(ClientError),
    Usage(String),
    /// A check in the CLI refused the input, such as a label name that
    /// names a workflow state. Exit code 6, like the server's validation.
    Validation(String),
    InputNeeded(String),
    Other(String),
}

impl CliError {
    fn exit_code(&self) -> i32 {
        match self {
            CliError::Client(e) => e.exit_code(),
            CliError::Usage(_) => 2,
            CliError::Validation(_) => ErrorCode::Validation.exit_code(),
            CliError::InputNeeded(_) => 8,
            CliError::Other(_) => 1,
        }
    }

    fn body(&self) -> serde_json::Value {
        match self {
            CliError::Client(e) => serde_json::to_value(e.to_body()).unwrap_or_default(),
            CliError::Usage(m) => error_body(ErrorCode::BadRequest, m),
            CliError::Validation(m) => error_body(ErrorCode::Validation, m),
            CliError::InputNeeded(m) => error_body(ErrorCode::InputNeeded, m),
            CliError::Other(m) => error_body(ErrorCode::Internal, m),
        }
    }
}

fn error_body(code: ErrorCode, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

impl From<ClientError> for CliError {
    fn from(e: ClientError) -> Self {
        CliError::Client(e)
    }
}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        CliError::Other(format!("{e:#}"))
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Other(e.to_string())
    }
}

pub type CliResult<T = ()> = Result<T, CliError>;

/// Shared state for one run of the CLI.
pub struct App {
    pub global: Global,
    pub out: Out,
    pub detected: context::Detected,
    pub data_dir: PathBuf,
    client: tokio::sync::OnceCell<llmtodo_client::Client>,
}

impl App {
    /// Connects to the server, starting it when it is not running.
    pub async fn client(&self) -> CliResult<&llmtodo_client::Client> {
        let mut fresh = false;
        let c = self
            .client
            .get_or_try_init(|| {
                fresh = true;
                self.connect()
            })
            .await?;
        if fresh {
            self.remember_local_path(c).await;
        }
        Ok(c)
    }

    /// Saves the repo's main checkout as the project's local folder, when
    /// this call runs in a repo whose `.llmtodo.toml` names a project that
    /// has no folder yet. The app shows the folder. A failure here never
    /// fails the command.
    async fn remember_local_path(&self, c: &llmtodo_client::Client) {
        let (Some(project), Some(root)) = (&self.detected.repo.project, &self.detected.repo_root)
        else {
            return;
        };
        let Ok(p) = c.project(project).await else {
            return;
        };
        if p.local_path.is_some() {
            return;
        }
        let Some(main) = llmtodo_client::git::main_checkout_root(root) else {
            return;
        };
        let update = llmtodo_api_types::UpdateProject {
            local_path: Some(main.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let _ = c.update_project(&p.slug, &update).await;
    }

    /// A new connection, for a caller that outlives a server restart. The
    /// server may come back on another port.
    pub async fn connect(&self) -> CliResult<llmtodo_client::Client> {
        let spawn = std::env::current_exe().ok();
        llmtodo_client::Connect {
            data_dir: self.data_dir.clone(),
            context: self.detected.identity.context.clone(),
            spawn,
            upgrade_old_server: true,
        }
        .connect()
        .await
        .map(
            |c| match self.detected.identity.usage_source(&self.data_dir) {
                Some(f) => c.with_usage(f),
                None => c,
            },
        )
        .map_err(CliError::from)
    }

    pub fn interactive(&self) -> bool {
        !self.global.no_input && std::io::stdin().is_terminal()
    }

    /// Asks before a destructive step, unless `-y` is set.
    pub fn confirm(&self, question: &str) -> CliResult {
        if self.global.yes {
            return Ok(());
        }
        if !self.interactive() {
            return Err(CliError::InputNeeded(format!(
                "{question} Pass -y to confirm without a prompt."
            )));
        }
        eprint!("{question} [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            Ok(())
        } else {
            Err(CliError::Other("cancelled".into()))
        }
    }

    /// The team to use: a flag, then `.llmtodo.toml`.
    pub fn team(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| self.detected.repo.team.clone())
    }

    pub fn project(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| self.detected.repo.project.clone())
    }
}

/// Reads text from a flag, or from a file, or from stdin when the file is `-`.
pub fn read_text(inline: Option<String>, file: Option<PathBuf>) -> CliResult<Option<String>> {
    match (inline, file) {
        (Some(_), Some(_)) => Err(CliError::Usage("pass the text or a file, not both".into())),
        (Some(t), None) => Ok(Some(t)),
        (None, Some(f)) if f.as_os_str() == "-" => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            Ok(Some(s))
        }
        (None, Some(f)) => {
            Ok(Some(std::fs::read_to_string(&f).map_err(|e| {
                CliError::Usage(format!("cannot read {}: {e}", f.display()))
            })?))
        }
        (None, None) => Ok(None),
    }
}

fn data_dir(global: &Global) -> PathBuf {
    // Clap already read `LLMTODO_DATA_DIR` into the flag.
    llmtodo_client::data_dir::resolve(global.data_dir.as_deref())
}

/// Whether output is JSON, decided from the raw arguments. Clap errors
/// happen before the flags are parsed, so they cannot use [`Out`].
fn wants_json(args: &[std::ffi::OsString]) -> bool {
    let has = |flag: &str| args.iter().skip(1).any(|a| a == flag);
    if has("--json") {
        true
    } else if has("--human") {
        false
    } else {
        !std::io::stdout().is_terminal()
    }
}

fn parse() -> Result<Cli, ExitCode> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    match Cli::try_parse_from(&args) {
        Ok(cli) => Ok(cli),
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            e.exit()
        }
        Err(e) if wants_json(&args) => {
            let message = e
                .render()
                .to_string()
                .lines()
                .next()
                .unwrap_or("bad arguments")
                .trim_start_matches("error: ")
                .to_string();
            eprintln!("{}", error_body(ErrorCode::BadRequest, &message));
            Err(ExitCode::from(ErrorCode::BadRequest.exit_code() as u8))
        }
        Err(e) => e.exit(),
    }
}

fn main() -> ExitCode {
    let cli = match parse() {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let out = Out::new(cli.global.json, cli.global.human, cli.global.fields.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = runtime.block_on(run(cli, out.clone()));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if out.json {
                eprintln!("{}", e.body());
            } else {
                let msg = e.body()["error"]["message"]
                    .as_str()
                    .unwrap_or("error")
                    .to_string();
                eprintln!("error: {msg}");
            }
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

async fn run(cli: Cli, out: Out) -> CliResult {
    let cwd = std::env::current_dir()?;
    let detected = context::detect(&cwd).map_err(CliError::Usage)?;
    let dir = data_dir(&cli.global);
    let app = App {
        global: cli.global,
        out,
        detected,
        data_dir: dir,
        client: tokio::sync::OnceCell::new(),
    };
    match cli.command {
        Cmd::Init(a) => commands::init::run(&app, a).await,
        Cmd::Skill(c) => commands::skill::run(&app, c).await,
        Cmd::Issue(c) => commands::issue::run(&app, c).await,
        Cmd::Session(c) => commands::sessions::run(&app, c).await,
        Cmd::Label(c) => commands::labels::run(&app, c).await,
        Cmd::Status(c) => commands::statuses::run(&app, c).await,
        Cmd::Project(c) => commands::projects::run(&app, c).await,
        Cmd::Milestone(c) => commands::milestones::run(&app, c).await,
        Cmd::Doc(c) => commands::documents::run(&app, c).await,
        Cmd::Team(c) => commands::teams::run(&app, c).await,
        Cmd::View(c) => commands::views::run(&app, c).await,
        Cmd::Settings(c) => commands::settings::run(&app, c).await,
        Cmd::Actor(c) => commands::actors::run(&app, c).await,
        Cmd::Usage(a) => commands::usage::run(&app, a).await,
        Cmd::Activity(a) => commands::activity::run(&app, a).await,
        Cmd::App => commands::app::run(&app),
        Cmd::Server(c) => commands::server::run(&app, c).await,
        Cmd::Sync(c) => commands::sync::run(&app, c).await,
        Cmd::Github(c) => commands::github::run(&app, c).await,
        Cmd::Import(c) => commands::import::run(&app, c).await,
        Cmd::File(c) => commands::files::run(&app, c).await,
        Cmd::Prune(a) => commands::prune::run(&app, a).await,
        Cmd::Schema => {
            app.out.value(commands::schema::schema());
            Ok(())
        }
        Cmd::Api(a) => commands::api::run(&app, a).await,
    }
}
