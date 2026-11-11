use std::collections::HashMap;

use llmtodo_api_types::{Actor, ActorInput, ActorKind};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{new_id, now, parse_uuid};
use crate::error::{ApiError, ApiResult};

pub const OWNER_NAME: &str = "owner";

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    kind: String,
    name: String,
    harness: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    is_owner: i64,
}

fn to_actor(r: Row) -> Actor {
    Actor {
        id: parse_uuid(&r.id),
        kind: r.kind.parse().unwrap_or(ActorKind::Human),
        name: r.name,
        harness: r.harness,
        model: r.model,
        effort: r.effort,
        is_owner: r.is_owner == 1,
    }
}

const COLS: &str = "id, kind, name, harness, model, effort, is_owner";

pub fn agent_name(harness: Option<&str>, model: Option<&str>, effort: Option<&str>) -> String {
    let parts: Vec<&str> = [harness, model, effort]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        "agent".into()
    } else {
        parts.join("/")
    }
}

/// Makes sure the owner and the built-in system actors exist.
pub async fn seed(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO actors (id, kind, name, is_owner, created_at) VALUES (?, 'human', ?, 1, ?) ON CONFLICT DO NOTHING")
        .bind(new_id())
        .bind(OWNER_NAME)
        .bind(now())
        .execute(pool)
        .await?;
    for name in ["github", "linear-import", "migration"] {
        sqlx::query("INSERT INTO actors (id, kind, name, created_at) VALUES (?, 'system', ?, ?) ON CONFLICT DO NOTHING")
            .bind(new_id())
            .bind(name)
            .bind(now())
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Finds or creates the actor a request names.
pub async fn resolve_input(pool: &SqlitePool, input: &ActorInput) -> ApiResult<Actor> {
    let clean = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match input.kind {
        ActorKind::Agent => {
            let (h, m, e) = (
                clean(&input.harness),
                clean(&input.model),
                clean(&input.effort),
            );
            let found = sqlx::query_as::<_, Row>(crate::sql!(
                "SELECT {COLS} FROM actors WHERE kind = 'agent' AND coalesce(harness, '') = coalesce(?, '') AND coalesce(model, '') = coalesce(?, '') AND coalesce(effort, '') = coalesce(?, '')"
            ))
            .bind(&h)
            .bind(&m)
            .bind(&e)
            .fetch_optional(pool)
            .await?;
            if let Some(r) = found {
                return Ok(to_actor(r));
            }
            let name = agent_name(h.as_deref(), m.as_deref(), e.as_deref());
            sqlx::query("INSERT INTO actors (id, kind, name, harness, model, effort, created_at) VALUES (?, 'agent', ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING")
                .bind(new_id())
                .bind(&name)
                .bind(&h)
                .bind(&m)
                .bind(&e)
                .bind(now())
                .execute(pool)
                .await?;
            Box::pin(resolve_input(pool, input)).await
        }
        ActorKind::Human => match clean(&input.name) {
            None => owner(pool).await,
            Some(name) if name.eq_ignore_ascii_case(OWNER_NAME) => owner(pool).await,
            Some(name) => {
                if let Some(r) = sqlx::query_as::<_, Row>(crate::sql!(
                    "SELECT {COLS} FROM actors WHERE kind = 'human' AND name = ?"
                ))
                .bind(&name)
                .fetch_optional(pool)
                .await?
                {
                    return Ok(to_actor(r));
                }
                sqlx::query("INSERT INTO actors (id, kind, name, created_at) VALUES (?, 'human', ?, ?) ON CONFLICT DO NOTHING")
                    .bind(new_id())
                    .bind(&name)
                    .bind(now())
                    .execute(pool)
                    .await?;
                Box::pin(resolve_input(pool, input)).await
            }
        },
        ActorKind::System => Err(ApiError::forbidden("clients cannot act as a system actor")),
    }
}

pub async fn owner(pool: &SqlitePool) -> ApiResult<Actor> {
    let r = sqlx::query_as::<_, Row>(crate::sql!("SELECT {COLS} FROM actors WHERE is_owner = 1"))
        .fetch_one(pool)
        .await?;
    Ok(to_actor(r))
}

pub async fn system(pool: &SqlitePool, name: &str) -> ApiResult<Actor> {
    let r = sqlx::query_as::<_, Row>(crate::sql!(
        "SELECT {COLS} FROM actors WHERE kind = 'system' AND name = ?"
    ))
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(to_actor(r))
}

pub async fn all(conn: &mut SqliteConnection) -> ApiResult<HashMap<String, Actor>> {
    let rows =
        sqlx::query_as::<_, Row>(crate::sql!("SELECT {COLS} FROM actors ORDER BY created_at"))
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.id.clone(), to_actor(r)))
        .collect())
}

/// The IDs of the actors whose name holds `part`, ignoring case.
pub async fn ids_with_name_containing(
    conn: &mut SqliteConnection,
    part: &str,
) -> ApiResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM actors WHERE instr(lower(name), lower(?)) > 0")
            .bind(part)
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// Resolves `me`, `owner`, an ID or a name. Returns the actor ID.
pub async fn resolve_ref(conn: &mut SqliteConnection, me: &Actor, r: &str) -> ApiResult<String> {
    let r = r.trim();
    if r.eq_ignore_ascii_case("me") || r == "@me" {
        return Ok(me.id.to_string());
    }
    let row: Option<String> = if r.eq_ignore_ascii_case(OWNER_NAME) {
        sqlx::query_scalar("SELECT id FROM actors WHERE is_owner = 1")
            .fetch_optional(&mut *conn)
            .await?
    } else {
        sqlx::query_scalar("SELECT id FROM actors WHERE id = ? OR name = ? COLLATE NOCASE ORDER BY is_owner DESC LIMIT 1")
            .bind(r)
            .bind(r)
            .fetch_optional(&mut *conn)
            .await?
    };
    row.ok_or_else(|| ApiError::not_found("actor", r))
}
