//! Revisions store deltas, not full copies, and still give back every
//! version byte for byte (LLM-346).

use std::path::Path;

use base64::Engine;
use llmtodo_api_types::*;
use serde_json::{Value, json};

async fn start(dir: &Path) -> llmtodo_server::Running {
    llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
        database: llmtodo_server::test_database(dir).await,
    })
    .await
    .unwrap()
}

async fn call(r: &llmtodo_server::Running, method: &str, path: &str, body: Value) -> Value {
    let who = ClientContext {
        session_id: Some("owner-session".into()),
        ..Default::default()
    };
    let ctx =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&who).unwrap());
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut req = reqwest::Client::new()
        .request(m, format!("{}{path}", r.url()))
        .header(CONTEXT_HEADER, ctx);
    if method != "GET" {
        req = req.json(&body);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    assert!(status < 300, "{method} {path} -> {status}: {v}");
    v
}

/// A small deterministic random source, so a failure replays.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// The next text in an edit history: mostly small edits to a long text,
/// with Unicode, CRLF, no final newline, an empty text and a full rewrite.
fn edit(text: &str, i: usize, rng: &mut Lcg) -> String {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    match i % 50 {
        25 => return String::new(),
        40 => {
            let mut text: String = (0..80)
                .map(|j| format!("Rewritten line {j}, edit {i}.\r\n"))
                .collect();
            text.push_str("No final newline ✓");
            return text;
        }
        _ => {}
    }
    let at = rng.next(lines.len() + 1);
    match rng.next(5) {
        0 => lines.insert(at, format!("Inserted line {i}: ünïcode 日本語 🚀\n")),
        1 if !lines.is_empty() => {
            lines.remove(at.min(lines.len() - 1));
        }
        2 if !lines.is_empty() => {
            let k = at.min(lines.len() - 1);
            lines[k] = format!("Changed line {i}\r\n");
        }
        3 => {
            lines.extend((0..rng.next(40) + 1).map(|j| format!("- item {i}.{j} with some words\n")))
        }
        _ => lines.push(format!("Tail {i} with no newline")),
    }
    let mut out: String = lines.concat();
    // Two tail edits in a row would join; a marker keeps every text new.
    out.push_str(&format!("\n<!-- {i} -->"));
    out
}

/// Builds an issue with `edits` description edits and returns every text,
/// version 1 first.
async fn history(r: &llmtodo_server::Running, edits: usize) -> Vec<String> {
    call(
        r,
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "Alex" }),
    )
    .await;
    let first: String = (0..120)
        .map(|i| format!("Line {i} of a long description.\n"))
        .collect();
    call(
        r,
        "POST",
        "/v1/issues",
        json!({ "title": "Long", "description": first }),
    )
    .await;
    let mut texts = vec![first];
    let mut rng = Lcg(346);
    for i in 0..edits {
        let next = edit(texts.last().unwrap(), i, &mut rng);
        let v = call(
            r,
            "PATCH",
            "/v1/issues/ALX-1",
            json!({ "body": next, "force": true }),
        )
        .await;
        assert_eq!(v["description"].as_str().unwrap(), next);
        texts.push(next);
    }
    texts
}

async fn bodies(r: &llmtodo_server::Running) -> Vec<(i64, String)> {
    let h = call(
        r,
        "GET",
        "/v1/issues/ALX-1/history?bodies=true",
        Value::Null,
    )
    .await;
    h["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["version"].as_i64().unwrap(),
                r["body"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn expect(texts: &[String]) -> Vec<(i64, String)> {
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| (i as i64 + 1, t.clone()))
        .collect()
}

async fn stored(pool: &llmtodo_server::store::backend::Pool) -> (i64, i64, i64) {
    use llmtodo_server::store::backend::{self, Backend};
    let bytes = match pool.backend() {
        Backend::Sqlite => "length(CAST(body AS BLOB))",
        Backend::Postgres => "octet_length(body)",
    };
    backend::query_as(llmtodo_server::sql!(
        "SELECT count(*), count(base_version), CAST(sum({bytes}) AS BIGINT) FROM revisions WHERE entity_type = 'issue'"
    ))
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn every_revision_of_a_long_history_restores_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let r = start(dir.path()).await;
    let texts = history(&r, 150).await;
    assert_eq!(bodies(&r).await, expect(&texts));

    // Deltas, not full copies, and far fewer bytes.
    let full: usize = texts.iter().map(String::len).sum();
    let (rows, deltas, bytes) = stored(&r.state.pool()).await;
    assert_eq!(rows, texts.len() as i64);
    assert!(deltas > 100, "only {deltas} of {rows} rows are deltas");
    assert!(
        (bytes as usize) * 4 < full,
        "{bytes} stored bytes for {full} bytes of text"
    );

    // Restore each version, oldest first. Each restore adds a version.
    let mut all = texts.clone();
    for (v, text) in expect(&texts) {
        let got = call(
            &r,
            "POST",
            "/v1/issues/ALX-1/restore",
            json!({ "revision": v }),
        )
        .await;
        assert_eq!(got["description"].as_str().unwrap(), text, "restore of {v}");
        all.push(text);
    }
    assert_eq!(bodies(&r).await, expect(&all));
    r.stop().await.unwrap();
}

/// The upgrade turns old full copies into deltas and keeps every text.
#[tokio::test]
async fn the_upgrade_compacts_full_copies() {
    let dir = tempfile::tempdir().unwrap();
    let r = start(dir.path()).await;
    let texts = history(&r, 70).await;
    let pool = r.state.pool();
    // Back to the old layout: a full copy in every row.
    for (v, text) in expect(&texts) {
        llmtodo_server::store::backend::query("UPDATE revisions SET body = ?, base_version = NULL WHERE entity_type = 'issue' AND version = ?")
            .bind(text)
            .bind(v)
            .execute(&pool)
            .await
            .unwrap();
    }
    llmtodo_server::store::backend::query(
        "INSERT INTO settings (key, value) VALUES ('revisions_compact', 'pending')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(stored(&pool).await.1, 0);
    r.stop().await.unwrap();

    let r = start(dir.path()).await;
    let (rows, deltas, _) = stored(&r.state.pool()).await;
    assert_eq!(rows, texts.len() as i64);
    assert!(deltas > 50, "only {deltas} rows are deltas");
    assert_eq!(bodies(&r).await, expect(&texts));
    let pending: i64 = llmtodo_server::store::backend::query_scalar(
        "SELECT count(*) FROM settings WHERE key = 'revisions_compact'",
    )
    .fetch_one(&r.state.pool())
    .await
    .unwrap();
    assert_eq!(pending, 0);
    // A new edit after the upgrade still reads back.
    let mut all = texts.clone();
    let next = format!("{}\nafter the upgrade", texts.last().unwrap());
    call(
        &r,
        "PATCH",
        "/v1/issues/ALX-1",
        json!({ "body": next, "force": true }),
    )
    .await;
    all.push(next);
    assert_eq!(bodies(&r).await, expect(&all));
    r.stop().await.unwrap();
}

/// A trim keeps the text of every revision it keeps, even when a kept delta
/// loses its base.
#[tokio::test]
async fn a_trim_keeps_the_text_of_kept_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let r = start(dir.path()).await;
    let texts = history(&r, 60).await;
    let pool = r.state.pool();
    // Versions 1 to 40 are old, except 20: its base, 21, goes, and it stays.
    llmtodo_server::store::backend::query(
        "UPDATE revisions SET created_at = '2001-01-01T00:00:00.000000Z'
         WHERE entity_type = 'issue' AND version <= 40 AND version != 20",
    )
    .execute(&pool)
    .await
    .unwrap();
    let base: Option<i64> = llmtodo_server::store::backend::query_scalar(
        "SELECT base_version FROM revisions WHERE version = 20",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        base,
        Some(21),
        "the test needs version 20 stored as a delta"
    );

    let report = call(
        &r,
        "POST",
        "/v1/prune",
        json!({ "revisions_before": "2010-01-01T00:00:00Z" }),
    )
    .await;
    assert_eq!(report["revisions"], 39);
    let kept: Vec<(i64, String)> = expect(&texts)
        .into_iter()
        .filter(|(v, _)| *v == 20 || *v > 40)
        .collect();
    assert_eq!(bodies(&r).await, kept);
    for (v, text) in kept {
        let got = call(
            &r,
            "POST",
            "/v1/issues/ALX-1/restore",
            json!({ "revision": v }),
        )
        .await;
        assert_eq!(got["description"].as_str().unwrap(), text, "restore of {v}");
    }
    r.stop().await.unwrap();
}
