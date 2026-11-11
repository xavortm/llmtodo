//! Labels and label groups.

use llmtodo_api_types::*;

use super::j;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, labels};

use super::check_color;
use crate::db::new_id;

pub async fn create(w: &mut Write<'_>, req: CreateLabel) -> ApiResult<String> {
    let name = req.name.trim();
    if name.is_empty() || name.contains('/') {
        return Err(ApiError::validation(
            "label name must be non-empty and hold no '/'",
        ));
    }
    let group_id = match &req.group {
        Some(g) => {
            if req.is_group {
                return Err(ApiError::validation(
                    "a label group cannot sit inside another group",
                ));
            }
            let g = labels::resolve_any(w.c(), g).await?;
            if g.is_group == 0 {
                return Err(ApiError::validation(format!(
                    "{:?} is a label, not a group",
                    g.name
                )));
            }
            Some(g.id)
        }
        None => None,
    };
    let color = check_color(req.color.as_deref().unwrap_or("#8b8d98"))?;
    let id = new_id();
    store::labels::insert(
        w.c(),
        &id,
        name,
        group_id,
        req.is_group,
        &color,
        &req.description,
    )
    .await?;
    w.event(
        EntityType::Label,
        &id,
        None,
        ActivityField::Created,
        None,
        j(name),
    )
    .await?;

    Ok(id)
}

pub async fn update(w: &mut Write<'_>, r: &str, req: UpdateLabel) -> ApiResult<String> {
    let l = labels::resolve_any(w.c(), r).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() || name.contains('/') {
            return Err(ApiError::validation(
                "label name must be non-empty and hold no '/'",
            ));
        }
        store::labels::set_name(w.c(), name, &l.id).await?;
        w.event(
            EntityType::Label,
            &l.id,
            None,
            ActivityField::Name,
            j(&l.name),
            j(name),
        )
        .await?;
    }
    if let Some(color) = &req.color {
        store::labels::set_color(w.c(), check_color(color)?, &l.id).await?;
    }
    if let Some(d) = &req.description {
        store::labels::set_description(w.c(), d, &l.id).await?;
    }

    Ok(l.id)
}

pub async fn delete(w: &mut Write<'_>, r: &str) -> ApiResult<String> {
    let l = labels::resolve_any(w.c(), r).await?;
    let issue_ids: Vec<String> = store::labels::issues_with(w.c(), &l.id).await?;
    store::labels::delete(w.c(), &l.id).await?;
    w.event(
        EntityType::Label,
        &l.id,
        None,
        ActivityField::Deleted,
        j(l.full_name()),
        None,
    )
    .await?;
    for i in issue_ids {
        w.event(
            EntityType::Issue,
            &i,
            Some(&i),
            ActivityField::Labels,
            j(vec![l.full_name()]),
            None,
        )
        .await?;
    }

    Ok(l.id)
}
