//! JSON or table output, with `--fields` filtering.

use std::io::IsTerminal;

use llmtodo_api_types::{DepNode, Issue, IssueDetail, format_active_time};
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct Out {
    pub json: bool,
    pub fields: Option<Vec<String>>,
}

fn print_dep(n: &DepNode, indent: usize) {
    println!(
        "{}{}  {}  depth {}  {}{}",
        "  ".repeat(indent),
        n.issue.key,
        n.issue.status.name,
        n.depth,
        n.issue.title,
        if n.truncated { "  ..." } else { "" }
    );
}

fn print_dep_tree(n: &DepNode, indent: usize) {
    print_dep(n, indent);
    for child in n.depends_on.iter().chain(&n.blocks) {
        print_dep_tree(child, indent + 1);
    }
}

impl Out {
    pub fn new(force_json: bool, force_human: bool, fields: Option<String>) -> Self {
        let json = if force_json {
            true
        } else if force_human {
            false
        } else {
            !std::io::stdout().is_terminal()
        };
        let fields = fields.map(|f| {
            f.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });
        Self { json, fields }
    }

    /// Prints any value: JSON, or pretty JSON for a person.
    pub fn value(&self, v: Value) {
        let v = self.filter(v);
        if self.json {
            println!("{}", serde_json::to_string(&v).unwrap_or_default());
        } else {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }
    }

    pub fn ser<T: serde::Serialize>(&self, v: &T) {
        self.value(serde_json::to_value(v).unwrap_or(Value::Null));
    }

    pub fn issues(&self, items: &[Issue]) {
        if self.json {
            return self.ser(&items);
        }
        if items.is_empty() {
            println!("No issues.");
            return;
        }
        let rows: Vec<[String; 5]> = items
            .iter()
            .map(|i| {
                [
                    i.key.to_string(),
                    i.status.name.clone(),
                    if i.priority == llmtodo_api_types::Priority::None {
                        "-".into()
                    } else {
                        i.priority.to_string()
                    },
                    i.assignee
                        .as_ref()
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| "-".into()),
                    i.title.clone(),
                ]
            })
            .collect();
        table(&["KEY", "STATUS", "PRIORITY", "ASSIGNEE", "TITLE"], &rows);
    }

    /// A dependency tree: JSON, or one indented line per issue.
    pub fn deps(&self, root: &DepNode) {
        if self.json {
            return self.ser(root);
        }
        print_dep(root, 0);
        for (title, list) in [("Depends on", &root.depends_on), ("Blocks", &root.blocks)] {
            if !list.is_empty() {
                println!("{title}:");
                for node in list {
                    print_dep_tree(node, 1);
                }
            }
        }
    }

    pub fn issue_detail(&self, d: &IssueDetail) {
        if self.json {
            return self.ser(d);
        }
        let i = &d.issue;
        println!("{}  {}", i.key, i.title);
        let mut meta = vec![
            format!("status: {}", i.status.name),
            format!("priority: {}", i.priority),
        ];
        if let Some(a) = &i.assignee {
            meta.push(format!("assignee: {}", a.name));
        }
        if let Some(p) = &i.project {
            meta.push(format!("project: {p}"));
        }
        if let Some(p) = &i.parent {
            meta.push(format!("parent: {p}"));
        }
        if !i.labels.is_empty() {
            meta.push(format!("labels: {}", i.labels.join(", ")));
        }
        if let Some(w) = i.waiting_on {
            meta.push(format!("waiting on: {w}"));
        }
        if !i.wanted_executor.is_empty() {
            meta.push(format!("for: {}", i.wanted_executor));
        }
        println!(
            "{}  ·  version {}",
            meta.join("  ·  "),
            i.description_version
        );
        if !i.open_blockers.is_empty() {
            let keys: Vec<String> = i.open_blockers.iter().map(|b| b.key.to_string()).collect();
            println!("depth {}  ·  waits for {}", i.depth, keys.join(", "));
        }
        if let Some(c) = &i.claim {
            println!(
                "claimed by {} in session {}{}",
                c.actor.name,
                c.session_id,
                c.branch
                    .as_ref()
                    .map(|b| format!(" on {b}"))
                    .unwrap_or_default()
            );
        }
        if let Some(a) = &i.analyzing {
            println!(
                "analyzed by {} in session {} until {}",
                a.actor.name, a.session_id, a.expires_at
            );
        }
        if let Some(desc) = &i.description
            && !desc.trim().is_empty()
        {
            println!("\n{}", desc.trim_end());
        }
        if !d.children.is_empty() {
            println!("\nSub-issues:");
            for c in &d.children {
                println!("  {}  [{}]  {}", c.key, c.status.name, c.title);
            }
        }
        if !d.relations.is_empty() {
            println!("\nRelations:");
            for r in &d.relations {
                println!(
                    "  {} {}  [{}]  {}",
                    r.direction, r.issue.key, r.issue.status.name, r.issue.title
                );
            }
        }
        let g = &d.git;
        if g.branch.is_some() || g.worktree.is_some() || g.pr.is_some() || !g.commits.is_empty() {
            println!("\nGit:");
            if let Some(b) = &g.branch {
                println!("  branch: {b}");
            }
            if let Some(w) = &g.worktree {
                match &g.host {
                    Some(h) => println!("  worktree: {w} (on {h})"),
                    None => println!("  worktree: {w}"),
                }
            }
            if let Some(pr) = &g.pr {
                let state = pr
                    .state
                    .as_deref()
                    .map(|s| format!(" [{s}]"))
                    .unwrap_or_default();
                println!("  pr: {}{state}", pr.url);
            }
            for c in &g.commits {
                println!("  {c}");
            }
        }
        if !d.attachments.is_empty() {
            println!("\nAttachments:");
            for a in &d.attachments {
                println!(
                    "  {}  {}",
                    a.kind,
                    a.title
                        .as_deref()
                        .map(|t| format!("{t} — {}", a.url))
                        .unwrap_or_else(|| a.url.clone())
                );
            }
        }
        if !d.comments.is_empty() {
            println!("\nComments:");
            for c in &d.comments {
                println!(
                    "  {} ({}):\n    {}",
                    c.author.as_ref().map(|a| a.name.as_str()).unwrap_or("?"),
                    c.created_at,
                    c.body.trim().replace('\n', "\n    ")
                );
            }
        }
        if !d.sessions.is_empty() {
            println!("\nWork log:");
            for s in &d.sessions {
                println!(
                    "  {}  {}  {}{}",
                    s.session_id,
                    s.actor.name,
                    s.last_seen_at,
                    s.branch
                        .as_ref()
                        .map(|b| format!("  {b}"))
                        .unwrap_or_default()
                );
                if s.usage.total_tokens > 0 {
                    println!("    usage: {}", crate::commands::usage::fmt_usage(&s.usage));
                }
                if let Some(t) = s.active_seconds {
                    println!("    active time: {}", format_active_time(t));
                }
                for c in &s.commits {
                    println!("    {c}");
                }
                for n in &s.notes {
                    println!("    note: {}", n.body.trim());
                }
            }
        }
        if d.usage.total_tokens > 0 {
            println!("\nUsage: {}", crate::commands::usage::fmt_usage(&d.usage));
        }
        if let Some(t) = d.issue.active_seconds {
            println!("Active time: {}", format_active_time(t));
        }
        let minutes = d.issue.estimate_minutes.map(|m| format!("{m} minutes"));
        let cost = d.issue.estimate_cost_usd.map(|c| format!("${c:.2}"));
        println!(
            "Estimate: {} · {}",
            minutes.as_deref().unwrap_or("no time"),
            cost.as_deref().unwrap_or("no cost")
        );
    }

    fn filter(&self, v: Value) -> Value {
        let Some(fields) = &self.fields else { return v };
        match v {
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|i| pick(i, fields)).collect())
            }
            Value::Object(ref o) if o.contains_key("items") && o.contains_key("next_cursor") => {
                let mut o = o.clone();
                if let Some(Value::Array(items)) = o.remove("items") {
                    o.insert(
                        "items".into(),
                        Value::Array(items.into_iter().map(|i| pick(i, fields)).collect()),
                    );
                }
                Value::Object(o)
            }
            other => pick(other, fields),
        }
    }
}

/// Keeps only the named fields. `status.name` keeps a nested field.
pub fn pick(v: Value, fields: &[String]) -> Value {
    let Value::Object(src) = v else { return v };
    let mut out = Map::new();
    for f in fields {
        let mut parts = f.split('.');
        let first = parts.next().unwrap_or_default();
        let rest: Vec<String> = parts.map(str::to_string).collect();
        if let Some(val) = src.get(first) {
            if rest.is_empty() {
                out.insert(first.to_string(), val.clone());
            } else {
                let nested = pick(val.clone(), &[rest.join(".")]);
                match out.get_mut(first) {
                    Some(Value::Object(existing)) => {
                        if let Value::Object(n) = nested {
                            existing.extend(n);
                        }
                    }
                    _ => {
                        out.insert(first.to_string(), nested);
                    }
                }
            }
        }
    }
    Value::Object(out)
}

pub fn table<const N: usize>(headers: &[&str; N], rows: &[[String; N]]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == N {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<w$}  ", w = widths[i]));
            }
        }
        println!("{}", s.trim_end());
    };
    line(headers.to_vec());
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_nested_fields() {
        let v = json!({ "key": "ALX-1", "title": "t", "status": { "name": "Todo", "type": "unstarted" } });
        let f: Vec<String> = ["key", "status.name"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        assert_eq!(
            pick(v, &f),
            json!({ "key": "ALX-1", "status": { "name": "Todo" } })
        );
    }
}
