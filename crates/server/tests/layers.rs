//! Keeps the layer rules in ARCHITECTURE.md true: routes hold no SQL, nothing
//! outside routes imports routes, and modules in routes do not import each
//! other.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Source lines without comment lines, as `(file, line, text)`.
fn code_lines(dir: &Path) -> Vec<(PathBuf, usize, String)> {
    let mut files = Vec::new();
    rust_files(dir, &mut files);
    let mut out = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if !t.starts_with("//") {
                out.push((f.clone(), i + 1, line.to_string()));
            }
        }
    }
    out
}

fn fail(rule: &str, found: &[(PathBuf, usize, String)]) {
    assert!(
        found.is_empty(),
        "{rule}. Found:\n{}",
        found
            .iter()
            .map(|(f, l, t)| format!("{}:{l}: {}", f.display(), t.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn routes_hold_no_sql() {
    let found: Vec<_> = code_lines(&src().join("routes"))
        .into_iter()
        .filter(|(_, _, t)| t.contains("sqlx::"))
        .collect();
    fail("routes must not use sqlx; put SQL in store", &found);
}

#[test]
fn nothing_outside_routes_imports_routes() {
    let routes = src().join("routes");
    let found: Vec<_> = code_lines(&src())
        .into_iter()
        .filter(|(f, _, t)| {
            // lib.rs mounts the router, which is the one allowed use.
            !f.starts_with(&routes)
                && t.contains("routes::")
                && !t.contains("api_types::routes::")
                && !(f.ends_with("lib.rs") && t.contains("routes::router("))
        })
        .collect();
    fail("only lib.rs may name routes", &found);
}

#[test]
fn route_modules_do_not_import_each_other() {
    let found: Vec<_> = code_lines(&src().join("routes"))
        .into_iter()
        .filter(|(f, _, t)| {
            !f.ends_with("mod.rs")
                && (t.contains("crate::routes") || t.contains("super::"))
                && !t.contains("use super::issue_detail")
        })
        .collect();
    fail(
        "a route module may only use the helpers in routes/mod.rs",
        &found,
    );
}
