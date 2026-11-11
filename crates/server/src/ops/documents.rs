//! Documents.

use llmtodo_api_types::*;

use super::j;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, activity, documents, projects, revisions};

use sqlx::SqliteConnection;

use super::revisions::apply_body_change;
use crate::db::{new_id, now};
use crate::store::revisions::add_revision;

/// Lists documents. `search` ranks by text match, best first.
pub async fn list(
    c: &mut SqliteConnection,
    project: Option<&str>,
    trashed: bool,
    search: Option<&str>,
) -> ApiResult<Vec<Document>> {
    let pid = match project {
        Some(r) => Some(projects::resolve(c, r).await?.id),
        None => None,
    };
    let rows = match search {
        Some(text) => {
            let terms = store::issues::fts_terms(text, 1, " ")
                .ok_or_else(|| ApiError::bad_request("text search needs at least one word"))?;
            documents::search_documents(c, pid.as_deref(), trashed, &terms).await?
        }
        None => documents::documents(c, pid.as_deref(), trashed).await?,
    };
    Ok(rows.iter().map(|d| d.to_api(false)).collect())
}

/// The revisions and the activity of a document.
pub async fn history(c: &mut SqliteConnection, r: &str) -> ApiResult<History> {
    let d = documents::resolve(c, r).await?;
    Ok(History {
        revisions: revisions::revisions(c, "document", &d.id).await?,
        activity: activity::for_entity(c, &d.id).await?,
    })
}

pub async fn create(w: &mut Write<'_>, req: CreateDocument) -> ApiResult<String> {
    let ctx = w.ctx;

    let p = projects::resolve(w.c(), &req.project).await?;
    let title = req.title.trim();
    if title.is_empty() {
        return Err(ApiError::validation("document title is empty"));
    }
    let slug = projects::slugify(req.slug.as_deref().unwrap_or(title));
    let id = new_id();
    let t = now();
    store::documents::insert(
        w.c(),
        &id,
        &p.id,
        &slug,
        title,
        &req.body,
        ctx.actor_id(),
        &t,
    )
    .await?;
    add_revision(
        w.c(),
        "document",
        &id,
        1,
        &req.body,
        &ctx.actor_id(),
        ctx.session(),
    )
    .await?;
    w.event(
        EntityType::Document,
        &id,
        None,
        ActivityField::Created,
        None,
        j(format!("{}/{slug}", p.slug)),
    )
    .await?;

    Ok(id)
}

pub async fn update(w: &mut Write<'_>, r: &str, req: UpdateDocument) -> ApiResult<String> {
    let ctx = w.ctx;

    let d = documents::resolve(w.c(), r).await?;
    if d.deleted_at.is_some() {
        return Err(ApiError::validation(
            "document is in the trash; restore it first",
        ));
    }
    let mut changed = false;
    if let Some(title) = req.title.as_deref().map(str::trim) {
        if title.is_empty() {
            return Err(ApiError::validation("document title is empty"));
        }
        if title != d.title {
            store::documents::set_title(w.c(), title, &d.id).await?;
            w.event(
                EntityType::Document,
                &d.id,
                None,
                ActivityField::Title,
                j(&d.title),
                j(title),
            )
            .await?;
            changed = true;
        }
    }
    if !req.body.is_empty() {
        let next = apply_body_change(&d.body, d.version, &req.body)?;
        if next != d.body {
            let v = d.version + 1;
            store::documents::set_body(w.c(), &next, v, &d.id).await?;
            add_revision(
                w.c(),
                "document",
                &d.id,
                v,
                &next,
                &ctx.actor_id(),
                ctx.session(),
            )
            .await?;
            w.event(
                EntityType::Document,
                &d.id,
                None,
                ActivityField::Body,
                j(d.version),
                j(v),
            )
            .await?;
            changed = true;
        }
    }
    if changed {
        store::documents::touch(w.c(), &d.id).await?;
    }

    Ok(d.id)
}

pub async fn delete(w: &mut Write<'_>, r: &str) -> ApiResult<String> {
    let d = documents::resolve(w.c(), r).await?;
    store::documents::trash(w.c(), &d.id).await?;
    w.event(
        EntityType::Document,
        &d.id,
        None,
        ActivityField::Deleted,
        None,
        None,
    )
    .await?;

    Ok(d.id)
}

pub async fn restore(w: &mut Write<'_>, r: &str, req: RestoreRequest) -> ApiResult<String> {
    let ctx = w.ctx;

    let d = documents::resolve(w.c(), r).await?;
    match req.revision {
        None => {
            if d.deleted_at.is_none() {
                return Err(ApiError::validation("document is not in the trash"));
            }
            store::documents::untrash(w.c(), &d.id).await?;
            w.event(
                EntityType::Document,
                &d.id,
                None,
                ActivityField::Restored,
                None,
                None,
            )
            .await?;
        }
        Some(v) => {
            let body = revisions::revision_body(w.c(), "document", &d.id, v).await?;
            if body != d.body {
                let nv = d.version + 1;
                store::documents::restore_body(w.c(), &body, nv, &d.id).await?;
                add_revision(
                    w.c(),
                    "document",
                    &d.id,
                    nv,
                    &body,
                    &ctx.actor_id(),
                    ctx.session(),
                )
                .await?;
                w.event(
                    EntityType::Document,
                    &d.id,
                    None,
                    ActivityField::Body,
                    j(d.version),
                    j(nv),
                )
                .await?;
            }
        }
    }

    Ok(d.id)
}
