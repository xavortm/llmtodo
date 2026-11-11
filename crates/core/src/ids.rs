//! One ID type per kind of record, so two kinds of ID cannot be swapped by
//! mistake. Each wraps a UUID and is a plain UUID string on the wire.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($($(#[$m:meta])* $name:ident;)+) => {$(
        $(#[$m])*
        #[derive(
            Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize, schemars::JsonSchema,
        )]
        #[serde(transparent)]
        #[schemars(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }

            /// An ID from a number, for tests and demo data.
            pub const fn from_u128(n: u128) -> Self {
                Self(Uuid::from_u128(n))
            }

            pub const fn nil() -> Self {
                Self(Uuid::nil())
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, uuid::Error> {
                Uuid::parse_str(s).map(Self)
            }
        }
    )+};
}

id_type! {
    /// An issue's ID. The key, such as `LLM-1`, can change; the ID cannot.
    IssueId;
    ProjectId;
    MilestoneId;
    TeamId;
    StatusId;
    LabelId;
    DocumentId;
    CommentId;
    AttachmentId;
    RelationId;
    ActorId;
    ViewId;
    WorkSessionId;
    SessionNoteId;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_plain_uuid_strings() {
        let u = Uuid::from_u128(7);
        let id = IssueId::from(u);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{u}\""));
        assert_eq!(serde_json::from_str::<IssueId>(&json).unwrap(), id);
        assert_eq!(id.to_string().parse::<IssueId>().unwrap(), id);
    }
}
