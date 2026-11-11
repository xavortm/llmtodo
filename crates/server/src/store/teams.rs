use llmtodo_api_types::{Status, StatusType, Team};
use sqlx::SqliteConnection;

use crate::db::{new_id, now, parse_ts, parse_uuid};
use crate::error::{ApiError, ApiResult};

#[derive(sqlx::FromRow, Clone)]
pub struct StatusRow {
    pub id: String,
    pub team_id: String,
    pub team_key: String,
    pub name: String,
    #[sqlx(rename = "type")]
    pub kind: String,
    pub color: String,
    pub position: i64,
    pub review: bool,
}

impl StatusRow {
    pub fn kind(&self) -> StatusType {
        self.kind.parse().unwrap_or(StatusType::Backlog)
    }

    pub fn to_api(&self) -> Status {
        Status {
            id: parse_uuid(&self.id),
            team: self.team_key.clone(),
            name: self.name.clone(),
            kind: self.kind(),
            color: self.color.clone(),
            position: self.position,
            review: self.review,
        }
    }

    pub fn to_ref(&self) -> llmtodo_api_types::StatusRef {
        llmtodo_api_types::StatusRef {
            id: parse_uuid(&self.id),
            name: self.name.clone(),
            kind: self.kind(),
            color: self.color.clone(),
            review: self.review,
        }
    }
}

const STATUS_COLS: &str =
    "s.id, s.team_id, t.key AS team_key, s.name, s.type, s.color, s.position, s.review";

#[derive(sqlx::FromRow, Clone)]
pub struct TeamRow {
    pub id: String,
    pub key: String,
    pub name: String,
    pub pr_open_status_id: Option<String>,
    pub pr_merged_status_id: Option<String>,
    pub created_at: String,
}

const TEAM_COLS: &str = "id, key, name, pr_open_status_id, pr_merged_status_id, created_at";

pub async fn statuses(
    conn: &mut SqliteConnection,
    team_id: Option<&str>,
) -> ApiResult<Vec<StatusRow>> {
    let rows = sqlx::query_as::<_, StatusRow>(crate::sql!(
        "SELECT {STATUS_COLS} FROM statuses s JOIN teams t ON t.id = s.team_id WHERE (? IS NULL OR s.team_id = ?) ORDER BY t.key, s.position, s.name"
    ))
    .bind(team_id)
    .bind(team_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

pub async fn status_by_id(conn: &mut SqliteConnection, id: &str) -> ApiResult<StatusRow> {
    sqlx::query_as::<_, StatusRow>(crate::sql!(
        "SELECT {STATUS_COLS} FROM statuses s JOIN teams t ON t.id = s.team_id WHERE s.id = ?"
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("status", id))
}

/// Finds a status of one team by name or ID.
pub async fn status_by_name(
    conn: &mut SqliteConnection,
    team_id: &str,
    r: &str,
) -> ApiResult<StatusRow> {
    let all = statuses(conn, Some(team_id)).await?;
    let r = r.trim();
    all.into_iter()
        .find(|s| s.id == r || s.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| ApiError::not_found("status", r))
}

/// The first status of a type, by position.
pub async fn first_of_type(
    conn: &mut SqliteConnection,
    team_id: &str,
    kind: StatusType,
) -> ApiResult<Option<StatusRow>> {
    Ok(statuses(conn, Some(team_id))
        .await?
        .into_iter()
        .find(|s| s.kind() == kind))
}

/// The status a claim moves an issue to: the first started status that is not
/// a review status. A review status needs a review section and hands the
/// ticket to the owner, so a claim must never land there, wherever the team
/// placed it.
pub async fn first_work_status(
    conn: &mut SqliteConnection,
    team_id: &str,
) -> ApiResult<Option<StatusRow>> {
    Ok(statuses(conn, Some(team_id))
        .await?
        .into_iter()
        .find(|s| s.kind() == StatusType::Started && !s.review))
}

pub async fn team_row(conn: &mut SqliteConnection, r: &str) -> ApiResult<TeamRow> {
    sqlx::query_as::<_, TeamRow>(crate::sql!(
        "SELECT {TEAM_COLS} FROM teams WHERE id = ? OR key = ? COLLATE NOCASE"
    ))
    .bind(r.trim())
    .bind(r.trim())
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("team", r))
}

pub async fn team_rows(conn: &mut SqliteConnection) -> ApiResult<Vec<TeamRow>> {
    Ok(
        sqlx::query_as::<_, TeamRow>(crate::sql!("SELECT {TEAM_COLS} FROM teams ORDER BY key"))
            .fetch_all(&mut *conn)
            .await?,
    )
}

pub async fn to_api(conn: &mut SqliteConnection, t: TeamRow) -> ApiResult<Team> {
    let sts = statuses(conn, Some(&t.id)).await?;
    let name_of = |id: &Option<String>| {
        id.as_ref()
            .and_then(|id| sts.iter().find(|s| &s.id == id))
            .map(|s| s.name.clone())
    };
    Ok(Team {
        id: parse_uuid(&t.id),
        key: t.key.clone(),
        name: t.name.clone(),
        pr_open_status: name_of(&t.pr_open_status_id),
        pr_merged_status: name_of(&t.pr_merged_status_id),
        statuses: sts.iter().map(StatusRow::to_api).collect(),
        created_at: parse_ts(&t.created_at),
    })
}

/// Creates a team with the default statuses. Returns its ID.
pub async fn create(
    conn: &mut SqliteConnection,
    key: &str,
    name: &str,
    linear_id: Option<&str>,
) -> ApiResult<String> {
    let key =
        llmtodo_core::normalize_team_key(key).map_err(|e| ApiError::validation(e.to_string()))?;
    if name.trim().is_empty() {
        return Err(ApiError::validation("team name is empty"));
    }
    let id = new_id();
    let t = now();
    sqlx::query("INSERT INTO teams (id, key, name, linear_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(&key)
        .bind(name.trim())
        .bind(linear_id)
        .bind(&t)
        .bind(&t)
        .execute(&mut *conn)
        .await?;
    let mut open = None;
    let mut merged = None;
    for (pos, (sname, kind, color)) in llmtodo_core::DEFAULT_STATUSES.iter().enumerate() {
        let sid = new_id();
        sqlx::query("INSERT INTO statuses (id, team_id, name, type, color, position, review) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&sid)
            .bind(&id)
            .bind(*sname)
            .bind(kind.as_str())
            .bind(*color)
            .bind(pos as i64)
            .bind(llmtodo_core::is_default_review_status(sname))
            .execute(&mut *conn)
            .await?;
        if *sname == "In Review" {
            open = Some(sid.clone());
        }
        if *sname == "Done" {
            merged = Some(sid);
        }
    }
    sqlx::query("UPDATE teams SET pr_open_status_id = ?, pr_merged_status_id = ? WHERE id = ?")
        .bind(open)
        .bind(merged)
        .bind(&id)
        .execute(&mut *conn)
        .await?;
    Ok(id)
}

/// The team to use when a request names none: the default team setting, or
/// the only team.
pub async fn default_team(conn: &mut SqliteConnection) -> ApiResult<TeamRow> {
    let setting: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_team'")
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(key) = setting.filter(|k| !k.is_empty()) {
        return team_row(conn, &key).await;
    }
    let teams = team_rows(conn).await?;
    match teams.len() {
        1 => Ok(teams.into_iter().next().unwrap()),
        0 => Err(ApiError::validation(
            "no team exists yet; create one with `llmtodo team create <KEY> <name>`",
        )),
        _ => Err(ApiError::validation(
            "several teams exist; pass a team or set `llmtodo settings set --default-team <KEY>`",
        )),
    }
}

pub async fn reserve_number(c: &mut SqliteConnection, number: i64, team_id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE teams SET next_number = max(next_number, ? + 1) WHERE id = ?")
        .bind(number)
        .bind(team_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn allocate_number(c: &mut SqliteConnection, team_id: &str) -> ApiResult<i64> {
    Ok(sqlx::query_scalar(
        "UPDATE teams SET next_number = next_number + 1 WHERE id = ? RETURNING next_number - 1",
    )
    .bind(team_id)
    .fetch_one(&mut *c)
    .await?)
}

pub async fn set_name(c: &mut SqliteConnection, name: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE teams SET name = ?, updated_at = ? WHERE id = ?")
        .bind(name)
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_pr_status(
    c: &mut SqliteConnection,
    field: &str,
    status_id: Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE teams SET {field} = ?, updated_at = ? WHERE id = ?"
    )))
    .bind(status_id)
    .bind(now())
    .bind(id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn next_status_position(c: &mut SqliteConnection, team_id: &str) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar(
            "SELECT coalesce(max(position), -1) + 1 FROM statuses WHERE team_id = ?",
        )
        .bind(team_id)
        .fetch_one(&mut *c)
        .await?,
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_status(
    c: &mut SqliteConnection,
    id: &str,
    team_id: &str,
    name: &str,
    kind: &str,
    color: &str,
    position: i64,
    review: bool,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO statuses (id, team_id, name, type, color, position, review) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(team_id)
    .bind(name)
    .bind(kind)
    .bind(color)
    .bind(position)
    .bind(review)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn set_status_name(c: &mut SqliteConnection, name: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE statuses SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_status_color(c: &mut SqliteConnection, color: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE statuses SET color = ? WHERE id = ?")
        .bind(color)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_status_review(c: &mut SqliteConnection, review: bool, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE statuses SET review = ? WHERE id = ?")
        .bind(review)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_status_position(
    c: &mut SqliteConnection,
    position: i64,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE statuses SET position = ? WHERE id = ?")
        .bind(position)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn count_issues_with_status(c: &mut SqliteConnection, id: &str) -> ApiResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM issues WHERE status_id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn delete_status(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM statuses WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
