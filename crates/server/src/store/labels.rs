use llmtodo_api_types::Label;
use sqlx::SqliteConnection;

use crate::db::{now, parse_uuid};
use crate::error::{ApiError, ApiResult};

#[derive(sqlx::FromRow, Clone)]
pub struct LabelRow {
    pub id: String,
    pub name: String,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub is_group: i64,
    pub color: String,
    pub description: Option<String>,
}

impl LabelRow {
    pub fn full_name(&self) -> String {
        match &self.group_name {
            Some(g) => format!("{g}/{}", self.name),
            None => self.name.clone(),
        }
    }

    pub fn to_api(&self) -> Label {
        Label {
            id: parse_uuid(&self.id),
            name: self.name.clone(),
            group: self.group_name.clone(),
            full_name: self.full_name(),
            color: self.color.clone(),
            description: self.description.clone(),
            is_group: self.is_group == 1,
        }
    }
}

const COLS: &str =
    "l.id, l.name, l.group_id, g.name AS group_name, l.is_group, l.color, l.description";

pub async fn all(conn: &mut SqliteConnection) -> ApiResult<Vec<LabelRow>> {
    Ok(sqlx::query_as::<_, LabelRow>(crate::sql!(
        "SELECT {COLS} FROM labels l LEFT JOIN labels g ON g.id = l.group_id ORDER BY coalesce(g.name, l.name) COLLATE NOCASE, l.is_group DESC, l.name COLLATE NOCASE"
    ))
    .fetch_all(&mut *conn)
    .await?)
}

/// Resolves an ID, `Group/name`, or a name. A bare name must match exactly
/// one label. Groups never match: they cannot go on an issue.
pub async fn resolve(conn: &mut SqliteConnection, r: &str) -> ApiResult<LabelRow> {
    let r = r.trim();
    let labels = all(conn).await?;
    if let Some(l) = labels.iter().find(|l| l.id == r && l.is_group == 0) {
        return Ok(l.clone());
    }
    if let Some(l) = labels.iter().find(|l| {
        l.is_group == 0 && l.group_name.is_some() && l.full_name().eq_ignore_ascii_case(r)
    }) {
        return Ok(l.clone());
    }
    let by_name: Vec<&LabelRow> = labels
        .iter()
        .filter(|l| l.is_group == 0 && l.name.eq_ignore_ascii_case(r))
        .collect();
    match by_name.len() {
        1 => Ok(by_name[0].clone()),
        0 => Err(ApiError::not_found("label", r)),
        _ => Err(ApiError::validation(format!(
            "label {r:?} is ambiguous; use one of: {}",
            by_name
                .iter()
                .map(|l| l.full_name())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Resolves any label or group, for updates and deletes.
pub async fn resolve_any(conn: &mut SqliteConnection, r: &str) -> ApiResult<LabelRow> {
    let r = r.trim();
    let labels = all(conn).await?;
    if let Some(l) = labels
        .iter()
        .find(|l| l.id == r || l.full_name().eq_ignore_ascii_case(r))
    {
        return Ok(l.clone());
    }
    let by_name: Vec<&LabelRow> = labels
        .iter()
        .filter(|l| l.name.eq_ignore_ascii_case(r))
        .collect();
    match by_name.len() {
        1 => Ok(by_name[0].clone()),
        0 => Err(ApiError::not_found("label", r)),
        _ => Err(ApiError::validation(format!(
            "label {r:?} is ambiguous; use Group/name"
        ))),
    }
}

pub async fn for_issue(conn: &mut SqliteConnection, issue_id: &str) -> ApiResult<Vec<LabelRow>> {
    Ok(sqlx::query_as::<_, LabelRow>(crate::sql!(
        "SELECT {COLS} FROM issue_labels il JOIN labels l ON l.id = il.label_id LEFT JOIN labels g ON g.id = l.group_id WHERE il.issue_id = ?"
    ))
    .bind(issue_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// Checks that a label set holds at most one label from each group.
pub fn check_groups(labels: &[LabelRow]) -> ApiResult<()> {
    let mut seen: std::collections::HashMap<&str, &LabelRow> = std::collections::HashMap::new();
    for l in labels {
        if let Some(g) = l.group_id.as_deref()
            && let Some(prev) = seen.insert(g, l)
            && prev.id != l.id
        {
            return Err(ApiError::validation(format!(
                        "an issue takes one label from group {:?}; got {} and {}",
                        l.group_name.as_deref().unwrap_or(""),
                        prev.full_name(),
                        l.full_name()
                    ))
                    .with_details(serde_json::json!({ "group": l.group_name, "labels": [prev.full_name(), l.full_name()] })));
        }
    }
    Ok(())
}

pub async fn remove_from_issue(
    c: &mut SqliteConnection,
    issue_id: &str,
    label_id: &str,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM issue_labels WHERE issue_id = ? AND label_id = ?")
        .bind(issue_id)
        .bind(label_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn add_to_issue(
    c: &mut SqliteConnection,
    issue_id: &str,
    label_id: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO issue_labels (issue_id, label_id) VALUES (?, ?)")
        .bind(issue_id)
        .bind(label_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn insert(
    c: &mut SqliteConnection,
    id: &str,
    name: &str,
    group_id: Option<String>,
    is_group: bool,
    color: &str,
    description: &Option<String>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO labels (id, name, group_id, is_group, color, description, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(name)
        .bind(group_id)
        .bind(is_group)
        .bind(color)
        .bind(description)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_name(c: &mut SqliteConnection, name: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE labels SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_color(c: &mut SqliteConnection, color: String, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE labels SET color = ? WHERE id = ?")
        .bind(color)
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
    sqlx::query("UPDATE labels SET description = ? WHERE id = ?")
        .bind(description)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn issues_with(c: &mut SqliteConnection, id: &str) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT issue_id FROM issue_labels WHERE label_id = ? OR label_id IN (SELECT id FROM labels WHERE group_id = ?)")
        .bind(id)
        .bind(id)
        .fetch_all(&mut *c)
        .await?)
}

pub async fn delete(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM labels WHERE id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}
