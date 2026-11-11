//! Rules for versioned text: issue descriptions and documents.

use llmtodo_api_types::*;
use serde_json::json;

use crate::error::{ApiError, ApiResult};

/// Applies a full replace (version checked) and then patches.
pub fn apply_body_change(current: &str, version: i64, change: &BodyChange) -> ApiResult<String> {
    let mut text = current.to_string();
    if let Some(body) = &change.body {
        match (change.base_version, change.force) {
            (_, true) => {}
            (Some(base), false) if base == version => {}
            (Some(base), false) => {
                return Err(ApiError::new(ErrorCode::StaleVersion, format!("the text changed since version {base}; it is now version {version}. Read it again and retry"))
                    .with_details(json!({ "base_version": base, "current_version": version })));
            }
            (None, false) => {
                return Err(ApiError::bad_request(format!(
                    "a full replace needs base_version (current is {version}), or force; a section patch needs neither"
                ))
                .with_details(json!({ "current_version": version })));
            }
        }
        text = body.clone();
    }
    for p in &change.patches {
        text = llmtodo_core::apply_patch(&text, p)?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_change_rules() {
        let full = BodyChange {
            body: Some("new".into()),
            base_version: Some(2),
            ..Default::default()
        };
        assert_eq!(apply_body_change("old", 2, &full).unwrap(), "new");
        assert_eq!(
            apply_body_change("old", 3, &full).unwrap_err().code,
            ErrorCode::StaleVersion
        );
        let no_base = BodyChange {
            body: Some("new".into()),
            ..Default::default()
        };
        assert_eq!(
            apply_body_change("old", 3, &no_base).unwrap_err().code,
            ErrorCode::BadRequest
        );
        let patch = BodyChange {
            patches: vec![Patch::ReplaceExact {
                old: "o".into(),
                new: "0".into(),
            }],
            ..Default::default()
        };
        assert_eq!(apply_body_change("old", 9, &patch).unwrap(), "0ld");
    }
}
