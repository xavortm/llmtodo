//! The product rules. One module per domain noun. Each write takes
//! `&mut Write`, and reads that carry a rule take a connection. Routes call
//! this layer, and this layer calls `store`.

pub mod activity;
pub mod actors;
pub mod analyzing;
pub mod attachments;
pub mod claims;
pub mod comments;
pub mod documents;
pub mod issues;
pub mod labels;
pub mod projects;
pub mod prune;
pub mod questions;
pub mod relations;
pub mod reviews;
pub mod revisions;
pub mod sessions;
pub mod settings;
pub mod sync;
pub mod teams;
pub mod usage;
pub mod views;

use crate::error::{ApiError, ApiResult};

/// A JSON value for an activity event.
pub(crate) fn j<T: serde::Serialize>(v: T) -> Option<serde_json::Value> {
    Some(serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
}

pub(crate) fn check_color(c: &str) -> ApiResult<String> {
    let c = c.trim();
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if ok {
        Ok(c.to_lowercase())
    } else {
        Err(ApiError::validation(format!(
            "color must look like #aabbcc, got {c:?}"
        )))
    }
}
