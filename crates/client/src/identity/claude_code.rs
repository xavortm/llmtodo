//! Claude Code, the first [`Harness`]. It sets `CLAUDECODE` and
//! `CLAUDE_CODE_SESSION_ID`, and keeps one JSON line per event in a
//! transcript under `~/.claude/projects`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{Harness, UsageMap, env};
use crate::TranscriptTotals;

/// The Claude Code harness.
pub struct ClaudeCode;

impl Harness for ClaudeCode {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn detected(&self) -> bool {
        env("CLAUDECODE").is_some() || env("AI_AGENT").is_some_and(|a| a.starts_with("claude-code"))
    }

    fn session_id(&self) -> Option<String> {
        env("CLAUDE_CODE_SESSION_ID")
    }

    /// `AI_AGENT` looks like `claude-code_2-1-322_agent`.
    fn version(&self) -> Option<String> {
        env("AI_AGENT")
            .filter(|a| a.starts_with("claude-code"))
            .and_then(|a| a.split('_').nth(1).map(|v| v.replace('-', ".")))
    }

    fn effort(&self) -> Option<String> {
        env("CLAUDE_EFFORT")
    }

    fn transcript(&self, dir: &Path, session: &str) -> Option<PathBuf> {
        transcript_path(dir, session)
    }

    fn model(&self, transcript: &Path) -> Option<String> {
        model_from_transcript(transcript).map(|m| short_model(&m))
    }

    fn totals(&self, transcript: &Path, cache_dir: &Path) -> TranscriptTotals {
        let (usage, active_seconds) = cached_transcript_totals(transcript, cache_dir);
        TranscriptTotals {
            usage,
            active_seconds,
        }
    }
}

/// `claude-opus-5-5` becomes `opus-5.5`.
fn short_model(id: &str) -> String {
    let id = id.trim().trim_start_matches("claude-");
    let id = id.split('[').next().unwrap_or(id);
    let parts: Vec<&str> = id.split('-').collect();
    let n = parts.len();
    if n >= 3
        && parts[n - 1].chars().all(|c| c.is_ascii_digit())
        && parts[n - 2].chars().all(|c| c.is_ascii_digit())
        && parts[n - 1].len() <= 2
    {
        format!(
            "{}-{}.{}",
            parts[..n - 2].join("-"),
            parts[n - 2],
            parts[n - 1]
        )
    } else {
        id.to_string()
    }
}

/// Claude Code keeps transcripts in `~/.claude/projects/<slug>/<session>.jsonl`,
/// where the slug is the project path with every `/` and `.` as `-`.
fn transcript_path(project_dir: &Path, session: &str) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let slug: String = project_dir
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    let p = home
        .join(".claude/projects")
        .join(slug)
        .join(format!("{session}.jsonl"));
    if p.is_file() {
        return Some(p);
    }
    // The session may have started in a parent folder; search by file name.
    let dir = home.join(".claude/projects");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path().join(format!("{session}.jsonl")))
        .find(|p| p.is_file())
}

/// The model of the latest assistant turn in a transcript.
fn model_from_transcript(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(256 * 1024);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let needle = "\"model\":\"";
    let at = text.rfind(needle)? + needle.len();
    let end = text[at..].find('"')? + at;
    let m = &text[at..end];
    (m.starts_with("claude") && m != "<synthetic>").then(|| m.to_string())
}

/// What [`cached_transcript_usage`] keeps between calls.
#[derive(Debug, Default, serde::Serialize, Deserialize)]
struct UsageCache {
    /// Bumped when the format or the parsing changes.
    format: u32,
    /// Bytes parsed so far, by file. Always the end of a whole line.
    offsets: std::collections::BTreeMap<String, u64>,
    /// The last usage line seen for each request ID, with its model key.
    requests: std::collections::BTreeMap<String, (String, llmtodo_api_types::TokenUsage)>,
    /// The time marks of every line so far. See [`super::active_time`].
    #[serde(default)]
    marks: Vec<super::active_time::Mark>,
}

const USAGE_CACHE_FORMAT: u32 = 2;

/// The transcript and its subagent transcripts, which live in a folder named
/// after the session.
fn transcript_files(path: &Path) -> Vec<PathBuf> {
    let mut files = vec![path.to_path_buf()];
    if let Some(dir) = path
        .to_str()
        .and_then(|p| p.strip_suffix(".jsonl"))
        .map(PathBuf::from)
    {
        collect_jsonl(&dir, &mut files, 0);
    }
    files
}

/// Parses the whole lines of `text` into `requests`. `base` is the byte
/// offset of `text` in the file, used as the ID of a line without one.
fn parse_usage_lines(
    file: &Path,
    text: &str,
    base: u64,
    requests: &mut std::collections::BTreeMap<String, (String, llmtodo_api_types::TokenUsage)>,
) {
    let mut at = base;
    for line in text.split_inclusive('\n') {
        let line_at = at;
        at += line.len() as u64;
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg = &v["message"];
        let u = &msg["usage"];
        let Some(model) = msg["model"].as_str().filter(|m| m.starts_with("claude")) else {
            continue;
        };
        let n64 = |x: &serde_json::Value| x.as_u64().unwrap_or(0);
        let (w5, w1) = match u["cache_creation"].as_object() {
            Some(cc) => (
                n64(&cc["ephemeral_5m_input_tokens"]),
                n64(&cc["ephemeral_1h_input_tokens"]),
            ),
            None => (n64(&u["cache_creation_input_tokens"]), 0),
        };
        let key = if u["speed"] == "fast" {
            format!("{model}:fast")
        } else {
            model.to_string()
        };
        let usage = llmtodo_api_types::TokenUsage {
            requests: 1,
            input: n64(&u["input_tokens"]),
            output: n64(&u["output_tokens"]),
            cache_write_5m: w5,
            cache_write_1h: w1,
            cache_read: n64(&u["cache_read_input_tokens"]),
        };
        let id = v["requestId"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{}@{line_at}", file.display()));
        requests.insert(id, (key, usage));
    }
}

fn sum_requests(
    requests: &std::collections::BTreeMap<String, (String, llmtodo_api_types::TokenUsage)>,
) -> UsageMap {
    let mut out = UsageMap::new();
    for (model, u) in requests.values() {
        out.entry(model.clone()).or_default().add(u);
    }
    out
}

/// Sums token usage by model over a Claude Code transcript, plus any
/// subagent transcripts in the session's folder. One API request can appear
/// on several lines, so each request ID counts once, with its last line.
/// The tests check [`cached_transcript_usage`] against it.
#[cfg(test)]
fn transcript_usage(path: &Path) -> UsageMap {
    let mut requests = std::collections::BTreeMap::new();
    for f in transcript_files(path) {
        if let Ok(text) = std::fs::read_to_string(&f) {
            parse_usage_lines(&f, &text, 0, &mut requests);
        }
    }
    sum_requests(&requests)
}

/// Same result as [`transcript_usage`], but it parses only the lines added
/// since the last call. The state is kept in `cache_dir`, one file per
/// transcript. A file that got shorter starts the cache over.
#[cfg(test)]
fn cached_transcript_usage(path: &Path, cache_dir: &Path) -> UsageMap {
    cached_transcript_totals(path, cache_dir).0
}

/// The usage by model and the active seconds of a transcript. The state is
/// kept as [`cached_transcript_usage`] describes.
fn cached_transcript_totals(path: &Path, cache_dir: &Path) -> (UsageMap, u64) {
    use std::io::{Read, Seek, SeekFrom};
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().replace(".jsonl", ".json"))
        .unwrap_or_else(|| "transcript.json".into());
    let cache_path = cache_dir.join(name);
    let mut cache: UsageCache = std::fs::read(&cache_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|c: &UsageCache| c.format == USAGE_CACHE_FORMAT)
        .unwrap_or_default();
    let files = transcript_files(path);
    let shrunk = files.iter().any(|f| {
        let len = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        cache
            .offsets
            .get(&f.display().to_string())
            .is_some_and(|&o| len < o)
    });
    if shrunk {
        cache = UsageCache::default();
    }
    cache.format = USAGE_CACHE_FORMAT;
    let mut changed = shrunk;
    for f in &files {
        let key = f.display().to_string();
        let from = cache.offsets.get(&key).copied().unwrap_or(0);
        let Ok(mut file) = std::fs::File::open(f) else {
            continue;
        };
        if file.seek(SeekFrom::Start(from)).is_err() {
            continue;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            continue;
        }
        // A line still being written waits for the next call.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n').map(|i| i + 1) else {
            continue;
        };
        let text = String::from_utf8_lossy(&buf[..end]);
        parse_usage_lines(f, &text, from, &mut cache.requests);
        cache
            .marks
            .extend(text.lines().filter_map(super::active_time::mark));
        cache.offsets.insert(key, from + end as u64);
        changed = true;
    }
    if changed && std::fs::create_dir_all(cache_dir).is_ok() {
        // Write to a temp file, then rename, so a parallel call never reads
        // half a file.
        let tmp = cache_path.with_extension(format!("json.{}.tmp", std::process::id()));
        if let Ok(bytes) = serde_json::to_vec(&cache)
            && std::fs::write(&tmp, bytes).is_ok()
            && std::fs::rename(&tmp, &cache_path).is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    (
        sum_requests(&cache.requests),
        super::active_time::active_seconds(&cache.marks),
    )
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "tool-results") {
                collect_jsonl(&p, out, depth + 1);
            }
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn cached_totals_count_active_time_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let cache = dir.path().join("cache");
        let line = |at: &str| format!("{{\"type\":\"assistant\",\"timestamp\":\"{at}\"}}\n");
        std::fs::write(
            &path,
            line("2026-11-11T07:00:00Z") + &line("2026-11-11T07:01:00Z"),
        )
        .unwrap();
        assert_eq!(cached_transcript_totals(&path, &cache).1, 60);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut f, line("2026-11-11T07:03:00Z").as_bytes()).unwrap();
        assert_eq!(cached_transcript_totals(&path, &cache).1, 180);
    }
    use super::*;

    #[test]
    fn models() {
        assert_eq!(short_model("claude-opus-5-5"), "opus-5.5");
        assert_eq!(short_model("claude-sonnet-5-5[1m]"), "sonnet-5.5");
        assert_eq!(short_model("qwen3-coder"), "qwen3-coder");
    }

    fn usage_line(id: &str, model: &str, input: u64, output: u64) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "requestId": id,
                "message": { "model": model, "usage": {
                    "input_tokens": input, "output_tokens": output,
                    "cache_read_input_tokens": 7,
                    "cache_creation": { "ephemeral_5m_input_tokens": 3, "ephemeral_1h_input_tokens": 1 }
                }}
            })
        )
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// Each call parses only the new lines, and still gives the same totals
    /// as a full parse: a request split over two calls counts once, a half
    /// written line waits, and subagent files count.
    #[test]
    fn cached_usage_matches_a_full_parse() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let t = dir.path().join("s1.jsonl");
        let sub = dir.path().join("s1/subagents/agent-a.jsonl");
        std::fs::create_dir_all(sub.parent().unwrap()).unwrap();

        append(&t, "{\"type\":\"user\"}\n");
        append(&t, &usage_line("r1", "claude-opus-5-5", 10, 1));
        let check = |step: &str| {
            assert_eq!(
                cached_transcript_usage(&t, &cache),
                transcript_usage(&t),
                "{step}"
            );
        };
        check("first call");
        // The same request again, with more output, after the cache saved.
        append(&t, &usage_line("r1", "claude-opus-5-5", 10, 5));
        append(&t, &usage_line("r2", "claude-opus-5-5", 20, 2));
        check("request split over two calls");
        assert_eq!(
            cached_transcript_usage(&t, &cache)["claude-opus-5-5"].requests,
            2
        );

        // A half written line is not counted until it ends.
        let line = usage_line("r3", "claude-sonnet-5-5", 4, 4);
        let (a, b) = line.split_at(30);
        append(&t, a);
        let partial = cached_transcript_usage(&t, &cache);
        assert!(!partial.contains_key("claude-sonnet-5-5"));
        append(&t, b);
        check("finished line");

        append(&sub, &usage_line("s1", "claude-haiku-5-5", 1, 1));
        check("subagent file");

        // The second call reads only the cache and the new bytes.
        let state: UsageCache =
            serde_json::from_slice(&std::fs::read(cache.join("s1.json")).unwrap()).unwrap();
        assert_eq!(
            state.offsets[&t.display().to_string()],
            std::fs::metadata(&t).unwrap().len()
        );

        // A new, shorter transcript with the same name starts over.
        std::fs::write(&t, usage_line("r9", "claude-opus-5-5", 1, 1)).unwrap();
        check("file got shorter");
    }
}
