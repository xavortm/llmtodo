//! The app's view of issues, mapped from the API types in
//! `llmtodo_api_types`. The list and the task view read these, so the API
//! shapes stay out of the rendering code.

use std::collections::HashMap;

use gpui_shared_string::SharedString;
use llmtodo_api_types as api;
pub use llmtodo_api_types::WaitingOn;

pub use llmtodo_api_types::StatusType;

/// A team status: its real name, such as In Review or Duplicate, and its
/// fixed type. The type picks the icon and the order; the name is what the
/// app shows. Two statuses are equal when their names match, so a status
/// group can hold issues from several teams.
#[derive(Debug, Clone)]
pub struct Status {
    pub name: SharedString,
    pub kind: StatusType,
    /// The team's order within one type.
    pub position: i64,
    /// A review status: tickets here wait for the owner's review.
    pub review: bool,
}

impl PartialEq for Status {
    fn eq(&self, other: &Self) -> bool {
        self.name.eq_ignore_ascii_case(&other.name)
    }
}

impl Status {
    pub fn new(name: impl Into<SharedString>, kind: StatusType, position: i64) -> Self {
        Self {
            name: name.into(),
            kind,
            position,
            review: false,
        }
    }

    /// The same status, with the review flag set to `review`.
    pub fn with_review(mut self, review: bool) -> Self {
        self.review = review;
        self
    }

    /// Sort order for "sort by status": active work first, closed last.
    /// Statuses of one type keep the team's order.
    pub fn rank(&self) -> (u8, i64) {
        let kind = match self.kind {
            StatusType::Started => 0,
            StatusType::Unstarted => 1,
            StatusType::Backlog => 2,
            StatusType::Completed => 3,
            StatusType::Canceled => 4,
        };
        (kind, self.position)
    }

    /// Completed work gets the green check.
    pub fn is_done(&self) -> bool {
        self.kind == StatusType::Completed
    }

    /// Completed or canceled. Closed titles turn gray.
    pub fn is_closed(&self) -> bool {
        !self.kind.is_open()
    }
}

/// An agent actor, such as `claude-code / opus-5.5 / high`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// The actor name the server knows it by. `:assign` takes it.
    pub name: SharedString,
    pub harness: Option<SharedString>,
    pub model: Option<SharedString>,
    pub effort: Option<SharedString>,
}

impl Agent {
    /// A short name for messages: the model, else the actor name.
    pub fn label(&self) -> SharedString {
        match (&self.harness, &self.model) {
            (Some(h), Some(m)) => format!("{h} {m}").into(),
            (None, Some(m)) => m.clone(),
            _ => self.name.clone(),
        }
    }
}

/// Who works on an issue. Agents show as an icon, people as a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignee {
    Person(SharedString),
    Agent(Agent),
}

/// The owner shows as `me`, which is also what `:assign me` takes.
pub const OWNER_NAME: &str = "me";

impl Assignee {
    pub fn from_actor(actor: &api::Actor) -> Self {
        match actor.kind {
            api::ActorKind::Agent => Self::Agent(Agent {
                name: actor.name.clone().into(),
                harness: actor.harness.clone().map(Into::into),
                model: actor.model.clone().map(Into::into),
                effort: actor.effort.clone().map(Into::into),
            }),
            _ if actor.is_owner => Self::Person(OWNER_NAME.into()),
            _ => Self::Person(actor.name.clone().into()),
        }
    }

    /// The name that groups issues and that `:assign` takes.
    pub fn name(&self) -> SharedString {
        match self {
            Self::Person(name) => name.clone(),
            Self::Agent(agent) => agent.name.clone(),
        }
    }

    /// A short name for messages.
    pub fn label(&self) -> SharedString {
        match self {
            Self::Person(name) => name.clone(),
            Self::Agent(agent) => agent.label(),
        }
    }

    /// The model of an agent, else its actor name. A person shows the name.
    pub fn model(&self) -> SharedString {
        match self {
            Self::Person(name) => name.clone(),
            Self::Agent(agent) => agent.model.clone().unwrap_or_else(|| agent.name.clone()),
        }
    }
}

/// Someone other than the owner working on a task. While a claim is held,
/// the task is read-only in the app. The owner's own claims, made from the
/// CLI, do not lock the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub actor: Assignee,
    pub session: SharedString,
    /// The git worktree the agent works in.
    pub worktree: Option<SharedString>,
    pub branch: Option<SharedString>,
    /// The herdr pane that runs the agent.
    pub pane: Option<SharedString>,
    /// When the agent took the task. The badge counts up from here.
    pub claimed_at: jiff::Timestamp,
}

impl Claim {
    pub fn from_api(claim: &api::ClaimSummary) -> Option<Self> {
        if claim.actor.is_owner {
            return None;
        }
        Some(Self {
            actor: Assignee::from_actor(&claim.actor),
            session: claim.session_id.clone().into(),
            worktree: claim.worktree.clone().map(Into::into),
            branch: claim.branch.clone().map(Into::into),
            pane: claim.herdr_pane.clone().map(Into::into),
            claimed_at: claim.claimed_at,
        })
    }

    /// How long the task has been claimed at `now`, such as `12m`.
    pub fn age(&self, now: jiff::Timestamp) -> String {
        claim_age(now.duration_since(self.claimed_at))
    }
}

/// An agent that reads or fills in a task without a claim. It never makes
/// the task read-only. The mark ends at `expires_at` unless the agent
/// writes to the task again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analyzing {
    pub actor: Assignee,
    pub session: SharedString,
    pub started_at: jiff::Timestamp,
    pub expires_at: jiff::Timestamp,
}

impl Analyzing {
    pub fn from_api(mark: &api::AnalyzingSummary) -> Self {
        Self {
            actor: Assignee::from_actor(&mark.actor),
            session: mark.session_id.clone().into(),
            started_at: mark.started_at,
            expires_at: mark.expires_at,
        }
    }

    /// True while the mark has not expired at `now`. The server drops an
    /// expired mark, but the app only hears of it on the next reload.
    pub fn is_live(&self, now: jiff::Timestamp) -> bool {
        now < self.expires_at
    }

    /// How long the agent has analyzed the task at `now`, such as `12m`.
    pub fn age(&self, now: jiff::Timestamp) -> String {
        claim_age(now.duration_since(self.started_at))
    }
}

/// What an agent does on a task now, as the LLM status column shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmStatus {
    Locked(Claim),
    Analyzing(Analyzing),
}

pub const LLM_RANK_LOCKED: u8 = 0;
pub const LLM_RANK_ANALYZING: u8 = 1;
pub const LLM_RANK_NONE: u8 = 2;

/// A claim's age in whole minutes: `<1m`, `12m`, `1h 05m`, `2d 03h`.
/// A claim time ahead of the clock counts as just now.
pub fn claim_age(age: jiff::SignedDuration) -> String {
    let minutes = age.as_secs().max(0) / 60;
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (days, minutes / 60) {
        (0, 0) if minutes == 0 => "<1m".to_string(),
        (0, 0) => format!("{mins}m"),
        (0, h) => format!("{h}h {mins:02}m"),
        (d, _) => format!("{d}d {hours:02}h"),
    }
}

/// `~/x` for a path under the home folder.
pub fn shorten_home(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Label colors. Each maps to a theme token, so themes can restyle them and
/// the contrast check in `theme.rs` covers them. Labels are the one place
/// the monochrome theme allows color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelColor {
    Gray,
    Red,
    Green,
    Blue,
    Yellow,
    Magenta,
    Cyan,
}

impl LabelColor {
    /// The nearest theme token for a hex color such as `#5e6ad2`. Grays and
    /// colors that do not parse map to `Gray`.
    pub fn from_hex(hex: &str) -> Self {
        let hex = hex.trim().trim_start_matches('#');
        let Ok(v) = u32::from_str_radix(hex, 16) else {
            return Self::Gray;
        };
        if hex.len() != 6 {
            return Self::Gray;
        }
        let (r, g, b) = (
            ((v >> 16) & 0xff) as f32 / 255.,
            ((v >> 8) & 0xff) as f32 / 255.,
            (v & 0xff) as f32 / 255.,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let light = (max + min) / 2.;
        let sat = if delta == 0. {
            0.
        } else {
            delta / (1. - (2. * light - 1.).abs())
        };
        if sat < 0.25 || delta < 0.08 {
            return Self::Gray;
        }
        let hue = if max == r {
            60. * (((g - b) / delta).rem_euclid(6.))
        } else if max == g {
            60. * ((b - r) / delta + 2.)
        } else {
            60. * ((r - g) / delta + 4.)
        };
        match hue {
            h if !(20.0..330.0).contains(&h) => Self::Red,
            h if h < 70. => Self::Yellow,
            h if h < 165. => Self::Green,
            h if h < 200. => Self::Cyan,
            h if h < 260. => Self::Blue,
            _ => Self::Magenta,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// The full name, `Group/name` for a label in a group.
    pub name: SharedString,
    pub color: LabelColor,
}

/// A date and time in local time, to the minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i16,
    pub month: i8,
    pub day: i8,
    pub hour: i8,
    pub minute: i8,
}

impl Date {
    pub fn from_timestamp(ts: jiff::Timestamp) -> Self {
        let t = ts.to_zoned(jiff::tz::TimeZone::system()).datetime();
        Self {
            year: t.year(),
            month: t.month(),
            day: t.day(),
            hour: t.hour(),
            minute: t.minute(),
        }
    }

    /// `Nov 3 14:05`, the short form the list shows.
    pub fn short(self) -> String {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let month = MONTHS[(self.month.clamp(1, 12) - 1) as usize];
        format!("{month} {} {:02}:{:02}", self.day, self.hour, self.minute)
    }
}

/// The stable ID of an issue. Menus, drags and the open task hold this, not
/// an index into the list, because a snapshot reorders the list.
pub use llmtodo_core::IssueId;

#[derive(Debug, Clone)]
pub struct Issue {
    /// The server's ID. It never changes, unlike the key, which a team move
    /// changes, and unlike the issue's index in the list.
    pub id: IssueId,
    pub key: SharedString,
    pub team: SharedString,
    /// The project slug, when the issue has one.
    pub project: Option<SharedString>,
    pub title: SharedString,
    pub status: Status,
    pub assignee: Option<Assignee>,
    pub labels: Vec<Label>,
    pub added: Date,
    /// Empty until the app holds the text: see `description_loaded`.
    pub description: SharedString,
    /// Whether `description` holds the text. The snapshot leaves the text
    /// out, and the app fetches it when a task opens. A single-issue fetch
    /// carries it.
    pub description_loaded: bool,
    /// The open owner questions in the description. The server sends it,
    /// and the store updates it when the app changes the text.
    pub open_questions: u32,
    /// The description version, sent as `base_version` on a save.
    pub version: i64,
    /// In the Trash. `:restore` brings it back.
    pub deleted: bool,
    /// When the task reached Done or Canceled.
    pub closed_at: Option<jiff::Timestamp>,
    /// Set while an agent works on the task.
    pub claim: Option<Claim>,
    /// Set while an agent reads or fills in the task without a claim.
    pub analyzing: Option<Analyzing>,
    /// Who the task waits on: an agent can pick it up, or the owner must
    /// answer first.
    pub waiting_on: Option<WaitingOn>,
    /// The longest chain of open blockers. The server computes it.
    pub depth: u32,
    /// The open tasks this one waits for directly.
    pub open_blockers: Vec<Blocker>,
    /// The token cost so far, in dollars. None when no usage is known.
    pub cost: Option<f64>,
    /// Active agent working time so far, in seconds. None when unknown.
    pub active_seconds: Option<u64>,
    /// The time estimate in minutes, set before work starts.
    pub estimate_minutes: Option<u32>,
    /// The cost estimate in dollars, set before work starts.
    pub estimate_cost: Option<f64>,
}

/// A cost as the app shows it. Amounts too small for cents still show that
/// the task cost something.
pub fn format_cost(usd: f64) -> String {
    if usd < 0.01 {
        "<$0.01".to_string()
    } else if usd < 1000.0 {
        format!("${usd:.2}")
    } else {
        format!("${usd:.0}")
    }
}

/// An open task that another task waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub key: SharedString,
    pub title: SharedString,
}

impl Blocker {
    pub fn from_api(issue: &api::Issue) -> Vec<Self> {
        issue
            .open_blockers
            .iter()
            .map(|b| Self {
                key: b.key.to_string().into(),
                title: b.title.clone().into(),
            })
            .collect()
    }
}

/// The moment a closed task must be older than to count as `days` old.
pub fn hide_cutoff(days: u32, now: jiff::Timestamp) -> jiff::Timestamp {
    let age = jiff::SignedDuration::from_hours(i64::from(days) * 24);
    now.checked_sub(age).unwrap_or(now)
}

impl Issue {
    /// The LLM status at `now`. A claim wins over an Analyzing mark.
    pub fn llm_status(&self, now: jiff::Timestamp) -> Option<LlmStatus> {
        if let Some(claim) = &self.claim {
            return Some(LlmStatus::Locked(claim.clone()));
        }
        self.analyzing
            .as_ref()
            .filter(|a| a.is_live(now))
            .map(|a| LlmStatus::Analyzing(a.clone()))
    }

    /// The sort rank of the LLM status at `now`: Locked, then Analyzing,
    /// then nothing.
    pub fn llm_rank(&self, now: jiff::Timestamp) -> u8 {
        if self.claim.is_some() {
            LLM_RANK_LOCKED
        } else if self.analyzing.as_ref().is_some_and(|a| a.is_live(now)) {
            LLM_RANK_ANALYZING
        } else {
            LLM_RANK_NONE
        }
    }

    /// Whether the task is closed and closed before `cutoff`.
    pub fn closed_before(&self, cutoff: jiff::Timestamp) -> bool {
        self.status.is_closed() && self.closed_at.is_some_and(|at| at < cutoff)
    }

    /// Keeps the text of `old`, the app's earlier copy, when this copy came
    /// without text and has the same version.
    pub fn keep_text_of(&mut self, old: &Issue) {
        if !self.description_loaded && old.description_loaded && old.version == self.version {
            self.description = old.description.clone();
            self.description_loaded = true;
        }
    }

    /// Maps the status, labels and project again after the catalog
    /// changed. `renames` carries the names that changed.
    pub fn remap(&mut self, catalog: &Catalog, renames: &Renames) {
        let status = renames
            .statuses
            .get(&(self.team.clone(), self.status.name.clone()))
            .unwrap_or(&self.status.name)
            .clone();
        self.status = catalog.status_named(&self.team, &status, self.status.kind);
        let mut labels: Vec<Label> = self
            .labels
            .iter()
            .map(|l| catalog.label(renames.labels.get(&l.name).unwrap_or(&l.name)))
            .collect();
        catalog.sort_labels(&mut labels);
        self.labels = labels;
        if let Some(new) = self.project.as_ref().and_then(|p| renames.projects.get(p)) {
            self.project = Some(new.clone());
        }
    }

    pub fn from_api(issue: &api::Issue, catalog: &Catalog) -> Self {
        let mut labels: Vec<Label> = issue.labels.iter().map(|l| catalog.label(l)).collect();
        catalog.sort_labels(&mut labels);
        Self {
            id: issue.id,
            key: issue.key.to_string().into(),
            team: issue.team.clone().into(),
            project: issue.project.clone().map(Into::into),
            title: issue.title.clone().into(),
            status: catalog.status(&issue.team, &issue.status),
            assignee: issue.assignee.as_ref().map(Assignee::from_actor),
            labels,
            added: Date::from_timestamp(issue.created_at),
            description: issue.description.clone().unwrap_or_default().into(),
            description_loaded: issue.description.is_some(),
            open_questions: issue.open_questions,
            version: issue.description_version,
            deleted: issue.deleted_at.is_some(),
            closed_at: issue.completed_at,
            claim: issue.claim.as_ref().and_then(Claim::from_api),
            analyzing: issue.analyzing.as_ref().map(Analyzing::from_api),
            waiting_on: issue.waiting_on,
            depth: issue.depth,
            open_blockers: Blocker::from_api(issue),
            cost: issue.cost_usd,
            active_seconds: issue.active_seconds,
            estimate_minutes: issue.estimate_minutes,
            estimate_cost: issue.estimate_cost_usd,
        }
    }
}

/// A team or a project as the sidebar lists it: the key or slug that
/// filters issues, and the name it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub key: SharedString,
    pub name: SharedString,
}

/// A project document as the sidebar lists it. The body loads when the
/// document opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocEntry {
    pub id: llmtodo_core::DocumentId,
    pub title: SharedString,
    /// The project slug.
    pub project: SharedString,
    pub version: i64,
}

/// The statuses of every team and the labels, from the server.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Each team's key with its statuses in team order.
    pub teams: Vec<(SharedString, Vec<Status>)>,
    /// Every team's key and name, in the server's order.
    pub team_names: Vec<Place>,
    /// Every live project's slug and name, by name.
    pub projects: Vec<Place>,
    /// The keys of the teams each project is linked to, by project slug.
    /// The sidebar lists a project under each of its teams.
    pub project_teams: HashMap<SharedString, Vec<SharedString>>,
    /// Each project's first linked repo as its web page, by project slug.
    /// The git panel links commits there.
    pub project_repos: Vec<(SharedString, String)>,
    /// Every saved view, by name. A view's key is its name.
    pub views: Vec<Place>,
    /// Every live project document, by title.
    pub documents: Vec<DocEntry>,
    /// Every label that can go on an issue, groups left out.
    pub labels: Vec<Label>,
    /// The server's IDs behind the names above, so a later catalog can tell
    /// a rename from a new entry.
    pub ids: CatalogIds,
}

/// The server's ID of each status, label and project in a catalog.
#[derive(Debug, Clone, Default)]
pub struct CatalogIds {
    /// ID, team key, status name.
    pub statuses: Vec<(llmtodo_core::StatusId, SharedString, SharedString)>,
    /// ID, full label name.
    pub labels: Vec<(llmtodo_core::LabelId, SharedString)>,
    /// ID, slug.
    pub projects: Vec<(llmtodo_core::ProjectId, SharedString)>,
}

/// The names that changed from one catalog to the next, old name first.
#[derive(Debug, Clone, Default)]
pub struct Renames {
    /// By team key and old status name.
    pub statuses: HashMap<(SharedString, SharedString), SharedString>,
    pub labels: HashMap<SharedString, SharedString>,
    /// Project slugs.
    pub projects: HashMap<SharedString, SharedString>,
}

/// The new name of each ID in `new` whose name differs in `old`.
fn renamed<I: Copy + std::hash::Hash + Eq, K: Clone + std::hash::Hash + Eq>(
    old: &[(I, K)],
    new: &[(I, K)],
) -> HashMap<K, K> {
    let old: HashMap<I, &K> = old.iter().map(|(id, k)| (*id, k)).collect();
    new.iter()
        .filter_map(|(id, k)| {
            let before = old.get(id)?;
            (*before != k).then(|| ((*before).clone(), k.clone()))
        })
        .collect()
}

impl Catalog {
    pub fn from_api(
        teams: &[api::Team],
        labels: &[api::Label],
        projects: &[api::Project],
        views: &[api::SavedView],
        documents: &[api::Document],
    ) -> Self {
        let (teams_api, labels_api, projects_api) = (teams, labels, projects);
        let mut documents: Vec<DocEntry> = documents
            .iter()
            .filter(|d| d.deleted_at.is_none())
            .map(|d| DocEntry {
                id: d.id,
                title: d.title.clone().into(),
                project: d.project.clone().into(),
                version: d.version,
            })
            .collect();
        documents.sort_by_key(|d| d.title.to_lowercase());
        let mut views: Vec<Place> = views
            .iter()
            .map(|v| Place {
                key: v.name.clone().into(),
                name: v.name.clone().into(),
            })
            .collect();
        views.sort_by_key(|v| v.name.to_lowercase());
        let team_names = teams
            .iter()
            .map(|t| Place {
                key: t.key.clone().into(),
                name: t.name.clone().into(),
            })
            .collect();
        let project_repos = projects
            .iter()
            .filter(|p| p.deleted_at.is_none())
            .filter_map(|p| {
                let web = p
                    .repos
                    .iter()
                    .find_map(|r| llmtodo_client::git::repo_web_url(r))?;
                Some((p.slug.clone().into(), web))
            })
            .collect();
        let project_teams = projects
            .iter()
            .filter(|p| p.deleted_at.is_none())
            .map(|p| {
                let teams = p.teams.iter().map(|t| t.clone().into()).collect();
                (p.slug.clone().into(), teams)
            })
            .collect();
        let mut projects: Vec<Place> = projects
            .iter()
            .filter(|p| p.deleted_at.is_none())
            .map(|p| Place {
                key: p.slug.clone().into(),
                name: p.name.clone().into(),
            })
            .collect();
        projects.sort_by_key(|p| p.name.to_lowercase());
        let teams = teams
            .iter()
            .map(|t| {
                let mut statuses: Vec<Status> = t
                    .statuses
                    .iter()
                    .map(|s| Status::new(s.name.clone(), s.kind, s.position).with_review(s.review))
                    .collect();
                statuses.sort_by_key(Status::rank);
                (t.key.clone().into(), statuses)
            })
            .collect();
        let mut labels: Vec<Label> = labels
            .iter()
            .filter(|l| !l.is_group)
            .map(|l| Label {
                name: l.full_name.clone().into(),
                color: LabelColor::from_hex(&l.color),
            })
            .collect();
        labels.sort_by_key(|l| l.name.to_lowercase());
        let ids = CatalogIds {
            statuses: teams_api
                .iter()
                .flat_map(|t| {
                    t.statuses
                        .iter()
                        .map(|s| (s.id, t.key.clone().into(), s.name.clone().into()))
                })
                .collect(),
            labels: labels_api
                .iter()
                .filter(|l| !l.is_group)
                .map(|l| (l.id, l.full_name.clone().into()))
                .collect(),
            projects: projects_api
                .iter()
                .map(|p| (p.id, p.slug.clone().into()))
                .collect(),
        };
        Self {
            teams,
            team_names,
            project_teams,
            projects,
            project_repos,
            views,
            documents,
            labels,
            ids,
        }
    }

    /// The names that changed since `old`, matched by ID.
    pub fn renames_from(&self, old: &Catalog) -> Renames {
        type StatusRow = (llmtodo_core::StatusId, SharedString, SharedString);
        let key = |v: &[StatusRow]| -> Vec<(llmtodo_core::StatusId, (SharedString, SharedString))> {
            v.iter()
                .map(|(id, t, n)| (*id, (t.clone(), n.clone())))
                .collect()
        };
        Renames {
            statuses: renamed(&key(&old.ids.statuses), &key(&self.ids.statuses))
                .into_iter()
                .map(|(old, (_, name))| (old, name))
                .collect(),
            labels: renamed(&old.ids.labels, &self.ids.labels),
            projects: renamed(&old.ids.projects, &self.ids.projects),
        }
    }

    /// The web page of a project's linked repo, when it has one.
    pub fn project_repo(&self, slug: &str) -> Option<&str> {
        self.project_repos
            .iter()
            .find(|(s, _)| s.as_ref() == slug)
            .map(|(_, web)| web.as_str())
    }

    /// One team's statuses, active ones first.
    pub fn team_statuses(&self, team: &str) -> &[Status] {
        self.teams
            .iter()
            .find(|(k, _)| k.as_ref() == team)
            .map_or(&[], |(_, s)| s.as_slice())
    }

    /// Every status name across teams, once each, active ones first.
    pub fn all_statuses(&self) -> Vec<Status> {
        let mut out: Vec<Status> = Vec::new();
        for (_, statuses) in &self.teams {
            for s in statuses {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        out.sort_by_key(Status::rank);
        out
    }

    pub fn status(&self, team: &str, s: &api::StatusRef) -> Status {
        self.status_named(team, &s.name, s.kind)
            .with_review(s.review)
    }

    /// The team's status called `name`. An unknown name keeps `kind`.
    pub fn status_named(&self, team: &str, name: &str, kind: StatusType) -> Status {
        self.team_statuses(team)
            .iter()
            .find(|x| x.name == name)
            .cloned()
            .unwrap_or_else(|| Status::new(name.to_string(), kind, 0))
    }

    /// The status `word` names for `team`: a status name, with `-` for
    /// spaces, or a type such as `done` or `todo` for its first status.
    pub fn find_status(&self, team: &str, word: &str) -> Option<Status> {
        let statuses = self.team_statuses(team);
        let wanted = word.trim().replace('-', " ");
        if let Some(s) = statuses
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(&wanted))
        {
            return Some(s.clone());
        }
        let kind = match wanted.to_lowercase().as_str() {
            "backlog" => StatusType::Backlog,
            "todo" | "unstarted" => StatusType::Unstarted,
            "started" | "progress" | "in progress" => StatusType::Started,
            "done" | "completed" => StatusType::Completed,
            "canceled" | "cancelled" => StatusType::Canceled,
            _ => return None,
        };
        statuses.iter().find(|s| s.kind == kind).cloned()
    }

    /// The label named `name`, ignoring case. An unknown name gets gray.
    pub fn label(&self, name: &str) -> Label {
        self.find_label(name).unwrap_or_else(|| Label {
            name: name.to_string().into(),
            color: LabelColor::Gray,
        })
    }

    pub fn find_label(&self, name: &str) -> Option<Label> {
        let name = name.trim();
        self.labels
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name))
            .or_else(|| {
                // `app` finds `Area/app` when only one group has it.
                let mut short = self.labels.iter().filter(|l| {
                    l.name
                        .rsplit_once('/')
                        .is_some_and(|(_, n)| n.eq_ignore_ascii_case(name))
                });
                match (short.next(), short.next()) {
                    (Some(l), None) => Some(l),
                    _ => None,
                }
            })
            .cloned()
    }

    /// Keeps tags in the same order as the label list.
    pub fn sort_labels(&self, labels: &mut [Label]) {
        labels.sort_by_key(|l| {
            self.labels
                .iter()
                .position(|x| x == l)
                .unwrap_or(usize::MAX)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_map_to_the_nearest_token() {
        assert_eq!(LabelColor::from_hex("#8b8d98"), LabelColor::Gray);
        assert_eq!(LabelColor::from_hex("#eb5757"), LabelColor::Red);
        assert_eq!(LabelColor::from_hex("#f2c94c"), LabelColor::Yellow);
        assert_eq!(LabelColor::from_hex("#4cb782"), LabelColor::Green);
        assert_eq!(LabelColor::from_hex("#26b5ce"), LabelColor::Cyan);
        assert_eq!(LabelColor::from_hex("#5e6ad2"), LabelColor::Blue);
        assert_eq!(LabelColor::from_hex("#bb87fc"), LabelColor::Magenta);
        assert_eq!(LabelColor::from_hex("nope"), LabelColor::Gray);
    }

    #[test]
    fn status_words_find_the_team_status() {
        let catalog = crate::model::demo::catalog();
        let find = |w| catalog.find_status("LLM", w).map(|s| s.name.to_string());
        assert_eq!(find("in-review").as_deref(), Some("In Review"));
        assert_eq!(find("done").as_deref(), Some("Done"));
        assert_eq!(find("todo").as_deref(), Some("Todo"));
        assert_eq!(find("started").as_deref(), Some("In Progress"));
        assert_eq!(find("nope"), None);
    }
}

#[cfg(test)]
mod date_tests {
    use super::Date;

    #[test]
    fn the_short_form_has_the_hour_and_minute() {
        let d = Date {
            year: 2026,
            month: 11,
            day: 3,
            hour: 9,
            minute: 5,
        };
        assert_eq!(d.short(), "Nov 3 09:05");
    }
}
