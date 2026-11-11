//! Workspace settings.

use llmtodo_api_types::*;

use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Write};
use crate::store;

pub async fn read(st: &AppState) -> ApiResult<Settings> {
    let get = |k: &'static str| {
        let pool = st.pool().clone();
        async move { db::get_setting(&pool, k).await }
    };
    Ok(Settings {
        workspace_name: get("workspace_name")
            .await?
            .unwrap_or_else(|| "llmtodo".into()),
        default_team: get("default_team").await?.filter(|s| !s.is_empty()),
        github_poll_seconds: get("github_poll_seconds")
            .await?
            .and_then(|s| s.parse().ok())
            .unwrap_or(300),
        sync_folder: get("sync_folder").await?.filter(|s| !s.is_empty()),
        model_prices: crate::store::usage::prices(&mut *st.pool().acquire().await?).await?,
    })
}

pub async fn update(w: &mut Write<'_>, req: UpdateSettings) -> ApiResult<()> {
    if let Some(v) = &req.workspace_name {
        db::set_setting(w.c(), "workspace_name", v.trim()).await?;
    }
    if let Some(v) = &req.default_team {
        let v = v.trim();
        let key = if v.is_empty() {
            String::new()
        } else {
            crate::store::teams::team_row(w.c(), v).await?.key
        };
        db::set_setting(w.c(), "default_team", &key).await?;
    }
    if let Some(v) = req.github_poll_seconds {
        if v < 60 {
            return Err(ApiError::validation(
                "GitHub poll interval must be at least 60 seconds",
            ));
        }
        db::set_setting(w.c(), "github_poll_seconds", &v.to_string()).await?;
    }
    if let Some(v) = &req.sync_folder {
        db::set_setting(w.c(), "sync_folder", v.trim()).await?;
    }
    if let Some(new) = &req.model_prices {
        if new.values().any(|p| {
            [
                p.input,
                p.output,
                p.cache_write_5m,
                p.cache_write_1h,
                p.cache_read,
            ]
            .iter()
            .any(|x| !x.is_finite() || *x < 0.0)
        }) {
            return Err(ApiError::validation("prices must be zero or more"));
        }
        let raw: Option<String> = store::settings::model_prices_raw(w.c()).await?;
        let mut stored: std::collections::BTreeMap<String, ModelPrice> = raw
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or_default();
        stored.extend(new.clone());
        db::set_setting(w.c(), "model_prices", &serde_json::to_string(&stored)?).await?;
    }
    w.event(
        EntityType::Settings,
        "00000000-0000-0000-0000-000000000000",
        None,
        ActivityField::Settings,
        None,
        serde_json::to_value(&req).ok(),
    )
    .await?;

    Ok(())
}
