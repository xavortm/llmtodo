# llmtodo

A Linear-style task tracker: a GPUI desktop app for people, and a CLI with full feature coverage for LLM agents. Both talk to one local API server backed by a SQLite file.

`SPEC.md` holds the scope, the stack and the decisions so far. Read it first. `ARCHITECTURE.md` states which crate and server layer may depend on which.

## Crates

- `crates/core`: domain types and Markdown helpers
- `crates/api-types`: request and response types shared by server and clients
- `crates/client`: typed HTTP client for the CLI and the app
- `crates/server`: axum + sqlx API server
- `crates/cli`: the `llmtodo` binary
- `crates/ui`: the GPUI desktop app's views and model, as a library
- `crates/app`: the `llmtodo-app` binary. It holds `main.rs` and start-up only
- `crates/ui-bench`: GPU frame benchmarks for `ui`, in their own package

`core`, `api-types`, `client`, `server` and `cli` are built and tested. `ui` and `app` are the GPUI app. It reads and writes through the client crate; `crates/ui/src/model/backend.rs` runs the client on its own tokio thread. App tests run with `cargo test -p llmtodo-ui`. `SPIKE.md` records what the spike proved and its open risks.

## Issue tracking

This repo tracks its own build in llmtodo. Load the `llmtodo` skill in `.claude/skills/llmtodo` before you touch a ticket. The map issue is LLM-1. Run `scripts/gen-agent-docs` after any CLI change, so `docs/agents/cli-reference.md` stays current. After a round of work lands on `main`, run `scripts/publish` from the main checkout. The issue tracker doc explains it.

A new git worktree starts without a `target` folder and compiles every dependency again. Several at once overload the machine. Before the first build in a worktree, copy the build cache from the main checkout: `cp -cRp <main checkout>/target <worktree>/target`. The copy takes under a minute and uses almost no disk, because APFS shares the file blocks.

The owner's real data lives in `~/Library/Application Support/llmtodo`. Tests and previews must use `LLMTODO_DATA_DIR` with a temporary folder.

## Reference repos

Shallow clones live outside this repo, in `~/Development/refs/`. Read them before you write GPUI or UI code. They show working solutions for most app problems, and GPUI has no stable docs.

- `~/Development/refs/zed`: Zed. GPUI lives in `crates/gpui`, with runnable examples in `crates/gpui/examples`.
- `~/Development/refs/gpui-component`: GPUI Kit, which the app is built on. It already bundles the Lucide icons in `crates/assets`. Usage examples are in `crates/story`.

Where to look:

| Problem | Look in |
|---|---|
| GPUI API: elements, layout, focus, actions, key contexts | `zed/crates/gpui`, `zed/crates/gpui/examples` |
| Lists, tables, inputs, dialogs, dock panels, Markdown | `gpui-component/crates/component`, `gpui-component/crates/story` |
| Icons | `gpui-component/crates/assets` |
| Zed look: panels, title bar, status bar | `zed/crates/ui`, `zed/crates/title_bar`, `zed/crates/workspace` |
| Theme colors and tokens | `zed/crates/theme`, `zed/assets/themes` |
| Vim modes and folding | `zed/crates/vim`, `zed/crates/editor` |

### Licenses: what may be copied

This repo is MIT, so the license decides what we can copy.

- **Copy freely, keep the notice:** `zed/crates/gpui` and all of GPUI Kit are Apache-2.0. Lucide icons are ISC. Add a header to copied files that names the source.
- **Read, do not copy:** most other Zed crates are GPL-3.0, including `ui`, `theme`, `vim`, `editor`, `workspace` and `title_bar`. Study how they solve a problem, then write our own code.
- **Theme files:** each theme in `zed/assets/themes` has its own `LICENSE` file. Check that file before you reuse colors from it.

### Skills

- `.claude/skills/gpui-kit` and `.claude/skills/gpui-kit-design-guides` are copied from GPUI Kit. Load them before any app work. The design guide applies on top of the Zed look from `SPEC.md`. `.claude/skills/NOTICE.md` records the source and license.
- `zed/.agents/skills/gpui-test` explains `#[gpui::test]`, seeds and flaky test replay. Read it in place when you write app tests. It is not copied, because Zed's license for repo-level files is unclear.
- `zed/.agents/skills/gpui-bench` covers GPUI benchmarks and UI hangs. Read it in place when the app is slow.

### Keep the clones in step

GPUI Kit is pinned in `crates/ui/Cargo.toml`, `crates/app/Cargo.toml` and `crates/ui-bench/Cargo.toml` at gpui-component `1029e2c`. It takes GPUI from crates.io as `gpui-pre =0.3.12`, which is zed `d2a4fb6`. Do not add a separate GPUI dependency: it would compile a second, mismatched copy. The zed clone sits at `d2a4fb6` and the gpui-component clone at `1029e2c`. On every upgrade, check out the matching revisions in both clones and recopy the GPUI Kit skills, so the code you read matches the code we compile:

```sh
git -C ~/Development/refs/zed fetch --depth 1 origin <rev> && git -C ~/Development/refs/zed checkout <rev>
```
