use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use llmtodo_api_types::{ErrorBody, ErrorCode, ErrorDetail};
use serde_json::Value;

/// An error that becomes `{"error": {"code", "message", "details"}}`.
#[derive(Debug)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Value,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Value::Null,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, m)
    }

    pub fn not_found(what: &str, reference: &str) -> Self {
        Self::new(
            ErrorCode::NotFound,
            format!("{what} {reference:?} not found"),
        )
        .with_details(serde_json::json!({ "kind": what, "ref": reference }))
    }

    pub fn validation(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Validation, m)
    }

    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, m)
    }

    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, m)
    }

    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, m)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.code == ErrorCode::Internal {
            tracing::error!(message = %self.message, "internal error");
        }
        let status = StatusCode::from_u16(self.code.http_status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: self.message,
                details: self.details,
            },
        };
        (status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &e {
            let msg = db.message();
            if msg.contains("UNIQUE constraint failed") {
                return ApiError::conflict(format!("already exists: {msg}"));
            }
            if msg.contains("FOREIGN KEY constraint failed") {
                return ApiError::validation(format!("still in use or missing a reference: {msg}"));
            }
            if msg.contains("CHECK constraint failed") {
                return ApiError::validation(msg.to_string());
            }
        }
        ApiError::internal(format!("database error: {e}"))
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::internal(format!("{e:#}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::internal(format!("json error: {e}"))
    }
}

impl From<llmtodo_core::PatchError> for ApiError {
    fn from(e: llmtodo_core::PatchError) -> Self {
        match e {
            llmtodo_core::PatchError::Question(m) => ApiError::validation(m),
            e => ApiError::new(ErrorCode::PatchMismatch, e.to_string()),
        }
    }
}
