# Architecture

One local server owns the SQLite file, and every client reaches it over HTTP and WebSocket. This page states which crate and which server layer may depend on which. `SPEC.md` holds the product scope and the decisions.

Each rule below either holds today or is a target. I checked every rule against the `Cargo.toml` files and the code at commit `cbfb271`.

## Crates

The workspace has eight crates. The last column lists the only workspace crates that each one may use.

| Crate | Job | May use |
|---|---|---|
| `core` | Domain types, ID types, issue keys, Markdown helpers, owner question parsing, the installed app bundle's path and commit, the standard data folder's path | nothing |
| `api-types` | Request and response types, the route table, error codes and exit codes | `core` |
| `client` | Typed HTTP client. It starts the server when none is running. It holds what every client shares: the live event stream, the data folder choice and the caller's identity | `core`, `api-types` |
| `server` | axum API, SQLite through sqlx, GitHub sync, Linear import, laptop sync | `core`, `api-types` |
| `cli` | The `llmtodo` binary | `core`, `api-types`, `client`, `server` |
| `ui` | The GPUI desktop app's views and model, as a library | `core`, `api-types`, `client` |
| `app` | The `llmtodo-app` binary: `main.rs` and start-up | `ui` |
| `ui-bench` | GPU frame benchmarks for `ui`, through headless Metal | `ui`, `core` |

`client` and `ui` also list `server` as a dev-dependency. Their tests start a real server inside the test process.

## Rules that hold today

Cargo enforces the table above, because a crate cannot import a dependency it does not declare. These rules follow from it or come from the code:

- `cli` uses `server` for one thing: to run `llmtodo server run`.
- Only `server` uses sqlx. No client opens the database file.
- Only `ui`, `app` and `ui-bench` use GPUI.
- `ui-bench` never turns on GPUI's `test-support` feature, because GPUI's benchmarks refuse it. `ui` turns it on only for its own tests, so the benchmarks need their own package.
- Each endpoint is a type in `api_types::ep` that implements `Endpoint`. It names the method, the path, the request and response types, and whether the route writes. The route table is generated from these types.
- The server mounts each handler by its endpoint type, and refuses to start when its handlers and the route table disagree. Nothing yet checks that a handler takes the endpoint's request type.
- The client sends every request through one generic `call` that takes an endpoint type. It spells no paths, and the route table tells it which requests write and carry usage.
- A CLI test runs the command named for each route and checks that the command calls that route. The Linear import route is exempt, because it needs a Linear key.
- `ErrorCode` in `api-types` is the one list of error codes and exit codes.
- `client::live` holds the one copy of the live protocol: the hello, the catch-up, the ping and the reload. The app's API thread, `activity --follow` and `review wait` use it.
- `client::data_dir::resolve` is the one place that picks a data folder, and the CLI and the app both call it. The folder is an input everywhere after that, so one process can open several. `core` names the standard folder's path, so the server and the app's tracker list can mark it.
- `client::identity` finds the actor, the session, git details and the agent's usage. Each harness, such as Claude Code, implements the `Harness` trait. The CLI adds only the repo's `.llmtodo.toml`.
- An update field that can be cleared is a `Change<T>`: Keep, Clear or Set. On the wire, a missing field or `null` keeps the value and an empty string clears it.
- Activity events name their entity type and field with the `EntityType` and `ActivityField` enums in `api-types`. An unknown name reads as `Other`, so the wire and the database keep plain strings.
- Each kind of record has its own ID type in `core`, such as `IssueId` and `DocumentId`. The API types and the app use them. The server's database rows still hold IDs as strings.
- Writes to issues and to the catalog go through `Write` in `server/src/state.rs`. `Write` is one transaction, and it records activity, the work log, token usage and live events.
- Every issue description write also rewrites the issue's rows in the derived `questions` table, in the same transaction. `ops/questions.rs` owns that step.
- Three kinds of write skip `Write`: creating actors, the file list of the import, and the bookkeeping of laptop sync.
- Outside services sit behind the `Fetch` and `GithubApi` traits. Tests use recorded responses through them.
- `IssueFilter` in `core` is the one issue filter. `store/filter.rs` turns it into SQL, and the app calls `IssueFilter::matches`. A server test checks that both keep the same issues.

## Three server layers

The server has three layers. Calls go in one direction: `routes` calls `ops`, and `ops` calls `store`.

| Layer | Job |
|---|---|
| `routes` | Parses the request, calls one operation and builds the response. It holds no SQL |
| `ops` | Holds every product rule. One module per domain noun. Each write takes `&mut Write`, and a read with a rule takes a connection |
| `store` | Holds SQL only and returns rows |

A read with no rule may call `store` from `routes` directly, to build a response. `github.rs` and `import.rs` are background jobs at the `ops` level: they call `ops` and `store`, and never `routes`.

These rules hold, and `crates/server/tests/layers.rs` fails when one breaks:

- `routes` holds no SQL.
- Nothing outside `routes` imports `routes`. `lib.rs` mounts the router, which is the one exception.
- Modules in `routes` do not import each other. They share only the helpers in `routes/mod.rs`.

A domain noun has the same module name in every layer: `issues`, `claims`, `relations`, `questions`, `documents`, `views`, `sessions`, `sync` and the others. A few modules hold a pair of nouns, such as statuses inside `teams` and milestones inside `projects`.

We will not add repository traits. SQLite is a settled decision, so a layer over it would add cost and no benefit.

The other findings of LLM-122 add more targets, and this page gains each one when its work lands.

## The app's layers

The `ui` crate groups its files into feature folders: `shell`, `issues`, `task`, `inbox`, `docs` and `settings`. A seventh folder, `model`, holds the state with no GPUI in it. These rules hold today, and tests check the two that name an import:

- `model` imports no GPUI and no module outside `model`, so it can move into a library crate. It takes `SharedString` from `gpui-pre-shared-string`, GPUI's own small string crate, which does not depend on GPUI.
- `model` holds the issue store, the write pipeline, the `$EDITOR` sessions and the API thread. Tests run the write pipeline with no window.
- The store holds every issue row, but a description only once a fetch brought it. The inbox reads the store's question list, which comes from the server.
- Views never name `Workspace`. They hold a `ViewEvents` handle and emit a `ViewEvent`. The workspace runs each event in its one update pass.
- `Workspace` owns the write pipeline, the `$EDITOR` sessions and the layout as plain structs, not as GPUI entities. The save logic needs one update order.

## What checks these rules

CI runs `cargo fmt --check`, clippy with warnings as errors, and the tests, on macOS. It runs on every push to `main` and on every pull request.
