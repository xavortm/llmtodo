//! Sync.

use crate::error::ApiResult;
use sqlx::SqliteConnection;

pub async fn dirty_value(c: &mut SqliteConnection) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'sync_dirty'")
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn clear_dirty(c: &mut SqliteConnection, seen: String) -> ApiResult<()> {
    sqlx::query("UPDATE settings SET value = '0' WHERE key = 'sync_dirty' AND value = ?")
        .bind(seen)
        .execute(&mut *c)
        .await?;
    Ok(())
}
