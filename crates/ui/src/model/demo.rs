//! Demo issues for the UI tests. The app itself reads issues from the API;
//! tests that need no server use these instead.

use crate::model::data::{
    Agent, Analyzing, Assignee, Catalog, Claim, Date, Issue, IssueId, Label, LabelColor, Place,
    Status, StatusType, WaitingOn,
};

/// One team, LLM, with the default statuses, and six colored labels.
pub fn catalog() -> Catalog {
    let statuses = [
        ("Backlog", StatusType::Backlog),
        ("Todo", StatusType::Unstarted),
        ("In Progress", StatusType::Started),
        ("In Review", StatusType::Started),
        ("Done", StatusType::Completed),
        ("Canceled", StatusType::Canceled),
        ("Duplicate", StatusType::Canceled),
    ];
    let mut team: Vec<Status> = statuses
        .iter()
        .enumerate()
        .map(|(ix, (name, kind))| {
            Status::new(*name, *kind, ix as i64)
                .with_review(llmtodo_core::is_default_review_status(name))
        })
        .collect();
    team.sort_by_key(Status::rank);
    let labels = [
        ("agent", LabelColor::Cyan),
        ("bug", LabelColor::Red),
        ("docs", LabelColor::Green),
        ("feature", LabelColor::Blue),
        ("infra", LabelColor::Yellow),
        ("ux", LabelColor::Magenta),
    ]
    .into_iter()
    .map(|(name, color)| Label {
        name: name.into(),
        color,
    })
    .collect();
    let place = |key: &str, name: &str| Place {
        key: key.to_string().into(),
        name: name.to_string().into(),
    };
    Catalog {
        teams: vec![("LLM".into(), team)],
        team_names: vec![place("LLM", "llmtodo")],
        projects: vec![place("app", "App"), place("server", "Server")],
        project_teams: [("app", "LLM"), ("server", "LLM")]
            .into_iter()
            .map(|(p, t)| (p.into(), vec![t.into()]))
            .collect(),
        project_repos: vec![("app".into(), "https://github.com/xavortm/llmtodo".into())],
        views: vec![place("Open bugs", "Open bugs")],
        documents: vec![crate::model::data::DocEntry {
            id: llmtodo_core::DocumentId::from_u128(901),
            title: "Release checklist".into(),
            project: "app".into(),
            version: 2,
        }],
        labels,
        ..Default::default()
    }
}

fn status(catalog: &Catalog, name: &str) -> Status {
    catalog.find_status("LLM", name).expect("demo status")
}

pub fn claude() -> Agent {
    Agent {
        name: "claude-code/opus-5.5/high".into(),
        harness: Some("claude-code".into()),
        model: Some("opus-5.5".into()),
        effort: Some("high".into()),
    }
}

fn local() -> Agent {
    Agent {
        name: "ollama/qwen3-coder".into(),
        harness: Some("ollama".into()),
        model: Some("qwen3-coder".into()),
        effort: None,
    }
}

/// A claim like the server sends for an agent at work.
pub fn claim(agent: Agent, key: &str) -> Claim {
    Claim {
        actor: Assignee::Agent(agent),
        session: format!("session-{key}").into(),
        worktree: Some(format!("/work/llmtodo-{}", key.to_lowercase()).into()),
        branch: Some("main".into()),
        pane: Some("w1:p2".into()),
        // An hour and five minutes ago, so previews show a timer.
        claimed_at: jiff::Timestamp::now() - jiff::SignedDuration::from_mins(65),
    }
}

/// An Analyzing mark like the server sends: started 3 minutes ago, ends
/// in 12.
pub fn analyzing(agent: Agent, key: &str) -> Analyzing {
    let now = jiff::Timestamp::now();
    Analyzing {
        actor: Assignee::Agent(agent),
        session: format!("session-{key}").into(),
        started_at: now - jiff::SignedDuration::from_mins(3),
        expires_at: now + jiff::SignedDuration::from_mins(12),
    }
}

pub fn claude_claim(key: &str) -> Claim {
    claim(claude(), key)
}

const FOLDING_DESCRIPTION: &str = r#"## Goal

Make `+++` sections fold in the editor and collapse in the preview.

- Editor: raw Markdown with highlighting
- Preview: rendered blocks

+++ Agent notes

Plan:

1. Parse `+++ Title` and `+++` lines.
2. Feed the ranges to the editor as fold candidates.
3. Render each section as a collapsible block.

```sh
cargo test -p llmtodo-app
```

+++

## Acceptance

The owner can fold every section with the mouse.

+++ Work log

- 09:12 started the spike
- 10:40 folding works in the editor
- 11:05 preview blocks collapse

+++
"#;

const VIM_DESCRIPTION: &str = r#"## Why

The owner lives in nvim and herdr. Every action in the app must work from the keyboard, and the keys must feel like vim. A mouse is optional. If a key does something different from vim, the owner has to stop and think, and that breaks the flow we are building this app for.

## Scope

The list, the task view and the editor each have their own keys. The keys never fire while the owner types in a text field.

### List

- `j` / `k` move, `gg` / `G` jump to the top and bottom
- `Enter` opens the task, and a second `Enter` moves into it
- `/` filters, `:` opens the command palette, `?` shows every key
- `h` / `l` and `Ctrl-A h` / `Ctrl-A l` move between the list and the task

### Task view

- `j` / `k` scroll the description, `Ctrl-D` / `Ctrl-U` scroll half a page
- `gg` / `G` jump to the top and bottom of the description
- `gt` / `gT` switch tabs. Each `+++` section is a tab, next to the Description tab
- `e` opens the editor, `E` opens `$EDITOR`, and `Esc` goes back to the list

### Editor

Normal mode supports motions, inserts, deletes, yank and paste, undo and redo. Insert mode types text. `Esc` moves from insert to normal mode, and from normal mode it saves and returns to the preview.

## Out of scope for now

Counts such as `3j`, visual mode, the `.` repeat, operator pairs such as `cw`, search inside the description, macros and marks. These need a real operator and motion model, and the spike does not have one yet.

## Open questions

1. Should `Ctrl-A` stay the pane prefix? herdr also uses a prefix key, and the two could clash when the app runs inside herdr.
2. Should the task view remember its scroll position per task?
3. Should `q` close the task view, like a help buffer in nvim?

+++ Agent notes

Key contexts carry the modes. The workspace context has `mode=normal` or `mode=insert`. The task view adds a `DetailPane` context, and its bindings win over the list bindings because it is deeper. The editor adds a `VimEditor` context with `vim=normal` or `vim=insert`.

Checks before merge:

- [x] `j` / `k` never type in the filter
- [x] `j` / `k` scroll the task view
- [x] `Ctrl-A h` / `Ctrl-A l` move focus
- [ ] the owner tries it in a real window

+++

+++ Work log

- Nov 9 14:02 list keys done
- Nov 9 16:40 editor normal mode done
- Nov 10 09:15 owner feedback: `Ctrl-N` / `Ctrl-P`, first match selected, `Ctrl-C` cancels
- Nov 10 09:30 owner feedback: pane focus, task view scrolling, focus outline

+++
"#;

const IMPORT_DESCRIPTION: &str = r#"## Goal

Import every issue, comment, label and project from Linear once, then stop using Linear. The import must be safe to run twice. A second run updates what changed and never makes duplicates.

## Plan

1. Read the Linear API key from the environment. Never store it in the database.
2. Fetch teams, projects, labels and users first, because issues refer to them.
3. Fetch issues page by page, oldest first, with their comments and history.
4. Map Linear states onto our four statuses. Keep the original state name in the import log.
5. Write everything in one transaction per page, so a failed page leaves no half rows.

## Mapping

| Linear | llmtodo |
|---|---|
| Backlog, Triage | Backlog |
| Todo, Unstarted | Todo |
| In Progress, In Review | In Progress |
| Done, Canceled | Done |

## Risks

Linear rate limits the API. The importer must back off and resume from the last page it finished, or a large workspace will fail halfway. Attachments are links to Linear's file storage, and those links stop working once the workspace closes, so the importer should download each file.

Long descriptions also use Linear's `+++` collapsible sections. The importer must keep them as they are, because the app folds them in the editor and the preview.

+++ Agent notes

Use the GraphQL API with cursor pagination. Store the last cursor per entity type in an `import_state` table. Retry each request three times with a growing delay, then stop and print the cursor so the owner can resume.

```sql
CREATE TABLE import_state (
  entity TEXT PRIMARY KEY,
  cursor TEXT,
  finished_at TEXT
);
```

+++
"#;

/// An owner question with options, in the format of
/// `llmtodo_core::questions`.
pub const CHOICE_QUESTION: &str = "+++ Owner questions\n### Q1. Which palette commands come first?\nThe palette can only ship a few commands in the first round.\n\n- a) :status and :label\n- b) :assign and :move\n- c) All of them, one week later\n\n**Answer:**\n+++\n";

/// A free-text owner question.
pub const TEXT_QUESTION: &str = "+++ Owner questions\n### Q1. What should a saved view be called by default?\n\n**Answer:**\n+++\n";

/// LLM-85: every Markdown element that once ran past the task view's
/// right edge. `scripts/seed-demo` uses the same file.
pub const WRAP_DESCRIPTION: &str = include_str!("../../../../scripts/demo-content/wrap.md");

pub fn issues(catalog: &Catalog) -> Vec<Issue> {
    let titles: [(&str, &str, &[&str]); 20] = [
        (
            "Fold +++ sections in the editor",
            "In Progress",
            &["feature", "ux"],
        ),
        (
            "Vim keys in the issue list",
            "In Progress",
            &["feature", "ux"],
        ),
        ("Command palette with :status", "Todo", &["feature"]),
        ("Show claims in the sidebar", "Todo", &["agent", "ux"]),
        ("Import issues from Linear", "Backlog", &["infra"]),
        ("Trash view with restore", "Backlog", &["feature"]),
        (
            "Side by side merge on save conflict",
            "Backlog",
            &["feature", "agent"],
        ),
        ("Open description in $EDITOR", "Todo", &["feature"]),
        ("Status bar shows server port", "Done", &["ux"]),
        ("Dark theme by default", "Done", &["ux"]),
        ("Saved views for filters", "Backlog", &["feature"]),
        ("Bulk select with x", "Todo", &["feature"]),
        ("Project documents panel", "Backlog", &["docs"]),
        ("Long text wraps in the task view", "Todo", &["ux"]),
        ("Monospace font for issue keys", "Done", &["ux"]),
        (
            "Start server when it is not running",
            "Todo",
            &["infra", "bug"],
        ),
        ("Light theme", "Backlog", &["ux"]),
        ("Icons per priority level", "Todo", &["ux"]),
        ("Crash when the filter matches nothing", "Done", &["bug"]),
        ("Create issue with C", "Todo", &[]),
    ];
    titles
        .iter()
        .enumerate()
        .map(|(ix, (title, status_name, labels))| {
            let key = format!("LLM-{}", 101 + ix);
            let mut labels: Vec<Label> = labels.iter().map(|name| catalog.label(name)).collect();
            catalog.sort_labels(&mut labels);
            Issue {
                id: IssueId::from_u128(101 + ix as u128),
                team: "LLM".into(),
                // Even rows belong to App, odd rows to Server, and every
                // fifth row to no project.
                project: match ix {
                    _ if ix % 5 == 4 => None,
                    _ if ix % 2 == 0 => Some("app".into()),
                    _ => Some("server".into()),
                },
                title: (*title).into(),
                status: status(catalog, status_name),
                assignee: match ix % 4 {
                    0 => Some(Assignee::Person(crate::model::data::OWNER_NAME.into())),
                    1 => Some(Assignee::Agent(claude())),
                    2 => Some(Assignee::Agent(local())),
                    _ => None,
                },
                labels,
                deleted: false,
                version: 1,
                // LLM-104 waits for LLM-101 through one more task.
                depth: if ix == 3 { 2 } else { 0 },
                open_blockers: if ix == 3 {
                    vec![crate::model::data::Blocker {
                        key: "LLM-101".into(),
                        title: titles[0].0.into(),
                    }]
                } else {
                    Vec::new()
                },
                // Only tasks with usage have a cost.
                cost: match ix {
                    0 => Some(4.27),
                    1 => Some(0.86),
                    2 => Some(0.004),
                    _ => None,
                },
                active_seconds: match ix {
                    0 => Some(2_940),
                    1 => Some(480),
                    2 => Some(35),
                    _ => None,
                },
                estimate_minutes: (ix < 3).then_some(60),
                estimate_cost: (ix < 3).then_some(2.5),
                claim: match ix {
                    1 => Some(claim(claude(), &key)),
                    2 => Some(claim(local(), &key)),
                    _ => None,
                },
                analyzing: match ix {
                    4 => Some(analyzing(claude(), &key)),
                    _ => None,
                },
                // The same tasks as in `scripts/seed-demo`: LLM-102 and
                // LLM-104 wait on an agent, LLM-103 and LLM-111 on the owner.
                waiting_on: match ix {
                    1 | 3 => Some(WaitingOn::Agent),
                    2 | 10 => Some(WaitingOn::Owner),
                    _ => None,
                },
                closed_at: None,
                // Spread over the last weeks, newest first.
                added: if ix < 10 {
                    Date {
                        year: 2026,
                        month: 11,
                        day: 10 - ix as i8,
                    }
                } else {
                    Date {
                        year: 2026,
                        month: 10,
                        day: 41 - ix as i8,
                    }
                },
                description: match ix {
                    0 => FOLDING_DESCRIPTION.into(),
                    1 => VIM_DESCRIPTION.into(),
                    4 => IMPORT_DESCRIPTION.into(),
                    // The two tasks that wait on the owner ask one question
                    // of each kind, as `scripts/seed-demo` does.
                    2 => format!(
                        "{title}.\n\n+++ Agent notes\n\nNothing yet for **{key}**.\n\n+++\n\n{CHOICE_QUESTION}"
                    )
                    .into(),
                    10 => format!(
                        "{title}.\n\n+++ Agent notes\n\nNothing yet for **{key}**.\n\n+++\n\n{TEXT_QUESTION}"
                    )
                    .into(),
                    13 => WRAP_DESCRIPTION.into(),
                    _ => format!(
                        "{title}.\n\n+++ Agent notes\n\nNothing yet for **{key}**.\n\n+++\n"
                    )
                    .into(),
                },
                description_loaded: true,
                open_questions: 0,
                key: key.into(),
            }
        })
        .map(|mut issue: Issue| {
            issue.open_questions = llmtodo_core::open_questions(&issue.description).len() as u32;
            issue
        })
        .collect()
}
