//! The names an activity event uses: what kind of thing changed, and which
//! field of it.
//!
//! Both are strings on the wire and in the database. An unknown name, such
//! as one from a newer server, reads as `Other`, so a client never fails on
//! it.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! open_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            /// A name this build does not know.
            Other(String),
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(&self) -> &str {
                match self {
                    $($name::$variant => $text,)+
                    $name::Other(s) => s,
                }
            }

            /// Reads a name. An unknown name becomes `Other`.
            pub fn parse(s: &str) -> Self {
                match s {
                    $($text => $name::$variant,)+
                    other => $name::Other(other.to_string()),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Ok($name::parse(&Cow::<str>::deserialize(d)?))
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                let known: Vec<&str> = vec![$($text),+];
                json_schema!({
                    "type": "string",
                    "description": format!(
                        "One of: {}. Clients must accept other values too.",
                        known.join(", ")
                    ),
                })
            }
        }
    };
}

open_enum!(
    /// The kind of thing an activity event changed.
    EntityType {
        Issue => "issue",
        Project => "project",
        Milestone => "milestone",
        Document => "document",
        Team => "team",
        Status => "status",
        Label => "label",
        Comment => "comment",
        Relation => "relation",
        Claim => "claim",
        Attachment => "attachment",
        View => "view",
        Settings => "settings",
        SessionNote => "session_note",
    }
);

open_enum!(
    /// What an activity event did: `created`, `deleted` and the like, or
    /// the field that changed.
    ActivityField {
        Created => "created",
        Updated => "updated",
        Deleted => "deleted",
        Restored => "restored",
        Imported => "imported",
        Pruned => "pruned",
        Name => "name",
        Title => "title",
        Slug => "slug",
        Key => "key",
        Description => "description",
        Body => "body",
        State => "state",
        Status => "status",
        Priority => "priority",
        Assignee => "assignee",
        Labels => "labels",
        Project => "project",
        Milestone => "milestone",
        Parent => "parent",
        WaitingOn => "waiting_on",
        WantedExecutor => "wanted_executor",
        EstimateMinutes => "estimate_minutes",
        EstimateCostUsd => "estimate_cost_usd",
        EstimateReason => "estimate_reason",
        GitBranch => "git_branch",
        GitWorktree => "git_worktree",
        GitPr => "git_pr",
        Analyzing => "analyzing",
        Review => "review",
        Claimed => "claimed",
        Ended => "ended",
        Actor => "actor",
        RelationAdded => "relation_added",
        RelationRemoved => "relation_removed",
        CommentAdded => "comment_added",
        CommentEdited => "comment_edited",
        CommentDeleted => "comment_deleted",
        Attached => "attached",
        Detached => "detached",
        RepoLinked => "repo_linked",
        RepoUnlinked => "repo_unlinked",
        PrOpenStatus => "pr_open_status",
        PrMergedStatus => "pr_merged_status",
        SessionNote => "session_note",
        Settings => "settings",
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for f in ActivityField::ALL {
            assert_eq!(&ActivityField::parse(f.as_str()), f);
        }
        for e in EntityType::ALL {
            assert_eq!(&EntityType::parse(e.as_str()), e);
        }
        let f: ActivityField = serde_json::from_str(r#""brand_new""#).unwrap();
        assert_eq!(f, ActivityField::Other("brand_new".into()));
        assert_eq!(serde_json::to_string(&f).unwrap(), r#""brand_new""#);
        assert_eq!(
            serde_json::to_string(&ActivityField::RelationAdded).unwrap(),
            r#""relation_added""#
        );
    }
}
