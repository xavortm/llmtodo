//! Import.

use crate::db::new_id;
use crate::db::now;
use crate::error::ApiResult;
use sqlx::SqliteConnection;

pub async fn source_hash(c: &mut SqliteConnection, url: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT hash FROM file_sources WHERE url = ?")
            .bind(url)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn insert_file(
    c: &mut SqliteConnection,
    hash: &str,
    mime: Option<String>,
    size: i64,
    url: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO files (hash, mime, size, original_url, created_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (hash) DO UPDATE SET original_url = coalesce(original_url, excluded.original_url)")
        .bind(hash)
        .bind(mime)
        .bind(size)
        .bind(url)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn link_file_source(c: &mut SqliteConnection, url: &str, hash: &str) -> ApiResult<()> {
    sqlx::query("INSERT INTO file_sources (url, hash) VALUES (?, ?) ON CONFLICT (url) DO UPDATE SET hash = excluded.hash")
        .bind(url)
        .bind(hash)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn find_linear(
    c: &mut SqliteConnection,
    table: &str,
    linear_id: &str,
) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar(crate::sql!("SELECT id FROM {table} WHERE linear_id = ?"))
            .bind(linear_id)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn owner_id(c: &mut SqliteConnection) -> ApiResult<String> {
    Ok(
        sqlx::query_scalar("SELECT id FROM actors WHERE is_owner = 1")
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn actor_linear_id(c: &mut SqliteConnection, owner: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT linear_id FROM actors WHERE id = ?")
            .bind(owner)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn clear_actor_linear_id(c: &mut SqliteConnection, lid: &str) -> ApiResult<()> {
    sqlx::query("UPDATE actors SET linear_id = NULL WHERE linear_id = ?")
        .bind(lid)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_actor_linear_id(c: &mut SqliteConnection, lid: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE actors SET linear_id = ? WHERE id = ?")
        .bind(lid)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn find_human_unlinked(
    c: &mut SqliteConnection,
    name: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT id FROM actors WHERE kind = 'human' AND name = ? AND linear_id IS NULL AND is_owner = 0")
        .bind(name)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn human_name_exists(c: &mut SqliteConnection, name: &str) -> ApiResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT 1 FROM actors WHERE kind = 'human' AND name = ?")
            .bind(name)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn insert_actor(
    c: &mut SqliteConnection,
    id: &str,
    name: &str,
    lid: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO actors (id, kind, name, linear_id, created_at) VALUES (?, 'human', ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(lid)
    .bind(now())
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn team_by_key(c: &mut SqliteConnection, key: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM teams WHERE key = ? COLLATE NOCASE")
            .bind(key)
            .fetch_optional(&mut *c)
            .await?,
    )
}

pub async fn update_team(
    c: &mut SqliteConnection,
    lid: &str,
    name: &str,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE teams SET linear_id = ?, name = ?, updated_at = ? WHERE id = ?")
        .bind(lid)
        .bind(name)
        .bind(now())
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn find_status(
    c: &mut SqliteConnection,
    team_id: &str,
    name: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT id FROM statuses WHERE team_id = ? AND name = ? COLLATE NOCASE AND linear_id IS NULL")
        .bind(team_id)
        .bind(name)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn status_name_type(c: &mut SqliteConnection, id: &str) -> ApiResult<(String, String)> {
    Ok(
        sqlx::query_as("SELECT name, type FROM statuses WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn update_status(
    c: &mut SqliteConnection,
    lid: &str,
    name: &str,
    color: String,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE statuses SET linear_id = ?, name = ?, color = ? WHERE id = ?")
        .bind(lid)
        .bind(name)
        .bind(color)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_status(
    c: &mut SqliteConnection,
    id: &str,
    team_id: &str,
    name: &str,
    kind: &str,
    color: String,
    position: i64,
    lid: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO statuses (id, team_id, name, type, color, position, linear_id, review) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(team_id)
        .bind(name)
        .bind(kind)
        .bind(color)
        .bind(position)
        .bind(lid)
        .bind(llmtodo_core::is_default_review_status(name))
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn find_label(
    c: &mut SqliteConnection,
    group: &Option<String>,
    name: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT id FROM labels WHERE coalesce(group_id, '') = coalesce(?, '') AND name = ? COLLATE NOCASE AND linear_id IS NULL")
        .bind(group)
        .bind(name)
        .fetch_optional(&mut *c)
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_label(
    c: &mut SqliteConnection,
    lid: &str,
    name: &str,
    color: String,
    description: Option<String>,
    group: &Option<String>,
    is_group: bool,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE labels SET linear_id = ?, name = ?, color = ?, description = ?, group_id = ?, is_group = ? WHERE id = ?")
        .bind(lid)
        .bind(name)
        .bind(color)
        .bind(description)
        .bind(group)
        .bind(is_group)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_label(
    c: &mut SqliteConnection,
    id: &str,
    name: &str,
    group: &Option<String>,
    is_group: bool,
    color: String,
    description: Option<String>,
    lid: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO labels (id, name, group_id, is_group, color, description, linear_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(name)
        .bind(group)
        .bind(is_group)
        .bind(color)
        .bind(description)
        .bind(lid)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn update_project(
    c: &mut SqliteConnection,
    name: &str,
    description: &str,
    state: &str,
    deleted: Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE projects SET name = ?, description = ?, state = ?, deleted_at = ? WHERE id = ?",
    )
    .bind(name)
    .bind(description)
    .bind(state)
    .bind(deleted)
    .bind(id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn find_project_by_name(
    c: &mut SqliteConnection,
    name: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM projects WHERE name = ? COLLATE NOCASE AND linear_id IS NULL",
    )
    .bind(name)
    .fetch_optional(&mut *c)
    .await?)
}

pub async fn set_project_linear_id(c: &mut SqliteConnection, lid: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE projects SET linear_id = ? WHERE id = ?")
        .bind(lid)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn project_slug_taken(c: &mut SqliteConnection, slug: &str) -> ApiResult<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT 1 FROM projects WHERE slug = ?")
        .bind(slug)
        .fetch_optional(&mut *c)
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_project(
    c: &mut SqliteConnection,
    id: &str,
    slug: &str,
    name: &str,
    description: &str,
    state: &str,
    lid: &str,
    created: String,
    updated: String,
    deleted: Option<String>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO projects (id, slug, name, description, state, linear_id, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(state)
        .bind(lid)
        .bind(created)
        .bind(updated)
        .bind(deleted)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn update_milestone(
    c: &mut SqliteConnection,
    name: &str,
    description: String,
    target_date: Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE milestones SET name = ?, description = ?, target_date = ? WHERE id = ?")
        .bind(name)
        .bind(description)
        .bind(target_date)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_milestone(
    c: &mut SqliteConnection,
    id: &str,
    project_id: &str,
    name: &str,
    description: String,
    target_date: Option<String>,
    position: i64,
    lid: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO milestones (id, project_id, name, description, target_date, position, linear_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(project_id)
        .bind(name)
        .bind(description)
        .bind(target_date)
        .bind(position)
        .bind(lid)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn document_body(c: &mut SqliteConnection, id: &str) -> ApiResult<(String, i64)> {
    Ok(
        sqlx::query_as("SELECT body, version FROM documents WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn update_document(
    c: &mut SqliteConnection,
    title: &str,
    body: &str,
    version: i64,
    id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE documents SET title = ?, body = ?, version = ?, updated_at = ? WHERE id = ?",
    )
    .bind(title)
    .bind(body)
    .bind(version)
    .bind(now())
    .bind(id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

pub async fn document_slug_taken(
    c: &mut SqliteConnection,
    project_id: &str,
    slug: &str,
) -> ApiResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT 1 FROM documents WHERE project_id = ? AND slug = ?")
            .bind(project_id)
            .bind(slug)
            .fetch_optional(&mut *c)
            .await?,
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_document(
    c: &mut SqliteConnection,
    id: &str,
    project_id: &str,
    slug: &str,
    title: &str,
    body: &str,
    lid: &str,
    actor_id: String,
    created: String,
    updated: String,
    deleted: Option<String>,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO documents (id, project_id, slug, title, body, linear_id, created_by, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(project_id)
        .bind(slug)
        .bind(title)
        .bind(body)
        .bind(lid)
        .bind(actor_id)
        .bind(created)
        .bind(updated)
        .bind(deleted)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn set_parent(
    c: &mut SqliteConnection,
    parent: &Option<String>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET parent_id = ? WHERE id = ? AND parent_id IS NOT ?")
        .bind(parent)
        .bind(id)
        .bind(parent)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn find_relation(
    c: &mut SqliteConnection,
    kind: &str,
    a: &str,
    b: &str,
) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM relations WHERE type = ? AND ((from_id = ? AND to_id = ?) OR (? = 'related' AND from_id = ? AND to_id = ?))",
    )
        .bind(kind)
        .bind(a)
        .bind(b)
        .bind(kind)
        .bind(b)
        .bind(a)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn link_relation(c: &mut SqliteConnection, lid: &str, id: &str) -> ApiResult<()> {
    sqlx::query("UPDATE relations SET linear_id = coalesce(linear_id, ?) WHERE id = ?")
        .bind(lid)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn insert_relation(
    c: &mut SqliteConnection,
    kind: &str,
    from: &str,
    to: &str,
    lid: &str,
    actor_id: String,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO relations (id, type, from_id, to_id, linear_id, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(new_id())
        .bind(kind)
        .bind(from)
        .bind(to)
        .bind(lid)
        .bind(actor_id)
        .bind(now())
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn changed_here(
    c: &mut SqliteConnection,
    issue_id: &str,
    actor: &str,
) -> ApiResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT 1 FROM activity WHERE issue_id = ?1 AND actor_id IS NOT ?2
         AND seq > coalesce((SELECT max(seq) FROM activity WHERE issue_id = ?1 AND actor_id = ?2), 0) LIMIT 1",
    )
        .bind(issue_id)
        .bind(actor)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn team_key(c: &mut SqliteConnection, team_id: &str) -> ApiResult<String> {
    Ok(sqlx::query_scalar("SELECT key FROM teams WHERE id = ?")
        .bind(team_id)
        .fetch_one(&mut *c)
        .await?)
}

pub async fn issue_by_key(
    c: &mut SqliteConnection,
    key: &str,
) -> ApiResult<Option<(String, Option<String>)>> {
    Ok(sqlx::query_as("SELECT i.id, i.linear_id FROM issue_keys k JOIN issues i ON i.id = k.issue_id WHERE k.key = ?")
        .bind(key)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn issue_fields(
    c: &mut SqliteConnection,
    id: &str,
) -> ApiResult<(
    String,
    String,
    i64,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    Ok(sqlx::query_as("SELECT title, description, description_version, status_id, priority, assignee_id, project_id, milestone_id, deleted_at FROM issues WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *c)
        .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_issue(
    c: &mut SqliteConnection,
    lid: &str,
    title: &str,
    description: &str,
    version: i64,
    status_id: &str,
    priority: i64,
    assignee: &Option<String>,
    project: &Option<String>,
    milestone: &Option<String>,
    completed: &Option<String>,
    deleted: &Option<String>,
    same: bool,
    updated: &str,
    id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE issues SET linear_id = ?, title = ?, description = ?, description_version = ?, status_id = ?, priority = ?, assignee_id = ?,
                 project_id = ?, milestone_id = ?, completed_at = ?, deleted_at = ?, updated_at = CASE WHEN ? THEN updated_at ELSE ? END WHERE id = ?",
    )
        .bind(lid)
        .bind(title)
        .bind(description)
        .bind(version)
        .bind(status_id)
        .bind(priority)
        .bind(assignee)
        .bind(project)
        .bind(milestone)
        .bind(completed)
        .bind(deleted)
        .bind(same)
        .bind(updated)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_issue(
    c: &mut SqliteConnection,
    id: &str,
    team_id: &str,
    number: i64,
    key: &str,
    title: &str,
    description: &str,
    status_id: &str,
    priority: i64,
    assignee: &Option<String>,
    project: &Option<String>,
    milestone: &Option<String>,
    lid: &str,
    creator: String,
    created: &str,
    updated: &str,
    completed: &Option<String>,
    deleted: &Option<String>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO issues (id, team_id, number, key, title, description, status_id, priority, assignee_id, project_id, milestone_id,
                 linear_id, created_by, created_at, updated_at, completed_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
        .bind(id)
        .bind(team_id)
        .bind(number)
        .bind(key)
        .bind(title)
        .bind(description)
        .bind(status_id)
        .bind(priority)
        .bind(assignee)
        .bind(project)
        .bind(milestone)
        .bind(lid)
        .bind(creator)
        .bind(created)
        .bind(updated)
        .bind(completed)
        .bind(deleted)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn add_issue_key(c: &mut SqliteConnection, key: &str, id: &str) -> ApiResult<()> {
    sqlx::query("INSERT OR IGNORE INTO issue_keys (key, issue_id) VALUES (?, ?)")
        .bind(key)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn finished_status_type(c: &mut SqliteConnection, id: &str) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT s.type FROM issues i JOIN statuses s ON s.id = i.status_id WHERE i.id = ? AND s.type IN ('completed', 'canceled')")
        .bind(id)
        .fetch_optional(&mut *c)
        .await?)
}

pub async fn set_waiting_on(
    c: &mut SqliteConnection,
    waiting: Option<&str>,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE issues SET waiting_on = ? WHERE id = ?")
        .bind(waiting)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn clear_issue_labels(c: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM issue_labels WHERE issue_id = ?")
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn add_issue_label(c: &mut SqliteConnection, id: &str, label_id: &str) -> ApiResult<()> {
    sqlx::query("INSERT OR IGNORE INTO issue_labels (issue_id, label_id) VALUES (?, ?)")
        .bind(id)
        .bind(label_id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn update_attachment(
    c: &mut SqliteConnection,
    lid: &str,
    title: Option<String>,
    subtitle: Option<String>,
    metadata: String,
    id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE attachments SET linear_id = ?, title = ?, subtitle = ?, metadata = ? WHERE id = ?",
    )
    .bind(lid)
    .bind(title)
    .bind(subtitle)
    .bind(metadata)
    .bind(id)
    .execute(&mut *c)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_attachment(
    c: &mut SqliteConnection,
    issue_id: &str,
    kind: &str,
    url: &str,
    title: Option<String>,
    subtitle: Option<String>,
    metadata: String,
    lid: &str,
    actor_id: String,
    created: String,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO attachments (id, issue_id, kind, url, title, subtitle, source, metadata, linear_id, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?, 'linear', ?, ?, ?, ?)")
        .bind(new_id())
        .bind(issue_id)
        .bind(kind)
        .bind(url)
        .bind(title)
        .bind(subtitle)
        .bind(metadata)
        .bind(lid)
        .bind(actor_id)
        .bind(created)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn comment_body(c: &mut SqliteConnection, id: &str) -> ApiResult<(String, i64)> {
    Ok(
        sqlx::query_as("SELECT body, version FROM comments WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *c)
            .await?,
    )
}

pub async fn update_comment(
    c: &mut SqliteConnection,
    body: &str,
    version: i64,
    updated: String,
    id: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE comments SET body = ?, version = ?, updated_at = ? WHERE id = ?")
        .bind(body)
        .bind(version)
        .bind(updated)
        .bind(id)
        .execute(&mut *c)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_comment(
    c: &mut SqliteConnection,
    id: &str,
    issue_id: &str,
    parent: Option<String>,
    author: Option<String>,
    body: &str,
    lid: &str,
    created: String,
    updated: String,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO comments (id, issue_id, parent_id, author_id, body, linear_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(id)
        .bind(issue_id)
        .bind(parent)
        .bind(author)
        .bind(body)
        .bind(lid)
        .bind(created)
        .bind(updated)
        .execute(&mut *c)
        .await?;
    Ok(())
}

pub async fn file_mime(c: &mut SqliteConnection, hash: &str) -> ApiResult<Option<Option<String>>> {
    Ok(sqlx::query_scalar("SELECT mime FROM files WHERE hash = ?")
        .bind(hash)
        .fetch_optional(&mut *c)
        .await?)
}
