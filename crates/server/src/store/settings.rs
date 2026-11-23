//! Settings.

use crate::error::ApiResult;
use crate::store::backend;
use crate::store::backend::Conn;

pub async fn model_prices_raw(c: &mut Conn) -> ApiResult<Option<String>> {
    Ok(
        backend::query_scalar("SELECT value FROM settings WHERE key = 'model_prices'")
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn bump_sync_dirty(c: &mut Conn) -> ApiResult<()> {
    backend::query(
        "INSERT INTO settings (key, value) VALUES ('sync_dirty', '1')
             ON CONFLICT (key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
    )
    .execute(&mut *c)
    .await?;
    Ok(())
}
