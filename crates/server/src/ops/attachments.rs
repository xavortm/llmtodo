//! Links and files attached to issues.

use llmtodo_api_types::*;
use serde_json::json;

use super::issues::touch_updated;
use super::j;
use crate::db::new_id;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, attachments, issues};

pub async fn add(w: &mut Write<'_>, r: &str, req: CreateAttachment) -> ApiResult<String> {
    let row = issues::resolve_live(w.c(), r).await?;
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Err(ApiError::validation("attachment URL is empty"));
    }
    let id = attach(
        w,
        &row.id,
        &url,
        req.title.as_deref(),
        req.kind,
        "manual",
        serde_json::Value::Object(Default::default()),
    )
    .await?;

    Ok(id)
}

/// Adds an attachment, or updates the title and metadata of the one with the
/// same URL. Returns its ID.
pub async fn attach(
    w: &mut Write<'_>,
    issue_id: &str,
    url: &str,
    title: Option<&str>,
    kind: Option<AttachmentKind>,
    source: &str,
    metadata: serde_json::Value,
) -> ApiResult<String> {
    let ctx = w.ctx;
    let kind = kind.unwrap_or_else(|| attachments::guess_kind(url));
    let existing: Option<String> = store::attachments::id_by_url(w.c(), issue_id, url).await?;
    if let Some(id) = existing {
        store::attachments::update_title_metadata(w.c(), title, metadata.to_string(), &id).await?;
        return Ok(id);
    }
    let id = new_id();
    store::attachments::insert(
        w.c(),
        &id,
        issue_id,
        kind.as_str(),
        url,
        title,
        source,
        metadata.to_string(),
        ctx.actor_id(),
    )
    .await?;
    w.event(
        EntityType::Attachment,
        &id,
        Some(issue_id),
        ActivityField::Attached,
        None,
        j(json!({ "url": url, "kind": kind })),
    )
    .await?;
    touch_updated(w, issue_id).await?;
    Ok(id)
}

pub async fn detach(w: &mut Write<'_>, id: &str) -> ApiResult<()> {
    let (issue_id, a) = attachments::attachment_by_id(w.c(), id).await?;
    store::attachments::delete(w.c(), id).await?;
    w.event(
        EntityType::Attachment,
        id,
        Some(&issue_id),
        ActivityField::Detached,
        j(json!({ "url": a.url })),
        None,
    )
    .await?;
    touch_updated(w, &issue_id).await?;

    Ok(())
}
