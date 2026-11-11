//! Laptop handoff through a shared folder, such as Dropbox or iCloud.
//!
//! The folder holds `llmtodo.db.gz`, a `files/` folder and `manifest.json`.
//! Every file is written under a temporary name and renamed, and the
//! manifest goes last, so a half-synced folder never looks complete.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use llmtodo_api_types::*;
use serde_json::json;
use sha2::Digest;

use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, send_reload};
use crate::store;

const DB_FILE: &str = "llmtodo.db.gz";
const MANIFEST: &str = "manifest.json";

async fn folder(st: &AppState, over: Option<&str>) -> ApiResult<PathBuf> {
    let f = match over.filter(|f| !f.trim().is_empty()) {
        Some(f) => f.to_string(),
        None => db::get_setting(&st.pool(), "sync_folder")
            .await?
            .filter(|f| !f.is_empty())
            .ok_or_else(|| {
                ApiError::validation(
                    "no sync folder; run `llmtodo settings set --sync-folder <path>`",
                )
            })?,
    };
    let f = expand_home(&f);
    if !f.is_absolute() {
        return Err(ApiError::validation(format!(
            "sync folder must be an absolute path, got {}",
            f.display()
        )));
    }
    Ok(f)
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}

fn read_manifest(folder: &Path) -> ApiResult<Option<SyncManifest>> {
    match std::fs::read(folder.join(MANIFEST)) {
        Ok(raw) => {
            Ok(Some(serde_json::from_slice(&raw).map_err(|e| {
                ApiError::internal(format!("bad sync manifest: {e}"))
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::internal(format!("read sync manifest: {e}"))),
    }
}

/// Writes `data` to `path` through a temporary file and a rename.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp-llmtodo");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Copies files that the target does not have yet. Names are content
/// hashes, so a present name means a present file.
fn copy_missing(from: &Path, to: &Path) -> std::io::Result<u64> {
    if !from.is_dir() {
        return Ok(0);
    }
    std::fs::create_dir_all(to)?;
    let mut n = 0;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_file() && !target.exists() {
            let tmp = target.with_extension("tmp-llmtodo");
            std::fs::copy(e.path(), &tmp)?;
            std::fs::rename(&tmp, &target)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Runs blocking file work on tokio's blocking pool, so a big database or
/// file never stalls the async workers that serve other requests.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ApiError::internal(format!("file task failed: {e}")))?
}

fn io_err(what: &str) -> impl Fn(std::io::Error) -> ApiError + '_ {
    move |e| ApiError::internal(format!("{what}: {e}"))
}

/// Copies `from` into `to` in chunks and returns the byte count and the
/// SHA-256 of the data.
fn copy_hashed(from: &mut impl Read, to: &mut impl Write) -> std::io::Result<(u64, String)> {
    let mut hash = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hash.update(&buf[..n]);
        to.write_all(&buf[..n])?;
        bytes += n as u64;
    }
    Ok((bytes, hex::encode(hash.finalize())))
}

/// Gzips `src` into `dest` through a temporary file and a rename. Returns
/// the size and SHA-256 of the uncompressed data.
fn pack(src: &Path, dest: &Path) -> std::io::Result<(u64, String)> {
    let mut input = std::io::BufReader::new(std::fs::File::open(src)?);
    let tmp = dest.with_extension("tmp-llmtodo");
    let out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    let mut gz = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let sum = copy_hashed(&mut input, &mut gz)?;
    let file = gz
        .finish()?
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    file.sync_all()?;
    std::fs::rename(&tmp, dest)?;
    Ok(sum)
}

/// Unpacks the gzip file `src` into `dest`. Returns the size and SHA-256
/// of the unpacked data.
fn unpack(src: &Path, dest: &Path) -> std::io::Result<(u64, String)> {
    let input = std::io::BufReader::new(std::fs::File::open(src)?);
    let mut gz = flate2::read::GzDecoder::new(input);
    let mut out = std::io::BufWriter::new(std::fs::File::create(dest)?);
    let sum = copy_hashed(&mut gz, &mut out)?;
    out.into_inner()
        .map_err(std::io::IntoInnerError::into_error)?
        .sync_all()?;
    Ok(sum)
}

async fn local_generation(st: &AppState) -> ApiResult<i64> {
    Ok(db::get_setting(&st.pool(), "sync_generation")
        .await?
        .and_then(|g| g.parse().ok())
        .unwrap_or(0))
}

async fn dirty(st: &AppState) -> ApiResult<bool> {
    Ok(db::get_setting(&st.pool(), "sync_dirty")
        .await?
        .is_some_and(|d| !d.is_empty() && d != "0"))
}

/// How the write counter `sync_dirty` is reset.
enum Clear {
    /// Pull replaces the whole database, so nothing local is unpushed.
    Always,
    /// Push clears it only when it still holds the count inside the copy.
    /// A write after the copy raised it, and stays unpushed.
    IfStill(Option<String>),
}

/// Reads the write counter from the copy that push took.
async fn snapshot_dirty(snapshot: &Path) -> ApiResult<Option<String>> {
    use sqlx::Connection as _;
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(snapshot)
        .read_only(true);
    let mut c = sqlx::SqliteConnection::connect_with(&opts).await?;
    let v = store::sync::dirty_value(&mut c).await?;
    c.close().await?;
    Ok(v)
}

async fn set_state(st: &AppState, generation: i64, primary: bool, clear: Clear) -> ApiResult<()> {
    let mut c = st.pool().acquire().await?;
    db::set_setting(&mut c, "sync_generation", &generation.to_string()).await?;
    match clear {
        Clear::Always => db::set_setting(&mut c, "sync_dirty", "0").await?,
        Clear::IfStill(Some(seen)) => {
            store::sync::clear_dirty(&mut c, seen).await?;
        }
        Clear::IfStill(None) => {}
    }
    db::set_setting(&mut c, "sync_is_primary", if primary { "1" } else { "0" }).await?;
    st.is_primary
        .store(primary, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

pub async fn status(st: &AppState) -> ApiResult<SyncStatus> {
    let f = folder(st, None).await.ok();
    let remote = match &f {
        Some(f) => read_manifest(f)?,
        None => None,
    };
    Ok(SyncStatus {
        host: st.host.clone(),
        folder: f.map(|f| f.display().to_string()),
        is_primary: st.primary(),
        local_generation: local_generation(st).await?,
        unpushed_writes: dirty(st).await?,
        remote,
    })
}

pub async fn push(st: &AppState, req: SyncPushRequest) -> ApiResult<SyncReport> {
    let f = folder(st, req.folder.as_deref()).await?;
    std::fs::create_dir_all(&f)
        .map_err(|e| ApiError::internal(format!("create {}: {e}", f.display())))?;
    if !st.primary() {
        return Err(ApiError::new(
            ErrorCode::NotPrimary,
            "this machine is not the primary; only the primary pushes",
        ));
    }
    let local_gen = local_generation(st).await?;
    let remote = read_manifest(&f)?;
    if let Some(r) = &remote {
        if r.generation > local_gen {
            return Err(ApiError::conflict(format!(
                "the sync folder holds generation {} from {}, newer than this machine's {local_gen}; pull first",
                r.generation, r.pushed_by
            ))
            .with_details(json!({ "remote": r, "local_generation": local_gen })));
        }
        if let Some(owner) = r.owner_host.as_deref().filter(|o| *o != st.host) {
            return Err(ApiError::new(
                ErrorCode::ClaimHeld,
                format!("{owner} holds the sync lock; pull with --take to move it here first"),
            )
            .with_details(json!({ "remote": r })));
        }
    }

    // A consistent copy, compressed into the folder under a temporary name.
    let snapshot = st.data_dir.join("sync-snapshot.db");
    std::fs::remove_file(&snapshot).ok();
    db::vacuum_into(&st.pool(), &snapshot).await?;
    let pushed_writes = snapshot_dirty(&snapshot).await;
    #[cfg(test)]
    tests::after_snapshot().await;
    let packed = match &pushed_writes {
        Ok(_) => {
            let (src, dest) = (snapshot.clone(), f.join(DB_FILE));
            blocking(move || pack(&src, &dest).map_err(io_err("pack the sync copy"))).await
        }
        Err(_) => Ok(Default::default()),
    };
    std::fs::remove_file(&snapshot).ok();
    let (pushed_writes, (db_bytes, db_sha256)) = (pushed_writes?, packed?);
    let files_copied = {
        let (from, to) = (st.data_dir.join("files"), f.join("files"));
        blocking(move || copy_missing(&from, &to).map_err(io_err("copy files"))).await?
    };

    let generation = local_gen.max(remote.as_ref().map_or(0, |r| r.generation)) + 1;
    let manifest = SyncManifest {
        owner_host: (!req.release).then(|| st.host.clone()),
        pushed_by: st.host.clone(),
        schema_version: db::schema_version(&st.pool()).await,
        generation,
        pushed_at: Timestamp::now(),
        db_bytes,
        db_sha256: Some(db_sha256),
        client_version: VERSION.to_string(),
    };
    write_atomic(&f.join(MANIFEST), &serde_json::to_vec_pretty(&manifest)?)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    set_state(st, generation, !req.release, Clear::IfStill(pushed_writes)).await?;
    tracing::info!(generation, release = req.release, "sync push");
    Ok(SyncReport {
        manifest,
        backup: None,
        files_copied,
    })
}

/// Migrates the unpacked copy in `staging` and seeds actors, then closes
/// it with the WAL folded into the main file.
async fn prepare_incoming(staging: &Path) -> ApiResult<()> {
    let pool = db::open(staging)
        .await
        .map_err(|e| ApiError::internal(format!("open sync copy: {e:#}")))?;
    let ready = async {
        crate::store::actors::seed(&pool).await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await?;
        ApiResult::Ok(())
    }
    .await;
    pool.close().await;
    ready
}

/// Backs up the live database, then moves the prepared copy over it and
/// reopens. On failure it puts the backup back, so the pool slot always
/// holds an open pool. The caller must hold the write guard.
async fn swap_in(st: &AppState, staging: &Path) -> ApiResult<PathBuf> {
    let backup = db::backup(&st.pool(), &st.data_dir, "pre-pull").await?;
    let live = db::db_path(&st.data_dir);
    let clear_wal = || {
        for ext in ["db-wal", "db-shm"] {
            std::fs::remove_file(live.with_extension(ext)).ok();
        }
    };
    st.pool().close().await;
    clear_wal();
    let opened = match std::fs::rename(db::db_path(staging), &live) {
        Ok(()) => db::open(&st.data_dir).await.map_err(ApiError::from),
        Err(e) => Err(ApiError::internal(format!("swap database: {e}"))),
    };
    match opened {
        Ok(pool) => {
            *st.pool_slot.write().expect("pool lock") = pool;
            Ok(backup)
        }
        Err(e) => {
            clear_wal();
            std::fs::copy(&backup, &live)
                .map_err(|r| ApiError::internal(format!("{}; restore backup: {r}", e.message)))?;
            let pool = db::open(&st.data_dir)
                .await
                .map_err(|r| ApiError::internal(format!("{}; reopen backup: {r:#}", e.message)))?;
            *st.pool_slot.write().expect("pool lock") = pool;
            tracing::warn!(error = %e.message, "sync pull swap failed; restored the backup");
            Err(e)
        }
    }
}

pub async fn pull(st: &AppState, req: SyncPullRequest) -> ApiResult<SyncReport> {
    let f = folder(st, req.folder.as_deref()).await?;
    let mut manifest = read_manifest(&f)?
        .ok_or_else(|| ApiError::not_found("sync copy", &f.display().to_string()))?;
    let ours = db::MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    if manifest.schema_version > ours {
        return Err(ApiError::validation(format!(
            "the sync copy has schema {}, newer than this build's {ours}; upgrade llmtodo on this machine first",
            manifest.schema_version
        )));
    }
    if let Some(owner) = manifest.owner_host.as_deref().filter(|o| *o != st.host)
        && !req.take
    {
        return Err(ApiError::new(
            ErrorCode::ClaimHeld,
            format!("{owner} holds the sync lock; pass --take to move it here"),
        )
        .with_details(json!({ "remote": manifest })));
    }
    if dirty(st).await? && !req.force {
        return Err(ApiError::conflict("this machine has writes that were never pushed; push them first, or pass --force to drop them")
            .with_details(json!({ "local_generation": local_generation(st).await? })));
    }

    // Unpack, open and migrate the copy in its own folder, so a bad
    // download or a bad copy changes nothing here.
    let staging = st.data_dir.join("sync-incoming");
    std::fs::remove_dir_all(&staging).ok();
    let prepared = async {
        let (src, dir) = (f.join(DB_FILE), staging.clone());
        let (bytes, sha) = blocking(move || {
            std::fs::create_dir_all(&dir).map_err(io_err("create the staging folder"))?;
            unpack(&src, &db::db_path(&dir)).map_err(io_err("unpack sync copy"))
        })
        .await?;
        if bytes != manifest.db_bytes {
            return Err(ApiError::internal(format!(
                "sync copy is {bytes} bytes, the manifest says {}; it may still be syncing",
                manifest.db_bytes
            )));
        }
        if manifest.db_sha256.as_ref().is_some_and(|want| *want != sha) {
            return Err(ApiError::internal(format!(
                "sync copy does not match the manifest for generation {}; it may still be syncing",
                manifest.generation
            )));
        }
        prepare_incoming(&staging).await
    }
    .await;
    if let Err(e) = prepared {
        std::fs::remove_dir_all(&staging).ok();
        return Err(e);
    }
    // Names are content hashes, so extra files do no harm if the swap fails.
    let files_copied = {
        let (from, to) = (f.join("files"), st.data_dir.join("files"));
        blocking(move || copy_missing(&from, &to).map_err(io_err("copy files"))).await?
    };

    // Stop new writes and wait for those in flight, then back up and swap.
    let was_primary = st.primary();
    st.is_primary
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let writes = st.writes.write().await;
    let swapped = swap_in(st, &staging).await;
    std::fs::remove_dir_all(&staging).ok();
    let backup = match swapped {
        Ok(b) => b,
        Err(e) => {
            st.is_primary
                .store(was_primary, std::sync::atomic::Ordering::SeqCst);
            return Err(e);
        }
    };
    set_state(st, manifest.generation, true, Clear::Always).await?;
    // The new file has its own seqs. Clients reload, so live events start
    // after its newest one.
    *st.sent_seq.lock().await = crate::store::activity::max_seq(&st.pool()).await?;
    drop(writes);

    // This machine holds the lock now.
    manifest.owner_host = Some(st.host.clone());
    write_atomic(&f.join(MANIFEST), &serde_json::to_vec_pretty(&manifest)?)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    send_reload(st);
    tracing::info!(generation = manifest.generation, "sync pull");
    Ok(SyncReport {
        manifest,
        backup: Some(backup.display().to_string()),
        files_copied,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use tokio::sync::Notify;

    /// When set, push stops after it takes the copy: it signals the first
    /// `Notify` and waits on the second.
    static PAUSE: Mutex<Option<(Arc<Notify>, Arc<Notify>)>> = Mutex::new(None);

    pub(super) async fn after_snapshot() {
        let pause = PAUSE.lock().unwrap().take();
        if let Some((reached, resume)) = pause {
            reached.notify_one();
            resume.notified().await;
        }
    }

    /// LLM-25: a write that lands after the copy is taken must stay unpushed.
    #[tokio::test]
    async fn push_keeps_writes_made_after_the_copy_unpushed() {
        let dir = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        let running = crate::start(crate::Config {
            data_dir: dir.path().to_path_buf(),
            port: 0,
            host: Some("mac-a".into()),
            github_poll: false,
        })
        .await
        .unwrap();
        let url = running.url();
        let http = reqwest::Client::new();
        http.post(format!("{url}/v1/teams"))
            .json(&json!({ "key": "ALX", "name": "Alex" }))
            .send()
            .await
            .unwrap();

        let (reached, resume) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        *PAUSE.lock().unwrap() = Some((reached.clone(), resume.clone()));
        let push = tokio::spawn({
            let (http, url) = (http.clone(), url.clone());
            let folder = shared.path().display().to_string();
            async move {
                http.post(format!("{url}/v1/sync/push"))
                    .json(&json!({ "folder": folder }))
                    .send()
                    .await
                    .unwrap()
                    .status()
            }
        });
        reached.notified().await;
        let r = http
            .post(format!("{url}/v1/issues"))
            .json(&json!({ "title": "Claimed during push" }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        resume.notify_one();
        assert!(push.await.unwrap().is_success());

        let st: Value = http
            .get(format!("{url}/v1/sync/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            st["unpushed_writes"], true,
            "the write after the copy is not in the folder"
        );
        running.stop().await.unwrap();
    }

    /// LLM-52: push and pull of a big database must not stall the runtime.
    /// On one thread, a blocking gzip would stop this heartbeat for the
    /// whole pack or unpack.
    #[tokio::test(flavor = "current_thread")]
    async fn push_and_pull_keep_the_runtime_responsive() {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        let running = crate::start(crate::Config {
            data_dir: dir.path().to_path_buf(),
            port: 0,
            host: Some("mac-a".into()),
            github_poll: false,
        })
        .await
        .unwrap();
        // About 16 MB that gzip cannot shrink much.
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('ballast', hex(randomblob(8000000)))",
        )
        .execute(&running.state.pool())
        .await
        .unwrap();
        let url = running.url();
        let http = reqwest::Client::new();
        let folder = shared.path().display().to_string();

        let worst = Arc::new(Mutex::new(Duration::ZERO));
        let beat = tokio::spawn({
            let worst = worst.clone();
            async move {
                loop {
                    let t = Instant::now();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let gap = t.elapsed();
                    let mut w = worst.lock().unwrap();
                    *w = (*w).max(gap);
                }
            }
        });
        for route in ["push", "pull"] {
            let r = http
                .post(format!("{url}/v1/sync/{route}"))
                .json(&json!({ "folder": folder }))
                .send()
                .await
                .unwrap();
            assert!(
                r.status().is_success(),
                "{route}: {}",
                r.text().await.unwrap()
            );
        }
        beat.abort();
        let worst = *worst.lock().unwrap();
        assert!(
            worst < Duration::from_millis(300),
            "the runtime stalled for {worst:?}"
        );
        running.stop().await.unwrap();
    }
}
