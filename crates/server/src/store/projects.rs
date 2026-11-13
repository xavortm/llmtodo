use llmtodo_api_types::{Milestone, Project, ProjectState};
use sqlx::SqliteConnection;

use crate::db::{now, parse_ts, parse_ts_opt, parse_uuid};
use crate::error::{ApiError, ApiResult};

#[derive(sqlx::FromRow, Clone)]
pub struct ProjectRow {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub local_path: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
}

const COLS: &str = "id, slug, name, description, state, created_at, updated_at, deleted_at, local_path, icon, color";

/// Turns a name into a slug: lower case, words joined by `-`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Resolves a slug, ID or name. Trashed projects resolve too.
pub async fn resolve(conn: &mut SqliteConnection, r: &str) -> ApiResult<ProjectRow> {
    let r = r.trim();
    sqlx::query_as::<_, ProjectRow>(crate::sql!(
        "SELECT {COLS} FROM projects WHERE id = ? OR slug = ? COLLATE NOCASE OR name = ? COLLATE NOCASE ORDER BY deleted_at IS NOT NULL, (id = ?) DESC LIMIT 1"
    ))
    .bind(r)
    .bind(r)
    .bind(r)
    .bind(r)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("project", r))
}

pub async fn rows(
    conn: &mut SqliteConnection,
    include_deleted: bool,
) -> ApiResult<Vec<ProjectRow>> {
    Ok(sqlx::query_as::<_, ProjectRow>(crate::sql!(
        "SELECT {COLS} FROM projects WHERE (? OR deleted_at IS NULL) ORDER BY name COLLATE NOCASE"
    ))
    .bind(include_deleted)
    .fetch_all(&mut *conn)
    .await?)
}

pub async fn to_api(conn: &mut SqliteConnection, p: ProjectRow) -> ApiResult<Project> {
    Ok(all_to_api(conn, vec![p]).await?.remove(0))
}

/// Builds every project from one query for teams and one for repos.
pub async fn all_to_api(
    conn: &mut SqliteConnection,
    rows: Vec<ProjectRow>,
) -> ApiResult<Vec<Project>> {
    let ids = serde_json::to_string(&rows.iter().map(|p| &p.id).collect::<Vec<_>>())?;
    let mut teams = grouped(
        sqlx::query_as("SELECT pt.project_id, t.key FROM project_teams pt JOIN teams t ON t.id = pt.team_id WHERE pt.project_id IN (SELECT value FROM json_each(?)) ORDER BY t.key")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await?,
    );
    let mut repos = grouped(
        sqlx::query_as("SELECT project_id, repo FROM project_repos WHERE project_id IN (SELECT value FROM json_each(?)) ORDER BY repo")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await?,
    );
    Ok(rows
        .into_iter()
        .map(|p| Project {
            id: parse_uuid(&p.id),
            teams: teams.remove(&p.id).unwrap_or_default(),
            repos: repos.remove(&p.id).unwrap_or_default(),
            slug: p.slug,
            name: p.name,
            description: p.description,
            local_path: p.local_path,
            icon: p.icon,
            color: p.color,
            state: p.state.parse().unwrap_or(ProjectState::Backlog),
            created_at: parse_ts(&p.created_at),
            updated_at: parse_ts(&p.updated_at),
            deleted_at: parse_ts_opt(p.deleted_at),
        })
        .collect())
}

/// Groups `(project_id, value)` pairs by project, keeping their order.
fn grouped(pairs: Vec<(String, String)>) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = Default::default();
    for (project, value) in pairs {
        out.entry(project).or_default().push(value);
    }
    out
}

#[derive(sqlx::FromRow, Clone)]
pub struct MilestoneRow {
    pub id: String,
    pub project_id: String,
    pub project_slug: String,
    pub name: String,
    pub description: String,
    pub target_date: Option<String>,
    pub position: i64,
}

impl MilestoneRow {
    pub fn to_api(&self) -> Milestone {
        Milestone {
            id: parse_uuid(&self.id),
            project: self.project_slug.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            target_date: self.target_date.clone(),
            position: self.position,
        }
    }
}

const M_COLS: &str =
    "m.id, m.project_id, p.slug AS project_slug, m.name, m.description, m.target_date, m.position";

pub async fn milestones(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> ApiResult<Vec<MilestoneRow>> {
    Ok(sqlx::query_as::<_, MilestoneRow>(crate::sql!(
        "SELECT {M_COLS} FROM milestones m JOIN projects p ON p.id = m.project_id WHERE m.project_id = ? ORDER BY m.position, m.name"
    ))
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await?)
}

pub async fn milestone_by_id(conn: &mut SqliteConnection, id: &str) -> ApiResult<MilestoneRow> {
    sqlx::query_as::<_, MilestoneRow>(crate::sql!(
        "SELECT {M_COLS} FROM milestones m JOIN projects p ON p.id = m.project_id WHERE m.id = ?"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("milestone", id))
}

/// Finds a milestone by ID or by name inside a project.
pub async fn resolve_milestone(
    conn: &mut SqliteConnection,
    project_id: Option<&str>,
    r: &str,
) -> ApiResult<MilestoneRow> {
    let r = r.trim();
    if let Ok(m) = milestone_by_id(conn, r).await {
        return Ok(m);
    }
    let Some(pid) = project_id else {
        return Err(ApiError::validation(format!(
            "milestone {r:?} needs a project; set the issue's project first"
        )));
    };
    milestones(conn, pid)
        .await?
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| ApiError::not_found("milestone", r))
}

pub fn validate_date(d: &str) -> ApiResult<()> {
    d.parse::<jiff::civil::Date>()
        .map(|_| ())
        .map_err(|_| ApiError::validation(format!("date must be YYYY-MM-DD, got {d:?}")))
}

pub async fn insert(
    c: &mut SqliteConnection,
    id: &str,
    slug: &str,
    name: &str,
    description: &str,
    state: &str,
    t: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO projects (id, slug, name, description, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(state)
        .bind(t)
        .bind(t)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn add_team(
    c: &mut SqliteConnection,
    project_id: &str,
    team_id: String,
) -> ApiResult<()> {
    sqlx::query("INSERT OR IGNORE INTO project_teams (project_id, team_id) VALUES (?, ?)")
        .bind(project_id)
        .bind(team_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_name(c: &mut SqliteConnection, name: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// Sets or clears the project's icon name.
pub async fn set_icon(c: &mut SqliteConnection, icon: Option<&str>, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET icon = ? WHERE id = ?")
        .bind(icon)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// Sets or clears the project's color.
pub async fn set_color(c: &mut SqliteConnection, color: Option<&str>, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET color = ? WHERE id = ?")
        .bind(color)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

/// Sets or clears the project's local folder.
pub async fn set_local_path(
    c: &mut SqliteConnection,
    path: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET local_path = ? WHERE id = ?")
        .bind(path)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_slug(c: &mut SqliteConnection, slug: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET slug = ? WHERE id = ?")
        .bind(slug)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_description(
    c: &mut SqliteConnection,
    description: &str,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET description = ? WHERE id = ?")
        .bind(description)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_state(c: &mut SqliteConnection, state: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET state = ? WHERE id = ?")
        .bind(state)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn remove_team(
    c: &mut SqliteConnection,
    project_id: &str,
    team_id: String,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM project_teams WHERE project_id = ? AND team_id = ?")
        .bind(project_id)
        .bind(team_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn touch(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn trash(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET deleted_at = ?, updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn link_repo(c: &mut SqliteConnection, project_id: &str, repo: &str) -> ApiResult<()> {
    sqlx::query("INSERT OR IGNORE INTO project_repos (project_id, repo) VALUES (?, ?)")
        .bind(project_id)
        .bind(repo)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn unlink_repo(
    c: &mut SqliteConnection,
    project_id: &str,
    repo: &str,
) -> ApiResult<sqlx::sqlite::SqliteQueryResult> {
    Ok(
        sqlx::query("DELETE FROM project_repos WHERE project_id = ? AND repo = ?")
            .bind(project_id)
            .bind(repo)
            .execute(&mut *c)
            .await?,
    )
}

pub async fn next_milestone_position(c: &mut SqliteConnection, project_id: &str) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT coalesce(max(position), -1) + 1 FROM milestones WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_one(&mut *c)
    .await?)
}

pub async fn insert_milestone(
    c: &mut SqliteConnection,
    id: &str,
    project_id: &str,
    name: &str,
    description: &str,
    target_date: &Option<String>,
    position: i64,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO milestones (id, project_id, name, description, target_date, position, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(project_id)
        .bind(name)
        .bind(description)
        .bind(target_date)
        .bind(position)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_milestone_name(c: &mut SqliteConnection, name: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE milestones SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_milestone_description(
    c: &mut SqliteConnection,
    description: &str,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE milestones SET description = ? WHERE id = ?")
        .bind(description)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_milestone_date(
    c: &mut SqliteConnection,
    target_date: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE milestones SET target_date = ? WHERE id = ?")
        .bind(target_date)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_milestone_position(
    c: &mut SqliteConnection,
    position: i64,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE milestones SET position = ? WHERE id = ?")
        .bind(position)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn delete_milestone(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM milestones WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugify() {
        assert_eq!(super::slugify("Notebook — Web App!"), "notebook-web-app");
        assert_eq!(super::slugify("  GardenPlanner "), "gardenplanner");
    }
}
