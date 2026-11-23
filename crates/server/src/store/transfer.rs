//! Copies a whole workspace from one database to another, on either backend
//! (LLM-460, LLM-462). Postgres backups copy into a fresh SQLite file, and
//! `llmtodo db migrate` copies between SQLite and Postgres both ways.
//!
//! IDs, keys and activity seqs stay the same. The search tables are not
//! copied: the target's triggers fill them as the issues, comments and
//! documents arrive.

use crate::error::{ApiError, ApiResult};
use crate::store::backend::{self, Arg, Backend, Conn};

/// One table to copy: its name, the columns that give a stable order, and
/// the columns that point at rows that may come later in the same table or
/// in a later table. Those are written in a second pass.
struct Table {
    name: &'static str,
    order: &'static str,
    late: &'static [&'static str],
}

const fn t(name: &'static str, order: &'static str, late: &'static [&'static str]) -> Table {
    Table { name, order, late }
}

/// Every table of a workspace, parents before children.
const TABLES: &[Table] = &[
    t("settings", "key", &[]),
    t("actors", "id", &[]),
    t("teams", "id", &["pr_open_status_id", "pr_merged_status_id"]),
    t("statuses", "id", &[]),
    t("labels", "id", &["group_id"]),
    t("projects", "id", &[]),
    t("project_teams", "project_id, team_id", &[]),
    t("project_repos", "project_id, repo", &[]),
    t("milestones", "id", &[]),
    t("issues", "id", &["parent_id"]),
    t("issue_keys", "key", &[]),
    t("issue_labels", "issue_id, label_id", &[]),
    t("relations", "id", &[]),
    t("attachments", "id", &[]),
    t("comments", "id", &["parent_id"]),
    t("documents", "id", &[]),
    t("revisions", "id", &[]),
    t("activity", "seq", &[]),
    t("claims", "id", &["stolen_from"]),
    t("work_sessions", "id", &[]),
    t("session_notes", "id", &[]),
    t("saved_views", "id", &[]),
    t("files", "hash", &[]),
    t("file_sources", "url", &[]),
    t("work_session_usage", "work_session_id, model", &[]),
    t("transcript_usage_seen", "transcript, model", &[]),
    t("transcript_time_seen", "transcript", &[]),
    t("analyzing", "issue_id", &[]),
    t("questions", "issue_id, position", &[]),
    t("reviews", "id", &[]),
];

/// Laptop handoff state belongs to one SQLite file, so it never moves.
const LOCAL_SETTINGS: &str = "key NOT LIKE 'sync\\_%' ESCAPE '\\'";

const PAGE: i64 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Real,
    Text,
    Blob,
}

/// Rows copied per table.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Report {
    pub tables: Vec<(String, u64)>,
}

impl Report {
    pub fn rows(&self, table: &str) -> u64 {
        self.tables
            .iter()
            .find(|(t, _)| t == table)
            .map_or(0, |(_, n)| *n)
    }
}

/// Each table's columns, per backend. One process runs one build, so the
/// schema does not change while it runs, and a refresh of the offline copy
/// skips the lookups (LLM-471).
type ColumnCache =
    std::sync::Mutex<std::collections::HashMap<(Backend, String), Vec<(String, Kind)>>>;
static COLUMNS: std::sync::OnceLock<ColumnCache> = std::sync::OnceLock::new();

async fn columns(c: &mut Conn, table: &str) -> ApiResult<Vec<(String, Kind)>> {
    let cache = COLUMNS.get_or_init(Default::default);
    let key = (c.backend(), table.to_string());
    if let Some(cols) = cache.lock().expect("column cache").get(&key) {
        return Ok(cols.clone());
    }
    let cols = load_columns(c, table).await?;
    cache
        .lock()
        .expect("column cache")
        .insert(key, cols.clone());
    Ok(cols)
}

async fn load_columns(c: &mut Conn, table: &str) -> ApiResult<Vec<(String, Kind)>> {
    let rows: Vec<(String, String)> = match c.backend() {
        Backend::Sqlite => {
            backend::query_as("SELECT name, type FROM pragma_table_info(?) ORDER BY cid")
                .bind(table)
                .fetch_all(&mut *c)
                .await?
        }
        Backend::Postgres => {
            backend::query_as(
                "SELECT column_name::text, data_type::text FROM information_schema.columns
                 WHERE table_schema = current_schema() AND table_name = ? AND is_generated = 'NEVER'
                 ORDER BY ordinal_position",
            )
            .bind(table)
            .fetch_all(&mut *c)
            .await?
        }
    };
    if rows.is_empty() {
        return Err(ApiError::internal(format!("table {table} is missing")));
    }
    Ok(rows
        .into_iter()
        .map(|(name, ty)| {
            let ty = ty.to_ascii_lowercase();
            let kind = if ty.contains("int") {
                Kind::Int
            } else if ty.contains("real") || ty.contains("double") || ty.contains("float") {
                Kind::Real
            } else if ty.contains("blob") || ty.contains("bytea") {
                Kind::Blob
            } else {
                Kind::Text
            };
            (name, kind)
        })
        .collect())
}

fn read(row: &backend::Row, col: &str, kind: Kind) -> ApiResult<Arg> {
    Ok(match kind {
        Kind::Int => Arg::Int(row.try_get(col)?),
        Kind::Real => Arg::Real(row.try_get(col)?),
        Kind::Text => Arg::Text(row.try_get(col)?),
        Kind::Blob => Arg::Blob(row.try_get(col)?),
    })
}

fn null(kind: Kind) -> Arg {
    match kind {
        Kind::Int => Arg::Int(None),
        Kind::Real => Arg::Real(None),
        Kind::Text => Arg::Text(None),
        Kind::Blob => Arg::Blob(None),
    }
}

fn is_null(a: &Arg) -> bool {
    matches!(
        a,
        Arg::Int(None) | Arg::Real(None) | Arg::Text(None) | Arg::Blob(None)
    )
}

/// Rows in the target's issues table, to refuse a copy over real data.
pub async fn issue_count(c: &mut Conn) -> ApiResult<i64> {
    Ok(backend::query_scalar("SELECT count(*) FROM issues")
        .fetch_one(&mut *c)
        .await?)
}

/// Copies every table from `from` into `to`, which must hold no issues. The
/// rows the target's start-up wrote, such as the seeded actors, go first.
/// Both connections should be transactions: `from` reads one snapshot, and
/// `to` commits all or nothing.
///
/// `files_dir` is the source's `files/` folder. Copying into Postgres reads
/// a file's bytes from there when its row holds none (LLM-458).
pub async fn copy_workspace(
    from: &mut Conn,
    to: &mut Conn,
    files_dir: Option<&std::path::Path>,
) -> ApiResult<Report> {
    if from.is_postgres() {
        backend::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *from)
            .await?;
    }
    if issue_count(to).await? > 0 {
        return Err(ApiError::validation(
            "the target database already holds issues",
        ));
    }
    for table in TABLES.iter().rev() {
        backend::query(crate::sql!("DELETE FROM {}", table.name))
            .execute(&mut *to)
            .await?;
    }

    let mut report = Report::default();
    // (table, column, kind, id, value) for the second pass.
    let mut late: Vec<(&'static str, String, Arg, Arg)> = Vec::new();
    for table in TABLES {
        let source = columns(from, table.name).await?;
        let target = columns(to, table.name).await?;
        let cols: Vec<(String, Kind)> = source
            .into_iter()
            .filter(|(n, _)| target.iter().any(|(t, _)| t == n))
            .collect();
        let list = cols
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let filter = if table.name == "settings" {
            format!(" WHERE {LOCAL_SETTINGS}")
        } else {
            String::new()
        };
        let mut copied = 0u64;
        let mut offset = 0i64;
        loop {
            let rows = backend::query(crate::sql!(
                "SELECT {list} FROM {}{filter} ORDER BY {} LIMIT ? OFFSET ?",
                table.name,
                table.order
            ))
            .bind(PAGE)
            .bind(offset)
            .fetch_all(&mut *from)
            .await?;
            if rows.is_empty() {
                break;
            }
            let mut insert = backend::QueryBuilder::new("");
            insert.push(crate::sql!("INSERT INTO {} ({list}) VALUES ", table.name).as_str());
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    insert.push(", ");
                }
                insert.push("(");
                let id = if table.late.is_empty() {
                    None
                } else {
                    Some(read(row, "id", Kind::Text)?)
                };
                for (j, (col, kind)) in cols.iter().enumerate() {
                    if j > 0 {
                        insert.push(", ");
                    }
                    let mut v = read(row, col, *kind)?;
                    if table.name == "files"
                        && col == "content"
                        && is_null(&v)
                        && to.is_postgres()
                        && let (Some(dir), Arg::Text(Some(hash))) =
                            (files_dir, read(row, "hash", Kind::Text)?)
                        && let Ok(bytes) = std::fs::read(dir.join(&hash))
                    {
                        v = Arg::Blob(Some(bytes));
                    }
                    if table.late.contains(&col.as_str()) {
                        if !is_null(&v) {
                            late.push((table.name, col.clone(), v, id.clone().expect("late id")));
                        }
                        insert.push_bind(null(*kind));
                    } else {
                        insert.push_bind(v);
                    }
                }
                insert.push(")");
            }
            insert.build().execute(&mut *to).await?;
            copied += rows.len() as u64;
            offset += PAGE;
        }
        report.tables.push((table.name.to_string(), copied));
    }
    for (table, col, value, id) in late {
        backend::query(crate::sql!("UPDATE {table} SET {col} = ? WHERE id = ?"))
            .bind(value)
            .bind(id)
            .execute(&mut *to)
            .await?;
    }
    if to.is_postgres() {
        // Explicit seqs leave the identity behind. The next one must follow
        // the copied ones.
        backend::query(
            "SELECT setval(pg_get_serial_sequence('activity', 'seq'),
                coalesce((SELECT max(seq) FROM activity), 0) + 1, false)",
        )
        .execute(&mut *to)
        .await?;
    }
    Ok(report)
}

/// Row counts of every copied table, to check a copy against its report.
pub async fn counts(c: &mut Conn) -> ApiResult<Vec<(String, u64)>> {
    let mut out = Vec::new();
    for table in TABLES {
        let filter = if table.name == "settings" {
            format!(" WHERE {LOCAL_SETTINGS}")
        } else {
            String::new()
        };
        let n: i64 =
            backend::query_scalar(crate::sql!("SELECT count(*) FROM {}{filter}", table.name))
                .fetch_one(&mut *c)
                .await?;
        out.push((table.name.to_string(), n as u64));
    }
    Ok(out)
}

/// The rows of a set of issues, table by table, for the offline copy's
/// refresh (LLM-471). `{ids}` stands for the list of issue IDs. A refresh
/// deletes in reverse order and copies in this order, so the subqueries
/// that name `comments` and `work_sessions` still find their rows.
pub const ISSUE_ROWS: &[(&str, &str)] = &[
    ("issues", "id IN ({ids})"),
    ("issue_keys", "issue_id IN ({ids})"),
    ("issue_labels", "issue_id IN ({ids})"),
    ("relations", "from_id IN ({ids}) OR to_id IN ({ids})"),
    ("attachments", "issue_id IN ({ids})"),
    ("comments", "issue_id IN ({ids})"),
    (
        "revisions",
        "(entity_type = 'issue' AND entity_id IN ({ids})) OR (entity_type = 'comment' AND entity_id IN (SELECT id FROM comments WHERE issue_id IN ({ids})))",
    ),
    ("claims", "issue_id IN ({ids})"),
    ("work_sessions", "issue_id IN ({ids})"),
    (
        "work_session_usage",
        "work_session_id IN (SELECT id FROM work_sessions WHERE issue_id IN ({ids}))",
    ),
    ("session_notes", "issue_id IN ({ids})"),
    ("analyzing", "issue_id IN ({ids})"),
    ("questions", "issue_id IN ({ids})"),
    ("reviews", "issue_id IN ({ids})"),
];

/// The tables that hold no issue rows: the catalog, documents and views.
/// They are small, so a refresh copies them whole when any of them changed.
pub const CATALOG_ROWS: &[&str] = &[
    "settings",
    "actors",
    "teams",
    "statuses",
    "labels",
    "projects",
    "project_teams",
    "project_repos",
    "milestones",
    "documents",
    "saved_views",
];

/// A filter from [`ISSUE_ROWS`], with one placeholder per ID for each
/// `{ids}`.
fn scoped(filter: &str, ids: &[String]) -> (String, Vec<Arg>) {
    let marks = vec!["?"; ids.len()].join(", ");
    let n = filter.matches("{ids}").count();
    let args = (0..n)
        .flat_map(|_| ids.iter().map(|i| Arg::Text(Some(i.clone()))))
        .collect();
    (filter.replace("{ids}", &marks), args)
}

/// Replaces the rows of `table` that match `filter` in `to` with the ones
/// in `from`. `filter` is constant SQL with `?` placeholders for `args`.
/// The target should have foreign keys off, since rows arrive in table
/// order and a parent may come later.
async fn replace_where(
    from: &mut Conn,
    to: &mut Conn,
    table: &'static str,
    filter: &str,
    args: &[Arg],
) -> ApiResult<u64> {
    let order = TABLES
        .iter()
        .find(|t| t.name == table)
        .map_or("rowid", |t| t.order);
    let source = columns(from, table).await?;
    let target = columns(to, table).await?;
    let cols: Vec<(String, Kind)> = source
        .into_iter()
        .filter(|(n, _)| target.iter().any(|(t, _)| t == n))
        .collect();
    let list = cols
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let local = if table == "settings" {
        format!(" AND {LOCAL_SETTINGS}")
    } else {
        String::new()
    };
    let mut delete = backend::QueryBuilder::new("");
    delete.push(crate::sql!("DELETE FROM {table} WHERE ({filter}){local}").as_str());
    let mut q = delete.build();
    for a in args {
        q = q.bind(a.clone());
    }
    q.execute(&mut *to).await?;
    let mut copied = 0u64;
    let mut offset = 0i64;
    loop {
        let mut select = backend::QueryBuilder::new("");
        select.push(
            crate::sql!(
                "SELECT {list} FROM {table} WHERE ({filter}){local} ORDER BY {order} LIMIT ? OFFSET ?"
            )
            .as_str(),
        );
        let mut q = select.build();
        for a in args {
            q = q.bind(a.clone());
        }
        let rows = q.bind(PAGE).bind(offset).fetch_all(&mut *from).await?;
        if rows.is_empty() {
            break;
        }
        let mut insert = backend::QueryBuilder::new("");
        insert.push(crate::sql!("INSERT INTO {table} ({list}) VALUES ").as_str());
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                insert.push(", ");
            }
            insert.push("(");
            for (j, (col, kind)) in cols.iter().enumerate() {
                if j > 0 {
                    insert.push(", ");
                }
                insert.push_bind(read(row, col, *kind)?);
            }
            insert.push(")");
        }
        insert.build().execute(&mut *to).await?;
        copied += rows.len() as u64;
        if (rows.len() as i64) < PAGE {
            break;
        }
        offset += PAGE;
    }
    Ok(copied)
}

/// Copies the rows of `issue_ids` from `from` into `to`, in place of the
/// ones `to` holds. An issue that `from` no longer has, such as a pruned
/// one, leaves `to` too.
pub async fn refresh_issues(from: &mut Conn, to: &mut Conn, issue_ids: &[String]) -> ApiResult<()> {
    for chunk in issue_ids.chunks(100) {
        // Delete children first, then copy parents first.
        for (table, filter) in ISSUE_ROWS.iter().rev() {
            let (f, args) = scoped(filter, chunk);
            let mut q = backend::QueryBuilder::new("");
            q.push(crate::sql!("DELETE FROM {table} WHERE {f}").as_str());
            let mut q = q.build();
            for a in args {
                q = q.bind(a);
            }
            q.execute(&mut *to).await?;
        }
        for (table, filter) in ISSUE_ROWS {
            let (f, args) = scoped(filter, chunk);
            replace_where(from, to, table, &f, &args).await?;
        }
    }
    Ok(())
}

/// Copies the catalog tables whole, and the revisions of documents.
pub async fn refresh_catalog(from: &mut Conn, to: &mut Conn) -> ApiResult<()> {
    for table in CATALOG_ROWS {
        replace_where(from, to, table, "1 = 1", &[]).await?;
    }
    replace_where(from, to, "revisions", "entity_type = 'document'", &[]).await?;
    Ok(())
}

/// Copies the activity rows after `seq`, up to `upto`.
pub async fn refresh_activity(
    from: &mut Conn,
    to: &mut Conn,
    seq: i64,
    upto: i64,
) -> ApiResult<u64> {
    replace_where(
        from,
        to,
        "activity",
        "seq > ? AND seq <= ?",
        &[Arg::Int(Some(seq)), Arg::Int(Some(upto))],
    )
    .await
}
