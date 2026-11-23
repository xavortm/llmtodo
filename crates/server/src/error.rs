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
        let lost = (self.code == ErrorCode::DatabaseUnreachable).then(|| LostDatabase {
            reason: self.details["reason"]
                .as_str()
                .unwrap_or(&self.message)
                .to_string(),
        });
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: self.message,
                details: self.details,
            },
        };
        let mut res = (status, Json(body)).into_response();
        if let Some(lost) = lost {
            res.extensions_mut().insert(lost);
        }
        res
    }
}

/// Marks a response whose request lost the database. The router reads it
/// to name the database host, which the error does not know.
#[derive(Debug, Clone)]
pub struct LostDatabase {
    pub reason: String,
}

/// The reason a database error means the server lost its database, or
/// `None` for an error in the query itself (LLM-470). The network is down,
/// the host refused the connection, the pool waited too long, or Postgres
/// closed the connection, as Neon does when it suspends.
pub fn lost_database(e: &sqlx::Error) -> Option<String> {
    match e {
        sqlx::Error::Io(e) => Some(e.to_string()),
        sqlx::Error::Tls(e) => Some(format!("TLS: {e}")),
        sqlx::Error::PoolTimedOut => Some(format!(
            "no connection within {} seconds",
            crate::db::ACQUIRE_WAIT.as_secs()
        )),
        sqlx::Error::PoolClosed => Some("the connection pool is closed".into()),
        sqlx::Error::WorkerCrashed => Some("the database connection crashed".into()),
        sqlx::Error::Protocol(m) => Some(m.clone()),
        // SQLSTATE class 08 is a connection error, and 57P01 to 57P03 mean
        // the server shut down or cannot take connections yet.
        sqlx::Error::Database(db) => db
            .code()
            .filter(|c| c.starts_with("08") || matches!(c.as_ref(), "57P01" | "57P02" | "57P03"))
            .map(|_| db.message().to_string()),
        _ => None,
    }
}

impl ApiError {
    /// The server cannot reach its shared database. The router adds the
    /// host, which this layer does not know.
    pub fn database_unreachable(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::new(
            ErrorCode::DatabaseUnreachable,
            format!("Shared database unreachable: {reason}"),
        )
        .with_details(serde_json::json!({ "reason": reason }))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        if let Some(reason) = lost_database(&e) {
            return ApiError::database_unreachable(reason);
        }
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
        if let Some(reason) = e.downcast_ref::<sqlx::Error>().and_then(lost_database) {
            return ApiError::database_unreachable(reason);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_connection_is_an_unreachable_database() {
        let e = ApiError::from(sqlx::Error::PoolTimedOut);
        assert_eq!(e.code, ErrorCode::DatabaseUnreachable);
        assert_eq!(e.code.exit_code(), 7);
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let e = ApiError::from(sqlx::Error::Io(io));
        assert_eq!(e.code, ErrorCode::DatabaseUnreachable);
        assert!(
            e.message.starts_with("Shared database unreachable"),
            "{}",
            e.message
        );
    }

    #[test]
    fn a_bad_query_stays_an_internal_error() {
        let e = ApiError::from(sqlx::Error::RowNotFound);
        assert_eq!(e.code, ErrorCode::Internal);
    }
}
