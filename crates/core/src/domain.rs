//! Domain enums shared by every crate.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                let s = s.trim().to_ascii_lowercase();
                $(if s == $text { return Ok($name::$variant); })+
                let all: Vec<&str> = vec![$($text),+];
                Err(format!("unknown {} {s:?}; expected one of: {}", stringify!($name), all.join(", ")))
            }
        }
    };
}

string_enum!(
    /// The fixed type behind every team status.
    StatusType {
        Backlog => "backlog",
        Unstarted => "unstarted",
        Started => "started",
        Completed => "completed",
        Canceled => "canceled",
    }
);

impl StatusType {
    /// "Open" means any type other than completed or canceled.
    pub fn is_open(self) -> bool {
        !matches!(self, StatusType::Completed | StatusType::Canceled)
    }
}

string_enum!(
    /// Linear's five priority levels.
    Priority {
        None => "none",
        Urgent => "urgent",
        High => "high",
        Medium => "medium",
        Low => "low",
    }
);

impl Priority {
    /// The number Linear uses: 0 none, 1 urgent ... 4 low.
    pub fn as_number(self) -> u8 {
        match self {
            Priority::None => 0,
            Priority::Urgent => 1,
            Priority::High => 2,
            Priority::Medium => 3,
            Priority::Low => 4,
        }
    }

    pub fn from_number(n: i64) -> Option<Self> {
        Some(match n {
            0 => Priority::None,
            1 => Priority::Urgent,
            2 => Priority::High,
            3 => Priority::Medium,
            4 => Priority::Low,
            _ => return None,
        })
    }

    /// Sort rank: urgent first, no priority last.
    pub fn sort_rank(self) -> u8 {
        match self {
            Priority::None => 5,
            p => p.as_number(),
        }
    }
}

string_enum!(
    /// Who an issue waits on: an agent can pick it up, or the owner must
    /// answer first. At most one of the two.
    WaitingOn {
        Agent => "agent",
        Owner => "owner",
    }
);

string_enum!(
    ActorKind {
        Human => "human",
        Agent => "agent",
        System => "system",
    }
);

string_enum!(
    /// A relation as stored. `blocked-by` is stored as the reverse `blocks`.
    RelationType {
        Blocks => "blocks",
        Related => "related",
        Duplicate => "duplicate",
    }
);

string_enum!(
    /// Who wrote a relation. `order` edges come from the order of a
    /// parent's sub-issues, and the server rewrites them on each order
    /// change. Every other relation is `manual`.
    RelationSource {
        Manual => "manual",
        Order => "order",
    }
);

// `string_enum!` cannot mark a variant `#[default]`, so this is by hand.
#[allow(clippy::derivable_impls)]
impl Default for RelationSource {
    fn default() -> Self {
        RelationSource::Manual
    }
}

string_enum!(
    /// A relation as a user names it on the command line.
    RelationInput {
        Blocks => "blocks",
        BlockedBy => "blocked-by",
        Related => "related",
        Duplicate => "duplicate",
    }
);

impl RelationInput {
    /// Returns the stored type and whether the two issues swap places.
    pub fn normalize(self) -> (RelationType, bool) {
        match self {
            RelationInput::Blocks => (RelationType::Blocks, false),
            RelationInput::BlockedBy => (RelationType::Blocks, true),
            RelationInput::Related => (RelationType::Related, false),
            RelationInput::Duplicate => (RelationType::Duplicate, false),
        }
    }
}

string_enum!(
    ProjectState {
        Backlog => "backlog",
        Planned => "planned",
        Started => "started",
        Paused => "paused",
        Completed => "completed",
        Canceled => "canceled",
    }
);

string_enum!(
    AttachmentKind {
        Link => "link",
        PullRequest => "pull_request",
        Commit => "commit",
        Path => "path",
        File => "file",
    }
);

/// The default statuses of a new team: name, type, color.
pub const DEFAULT_STATUSES: &[(&str, StatusType, &str)] = &[
    ("Backlog", StatusType::Backlog, "#95a2b3"),
    ("Todo", StatusType::Unstarted, "#e2e2e2"),
    ("In Progress", StatusType::Started, "#f2c94c"),
    ("In Review", StatusType::Started, "#5e6ad2"),
    ("Done", StatusType::Completed, "#5e6ad2"),
    ("Canceled", StatusType::Canceled, "#95a2b3"),
    ("Duplicate", StatusType::Canceled, "#95a2b3"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for s in StatusType::ALL {
            assert_eq!(s.as_str().parse::<StatusType>().unwrap(), *s);
        }
        assert_eq!(
            serde_json::to_string(&RelationInput::BlockedBy).unwrap(),
            "\"blocked-by\""
        );
        assert!("nope".parse::<Priority>().is_err());
    }

    #[test]
    fn priority_order() {
        let mut v = vec![Priority::None, Priority::Low, Priority::Urgent];
        v.sort_by_key(|p| p.sort_rank());
        assert_eq!(v, [Priority::Urgent, Priority::Low, Priority::None]);
    }
}
