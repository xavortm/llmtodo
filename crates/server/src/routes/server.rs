//! Server health, stop and backup.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;
use serde_json::json;

use crate::db;
use crate::error::ApiResult;
use crate::state::AppState;

pub async fn health(State(st): State<AppState>) -> ApiResult<Json<Health>> {
    Ok(Json(Health {
        version: VERSION.to_string(),
        pid: std::process::id(),
        data_dir: st.data_dir.display().to_string(),
        schema_version: db::schema_version(&st.pool()).await,
        is_primary: st.primary(),
        started_at: st.started_at,
    }))
}

pub async fn stop(State(st): State<AppState>) -> Json<serde_json::Value> {
    tracing::info!("stop requested");
    let st2 = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        st2.shutdown.notify_waiters();
    });
    Json(json!({ "stopping": true, "pid": std::process::id() }))
}

pub async fn backup(State(st): State<AppState>) -> ApiResult<Json<BackupReport>> {
    let path = db::backup(&st.pool(), &st.data_dir, "manual").await?;
    Ok(Json(BackupReport {
        path: path.display().to_string(),
    }))
}
