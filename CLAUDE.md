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

`core`, `api-types`, `client`, `server` and `cli` are built and tested. `ui` and `app` are the GPUI app. It reads and writes through the client crate; `crates/ui/src/model/backend.rs` runs the client on its own tokio thread. App tests run with `cargo test -p llmtodo-ui`. The server tests also run on a shared Postgres database (LLM-450): `docker compose up -d --wait postgres`, then `LLMTODO_TEST_POSTGRES_URL=postgres://llmtodo:llmtodo@127.0.0.1:54329/llmtodo cargo test -p llmtodo-server`. The same variable runs `cargo test -p llmtodo-cli --test db_migrate_restart`. `scripts/check` runs every check, both backends included. The repo has no GitHub Actions CI (LLM-624), so run it before you request a review. Any schema change needs a migration in `crates/server/migrations` and in `crates/server/migrations-postgres`. `SPIKE.md` records what the spike proved and its open risks.

## Issue tracking

This repo tracks its own build in llmtodo. Load the `llmtodo` skill in `.claude/skills/llmtodo` before you touch a ticket. The map issue is LLM-1. Run `scripts/gen-agent-docs` after any CLI change, so `docs/agents/cli-reference.md` stays current. After a round of work lands on `main`, run `scripts/publish` from the main checkout. The issue tracker doc explains it.

## Build and review

Claude Code builds. agy reviews. Every ticket that ends in a commit gets at least one agy review before the owner sees it.

1. **Build.** Claim the ticket and work on a branch named after it, such as `llm-589-review-queue`. When the checks pass, commit and push the branch.
2. **Request a review.** Move the ticket to In Review: `llmtodo issue update <KEY> --status "In Review"`. That ends your claim and makes it wait on the reviewer. Then run `scripts/request-review <KEY>` (base `main`, or pass a base ref). It runs agy headless with the `llmtodo-review` skill. When agy cannot review (it hits its quota, is missing, or writes no answer), the script runs Claude Code headless with the same skill instead. Do not wait for agy's quota to reset. The reviewer answers with `llmtodo issue review answer`: numbered findings marked `blocking`, `should-fix` or `nit`. The script prints the answer and exits 0 on pass, 3 on changes requested, 1 when no reviewer wrote an answer.
3. **Pass** moves the ticket to User review for the owner, with agy's notes kept. Finish as the llmtodo skill says.
4. **Changes requested** moves the ticket back to In Progress. Claim it, fix every blocking and should-fix finding or decline it with a reason, and commit. Add one line per finding to the Agent notes under `Round N (agy, changes requested)`: `1. fixed in <sha>`, `2. declined: <why>`, `3. skipped (nit)`. Then go back to step 2.
5. Stop after round 3 and ask the owner with `llmtodo issue ask`.

agy reviews a throwaway clone (no hardlinks, no remote, build cache copied in) with a clean environment, so its commands cannot change this checkout. Its permission rules are not the guard; the clone is, and it does not jail agy, which still runs as the owner. llmtodo does not detect agy by itself, so the script sets `LLMTODO_ACTOR=agent:antigravity/<model>/<effort>`. Without it agy would act as the owner, who may approve. `AGY_MODEL` picks the model, default `gemini-4-neon-high`. The Claude reviewer gets the same clone and clean environment, the `dontAsk` permission mode with only read, git, cargo and `llmtodo issue view`/`review show` allowed, and only project settings. It returns its verdict as structured output, and the script posts the answer as `agent:claude-code/<model>/<effort>`. `CLAUDE_REVIEW_MODEL` and `CLAUDE_REVIEW_EFFORT` pick it, default `opus` and `high`. `REVIEWER=agy` or `REVIEWER=claude` forces one reviewer with no fallback.

The reviewer never edits code, commits, branches or claims. A builder never edits a reviewer's answer.

## Worktrees and data

A new git worktree starts without a `target` folder and compiles every dependency again. Several at once overload the machine. Before the first build in a worktree, copy the build cache from the main checkout: `cp -cRp <main checkout>/target <worktree>/target`. The copy takes under a minute and uses almost no disk, because APFS shares the file blocks.

The owner's real data lives in `~/Library/Application Support/llmtodo`. Tests and previews must use `LLMTODO_DATA_DIR` with a temporary folder.

## Reference repos

Shallow clones live outside this repo, in `~/code/oss/`. Read them before you write GPUI or UI code. They show working solutions for most app problems, and GPUI has no stable docs.

- `~/code/oss/zed`: Zed. GPUI lives in `crates/gpui`, with runnable examples in `crates/gpui/examples`.
- `~/code/oss/gpui-component`: GPUI Kit, which the app is built on. It already bundles the Lucide icons in `crates/assets`. Usage examples are in `crates/story`.

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
- `.claude/skills/llmtodo-review` is the reviewer's procedure for agy (see "Build and review").
- `zed/.agents/skills/gpui-test` explains `#[gpui::test]`, seeds and flaky test replay. Read it in place when you write app tests. It is not copied, because Zed's license for repo-level files is unclear.
- `zed/.agents/skills/gpui-bench` covers GPUI benchmarks and UI hangs. Read it in place when the app is slow.

### Keep the clones in step

GPUI Kit is pinned in `crates/ui/Cargo.toml`, `crates/app/Cargo.toml` and `crates/ui-bench/Cargo.toml` at gpui-component `1029e2c`. It takes GPUI from crates.io as `gpui-pre =0.3.12`, which is zed `d2a4fb6`. Do not add a separate GPUI dependency: it would compile a second, mismatched copy. The zed clone sits at `d2a4fb6` and the gpui-component clone at `1029e2c`. On every upgrade, check out the matching revisions in both clones and recopy the GPUI Kit skills, so the code you read matches the code we compile:

```sh
git -C ~/code/oss/zed fetch --depth 1 origin <rev> && git -C ~/code/oss/zed checkout <rev>
```
