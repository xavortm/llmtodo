//! Which issues the list shows, as the sidebar and the settings name it.

use std::collections::HashSet;

use gpui_shared_string::SharedString;

use crate::model::data::{Catalog, Issue, IssueId};

/// Which issues the list shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Scope {
    #[default]
    All,
    /// A team, by key.
    Team(SharedString),
    /// A project, by slug.
    Project(SharedString),
    /// A saved view, by name. The server decides which issues it holds.
    View(SharedString),
    /// Issues moved to the Trash.
    Trash,
    /// The owner inbox: every open owner question. It shows in place of
    /// the issue list.
    Inbox,
    /// A project document, by ID. The main area shows it in place of the
    /// issue list.
    Document(SharedString),
    /// The Settings view, in place of the list and the task panel. It is
    /// never saved as the last view.
    Settings,
}

/// The issues a saved view held when the server last answered.
#[derive(Debug, Clone, Default)]
pub struct ViewMembers {
    pub name: SharedString,
    pub ids: HashSet<IssueId>,
}

impl Scope {
    /// Whether the list shows `issue` in this scope. `view` holds the
    /// members of the saved view the server sent last.
    pub fn matches(&self, issue: &Issue, view: Option<&ViewMembers>) -> bool {
        match self {
            Self::Document(_) | Self::Inbox | Self::Settings => false,
            Self::Trash => issue.deleted,
            _ if issue.deleted => false,
            Self::All => true,
            Self::Team(key) => issue.team == *key,
            Self::Project(slug) => issue.project.as_ref() == Some(slug),
            Self::View(name) => view.is_some_and(|v| v.name == *name && v.ids.contains(&issue.id)),
        }
    }

    /// How the settings file stores it: `all`, `team:LLM`, `project:app`,
    /// `view:Open bugs`, `trash`.
    pub fn to_setting(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Team(key) => format!("team:{key}"),
            Self::Project(slug) => format!("project:{slug}"),
            Self::View(name) => format!("view:{name}"),
            Self::Trash => "trash".into(),
            Self::Inbox => "inbox".into(),
            Self::Document(id) => format!("doc:{id}"),
            Self::Settings => "settings".into(),
        }
    }

    pub fn from_setting(s: &str) -> Option<Self> {
        match s.split_once(':') {
            None if s == "all" => Some(Self::All),
            None if s == "trash" => Some(Self::Trash),
            None if s == "inbox" => Some(Self::Inbox),
            Some(("view", name)) if !name.is_empty() => Some(Self::View(name.to_string().into())),
            Some(("doc", id)) if uuid::Uuid::parse_str(id).is_ok() => {
                Some(Self::Document(id.to_string().into()))
            }
            Some(("team", key)) if !key.is_empty() => Some(Self::Team(key.to_string().into())),
            Some(("project", slug)) if !slug.is_empty() => {
                Some(Self::Project(slug.to_string().into()))
            }
            _ => None,
        }
    }

    /// The name the sidebar row shows, which the status bar repeats.
    pub fn name(&self, catalog: &Catalog) -> SharedString {
        let find = |places: &[crate::model::data::Place], key: &SharedString| {
            places
                .iter()
                .find(|p| p.key == *key)
                .map_or_else(|| key.clone(), |p| p.name.clone())
        };
        match self {
            Self::All => "All issues".into(),
            Self::Team(key) => find(&catalog.team_names, key),
            Self::Project(slug) => find(&catalog.projects, slug),
            Self::View(name) => name.clone(),
            Self::Trash => "Trash".into(),
            Self::Inbox => "Inbox".into(),
            Self::Settings => "Settings".into(),
            Self::Document(id) => catalog
                .documents
                .iter()
                .find(|d| d.id.to_string() == id.as_ref())
                .map_or_else(|| "A document".into(), |d| d.title.clone()),
        }
    }

    /// Whether the catalog still has this team, project or view.
    pub fn exists(&self, catalog: &Catalog) -> bool {
        match self {
            Self::All => true,
            Self::Team(key) => catalog.team_names.iter().any(|t| t.key == *key),
            Self::Project(slug) => catalog.projects.iter().any(|p| p.key == *slug),
            Self::View(name) => catalog.views.iter().any(|v| v.key == *name),
            Self::Trash | Self::Inbox | Self::Settings => true,
            Self::Document(id) => catalog
                .documents
                .iter()
                .any(|d| d.id.to_string() == id.as_ref()),
        }
    }
}
