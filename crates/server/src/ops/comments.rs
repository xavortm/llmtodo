//! Comments on issues.

use llmtodo_api_types::*;

use super::issues::touch_updated;
use super::j;
use crate::db::{new_id, now};
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::revisions::add_revision;
use crate::store::{self, comments, issues};

pub async fn add(w: &mut Write<'_>, r: &str, req: CreateComment) -> ApiResult<String> {
    let ctx = w.ctx;
    if req.body.trim().is_empty() {
        return Err(ApiError::validation("comment body is empty"));
    }

    let row = issues::resolve_live(w.c(), r).await?;
    let id = new_id();
    let t = now();
    store::comments::insert(
        w.c(),
        &id,
        &row.id,
        req.parent_id.map(|p| p.to_string()),
        ctx.actor_id(),
        &req.body,
        &t,
    )
    .await?;
    add_revision(
        w.c(),
        "comment",
        &id,
        1,
        &req.body,
        &ctx.actor_id(),
        ctx.session(),
    )
    .await?;
    w.event(
        EntityType::Comment,
        &id,
        Some(&row.id),
        ActivityField::CommentAdded,
        None,
        None,
    )
    .await?;
    touch_updated(w, &row.id).await?;

    Ok(id)
}

pub async fn update(w: &mut Write<'_>, id: &str, req: UpdateComment) -> ApiResult<()> {
    let ctx = w.ctx;
    if req.body.trim().is_empty() {
        return Err(ApiError::validation("comment body is empty"));
    }

    let (issue_id, cur) = comments::comment_by_id(w.c(), id).await?;
    if cur.body != req.body {
        let v = cur.version + 1;
        store::comments::update_body(w.c(), &req.body, v, id).await?;
        add_revision(
            w.c(),
            "comment",
            id,
            v,
            &req.body,
            &ctx.actor_id(),
            ctx.session(),
        )
        .await?;
        w.event(
            EntityType::Comment,
            id,
            Some(&issue_id),
            ActivityField::CommentEdited,
            j(cur.version),
            j(v),
        )
        .await?;
    }

    Ok(())
}

pub async fn delete(w: &mut Write<'_>, id: &str) -> ApiResult<()> {
    let (issue_id, _) = comments::comment_by_id(w.c(), id).await?;
    store::comments::soft_delete(w.c(), id).await?;
    w.event(
        EntityType::Comment,
        id,
        Some(&issue_id),
        ActivityField::CommentDeleted,
        None,
        None,
    )
    .await?;

    Ok(())
}
