//! Server health, stop and backup.

use axum::Json;
use axum::extract::State;
use llmtodo_api_types::*;
use serde_json::json;

use crate::db;
use crate::error::ApiResult;
use crate::state::AppState;
use crate::store::backend::Backend;

/// How long the health check waits for the database. Clients find the
/// server through the health check, so it answers even while the shared
/// database is away, and says so (LLM-470).
const HEALTH_DATABASE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

pub async fn health(State(st): State<AppState>) -> ApiResult<Json<Health>> {
    // Offline, the pool is the local copy, which always answers (LLM-471).
    let pool = st.pool();
    let start = std::time::Instant::now();
    let checked = tokio::time::timeout(HEALTH_DATABASE_WAIT, db::try_schema_version(&pool)).await;
    let round_trip_ms = start.elapsed().as_secs_f64() * 1000.0;
    let (schema_version, unreachable) = match checked {
        Ok(Ok(v)) => (v, None),
        Ok(Err(e)) => (
            0,
            Some(crate::error::lost_database(&e).unwrap_or_else(|| e.to_string())),
        ),
        Err(_) => (
            0,
            Some(format!(
                "no answer within {} seconds",
                HEALTH_DATABASE_WAIT.as_secs()
            )),
        ),
    };
    // A lost shared database with a local copy is offline mode, not an
    // outage: the server goes on serving the copy.
    let sync = crate::offline::health_state(&st, unreachable.as_deref()).await;
    let unreachable = if sync.offline { None } else { unreachable };
    Ok(Json(Health {
        version: VERSION.to_string(),
        pid: std::process::id(),
        data_dir: st.data_dir.display().to_string(),
        schema_version,
        is_primary: st.primary(),
        started_at: st.started_at,
        database: DatabaseInfo {
            backend: match pool.backend() {
                _ if st.shared.is_some() => DatabaseBackend::Postgres,
                Backend::Sqlite => DatabaseBackend::Sqlite,
                Backend::Postgres => DatabaseBackend::Postgres,
            },
            host: st.database_host.clone(),
            round_trip_ms,
            unreachable,
            sync,
        },
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
