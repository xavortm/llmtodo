//! Teams and their statuses.

use llmtodo_api_types::*;

use super::j;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, teams};

use super::check_color;
use crate::db::new_id;

pub async fn create(w: &mut Write<'_>, req: CreateTeam) -> ApiResult<String> {
    let id = teams::create(w.c(), &req.key, &req.name, None).await?;
    w.event(
        EntityType::Team,
        &id,
        None,
        ActivityField::Created,
        None,
        j(&req.key),
    )
    .await?;

    Ok(id)
}

pub async fn update(w: &mut Write<'_>, r: &str, req: UpdateTeam) -> ApiResult<String> {
    let t = teams::team_row(w.c(), r).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::validation("team name is empty"));
        }
        store::teams::set_name(w.c(), name, &t.id).await?;
        w.event(
            EntityType::Team,
            &t.id,
            None,
            ActivityField::Name,
            j(&t.name),
            j(name),
        )
        .await?;
    }
    for (column, field, value) in [
        (
            "pr_open_status_id",
            ActivityField::PrOpenStatus,
            &req.pr_open_status,
        ),
        (
            "pr_merged_status_id",
            ActivityField::PrMergedStatus,
            &req.pr_merged_status,
        ),
    ] {
        if let Some(v) = value {
            let sid = if v.trim().is_empty() {
                None
            } else {
                Some(teams::status_by_name(w.c(), &t.id, v).await?.id)
            };
            store::teams::set_pr_status(w.c(), column, sid, &t.id).await?;
            w.event(EntityType::Team, &t.id, None, field, None, j(v))
                .await?;
        }
    }

    Ok(t.id)
}

pub async fn create_status(w: &mut Write<'_>, r: &str, req: CreateStatus) -> ApiResult<String> {
    let t = teams::team_row(w.c(), r).await?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::validation("status name is empty"));
    }
    let color = check_color(req.color.as_deref().unwrap_or("#95a2b3"))?;
    let pos = match req.position {
        Some(p) => p,
        None => store::teams::next_status_position(w.c(), &t.id).await?,
    };
    let id = new_id();
    let review = req.review.unwrap_or(false);
    store::teams::insert_status(
        w.c(),
        &id,
        &t.id,
        name,
        req.kind.as_str(),
        &color,
        pos,
        review,
    )
    .await?;
    w.event(
        EntityType::Status,
        &id,
        None,
        ActivityField::Created,
        None,
        j(name),
    )
    .await?;

    Ok(id)
}

pub async fn update_status(w: &mut Write<'_>, id: &str, req: UpdateStatus) -> ApiResult<()> {
    let s = teams::status_by_id(w.c(), id).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::validation("status name is empty"));
        }
        store::teams::set_status_name(w.c(), name, id).await?;
        w.event(
            EntityType::Status,
            id,
            None,
            ActivityField::Name,
            j(&s.name),
            j(name),
        )
        .await?;
    }
    if let Some(color) = &req.color {
        let color = check_color(color)?;
        store::teams::set_status_color(w.c(), &color, id).await?;
    }
    if let Some(p) = req.position {
        store::teams::set_status_position(w.c(), p, id).await?;
    }
    if let Some(review) = req.review
        && review != s.review
    {
        store::teams::set_status_review(w.c(), review, id).await?;
        w.event(
            EntityType::Status,
            id,
            None,
            ActivityField::Review,
            j(s.review),
            j(review),
        )
        .await?;
        if review {
            super::claims::end_review_claims(w, Some(id)).await?;
        }
    }

    Ok(())
}

pub async fn delete_status(w: &mut Write<'_>, id: &str) -> ApiResult<()> {
    let s = teams::status_by_id(w.c(), id).await?;
    let used: i64 = store::teams::count_issues_with_status(w.c(), id).await?;
    if used > 0 {
        return Err(ApiError::validation(format!(
            "status {:?} is used by {used} issues (trash included); move them first",
            s.name
        )));
    }
    store::teams::delete_status(w.c(), id).await?;
    w.event(
        EntityType::Status,
        id,
        None,
        ActivityField::Deleted,
        j(&s.name),
        None,
    )
    .await?;

    Ok(())
}
