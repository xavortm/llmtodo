//! Domain types and Markdown helpers for llmtodo.

pub mod bundle;
pub mod data_dir;
pub mod domain;
pub mod executor;
pub mod filter;
pub mod ids;
pub mod key;
pub mod markdown;
pub mod questions;
pub mod review;

pub use domain::*;
pub use executor::{ActorFields, WantedExecutor};
pub use filter::{ASSIGNEE_NONE, ASSIGNEE_OWNER, AssigneeRef, FilterSubject, IssueFilter};
pub use ids::{
    ActorId, AttachmentId, CommentId, DocumentId, IssueId, LabelId, MilestoneId, ProjectId,
    RelationId, SessionNoteId, StatusId, TeamId, ViewId, WorkSessionId,
};
pub use key::{IssueKey, KeyError, find_keys, find_keys_any_case, normalize_team_key};
pub use markdown::{Patch, PatchError, Section, SectionKind, apply_patch, sections};
pub use questions::{Question, QuestionOption, open_questions, questions};
pub use review::{Review, is_default_review_status, render_review, review, review_missing};
