use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use llmtodo_api_types::*;

use crate::{App, CliError, CliResult, read_text};

#[derive(Subcommand)]
pub enum IssueCmd {
    /// Read an issue with its comments, relations and work log.
    View { key: String },
    /// List issues that match filters. Sorted by priority, then last update.
    Query(QueryArgs),
    /// Create an issue.
    Create(CreateArgs),
    /// Change an issue. Only the flags you pass change.
    Update(Box<UpdateArgs>),
    /// Move an issue to the trash. `issue restore` brings it back.
    Delete { key: String },
    /// Take an issue out of the trash, or bring back an old description.
    Restore {
        key: String,
        /// Description revision to restore.
        #[arg(long)]
        revision: Option<i64>,
    },
    /// List description revisions and field changes.
    History {
        key: String,
        /// Include the full text of every revision.
        #[arg(long)]
        bodies: bool,
    },
    /// Claim an issue for this session: assignee and In Progress at once.
    /// Fails with exit code 4 when another session holds it.
    Claim {
        key: String,
        /// Take the issue over from another session.
        #[arg(long)]
        steal: bool,
    },
    /// Mark this session as analyzing an issue: reading it or filling it in
    /// without a claim. It never blocks other writers. It ends with --done,
    /// on a claim, or after 15 minutes without a write by this session.
    Analyze {
        key: String,
        /// End this session's mark.
        #[arg(long)]
        done: bool,
    },
    /// Release this session's claim. A started issue goes back to its
    /// team's first unstarted status unless --status is set.
    Release {
        key: String,
        #[arg(long)]
        status: Option<String>,
        /// Release a claim another session holds.
        #[arg(long)]
        force: bool,
    },
    /// Open, unblocked, unassigned sub-issues of a map issue, or of a
    /// project with --project.
    Frontier(FrontierArgs),
    /// Comments.
    #[command(subcommand)]
    Comment(CommentCmd),
    /// The dependency tree of an issue: what it waits for and what waits for
    /// it, with the depth and status of each issue.
    Deps {
        key: String,
        /// Hide closed issues. The tree then stops at them.
        #[arg(long)]
        open: bool,
    },
    /// Relations: blocks, blocked-by, related, duplicate.
    #[command(subcommand)]
    Relation(RelationCmd),
    /// The order of an epic's sub-issues, in steps. A task waits until every
    /// task of the step before it is done.
    #[command(subcommand)]
    Order(super::order::OrderCmd),
    /// Attach a URL, a PR link or a local path.
    Attach {
        key: String,
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        kind: Option<AttachmentKind>,
    },
    /// Remove an attachment by its ID.
    Detach { id: String },
    /// Ask the owner a question. It goes into the `+++ Owner questions`
    /// section, and the issue waits on the owner. Repeat --option for a
    /// choice; leave it out for a free-text answer.
    Ask {
        key: String,
        /// The question, on one line.
        question: String,
        /// `a=Fork` or just `Fork`. Letters go a, b, c ... when left out.
        #[arg(long = "option", short = 'o')]
        options: Vec<String>,
        /// Background for the owner, in Markdown.
        #[arg(long)]
        context: Option<String>,
    },
    /// Answer an owner question, such as `issue answer LLM-12 Q1 b`. A
    /// single option letter picks that option. When no question stays
    /// open, the issue waits on agents again.
    Answer {
        key: String,
        /// `Q1`, or just `1`.
        id: String,
        /// The answer. An empty string opens the question again.
        answer: String,
    },
    /// Take back an answer: the question opens again and the issue waits on
    /// the owner. Only the actor who answered can do this. It fails with
    /// exit code 6 when another actor wrote to the issue after the answer,
    /// the question changed, or the issue is closed.
    UndoAnswer {
        key: String,
        /// `Q1`, or just `1`.
        id: String,
    },
    /// Review rounds: the steps and checks the owner tries, and the
    /// owner's answer.
    #[command(subcommand)]
    Review(super::review::ReviewCmd),
    /// List owner questions. With a key, every question on that issue.
    /// Without one, the open questions on every open issue.
    Questions {
        key: Option<String>,
        /// Include answered questions.
        #[arg(long)]
        all: bool,
        /// Search every project, not only the default one.
        #[arg(long)]
        all_projects: bool,
    },
}

/// Parses `--option` values: `a=Fork`, or `Fork` with the next free letter.
pub fn parse_options(raw: &[String]) -> CliResult<Vec<llmtodo_core::QuestionOption>> {
    let mut out: Vec<llmtodo_core::QuestionOption> = Vec::new();
    for r in raw {
        let explicit = r.split_once('=').and_then(|(l, t)| {
            let mut cs = l.trim().chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Some((c.to_ascii_lowercase(), t)),
                _ => None,
            }
        });
        let (letter, text) = match explicit {
            Some(e) => e,
            None => {
                let next = ('a'..='z')
                    .find(|c| out.iter().all(|o| o.letter != *c))
                    .ok_or_else(|| CliError::Usage("at most 26 options".into()))?;
                (next, r.as_str())
            }
        };
        out.push(llmtodo_core::QuestionOption {
            letter,
            text: text.trim().to_string(),
        });
    }
    llmtodo_core::questions::validate_options(&out).map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(out)
}

#[derive(Args, Default)]
pub struct QueryArgs {
    /// Only this issue, by key. Repeat for several. Unknown keys are
    /// skipped. Skips the default project from `.llmtodo.toml`.
    #[arg(long = "key")]
    pub keys: Vec<String>,
    #[arg(long)]
    pub team: Vec<String>,
    #[arg(long)]
    pub project: Vec<String>,
    /// Search every project. Skips the default project from `.llmtodo.toml`.
    #[arg(long, conflicts_with = "project")]
    pub all_projects: bool,
    /// Status name. Repeat for several.
    #[arg(long)]
    pub status: Vec<String>,
    /// Status type: backlog, unstarted, started, completed, canceled.
    #[arg(long = "type")]
    pub status_type: Vec<StatusType>,
    /// Only open issues.
    #[arg(long)]
    pub open: bool,
    /// Label the issue must have. Repeat for several.
    #[arg(long)]
    pub label: Vec<String>,
    /// `me`, `owner`, `none`, or an actor name.
    #[arg(long)]
    pub assignee: Option<String>,
    /// Parent key, or `none` for top-level issues.
    #[arg(long)]
    pub parent: Option<String>,
    #[arg(long)]
    pub milestone: Option<String>,
    #[arg(long)]
    pub priority: Option<Priority>,
    /// Wanted-executor filter, such as `kind=agent,model=opus-5.5`.
    #[arg(long = "for")]
    pub for_executor: Option<WantedExecutor>,
    /// `agent`, `owner` or `none`.
    #[arg(long)]
    pub waiting_on: Option<String>,
    /// Open issues that wait for this issue, directly or through a chain.
    #[arg(long)]
    pub depends_on: Option<String>,
    /// Only issues an agent has claimed. With `--analyzing`, keeps either.
    #[arg(long)]
    pub claimed: bool,
    /// Only issues an agent is analyzing. With `--claimed`, keeps either.
    #[arg(long)]
    pub analyzing: bool,
    /// Full-text search over titles, descriptions and comments.
    #[arg(long, short = 'q')]
    pub text: Option<String>,
    /// Duplicate search: issues that look like this text.
    #[arg(long)]
    pub similar: Option<String>,
    /// List the trash.
    #[arg(long)]
    pub trashed: bool,
    /// Start from a saved view.
    #[arg(long)]
    pub view: Option<String>,
    /// Include descriptions in the output.
    #[arg(long)]
    pub description: bool,
    #[arg(long)]
    pub updated_after: Option<jiff::Timestamp>,
    /// Stop after this many issues. All pages are fetched otherwise.
    #[arg(long)]
    pub limit: Option<u32>,
    /// Print only the IDs of every match, as one list, in one request.
    #[arg(long, conflicts_with_all = ["limit", "description"])]
    pub ids: bool,
}

impl QueryArgs {
    pub fn into_query(self, app: &App) -> IssueQuery {
        // Duplicate search always spans every project, so an agent finds a
        // duplicate that lives outside the repo's default project.
        let use_default = self.project.is_empty()
            && self.keys.is_empty()
            && !self.all_projects
            && self.similar.is_none()
            && self.view.is_none()
            && self.parent.is_none();
        let projects: Vec<String> = if use_default {
            app.project(None).into_iter().collect()
        } else {
            self.project
        };
        if use_default
            && self.text.is_some()
            && let Some(p) = projects.first()
        {
            eprintln!(
                "note: searching project {p} only, the default from .llmtodo.toml; pass --all-projects to search every project"
            );
        }
        IssueQuery {
            keys: self.keys,
            teams: self.team,
            projects,
            filter: IssueFilter {
                statuses: self.status,
                status_types: self.status_type,
                open: self.open,
                labels: self.label,
                assignee: self.assignee,
                claimed: self.claimed,
                analyzing: self.analyzing,
            },
            parent: self.parent,
            milestone: self.milestone,
            priority: self.priority,
            for_executor: self.for_executor,
            waiting_on: self.waiting_on,
            depends_on: self.depends_on,
            text: self.text,
            similar: self.similar,
            trashed: self.trashed,
            include_description: self.description,
            view: self.view,
            updated_after: self.updated_after,
            order: None,
            cursor: None,
            limit: self.limit,
        }
    }
}

#[derive(Args)]
pub struct CreateArgs {
    #[arg(long, short = 't')]
    pub title: String,
    #[arg(long, short = 'd', conflicts_with = "description_file")]
    pub description: Option<String>,
    /// Read the description from a file, or `-` for stdin.
    #[arg(long)]
    pub description_file: Option<PathBuf>,
    #[arg(long)]
    pub team: Option<String>,
    #[arg(long, short = 's')]
    pub status: Option<String>,
    #[arg(long, short = 'p')]
    pub priority: Option<Priority>,
    /// `me`, `owner`, or an actor name.
    #[arg(long, short = 'a')]
    pub assignee: Option<String>,
    #[arg(long, short = 'l')]
    pub label: Vec<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub milestone: Option<String>,
    #[arg(long)]
    pub parent: Option<String>,
    /// Wanted executor, such as `kind=agent` or `model=opus-5.5,effort=high`.
    #[arg(long = "for")]
    pub for_executor: Option<WantedExecutor>,
    /// `agent` when it is ready for an agent, `owner` when it needs your input.
    #[arg(long, short = 'w')]
    pub waiting_on: Option<WaitingOn>,
    #[arg(long)]
    pub blocks: Vec<String>,
    #[arg(long)]
    pub blocked_by: Vec<String>,
    #[arg(long)]
    pub related: Vec<String>,
    /// Estimated agent working time in minutes. A claim needs this.
    #[arg(long)]
    pub estimate_minutes: Option<u32>,
    /// Estimated token cost in USD. A claim needs this.
    #[arg(long)]
    pub estimate_cost: Option<f64>,
}

#[derive(Args)]
pub struct UpdateArgs {
    pub key: String,
    #[arg(long, short = 't')]
    pub title: Option<String>,
    #[arg(long, short = 's')]
    pub status: Option<String>,
    #[arg(long, short = 'p')]
    pub priority: Option<Priority>,
    /// `me`, `owner`, an actor name, or `none` to clear.
    #[arg(long, short = 'a')]
    pub assignee: Option<String>,
    /// Replace the whole label set. Repeat for several.
    #[arg(long)]
    pub label: Vec<String>,
    /// Remove every label.
    #[arg(long, conflicts_with = "label")]
    pub clear_labels: bool,
    #[arg(long)]
    pub add_label: Vec<String>,
    #[arg(long)]
    pub remove_label: Vec<String>,
    /// Project, or `none` to clear.
    #[arg(long)]
    pub project: Option<String>,
    /// Milestone, or `none` to clear.
    #[arg(long)]
    pub milestone: Option<String>,
    /// Parent key, or `none` to clear.
    #[arg(long)]
    pub parent: Option<String>,
    /// The step in the parent's order: an existing step, the last step plus
    /// one for a new last step, or `none` to take the task out of the order.
    #[arg(long, value_name = "N|none")]
    pub order_step: Option<String>,
    /// Move to another team. The issue gets a new key and keeps the old one
    /// as an alias.
    #[arg(long)]
    pub team: Option<String>,
    /// Wanted executor, or `none` to clear.
    #[arg(long = "for")]
    pub for_executor: Option<String>,
    /// `agent`, `owner`, or `none` to clear.
    #[arg(long, short = 'w')]
    pub waiting_on: Option<String>,
    /// The git branch for the git panel, or `none` to clear.
    #[arg(long)]
    pub branch: Option<String>,
    /// The worktree path for the git panel, or `none` to clear. This
    /// machine's host name is stored with it.
    #[arg(long)]
    pub worktree: Option<String>,
    /// The PR URL for the git panel, or `none` to clear. It is also attached.
    #[arg(long)]
    pub pr: Option<String>,
    /// Set --branch and --worktree from the current git checkout.
    #[arg(long, conflicts_with_all = ["branch", "worktree"])]
    pub git_here: bool,
    /// Replace the whole description. Needs --base-version or --force.
    #[arg(long, short = 'd', conflicts_with = "description_file")]
    pub description: Option<String>,
    #[arg(long)]
    pub description_file: Option<PathBuf>,
    /// The description version your text is based on.
    #[arg(long)]
    pub base_version: Option<i64>,
    /// Replace the description even if it changed since you read it.
    #[arg(long)]
    pub force: bool,
    /// Replace the body of one section, chosen by heading or `+++` title.
    #[arg(long, requires = "body_source")]
    pub section: Option<String>,
    /// Like --section for a `+++` section, but add it at the end when it is
    /// missing.
    #[arg(long, conflicts_with = "section", requires = "body_source")]
    pub upsert_section: Option<String>,
    #[arg(long, group = "body_source")]
    pub body: Option<String>,
    #[arg(long, group = "body_source")]
    pub body_file: Option<PathBuf>,
    /// Replace one exact string, which must occur exactly once.
    #[arg(long, requires = "with")]
    pub replace: Option<String>,
    /// The new text for --replace.
    #[arg(long, requires = "replace")]
    pub with: Option<String>,
    /// Estimated agent working time in minutes. Changing a set estimate
    /// needs --estimate-reason.
    #[arg(long)]
    pub estimate_minutes: Option<u32>,
    /// Estimated token cost in USD. Changing a set estimate needs
    /// --estimate-reason.
    #[arg(long)]
    pub estimate_cost: Option<f64>,
    /// The spec change that justifies changing a set estimate.
    #[arg(long)]
    pub estimate_reason: Option<String>,
}

#[derive(Args)]
pub struct FrontierArgs {
    pub map: Option<String>,
    #[arg(long, conflicts_with = "map")]
    pub project: Option<String>,
    /// Levels of sub-issues to include. Default 1.
    #[arg(long)]
    pub depth: Option<u32>,
    #[arg(long = "for")]
    pub for_executor: Option<WantedExecutor>,
    #[arg(long)]
    pub description: bool,
}

#[derive(Subcommand)]
pub enum CommentCmd {
    /// Add a comment. Most notes belong in the description instead.
    Add {
        key: String,
        #[arg(long, short = 'b')]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        parent: Option<CommentId>,
    },
    Update {
        id: String,
        #[arg(long, short = 'b')]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum RelationCmd {
    /// `issue relation add ALX-1 blocks ALX-2`
    Add {
        key: String,
        kind: RelationInput,
        other: String,
    },
    Remove {
        key: String,
        kind: RelationInput,
        other: String,
    },
}

/// `--order-step`: unset keeps, `none` clears, a number sets.
fn step_change(s: Option<String>) -> CliResult<Change<u32>> {
    let Some(s) = s else {
        return Ok(Change::Keep);
    };
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") || s.is_empty() {
        return Ok(Change::Clear);
    }
    match s.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Change::Set(n)),
        _ => Err(CliError::Usage(format!(
            "--order-step takes a step number from 1, or none; got {s:?}"
        ))),
    }
}

/// A text flag as a change: unset keeps, `none` or blank clears.
fn text_change(s: Option<String>) -> Change<String> {
    match s {
        Some(v) if v.trim().eq_ignore_ascii_case("none") => Change::Clear,
        s => Change::text(s),
    }
}

/// The `--waiting-on` flag as a change: unset keeps, `none` clears.
fn waiting_change(s: Option<String>) -> Result<Change<WaitingOn>, CliError> {
    Ok(match text_change(s) {
        Change::Keep => Change::Keep,
        Change::Clear => Change::Clear,
        Change::Set(v) => Change::Set(v.parse().map_err(CliError::Usage)?),
    })
}

pub async fn run(app: &App, cmd: IssueCmd) -> CliResult {
    let c = app.client().await?;
    match cmd {
        IssueCmd::View { key } => {
            let mut d = c.issue(&key).await?;
            add_commits(app, &mut d);
            app.out.issue_detail(&d);
        }
        IssueCmd::Query(a) if a.ids => {
            let q = a.into_query(app);
            app.out.ser(&c.query_ids(&q).await?);
        }
        IssueCmd::Query(a) => {
            let q = a.into_query(app);
            let items = c.query_all(&q).await?;
            app.out.issues(&items);
        }
        IssueCmd::Create(a) => create_issue(app, c, a).await?,
        IssueCmd::Update(a) => update_issue(app, c, a).await?,
        IssueCmd::Delete { key } => {
            app.confirm(&format!("Move {key} to the trash?"))?;
            app.out.ser(&c.delete_issue(&key).await?);
        }
        IssueCmd::Restore { key, revision } => {
            app.out
                .issue_detail(&c.restore_issue(&key, &RestoreRequest { revision }).await?);
        }
        IssueCmd::History { key, bodies } => {
            let q = HistoryQuery { bodies };
            app.out.ser(&c.issue_history(&key, &q).await?)
        }
        IssueCmd::Claim { key, steal } => claim_issue(app, c, &key, steal).await?,
        IssueCmd::Analyze { key, done } => analyze_issue(app, c, &key, done).await?,
        IssueCmd::Release { key, status, force } => app
            .out
            .ser(&c.release(&key, &ReleaseRequest { status, force }).await?),
        IssueCmd::Frontier(a) => show_frontier(app, c, a).await?,
        IssueCmd::Comment(cc) => run_comment(app, c, cc).await?,
        IssueCmd::Deps { key, open } => app.out.deps(&c.deps(&key, open).await?),
        IssueCmd::Relation(rc) => run_relation(app, c, rc).await?,
        IssueCmd::Order(oc) => super::order::run(app, c, oc).await?,
        IssueCmd::Attach {
            key,
            url,
            title,
            kind,
        } => attach(app, c, &key, &url, title, kind).await?,
        IssueCmd::Detach { id } => app.out.ser(&c.detach(&id).await?),
        IssueCmd::Ask {
            key,
            question,
            options,
            context,
        } => ask_question(app, c, &key, question, &options, context).await?,
        IssueCmd::Answer { key, id, answer } => answer_question(app, c, &key, id, answer).await?,
        IssueCmd::UndoAnswer { key, id } => {
            let d = c.undo_answer(&key, &id).await?;
            print_answer_result(app, &d, &id);
        }
        IssueCmd::Questions {
            key,
            all,
            all_projects,
        } => list_questions(app, c, key, all, all_projects).await?,
        IssueCmd::Review(rc) => super::review::run(app, c, rc).await?,
    }
    Ok(())
}

async fn create_issue(app: &App, c: &llmtodo_client::Client, a: CreateArgs) -> CliResult {
    let description = read_text(a.description, a.description_file)?.unwrap_or_default();
    let mut relations = BTreeMap::new();
    for (k, v) in [
        (RelationInput::Blocks, a.blocks),
        (RelationInput::BlockedBy, a.blocked_by),
        (RelationInput::Related, a.related),
    ] {
        if !v.is_empty() {
            relations.insert(k, v);
        }
    }
    let req = CreateIssue {
        team: app.team(a.team),
        title: a.title,
        description,
        status: a.status,
        priority: a.priority,
        assignee: a.assignee,
        labels: a.label,
        project: app.project(a.project),
        milestone: a.milestone,
        parent: a.parent,
        wanted_executor: a.for_executor,
        waiting_on: a.waiting_on,
        relations,
        estimate_minutes: a.estimate_minutes,
        estimate_cost_usd: a.estimate_cost,
    };
    let d = c.create_issue(&req).await?;
    app.out.issue_detail(&d);
    Ok(())
}

async fn update_issue(app: &App, c: &llmtodo_client::Client, mut a: Box<UpdateArgs>) -> CliResult {
    let key = a.key.clone();
    if a.git_here {
        let ctx = &app.detected.identity.context;
        if ctx.worktree.is_none() {
            return Err(CliError::Usage(
                "--git-here needs a git checkout in the current folder".into(),
            ));
        }
        a.branch = Some(ctx.branch.clone().unwrap_or_else(|| "none".into()));
        a.worktree = ctx.worktree.clone();
    }
    let req = build_update(*a)?;
    let d = c.update_issue(&key, &req).await?;
    app.out.issue_detail(&d);
    Ok(())
}

async fn claim_issue(app: &App, c: &llmtodo_client::Client, key: &str, steal: bool) -> CliResult {
    let r = c.claim(key, &ClaimRequest { steal }).await?;
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
    if !r.changed {
        eprintln!(
            "warning: this session already holds {}; another agent that shares this session may be working on it. Give each subagent its own LLMTODO_SESSION.",
            r.issue.key
        );
    }
    if app.out.json {
        app.out.ser(&r);
    } else {
        println!(
            "{} {} by {}",
            r.issue.key,
            if r.changed {
                "claimed"
            } else {
                "already claimed"
            },
            r.issue
                .assignee
                .as_ref()
                .map(|a| a.name.as_str())
                .unwrap_or("?")
        );
    }
    Ok(())
}

async fn analyze_issue(app: &App, c: &llmtodo_client::Client, key: &str, done: bool) -> CliResult {
    let r = c.analyze(key, &AnalyzeRequest { done }).await?;
    for w in &r.warnings {
        eprintln!("warning: {w}");
    }
    if app.out.json {
        app.out.ser(&r);
    } else {
        let what = match (done, r.changed) {
            (false, true) => "marked as analyzing",
            (false, false) => "still analyzing",
            (true, true) => "no longer analyzing",
            (true, false) => "had no analyzing mark from this session",
        };
        println!("{} {what}", r.issue.key);
    }
    Ok(())
}

async fn show_frontier(app: &App, c: &llmtodo_client::Client, a: FrontierArgs) -> CliResult {
    let project = if a.map.is_none() {
        app.project(a.project)
    } else {
        None
    };
    if a.map.is_none() && project.is_none() {
        return Err(CliError::Usage("pass a map issue key or --project".into()));
    }
    let q = FrontierQuery {
        issue: a.map,
        project,
        depth: a.depth,
        for_executor: a.for_executor,
        include_description: a.description,
    };
    app.out.issues(&c.frontier(&q).await?);
    Ok(())
}

async fn run_comment(app: &App, c: &llmtodo_client::Client, cc: CommentCmd) -> CliResult {
    match cc {
        CommentCmd::Add {
            key,
            body,
            body_file,
            parent,
        } => {
            let body = read_text(body, body_file)?
                .ok_or_else(|| CliError::Usage("pass --body or --body-file".into()))?;
            app.out.ser(
                &c.add_comment(
                    &key,
                    &CreateComment {
                        body,
                        parent_id: parent,
                    },
                )
                .await?,
            );
        }
        CommentCmd::Update {
            id,
            body,
            body_file,
        } => {
            let body = read_text(body, body_file)?
                .ok_or_else(|| CliError::Usage("pass --body or --body-file".into()))?;
            app.out
                .ser(&c.update_comment(&id, &UpdateComment { body }).await?);
        }
        CommentCmd::Delete { id } => {
            app.confirm(&format!("Delete comment {id}?"))?;
            app.out.ser(&c.delete_comment(&id).await?);
        }
    }
    Ok(())
}

async fn run_relation(app: &App, c: &llmtodo_client::Client, rc: RelationCmd) -> CliResult {
    match rc {
        RelationCmd::Add { key, kind, other } => app.out.issue_detail(
            &c.add_relation(&key, &CreateRelation { kind, issue: other })
                .await?,
        ),
        RelationCmd::Remove { key, kind, other } => app.out.issue_detail(
            &c.remove_relation(&key, &RemoveRelation { kind, issue: other })
                .await?,
        ),
    }
    Ok(())
}

async fn attach(
    app: &App,
    c: &llmtodo_client::Client,
    key: &str,
    url: &str,
    title: Option<String>,
    kind: Option<AttachmentKind>,
) -> CliResult {
    let url = absolutize(url);
    app.out.ser(
        &c.attach(key, &CreateAttachment { url, title, kind })
            .await?,
    );
    Ok(())
}

async fn ask_question(
    app: &App,
    c: &llmtodo_client::Client,
    key: &str,
    question: String,
    options: &[String],
    context: Option<String>,
) -> CliResult {
    let options = parse_options(options)?;
    let req = UpdateIssue {
        waiting_on: Change::Set(WaitingOn::Owner),
        description: BodyChange {
            patches: vec![Patch::AskQuestion {
                question,
                context: context.unwrap_or_default(),
                options,
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let d = c.update_issue(key, &req).await?;
    let desc = d.issue.description.as_deref().unwrap_or_default();
    let qs = llmtodo_core::questions(desc);
    let asked = qs.iter().max_by_key(|q| q.number);
    if app.out.json {
        app.out.ser(&serde_json::json!({
            "key": d.issue.key,
            "question": asked,
            "waiting_on": d.issue.waiting_on,
        }));
    } else if let Some(q) = asked {
        println!("{} {}: {}", d.issue.key, q.id, q.text);
    }
    Ok(())
}

async fn answer_question(
    app: &App,
    c: &llmtodo_client::Client,
    key: &str,
    id: String,
    answer: String,
) -> CliResult {
    let req = UpdateIssue {
        description: BodyChange {
            patches: vec![Patch::AnswerQuestion {
                id: id.clone(),
                answer,
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let d = c.update_issue(key, &req).await?;
    print_answer_result(app, &d, &id);
    Ok(())
}

/// Prints one question after a change to its answer, and how many stay open.
fn print_answer_result(app: &App, d: &IssueDetail, id: &str) {
    let desc = d.issue.description.as_deref().unwrap_or_default();
    let qs = llmtodo_core::questions(desc);
    let want = id.trim().trim_start_matches(['Q', 'q']);
    let q = qs.iter().find(|q| q.number.to_string() == want);
    let open = qs.iter().filter(|q| q.is_open()).count();
    if app.out.json {
        app.out.ser(&serde_json::json!({
            "key": d.issue.key,
            "question": q,
            "open": open,
            "waiting_on": d.issue.waiting_on,
        }));
    } else {
        println!(
            "{} {}: {} ({open} open)",
            d.issue.key,
            q.map_or(id, |q| q.id.as_str()),
            q.and_then(|q| q.answer.as_deref()).unwrap_or("reopened"),
        );
    }
}

async fn list_questions(
    app: &App,
    c: &llmtodo_client::Client,
    key: Option<String>,
    all: bool,
    all_projects: bool,
) -> CliResult {
    let project = if key.is_some() || all_projects {
        None
    } else {
        app.project(None)
    };
    let rows = c
        .questions(&QuestionQuery {
            issue: key,
            project,
            all,
            ..Default::default()
        })
        .await?;
    if app.out.json {
        app.out.ser(&rows);
    } else if rows.is_empty() {
        println!("No questions.");
    } else {
        for r in &rows {
            println!("{} {}  {}", r.key, r.question.id, r.question.text);
            for o in &r.question.options {
                println!("    {}) {}", o.letter, o.text);
            }
            if let Some(a) = &r.question.answer {
                println!("    Answer: {a}");
            }
        }
    }
    Ok(())
}

/// Local paths become absolute, so they still make sense from elsewhere.
fn absolutize(url: &str) -> String {
    if url.contains("://") || url.starts_with('/') || url.starts_with('~') {
        return url.to_string();
    }
    let p = std::path::Path::new(url);
    if p.exists()
        && let Ok(abs) = std::fs::canonicalize(p)
    {
        return abs.display().to_string();
    }
    url.to_string()
}

pub fn build_update(a: UpdateArgs) -> CliResult<UpdateIssue> {
    let mut patches = Vec::new();
    let body = read_text(a.body, a.body_file)?;
    if let Some(section) = a.section {
        patches.push(Patch::ReplaceSection {
            section,
            body: body.clone().unwrap_or_default(),
        });
    } else if let Some(section) = a.upsert_section {
        patches.push(Patch::UpsertCollapsible {
            section,
            body: body.clone().unwrap_or_default(),
        });
    } else if body.is_some() {
        return Err(CliError::Usage(
            "--body needs --section or --upsert-section".into(),
        ));
    }
    if let Some(old) = a.replace {
        patches.push(Patch::ReplaceExact {
            old,
            new: a.with.unwrap_or_default(),
        });
    }
    let wanted_executor = match a.for_executor.as_deref() {
        None => None,
        Some(s) if s.eq_ignore_ascii_case("none") => Some(WantedExecutor::default()),
        Some(s) => Some(s.parse().map_err(CliError::Usage)?),
    };
    let labels = if a.clear_labels {
        Some(Vec::new())
    } else if a.label.is_empty() {
        None
    } else {
        Some(a.label)
    };
    let u = UpdateIssue {
        title: a.title,
        status: a.status,
        priority: a.priority,
        assignee: text_change(a.assignee),
        labels,
        add_labels: a.add_label,
        remove_labels: a.remove_label,
        project: text_change(a.project),
        milestone: text_change(a.milestone),
        parent: text_change(a.parent),
        order_step: step_change(a.order_step)?,
        team: a.team,
        wanted_executor,
        waiting_on: waiting_change(a.waiting_on)?,
        branch: text_change(a.branch),
        worktree: text_change(a.worktree),
        pr: text_change(a.pr),
        estimate_minutes: a.estimate_minutes,
        estimate_cost_usd: a.estimate_cost,
        estimate_reason: a.estimate_reason,
        description: BodyChange {
            body: read_text(a.description, a.description_file)?,
            base_version: a.base_version,
            force: a.force,
            patches,
        },
    };
    let unchanged = u.title.is_none()
        && u.status.is_none()
        && u.priority.is_none()
        && u.assignee.is_keep()
        && u.labels.is_none()
        && u.add_labels.is_empty()
        && u.remove_labels.is_empty()
        && u.project.is_keep()
        && u.milestone.is_keep()
        && u.parent.is_keep()
        && u.order_step.is_keep()
        && u.team.is_none()
        && u.wanted_executor.is_none()
        && u.waiting_on.is_keep()
        && u.branch.is_keep()
        && u.worktree.is_keep()
        && u.pr.is_keep()
        && u.estimate_minutes.is_none()
        && u.estimate_cost_usd.is_none()
        && u.estimate_reason.is_none()
        && u.description.body.is_none()
        && u.description.patches.is_empty();
    if unchanged {
        return Err(CliError::Usage(
            "nothing to change: pass at least one field".into(),
        ));
    }
    Ok(u)
}

/// Fills the commit lists of the work sessions and the git panel from local
/// repos. A removed worktree falls back to the current repo.
fn add_commits(app: &App, d: &mut IssueDetail) {
    llmtodo_client::git::fill_commits(
        d,
        app.detected.identity.context.host.as_deref(),
        app.detected.repo_root.as_deref(),
    );
}
