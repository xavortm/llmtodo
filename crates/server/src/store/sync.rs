//! Sync.

use crate::error::ApiResult;
use crate::store::backend;
use crate::store::backend::Conn;

pub async fn dirty_value(c: &mut Conn) -> ApiResult<Option<String>> {
    Ok(
        backend::query_scalar("SELECT value FROM settings WHERE key = 'sync_dirty'")
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn clear_dirty(c: &mut Conn, seen: String) -> ApiResult<()> {
    backend::query("UPDATE settings SET value = '0' WHERE key = 'sync_dirty' AND value = ?")
        .bind(seen)
        .execute(&mut *c)
        .await?;
    Ok(())
}
