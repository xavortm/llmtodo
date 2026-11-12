//! The `/` filter: free text plus filter words.
//!
//! - `status:todo` keeps one status. The word is a status name with `-` for
//!   spaces, such as `in-review`, or a type: `backlog`, `todo`, `started`,
//!   `done`, `canceled`. `status:open` keeps open issues. Several status
//!   words keep issues in any of them.
//! - `label:bug` keeps issues with that label. Several label words keep
//!   issues with all of them.
//! - `assignee:me`, `assignee:none` or part of an actor name.
//! - `llm:any` keeps tasks with an agent on them, Locked or Analyzing.
//!   `llm:locked` and `llm:analyzing` keep one of the two. A bare `llm` is
//!   free text, because it matches every `LLM-` key.
//!
//! Every other word must appear in the key, the title or a label.
//! `:view save <name>` turns the filter and the sidebar scope into a saved
//! view on the server.
//!
//! The words become an `IssueFilter` from `llmtodo_core`, the filter the
//! server's queries use too. Only the free text stays here.

use llmtodo_api_types::{
    ASSIGNEE_OWNER, AssigneeRef, FilterSubject, IssueFilter, IssueQuery, StatusType,
};

use crate::model::data::{Assignee, Catalog, Issue, LLM_RANK_ANALYZING, OWNER_NAME, Status};
use crate::model::scope::Scope;

/// Which LLM status an `llm:` word keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmWord {
    Any,
    Locked,
    Analyzing,
}

impl LlmWord {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "any" => Some(Self::Any),
            "locked" => Some(Self::Locked),
            "analyzing" => Some(Self::Analyzing),
            _ => None,
        }
    }
}

/// The filter words as the owner typed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Free text, lowercased.
    pub text: String,
    pub statuses: Vec<String>,
    /// `status:open`.
    pub open: bool,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub llm: Option<LlmWord>,
}

/// A filter ready to run: the shared filter plus the free text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedFilter {
    pub text: String,
    pub shared: IssueFilter,
}

/// The status type a filter word names, if it names one.
fn status_type(word: &str) -> Option<StatusType> {
    Some(match word {
        "backlog" => StatusType::Backlog,
        "todo" | "unstarted" => StatusType::Unstarted,
        "started" | "progress" => StatusType::Started,
        "done" | "completed" => StatusType::Completed,
        "canceled" | "cancelled" => StatusType::Canceled,
        _ => return None,
    })
}

fn status_matches(status: &Status, word: &str) -> bool {
    status.name.to_lowercase().replace(' ', "-") == word
        || status_type(word).is_some_and(|kind| kind == status.kind)
}

/// An issue at one moment, as the shared filter reads it. The clock
/// matters because an Analyzing mark expires without a server event.
struct IssueAt<'a> {
    issue: &'a Issue,
    now: jiff::Timestamp,
}

impl FilterSubject for IssueAt<'_> {
    fn status_name(&self) -> &str {
        &self.issue.status.name
    }

    fn status_type(&self) -> StatusType {
        self.issue.status.kind
    }

    fn label_names(&self) -> impl Iterator<Item = &str> {
        self.issue.labels.iter().map(|l| l.name.as_ref())
    }

    fn assignee(&self) -> Option<AssigneeRef<'_>> {
        self.issue.assignee.as_ref().map(|a| match a {
            Assignee::Person(name) => AssigneeRef {
                name,
                is_owner: name.as_ref() == OWNER_NAME,
            },
            Assignee::Agent(agent) => AssigneeRef {
                name: &agent.name,
                is_owner: false,
            },
        })
    }

    fn claimed(&self) -> bool {
        self.issue.claim.is_some()
    }

    fn analyzing(&self) -> bool {
        self.issue.llm_rank(self.now) == LLM_RANK_ANALYZING
    }
}

impl Filter {
    pub fn parse(query: &str) -> Self {
        let mut out = Self::default();
        let mut text = Vec::new();
        let lower = query.to_lowercase();
        for word in lower.split_whitespace() {
            match word.split_once(':') {
                Some(("status", "open")) => out.open = true,
                Some(("status", v)) if !v.is_empty() => out.statuses.push(v.to_string()),
                Some(("label", v)) if !v.is_empty() => out.labels.push(v.to_string()),
                Some(("assignee", v)) if !v.is_empty() => out.assignee = Some(v.to_string()),
                Some(("llm", v)) if LlmWord::parse(v).is_some() => out.llm = LlmWord::parse(v),
                _ => text.push(word),
            }
        }
        out.text = text.join(" ");
        out
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The status names one status word keeps, across every team, in the
    /// catalog's order.
    fn status_names(word: &str, catalog: &Catalog) -> Vec<String> {
        catalog
            .all_statuses()
            .iter()
            .filter(|s| status_matches(s, word))
            .map(|s| s.name.to_string())
            .collect()
    }

    /// The shared filter for these words. A status word becomes the names
    /// of the statuses it names, so several words keep issues in any one.
    /// A word that names no status stays as it is, and matches nothing.
    pub fn shared(&self, catalog: &Catalog) -> IssueFilter {
        let mut statuses: Vec<String> = Vec::new();
        for word in &self.statuses {
            let mut names = Self::status_names(word, catalog);
            if names.is_empty() {
                names.push(word.clone());
            }
            for name in names {
                if !statuses.contains(&name) {
                    statuses.push(name);
                }
            }
        }
        IssueFilter {
            statuses,
            status_types: Vec::new(),
            open: self.open,
            labels: self.labels.clone(),
            assignee: self.assignee.as_ref().map(|who| match who.as_str() {
                // The app shows the owner as `me`. The server's `me` is the
                // caller, so the filter names the owner.
                OWNER_NAME => ASSIGNEE_OWNER.to_string(),
                other => other.to_string(),
            }),
            claimed: matches!(self.llm, Some(LlmWord::Any | LlmWord::Locked)),
            analyzing: matches!(self.llm, Some(LlmWord::Any | LlmWord::Analyzing)),
        }
    }

    /// The filter ready to run on the list.
    pub fn resolve(&self, catalog: &Catalog) -> ResolvedFilter {
        ResolvedFilter {
            text: self.text.clone(),
            shared: self.shared(catalog),
        }
    }

    /// The server query a saved view stores for this filter in `scope`.
    /// It holds the same shared filter as the list, with labels by their
    /// full names. Free text becomes a full-text search, which also looks
    /// in descriptions.
    pub fn to_query(&self, scope: &Scope, catalog: &Catalog) -> Result<IssueQuery, String> {
        let mut query = IssueQuery::default();
        match scope {
            Scope::All => {}
            Scope::Team(key) => query.teams.push(key.to_string()),
            Scope::Project(slug) => query.projects.push(slug.to_string()),
            Scope::View(_) | Scope::Trash | Scope::Document(_) | Scope::Inbox | Scope::Settings => {
                return Err(
                    "Pick All issues, a team or a project first: a view saves from those".into(),
                );
            }
        }
        if let Some(word) = self
            .statuses
            .iter()
            .find(|w| Self::status_names(w, catalog).is_empty())
        {
            return Err(format!("Unknown status: {word}"));
        }
        query.filter = self.shared(catalog);
        query.filter.labels = self
            .labels
            .iter()
            .map(|word| match catalog.find_label(word) {
                Some(label) => Ok(label.name.to_string()),
                None => Err(format!("Unknown label: {word}")),
            })
            .collect::<Result<_, _>>()?;
        query.text = (!self.text.is_empty()).then(|| self.text.clone());
        Ok(query)
    }
}

/// Whether `haystack`, lowercased, contains `needle`, which is lowercase
/// already. ASCII text, the common case, is lowercased into a buffer that
/// each thread keeps, so a keystroke does not allocate a copy of every
/// title. Other text is lowercased the usual way, with the full Unicode
/// rules.
pub(crate) fn contains_lowercase(haystack: &str, needle: &str) -> bool {
    thread_local! {
        static BUFFER: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    if !haystack.is_ascii() {
        return haystack.to_lowercase().contains(needle);
    }
    BUFFER.with_borrow_mut(|buffer| {
        buffer.clear();
        buffer.push_str(haystack);
        buffer.make_ascii_lowercase();
        buffer.contains(needle)
    })
}

impl ResolvedFilter {
    /// Whether `issue` passes at `now`. A pure function, so the list can
    /// run it off the main thread.
    pub fn matches(&self, issue: &Issue, now: jiff::Timestamp) -> bool {
        let text = self.text.as_str();
        let text_ok = text.is_empty()
            || contains_lowercase(&issue.title, text)
            || contains_lowercase(&issue.key, text)
            || issue
                .labels
                .iter()
                .any(|l| contains_lowercase(&l.name, text));
        text_ok && self.shared.matches(&IssueAt { issue, now })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_lowercase_matches_like_lowercasing_first() {
        for (hay, needle) in [
            ("Fix the List", "the l"),
            ("LLM-12", "llm-1"),
            ("LLM-12", "llm-13"),
            ("ab", "abc"),
            ("Straße", "straße"),
            ("ÉCOLE", "école"),
            ("x", ""),
        ] {
            assert_eq!(
                contains_lowercase(hay, needle),
                hay.to_lowercase().contains(needle),
                "{hay:?} {needle:?}"
            );
        }
    }

    fn demo() -> (Catalog, Vec<Issue>) {
        let catalog = crate::model::demo::catalog();
        let issues = crate::model::demo::issues(&catalog);
        (catalog, issues)
    }

    fn keys(filter: &str) -> Vec<String> {
        let (catalog, issues) = demo();
        let filter = Filter::parse(filter).resolve(&catalog);
        issues
            .iter()
            .filter(|i| filter.matches(i, jiff::Timestamp::now()))
            .map(|i| i.key.to_string())
            .collect()
    }

    #[test]
    fn words_split_from_free_text() {
        let f = Filter::parse("Status:Todo label:bug  start  assignee:me server");
        assert_eq!(f.statuses, ["todo"]);
        assert_eq!(f.labels, ["bug"]);
        assert_eq!(f.assignee.as_deref(), Some("me"));
        assert_eq!(f.text, "start server");
        assert!(Filter::parse("  ").is_empty());
    }

    #[test]
    fn status_words_take_names_and_types() {
        // LLM-101 and LLM-102 are In Progress, and no demo issue is In Review.
        assert_eq!(keys("status:in-progress"), ["LLM-101", "LLM-102"]);
        assert_eq!(keys("status:started"), ["LLM-101", "LLM-102"]);
        assert_eq!(keys("status:in-review"), Vec::<String>::new());
        let done = keys("status:done");
        assert_eq!(done, ["LLM-109", "LLM-110", "LLM-115", "LLM-119"]);
        // Two status words keep issues in either one.
        assert_eq!(keys("status:done status:started").len(), 6);
        assert_eq!(keys("status:open").len(), 16);
    }

    #[test]
    fn label_and_assignee_words_narrow() {
        assert_eq!(keys("label:bug"), ["LLM-116", "LLM-119"]);
        // Both labels must be on the issue.
        assert_eq!(keys("label:infra label:bug"), ["LLM-116"]);
        // Every fourth demo issue is the owner's, starting with LLM-101.
        assert_eq!(keys("assignee:me").len(), 5);
        assert!(keys("assignee:me").iter().all(|k| k != "LLM-102"));
        assert_eq!(keys("assignee:none").len(), 5);
        assert_eq!(keys("assignee:qwen").len(), 5);
        assert_eq!(keys("label:bug crash"), ["LLM-119"]);
    }

    #[test]
    fn llm_words_keep_tasks_with_an_agent_on_them() {
        // LLM-102 and LLM-103 are claimed, and an agent reads LLM-105.
        assert_eq!(keys("llm:any"), ["LLM-102", "LLM-103", "LLM-105"]);
        assert_eq!(keys("llm:locked"), ["LLM-102", "LLM-103"]);
        assert_eq!(keys("llm:analyzing"), ["LLM-105"]);
        // A bare word stays free text: every key here starts with LLM.
        assert_eq!(Filter::parse("llm").llm, None);
        assert_eq!(Filter::parse("llm").text, "llm");
        assert_eq!(Filter::parse("llm:nope").llm, None);
    }

    #[test]
    fn an_expired_mark_is_not_an_agent_at_work() {
        let (catalog, issues) = demo();
        let reading = issues.iter().find(|i| i.key.as_ref() == "LLM-105").unwrap();
        let expires = reading.analyzing.as_ref().unwrap().expires_at;
        let filter = Filter::parse("llm:any").resolve(&catalog);
        assert!(filter.matches(reading, expires - jiff::SignedDuration::from_secs(1)));
        assert!(!filter.matches(reading, expires));
    }

    #[test]
    fn llm_words_reach_the_server_query() {
        let (catalog, _) = demo();
        let query = |word: &str| Filter::parse(word).to_query(&Scope::All, &catalog).unwrap();
        let flags = |q: IssueQuery| (q.filter.claimed, q.filter.analyzing);
        assert_eq!(flags(query("llm:any")), (true, true));
        assert_eq!(flags(query("llm:locked")), (true, false));
        assert_eq!(flags(query("llm:analyzing")), (false, true));
        assert_eq!(flags(query("crash")), (false, false));
    }

    #[test]
    fn a_saved_view_gets_the_server_query() {
        let (catalog, _) = demo();
        let f = Filter::parse("status:started status:todo label:bug assignee:me crash");
        let q = f.to_query(&Scope::Project("app".into()), &catalog).unwrap();
        assert_eq!(q.projects, ["app"]);
        assert_eq!(q.filter.statuses, ["In Progress", "In Review", "Todo"]);
        assert_eq!(q.filter.labels, ["bug"]);
        assert_eq!(q.filter.assignee.as_deref(), Some("owner"));
        assert_eq!(q.text.as_deref(), Some("crash"));

        let err = Filter::parse("label:nope").to_query(&Scope::All, &catalog);
        assert_eq!(err.unwrap_err(), "Unknown label: nope");
        let err = Filter::parse("status:nope").to_query(&Scope::All, &catalog);
        assert_eq!(err.unwrap_err(), "Unknown status: nope");
        assert!(Filter::default().to_query(&Scope::Trash, &catalog).is_err());
    }

    #[test]
    fn an_unknown_status_word_keeps_nothing() {
        assert!(keys("status:nope").is_empty());
    }

    #[test]
    fn the_list_and_a_saved_view_hold_the_same_filter() {
        let (catalog, _) = demo();
        let f = Filter::parse("status:started label:bug assignee:me llm:any");
        let q = f.to_query(&Scope::All, &catalog).unwrap();
        assert_eq!(q.filter, f.resolve(&catalog).shared);
    }
}
