//! Settings.

use crate::error::ApiResult;
use sqlx::SqliteConnection;

pub async fn model_prices_raw(c: &mut SqliteConnection) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'model_prices'")
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn bump_sync_dirty(c: &mut SqliteConnection) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('sync_dirty', '1')
             ON CONFLICT (key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
    )
    .execute(&mut *c)
    .await?;
    Ok(())
}
