//! The Linear import and stored files.

use axum::Json;
use axum::extract::{Path, State};
use llmtodo_api_types::*;

use crate::error::{ApiError, ApiResult};
use crate::import;
use crate::state::AppState;
use crate::store;

pub async fn linear(
    State(st): State<AppState>,
    Json(req): Json<LinearImportRequest>,
) -> ApiResult<Json<ImportReport>> {
    if req.token.trim().is_empty() {
        return Err(ApiError::bad_request(
            "the import needs a Linear API key; set LINEAR_API_KEY",
        ));
    }
    let fetch = import::LinearHttp::new(&req.token);
    let mut report = import::run(&st, &fetch, &req).await?;
    report.requests = fetch.requests.load(std::sync::atomic::Ordering::Relaxed);
    Ok(Json(report))
}

/// Serves a stored file by its hash.
pub async fn file(
    State(st): State<AppState>,
    Path(hash): Path<String>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("file hash must be 64 hex characters"));
    }
    let mime: Option<String> = store::import::file_mime(&mut *st.pool().acquire().await?, &hash)
        .await?
        .flatten();
    // Streamed in chunks, so a big file neither sits in memory nor blocks
    // an async worker while it is read.
    let file = tokio::fs::File::open(st.data_dir.join("files").join(&hash))
        .await
        .map_err(|_| ApiError::not_found("file", &hash))?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            mime.unwrap_or_else(|| "application/octet-stream".into()),
        )],
        body,
    )
        .into_response())
}
