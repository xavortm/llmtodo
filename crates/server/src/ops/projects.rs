//! Projects, their linked repositories and their milestones.

use llmtodo_api_types::*;

use super::j;
use crate::error::{ApiError, ApiResult};
use crate::state::Write;
use crate::store::{self, projects, teams};

use crate::db::{new_id, now};

fn check_repo(repo: &str) -> ApiResult<String> {
    let repo = repo
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_end_matches(".git")
        .trim_matches('/');
    let ok = repo.split('/').count() == 2
        && repo.split('/').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        });
    if ok {
        Ok(repo.to_string())
    } else {
        Err(ApiError::validation(format!(
            "repo must look like owner/name, got {repo:?}"
        )))
    }
}

/// Checks an icon name. An empty one clears the icon.
fn check_icon(icon: &str) -> ApiResult<Option<String>> {
    let icon = icon.trim();
    if icon.is_empty() {
        return Ok(None);
    }
    if !llmtodo_core::project_style::is_icon(icon) {
        return Err(ApiError::validation(format!(
            "{icon:?} is not an icon the app can draw. `llmtodo project icons <filter>` lists the valid names"
        )));
    }
    Ok(Some(icon.to_string()))
}

/// Checks a color. An empty one clears the color.
fn check_color(color: &str) -> ApiResult<Option<String>> {
    if color.trim().is_empty() {
        return Ok(None);
    }
    llmtodo_core::project_style::normalize_color(color)
        .map(Some)
        .ok_or_else(|| ApiError::validation(format!("color must look like #rrggbb, got {color:?}")))
}

/// Sets the icon and logs the change, when it changes.
async fn set_icon(w: &mut Write<'_>, id: &str, old: Option<&str>, icon: &str) -> ApiResult<()> {
    let icon = check_icon(icon)?;
    if icon.as_deref() == old {
        return Ok(());
    }
    store::projects::set_icon(w.c(), icon.as_deref(), id).await?;
    w.event(
        EntityType::Project,
        id,
        None,
        ActivityField::Icon,
        j(old),
        j(&icon),
    )
    .await
}

/// Sets the color and logs the change, when it changes.
async fn set_color(w: &mut Write<'_>, id: &str, old: Option<&str>, color: &str) -> ApiResult<()> {
    let color = check_color(color)?;
    if color.as_deref() == old {
        return Ok(());
    }
    store::projects::set_color(w.c(), color.as_deref(), id).await?;
    w.event(
        EntityType::Project,
        id,
        None,
        ActivityField::Color,
        j(old),
        j(&color),
    )
    .await
}

pub async fn create(w: &mut Write<'_>, req: CreateProject) -> ApiResult<String> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::validation("project name is empty"));
    }
    let slug = projects::slugify(req.slug.as_deref().unwrap_or(name));
    if slug.is_empty() {
        return Err(ApiError::validation("project slug is empty"));
    }
    let icon = req.icon.as_deref().map(check_icon).transpose()?.flatten();
    let color = req.color.as_deref().map(check_color).transpose()?.flatten();
    let id = new_id();
    let t = now();
    store::projects::insert(
        w.c(),
        &id,
        &slug,
        name,
        &req.description,
        req.state.unwrap_or(ProjectState::Started).as_str(),
        &t,
    )
    .await?;
    for team in &req.teams {
        let tid = teams::team_row(w.c(), team).await?.id;
        store::projects::add_team(w.c(), &id, tid).await?;
    }
    store::projects::set_icon(w.c(), icon.as_deref(), &id).await?;
    store::projects::set_color(w.c(), color.as_deref(), &id).await?;
    w.event(
        EntityType::Project,
        &id,
        None,
        ActivityField::Created,
        None,
        j(&slug),
    )
    .await?;

    Ok(id)
}

pub async fn update(w: &mut Write<'_>, r: &str, req: UpdateProject) -> ApiResult<String> {
    let p = projects::resolve(w.c(), r).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::validation("project name is empty"));
        }
        store::projects::set_name(w.c(), name, &p.id).await?;
        w.event(
            EntityType::Project,
            &p.id,
            None,
            ActivityField::Name,
            j(&p.name),
            j(name),
        )
        .await?;
    }
    if let Some(slug) = &req.slug {
        let slug = projects::slugify(slug);
        if slug.is_empty() {
            return Err(ApiError::validation("project slug is empty"));
        }
        store::projects::set_slug(w.c(), &slug, &p.id).await?;
        w.event(
            EntityType::Project,
            &p.id,
            None,
            ActivityField::Slug,
            j(&p.slug),
            j(&slug),
        )
        .await?;
    }
    if let Some(d) = &req.description {
        store::projects::set_description(w.c(), d, &p.id).await?;
        w.event(
            EntityType::Project,
            &p.id,
            None,
            ActivityField::Description,
            None,
            None,
        )
        .await?;
    }
    if let Some(s) = req.state {
        store::projects::set_state(w.c(), s.as_str(), &p.id).await?;
        w.event(
            EntityType::Project,
            &p.id,
            None,
            ActivityField::State,
            j(&p.state),
            j(s),
        )
        .await?;
    }
    if let Some(icon) = &req.icon {
        set_icon(w, &p.id, p.icon.as_deref(), icon).await?;
    }
    if let Some(color) = &req.color {
        set_color(w, &p.id, p.color.as_deref(), color).await?;
    }
    if let Some(path) = req.local_path.as_deref().map(str::trim) {
        if !path.is_empty() && !std::path::Path::new(path).is_absolute() {
            return Err(ApiError::validation(format!(
                "local path must be absolute, got {path:?}"
            )));
        }
        let path = path.trim_end_matches('/');
        let path = (!path.is_empty()).then_some(path);
        store::projects::set_local_path(w.c(), path, &p.id).await?;
    }
    for team in req.add_teams.iter().flatten() {
        let tid = teams::team_row(w.c(), team).await?.id;
        store::projects::add_team(w.c(), &p.id, tid).await?;
    }
    for team in req.remove_teams.iter().flatten() {
        let tid = teams::team_row(w.c(), team).await?.id;
        store::projects::remove_team(w.c(), &p.id, tid).await?;
    }
    store::projects::touch(w.c(), &p.id).await?;

    Ok(p.id)
}

pub async fn delete(w: &mut Write<'_>, r: &str) -> ApiResult<String> {
    let p = projects::resolve(w.c(), r).await?;
    store::projects::trash(w.c(), &p.id).await?;
    w.event(
        EntityType::Project,
        &p.id,
        None,
        ActivityField::Deleted,
        None,
        None,
    )
    .await?;

    Ok(p.id)
}

pub async fn link_repo(w: &mut Write<'_>, r: &str, req: RepoLink) -> ApiResult<String> {
    let p = projects::resolve(w.c(), r).await?;
    let repo = check_repo(&req.repo)?;
    store::projects::link_repo(w.c(), &p.id, &repo).await?;
    w.event(
        EntityType::Project,
        &p.id,
        None,
        ActivityField::RepoLinked,
        None,
        j(&repo),
    )
    .await?;

    Ok(p.id)
}

pub async fn unlink_repo(w: &mut Write<'_>, r: &str, owner: &str, name: &str) -> ApiResult<String> {
    let p = projects::resolve(w.c(), r).await?;
    let repo = check_repo(&format!("{owner}/{name}"))?;
    let n = store::projects::unlink_repo(w.c(), &p.id, &repo)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::not_found("repo link", &repo));
    }
    w.event(
        EntityType::Project,
        &p.id,
        None,
        ActivityField::RepoUnlinked,
        j(&repo),
        None,
    )
    .await?;

    Ok(p.id)
}

pub async fn create_milestone(
    w: &mut Write<'_>,
    r: &str,
    req: CreateMilestone,
) -> ApiResult<String> {
    let p = projects::resolve(w.c(), r).await?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::validation("milestone name is empty"));
    }
    if let Some(d) = &req.target_date {
        projects::validate_date(d)?;
    }
    let pos: i64 = store::projects::next_milestone_position(w.c(), &p.id).await?;
    let id = new_id();
    store::projects::insert_milestone(
        w.c(),
        &id,
        &p.id,
        name,
        &req.description,
        &req.target_date,
        pos,
    )
    .await?;
    w.event(
        EntityType::Milestone,
        &id,
        None,
        ActivityField::Created,
        None,
        j(name),
    )
    .await?;

    Ok(id)
}

pub async fn update_milestone(w: &mut Write<'_>, id: &str, req: UpdateMilestone) -> ApiResult<()> {
    let m = projects::milestone_by_id(w.c(), id).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(ApiError::validation("milestone name is empty"));
        }
        store::projects::set_milestone_name(w.c(), name, id).await?;
        w.event(
            EntityType::Milestone,
            id,
            None,
            ActivityField::Name,
            j(&m.name),
            j(name),
        )
        .await?;
    }
    if let Some(d) = &req.description {
        store::projects::set_milestone_description(w.c(), d, id).await?;
    }
    if let Some(d) = &req.target_date {
        let d = d.trim();
        if !d.is_empty() {
            projects::validate_date(d)?;
        }
        store::projects::set_milestone_date(w.c(), (!d.is_empty()).then_some(d), id).await?;
    }
    if let Some(p) = req.position {
        store::projects::set_milestone_position(w.c(), p, id).await?;
    }

    Ok(())
}

pub async fn delete_milestone(w: &mut Write<'_>, id: &str) -> ApiResult<()> {
    let m = projects::milestone_by_id(w.c(), id).await?;
    store::projects::delete_milestone(w.c(), id).await?;
    w.event(
        EntityType::Milestone,
        id,
        None,
        ActivityField::Deleted,
        j(&m.name),
        None,
    )
    .await?;

    Ok(())
}
