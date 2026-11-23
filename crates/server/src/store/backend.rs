//! The database backend layer (LLM-452). One pool type and one connection
//! type over SQLite and Postgres, and query builders that run on either.
//!
//! Nothing outside this module and `db.rs` names a sqlx driver type. Store
//! code writes SQL once, with `?` placeholders, and binds values as `Arg`s.
//! On Postgres two rewrites run before the query: placeholders become `$1`,
//! `$2` and so on, and `CROSS JOIN ... ON`, which SQLite uses to keep the
//! written join order, becomes `JOIN ... ON`. The Postgres schema provides
//! the few SQLite names the queries use: the `nocase` collation, and
//! `json_each`, `json_valid`, `json_extract` and `instr` for text.

use std::borrow::Cow;

use sqlx::pool::PoolConnection;
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::{
    AssertSqlSafe, Decode, FromRow, PgConnection, PgPool, Postgres, Sqlite, SqliteConnection,
    SqlitePool, Transaction, Type,
};

/// Which database a pool or connection talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
        }
    }
}

/// Postgres serializes all writes with this transaction-scoped advisory
/// lock, the way SQLite's `BEGIN IMMEDIATE` does. Every server on one
/// database takes it, so claims, keys and activity seqs stay in order.
const PG_WRITE_LOCK: i64 = 0x6c6c_6d74_6f64_6f01;

/// A connection pool on one backend.
#[derive(Clone, Debug)]
pub enum Pool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

impl Pool {
    pub fn backend(&self) -> Backend {
        match self {
            Pool::Sqlite(_) => Backend::Sqlite,
            Pool::Postgres(_) => Backend::Postgres,
        }
    }

    /// A pooled connection for reads, or for writes that need no lock.
    pub async fn acquire(&self) -> Result<Conn, sqlx::Error> {
        let inner = match self {
            Pool::Sqlite(p) => ConnInner::SqlitePooled(p.acquire().await?),
            Pool::Postgres(p) => ConnInner::PgPooled(p.acquire().await?),
        };
        Ok(Conn::new(inner))
    }

    /// A plain transaction, with no write lock.
    pub async fn begin(&self) -> Result<Conn, sqlx::Error> {
        let inner = match self {
            Pool::Sqlite(p) => ConnInner::SqliteTx(p.begin().await?),
            Pool::Postgres(p) => ConnInner::PgTx(p.begin().await?),
        };
        Ok(Conn::new(inner))
    }

    /// A write transaction. Only one runs at a time on one database: SQLite
    /// takes its write lock at once, and Postgres takes the shared advisory
    /// lock, so two servers on one database also take turns.
    pub async fn begin_write(&self) -> Result<Conn, sqlx::Error> {
        let inner = match self {
            Pool::Sqlite(p) => ConnInner::SqliteTx(p.begin_with("BEGIN IMMEDIATE").await?),
            Pool::Postgres(p) => {
                let mut tx = p.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(PG_WRITE_LOCK)
                    .execute(&mut *tx)
                    .await?;
                ConnInner::PgTx(tx)
            }
        };
        Ok(Conn::new(inner))
    }

    /// Takes a lock that lasts while the returned guard lives, for jobs that
    /// only one server on a database may run, such as the GitHub poll. On
    /// Postgres it is a session advisory lock on a connection the guard
    /// holds, so a server that dies or loses its network frees it. SQLite
    /// has one server per file, so the lock always succeeds. `None` means
    /// another server holds it.
    pub async fn try_job_lock(&self, key: i64) -> Result<Option<JobLock>, sqlx::Error> {
        match self {
            Pool::Sqlite(_) => Ok(Some(JobLock { _conn: None })),
            Pool::Postgres(p) => {
                // Detached, so dropping the guard closes the connection and
                // frees the lock, instead of handing a locked session back
                // to the pool.
                let mut c = p.acquire().await?.detach();
                let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                    .bind(key)
                    .fetch_one(&mut c)
                    .await?;
                Ok(got.then_some(JobLock { _conn: Some(c) }))
            }
        }
    }

    pub async fn close(&self) {
        match self {
            Pool::Sqlite(p) => p.close().await,
            Pool::Postgres(p) => p.close().await,
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Pool::Sqlite(p) => p.is_closed(),
            Pool::Postgres(p) => p.is_closed(),
        }
    }

    /// The SQLite pool, for the SQLite-only parts: backups and laptop sync.
    pub fn sqlite(&self) -> Option<&SqlitePool> {
        match self {
            Pool::Sqlite(p) => Some(p),
            Pool::Postgres(_) => None,
        }
    }

    /// The Postgres pool, for the Postgres-only parts: the event listener.
    pub fn postgres(&self) -> Option<&PgPool> {
        match self {
            Pool::Postgres(p) => Some(p),
            Pool::Sqlite(_) => None,
        }
    }
}

/// A held job lock. Dropping it closes its connection, which frees the
/// lock.
pub struct JobLock {
    _conn: Option<PgConnection>,
}

impl JobLock {
    /// Whether the connection that holds the lock still works. A laptop that
    /// slept may have lost it, and with it the lock.
    pub async fn alive(&mut self) -> bool {
        match &mut self._conn {
            None => true,
            Some(c) => sqlx::query("SELECT 1").execute(&mut *c).await.is_ok(),
        }
    }
}

enum ConnInner {
    SqlitePooled(PoolConnection<Sqlite>),
    SqliteTx(Transaction<'static, Sqlite>),
    SqliteOwned(SqliteConnection),
    PgPooled(PoolConnection<Postgres>),
    PgTx(Transaction<'static, Postgres>),
}

/// One connection: pooled, owned, or inside a transaction.
pub struct Conn {
    inner: ConnInner,
    /// Rows changed through this connection so far, from `execute`.
    changes: u64,
}

pub(crate) enum Raw<'a> {
    Sqlite(&'a mut SqliteConnection),
    Pg(&'a mut PgConnection),
}

impl Conn {
    fn new(inner: ConnInner) -> Self {
        Conn { inner, changes: 0 }
    }

    /// Wraps a connection opened outside a pool, such as a second database
    /// file that sync reads.
    pub fn from_sqlite(c: SqliteConnection) -> Self {
        Conn::new(ConnInner::SqliteOwned(c))
    }

    pub fn backend(&self) -> Backend {
        match self.inner {
            ConnInner::SqlitePooled(_) | ConnInner::SqliteTx(_) | ConnInner::SqliteOwned(_) => {
                Backend::Sqlite
            }
            ConnInner::PgPooled(_) | ConnInner::PgTx(_) => Backend::Postgres,
        }
    }

    pub fn is_postgres(&self) -> bool {
        self.backend() == Backend::Postgres
    }

    pub(crate) fn raw(&mut self) -> Raw<'_> {
        match &mut self.inner {
            ConnInner::SqlitePooled(c) => Raw::Sqlite(c),
            ConnInner::SqliteTx(t) => Raw::Sqlite(t),
            ConnInner::SqliteOwned(c) => Raw::Sqlite(c),
            ConnInner::PgPooled(c) => Raw::Pg(c),
            ConnInner::PgTx(t) => Raw::Pg(t),
        }
    }

    /// Rows changed through this connection by `execute` so far. Compare two
    /// readings to learn whether a step changed anything.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Commits a transaction. A connection outside a transaction has nothing
    /// to commit.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self.inner {
            ConnInner::SqliteTx(t) => t.commit().await,
            ConnInner::PgTx(t) => t.commit().await,
            _ => Ok(()),
        }
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self.inner {
            ConnInner::SqliteTx(t) => t.rollback().await,
            ConnInner::PgTx(t) => t.rollback().await,
            _ => Ok(()),
        }
    }

    /// The SQLite connection, for SQLite-only statements such as `VACUUM`.
    pub fn sqlite(&mut self) -> Option<&mut SqliteConnection> {
        match self.raw() {
            Raw::Sqlite(c) => Some(c),
            Raw::Pg(_) => None,
        }
    }
}

/// SQL text. Built only from constants: a string literal, or the `sql!`
/// macro, which formats constants together. Values always go through
/// `bind`.
#[derive(Clone, Debug)]
pub struct Sql(Cow<'static, str>);

impl Sql {
    /// For the `sql!` macro only.
    #[doc(hidden)]
    pub fn assert_safe(s: String) -> Self {
        Sql(Cow::Owned(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn for_backend(&self, b: Backend) -> Cow<'static, str> {
        match b {
            Backend::Sqlite => self.0.clone(),
            Backend::Postgres => Cow::Owned(pg_placeholders(&self.0).replace("CROSS JOIN", "JOIN")),
        }
    }
}

impl From<&'static str> for Sql {
    fn from(s: &'static str) -> Self {
        Sql(Cow::Borrowed(s))
    }
}

/// With `LLMTODO_SQL_DEBUG` set, prints each failed statement to stderr,
/// for porting queries between backends.
fn debug_failure<T>(r: Result<T, sqlx::Error>, sql: &str) -> Result<T, sqlx::Error> {
    if let Err(e) = &r
        && std::env::var_os("LLMTODO_SQL_DEBUG").is_some()
        && !matches!(e, sqlx::Error::RowNotFound)
    {
        eprintln!("SQL failed: {e}\n{sql}\n");
    }
    r
}

/// Turns each `?` outside quotes into `$1`, `$2` and so on. A numbered
/// `?3` becomes `$3`.
pub fn pg_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 16);
    let mut n = 0;
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
                out.push(ch);
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    out.push(ch);
                }
                '?' => {
                    out.push('$');
                    if chars.peek().is_some_and(char::is_ascii_digit) {
                        while let Some(d) = chars.next_if(char::is_ascii_digit) {
                            out.push(d);
                        }
                    } else {
                        n += 1;
                        out.push_str(&n.to_string());
                    }
                }
                _ => out.push(ch),
            },
        }
    }
    out
}

/// One bound value. Booleans bind as 0 or 1, as SQLite stores them, so both
/// schemas keep integer flags.
#[derive(Clone, Debug)]
pub enum Arg {
    Int(Option<i64>),
    Real(Option<f64>),
    Text(Option<String>),
    Blob(Option<Vec<u8>>),
}

pub trait IntoArg {
    fn into_arg(self) -> Arg;
    /// The typed null for `None`.
    fn null_arg() -> Arg
    where
        Self: Sized;
}

macro_rules! int_arg {
    ($($t:ty),*) => {$(
        impl IntoArg for $t {
            fn into_arg(self) -> Arg { Arg::Int(Some(self as i64)) }
            fn null_arg() -> Arg { Arg::Int(None) }
        }
        impl IntoArg for &$t {
            fn into_arg(self) -> Arg { Arg::Int(Some(*self as i64)) }
            fn null_arg() -> Arg { Arg::Int(None) }
        }
    )*};
}
int_arg!(i64, i32, u32, u16, u8, usize, u64);

impl IntoArg for bool {
    fn into_arg(self) -> Arg {
        Arg::Int(Some(self as i64))
    }
    fn null_arg() -> Arg {
        Arg::Int(None)
    }
}

impl IntoArg for f64 {
    fn into_arg(self) -> Arg {
        Arg::Real(Some(self))
    }
    fn null_arg() -> Arg {
        Arg::Real(None)
    }
}

impl IntoArg for &f64 {
    fn into_arg(self) -> Arg {
        Arg::Real(Some(*self))
    }
    fn null_arg() -> Arg {
        Arg::Real(None)
    }
}

impl IntoArg for String {
    fn into_arg(self) -> Arg {
        Arg::Text(Some(self))
    }
    fn null_arg() -> Arg {
        Arg::Text(None)
    }
}

impl IntoArg for &String {
    fn into_arg(self) -> Arg {
        Arg::Text(Some(self.clone()))
    }
    fn null_arg() -> Arg {
        Arg::Text(None)
    }
}

impl IntoArg for &str {
    fn into_arg(self) -> Arg {
        Arg::Text(Some(self.to_string()))
    }
    fn null_arg() -> Arg {
        Arg::Text(None)
    }
}

impl IntoArg for &&str {
    fn into_arg(self) -> Arg {
        Arg::Text(Some(self.to_string()))
    }
    fn null_arg() -> Arg {
        Arg::Text(None)
    }
}

impl IntoArg for Cow<'_, str> {
    fn into_arg(self) -> Arg {
        Arg::Text(Some(self.into_owned()))
    }
    fn null_arg() -> Arg {
        Arg::Text(None)
    }
}

impl IntoArg for Vec<u8> {
    fn into_arg(self) -> Arg {
        Arg::Blob(Some(self))
    }
    fn null_arg() -> Arg {
        Arg::Blob(None)
    }
}

impl IntoArg for &[u8] {
    fn into_arg(self) -> Arg {
        Arg::Blob(Some(self.to_vec()))
    }
    fn null_arg() -> Arg {
        Arg::Blob(None)
    }
}

impl IntoArg for &Vec<u8> {
    fn into_arg(self) -> Arg {
        Arg::Blob(Some(self.clone()))
    }
    fn null_arg() -> Arg {
        Arg::Blob(None)
    }
}

impl<T: IntoArg> IntoArg for Option<T> {
    fn into_arg(self) -> Arg {
        match self {
            Some(v) => v.into_arg(),
            None => T::null_arg(),
        }
    }
    fn null_arg() -> Arg {
        T::null_arg()
    }
}

impl<'a, T> IntoArg for &'a Option<T>
where
    &'a T: IntoArg,
{
    fn into_arg(self) -> Arg {
        match self {
            Some(v) => v.into_arg(),
            None => <&'a T>::null_arg(),
        }
    }
    fn null_arg() -> Arg {
        <&'a T>::null_arg()
    }
}

impl IntoArg for Arg {
    fn into_arg(self) -> Arg {
        self
    }
    fn null_arg() -> Arg {
        Arg::Text(None)
    }
}

macro_rules! bind_args {
    ($q:expr, $args:expr) => {{
        let mut q = $q;
        for a in $args {
            q = match a {
                Arg::Int(v) => q.bind(v),
                Arg::Real(v) => q.bind(v),
                Arg::Text(v) => q.bind(v),
                Arg::Blob(v) => q.bind(v),
            };
        }
        q
    }};
}

/// Where a query runs: a connection, or a fresh connection from a pool.
pub enum Target<'a> {
    Conn(&'a mut Conn),
    Pool(&'a Pool),
}

impl<'a> From<&'a mut Conn> for Target<'a> {
    fn from(c: &'a mut Conn) -> Self {
        Target::Conn(c)
    }
}

impl<'a> From<&'a Pool> for Target<'a> {
    fn from(p: &'a Pool) -> Self {
        Target::Pool(p)
    }
}

/// Runs `$body` with `$c` bound to the target's connection.
macro_rules! with_conn {
    ($target:expr, |$c:ident| $body:expr) => {{
        match $target.into() {
            Target::Conn($c) => $body,
            Target::Pool(p) => {
                let mut owned = p.acquire().await?;
                let $c = &mut owned;
                $body
            }
        }
    }};
}

/// A row of either backend, for queries that read columns by name.
pub enum Row {
    Sqlite(SqliteRow),
    Pg(PgRow),
}

impl Row {
    pub fn try_get<T>(&self, col: &str) -> Result<T, sqlx::Error>
    where
        T: for<'r> Decode<'r, Sqlite>
            + Type<Sqlite>
            + for<'r> Decode<'r, Postgres>
            + Type<Postgres>,
    {
        use sqlx::Row as _;
        match self {
            Row::Sqlite(r) => r.try_get(col),
            Row::Pg(r) => r.try_get(col),
        }
    }
}

/// The result of `execute`.
#[derive(Clone, Copy, Debug)]
pub struct Done {
    rows: u64,
}

impl Done {
    pub fn rows_affected(&self) -> u64 {
        self.rows
    }
}

/// A statement with no typed result.
pub struct Query {
    sql: Sql,
    args: Vec<Arg>,
}

pub fn query(sql: impl Into<Sql>) -> Query {
    Query {
        sql: sql.into(),
        args: Vec::new(),
    }
}

impl Query {
    pub fn bind(mut self, v: impl IntoArg) -> Self {
        self.args.push(v.into_arg());
        self
    }

    pub async fn execute<'a>(self, t: impl Into<Target<'a>>) -> Result<Done, sqlx::Error> {
        with_conn!(t, |c| execute_on(c, self.sql, self.args).await)
    }

    pub async fn fetch_all<'a>(self, t: impl Into<Target<'a>>) -> Result<Vec<Row>, sqlx::Error> {
        with_conn!(t, |c| {
            let sql = self.sql.for_backend(c.backend());
            match c.raw() {
                Raw::Sqlite(c) => Ok(bind_args!(sqlx::query(AssertSqlSafe(sql)), self.args)
                    .fetch_all(c)
                    .await?
                    .into_iter()
                    .map(Row::Sqlite)
                    .collect()),
                Raw::Pg(c) => {
                    let shown = sql.to_string();
                    Ok(debug_failure(
                        bind_args!(sqlx::query(AssertSqlSafe(sql)), self.args)
                            .fetch_all(c)
                            .await,
                        &shown,
                    )?
                    .into_iter()
                    .map(Row::Pg)
                    .collect())
                }
            }
        })
    }
}

async fn execute_on(c: &mut Conn, sql: Sql, args: Vec<Arg>) -> Result<Done, sqlx::Error> {
    let sql = sql.for_backend(c.backend());
    let rows = match c.raw() {
        Raw::Sqlite(c) => bind_args!(sqlx::query(AssertSqlSafe(sql)), args)
            .execute(c)
            .await?
            .rows_affected(),
        Raw::Pg(c) => {
            let shown = sql.to_string();
            debug_failure(
                bind_args!(sqlx::query(AssertSqlSafe(sql)), args)
                    .execute(c)
                    .await,
                &shown,
            )?
            .rows_affected()
        }
    };
    c.changes += rows;
    Ok(Done { rows })
}

/// Rows of either backend that decode into `Self`. `#[derive(sqlx::FromRow)]`
/// gives both.
pub trait AnyFromRow:
    for<'r> FromRow<'r, SqliteRow> + for<'r> FromRow<'r, PgRow> + Send + Unpin
{
}
impl<T> AnyFromRow for T where
    T: for<'r> FromRow<'r, SqliteRow> + for<'r> FromRow<'r, PgRow> + Send + Unpin
{
}

/// A value of either backend that decodes into `Self`.
pub trait AnyDecode:
    for<'r> Decode<'r, Sqlite>
    + Type<Sqlite>
    + for<'r> Decode<'r, Postgres>
    + Type<Postgres>
    + Send
    + Unpin
{
}
impl<T> AnyDecode for T where
    T: for<'r> Decode<'r, Sqlite>
        + Type<Sqlite>
        + for<'r> Decode<'r, Postgres>
        + Type<Postgres>
        + Send
        + Unpin
{
}

/// A statement whose rows decode into `O`.
pub struct QueryAs<O> {
    sql: Sql,
    args: Vec<Arg>,
    _o: std::marker::PhantomData<fn() -> O>,
}

pub fn query_as<O: AnyFromRow>(sql: impl Into<Sql>) -> QueryAs<O> {
    QueryAs {
        sql: sql.into(),
        args: Vec::new(),
        _o: std::marker::PhantomData,
    }
}

macro_rules! fetch_methods {
    ($ctor:path, $bound:ident) => {
        pub async fn fetch_all<'a>(self, t: impl Into<Target<'a>>) -> Result<Vec<O>, sqlx::Error> {
            with_conn!(t, |c| {
                let sql = self.sql.for_backend(c.backend());
                match c.raw() {
                    Raw::Sqlite(c) => {
                        bind_args!($ctor(AssertSqlSafe(sql)), self.args)
                            .fetch_all(c)
                            .await
                    }
                    Raw::Pg(c) => {
                        let shown = sql.to_string();
                        debug_failure(
                            bind_args!($ctor(AssertSqlSafe(sql)), self.args)
                                .fetch_all(c)
                                .await,
                            &shown,
                        )
                    }
                }
            })
        }

        pub async fn fetch_one<'a>(self, t: impl Into<Target<'a>>) -> Result<O, sqlx::Error> {
            with_conn!(t, |c| {
                let sql = self.sql.for_backend(c.backend());
                match c.raw() {
                    Raw::Sqlite(c) => {
                        bind_args!($ctor(AssertSqlSafe(sql)), self.args)
                            .fetch_one(c)
                            .await
                    }
                    Raw::Pg(c) => {
                        let shown = sql.to_string();
                        debug_failure(
                            bind_args!($ctor(AssertSqlSafe(sql)), self.args)
                                .fetch_one(c)
                                .await,
                            &shown,
                        )
                    }
                }
            })
        }

        pub async fn fetch_optional<'a>(
            self,
            t: impl Into<Target<'a>>,
        ) -> Result<Option<O>, sqlx::Error> {
            with_conn!(t, |c| {
                let sql = self.sql.for_backend(c.backend());
                match c.raw() {
                    Raw::Sqlite(c) => {
                        bind_args!($ctor(AssertSqlSafe(sql)), self.args)
                            .fetch_optional(c)
                            .await
                    }
                    Raw::Pg(c) => {
                        let shown = sql.to_string();
                        debug_failure(
                            bind_args!($ctor(AssertSqlSafe(sql)), self.args)
                                .fetch_optional(c)
                                .await,
                            &shown,
                        )
                    }
                }
            })
        }
    };
}

impl<O: AnyFromRow> QueryAs<O> {
    pub fn bind(mut self, v: impl IntoArg) -> Self {
        self.args.push(v.into_arg());
        self
    }

    fetch_methods!(sqlx::query_as, AnyFromRow);
}

/// A statement whose first column decodes into `O`.
pub struct QueryScalar<O> {
    sql: Sql,
    args: Vec<Arg>,
    _o: std::marker::PhantomData<fn() -> O>,
}

pub fn query_scalar<O: AnyDecode>(sql: impl Into<Sql>) -> QueryScalar<O> {
    QueryScalar {
        sql: sql.into(),
        args: Vec::new(),
        _o: std::marker::PhantomData,
    }
}

impl<O: AnyDecode> QueryScalar<O> {
    pub fn bind(mut self, v: impl IntoArg) -> Self {
        self.args.push(v.into_arg());
        self
    }

    fetch_methods!(sqlx::query_scalar, AnyDecode);
}

/// Builds SQL from constant pieces and bound values, like sqlx's
/// `QueryBuilder`, for either backend.
pub struct QueryBuilder {
    sql: String,
    args: Vec<Arg>,
}

impl QueryBuilder {
    /// Starts with constant SQL.
    pub fn new(init: &str) -> Self {
        QueryBuilder {
            sql: init.to_string(),
            args: Vec::new(),
        }
    }

    /// Appends constant SQL.
    pub fn push(&mut self, s: impl std::fmt::Display) -> &mut Self {
        use std::fmt::Write as _;
        let _ = write!(self.sql, "{s}");
        self
    }

    /// Appends a placeholder and binds the value to it.
    pub fn push_bind(&mut self, v: impl IntoArg) -> &mut Self {
        self.sql.push('?');
        self.args.push(v.into_arg());
        self
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    fn parts(&mut self) -> (Sql, Vec<Arg>) {
        (
            Sql::assert_safe(self.sql.clone()),
            std::mem::take(&mut self.args),
        )
    }

    pub fn build(&mut self) -> Query {
        let (sql, args) = self.parts();
        Query { sql, args }
    }

    pub fn build_query_as<O: AnyFromRow>(&mut self) -> QueryAs<O> {
        let (sql, args) = self.parts();
        QueryAs {
            sql,
            args,
            _o: std::marker::PhantomData,
        }
    }

    pub fn build_query_scalar<O: AnyDecode>(&mut self) -> QueryScalar<O> {
        let (sql, args) = self.parts();
        QueryScalar {
            sql,
            args,
            _o: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pg_placeholders;

    #[test]
    fn placeholders_skip_quotes() {
        assert_eq!(
            pg_placeholders("SELECT '?' , \"a?\" FROM t WHERE a = ? AND b IN (?, ?)"),
            "SELECT '?' , \"a?\" FROM t WHERE a = $1 AND b IN ($2, $3)"
        );
        assert_eq!(
            pg_placeholders("a = ?1 OR b = ?2 OR c = ?1"),
            "a = $1 OR b = $2 OR c = $1"
        );
    }
}
