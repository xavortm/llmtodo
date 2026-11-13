# llmtodo — spec and stack

A Linear-style task tracker. People use a fast native app. LLM agents use a CLI that covers every feature.

Agents do about 99% of the reading and writing. The owner mostly reads headings and skims descriptions in the app, and directs the work. So the CLI and the API come first, and agents choose the technical details. The app is a fast, good-looking window onto the same data.

Status: approved by the owner on 2026-11-10, after a design review. A gap review on the same day added the sections marked "(gap review)" and decisions 7 to 16. The owner can veto any of them.

## What the two projects use from Linear today

I scanned two of the owner's projects. Both use one Linear workspace and the team ALX. I also counted field use on 250 issues from the two projects.

| Feature | How it is used | Use rate |
|---|---|---|
| Issue key `ALX-123` | Every doc, commit and PR refers to issues by key | all |
| Markdown description | The main record. Notes, PR links and triage blocks go here, not in comments | all |
| Collapsible sections `+++ Title` / `+++` | Agent notes at the end of a description | common |
| Statuses | Backlog, Todo, In Progress, In Review, Done, Canceled, Duplicate. Each has a type | all |
| Assignee | Set on started work, `@me` on claim | 80% |
| Parent and sub-issues | Wayfinder maps, sub-tasks | 44% |
| Relations | `related` (254) and `blocks` (42) | heavy |
| Attachments | GitHub PR links (212), commits, API links | 85% |
| Labels | Flat labels plus one group (`Area` → fe, be, design). Workflow states use waiting on, relations and statuses, not labels | all |
| Priority | Default sort order | 20% |
| Projects | One per product, with a state | all |
| Project documents | Long Markdown docs inside a project | one project |
| Milestones | Two exist in one project. No issue uses them yet | low |
| Comments | Only when the owner asks | 5 of 250 |
| Cycles, estimates, due dates | Not used | none |

### Agent workflows the CLI must support

- Read an issue with its comments and work log in one call.
- Query by project, status, label, assignee, parent, wanted executor, or text search. Output JSON.
- Create an issue from a description file, with a parent, labels and a project.
- Replace a description from a file, or patch one section or one exact string in it.
- Add or remove one label without touching the others.
- Claim an issue atomically: set the assignee and In Progress in one call, and fail if someone else holds it.
- Add relations: `blocks`, `blocked-by`, `related`, `duplicate`.
- Attach a PR link or a local path to an issue.
- Run the wayfinder "frontier" query. It lists the open sub-issues of a map issue that have no open blocker and no assignee.
- Search the backlog for duplicates before creating an issue.
- Find which tickets a session worked on, and which sessions worked on a ticket.
- Fail instead of prompting (`--no-input`), and skip confirmation on writes (`-y`).

## Scope

### v1

- macOS only. Paths, launchd and the app bundle assume a Mac.
- Teams with a key, projects, issues, sub-issues. One installation is one workspace, so the workspace exists only as settings.
- Statuses per team, each with a fixed type. Priority, assignee, labels with one level of groups.
- Actors: the human owner and agents, with harness, model and effort.
- Atomic claims, and a work log of agent sessions per ticket.
- Relations, link attachments, comments.
- Description revisions and an activity log.
- Project documents in Markdown.
- Milestones, because one of the projects already has them.
- Full-text search and duplicate search.
- GitHub sync by polling.
- Sharing the database between laptops by handoff.
- Native desktop app with vim keys, and the CLI.
- Import from Linear, so the existing ALX issues move over with their keys.
- A read-only file store for images copied from Linear during the import. New uploads stay in "Later".

### Later

- Web frontend (v2).
- A basic vim mode inside the built-in Markdown editor.
- An S3-compatible sync target, such as Cloudflare R2.
- Board (kanban) view.
- File uploads, such as screenshots.
- Cycles and due dates, if a project needs them.
- Comparing estimates with actual time and cost, as deltas and charts.
- MCP server, as a thin wrapper over the same API.
- Code signing and a Homebrew tap.
- Linux support.

## Architecture

```
 GPUI app ──┐
            ├──HTTP + WebSocket──> local API server ──> SQLite file
 CLI ───────┘   (127.0.0.1:7878)
```

Everything runs on the owner's Mac. All clients talk to one local API server. No client opens the database file directly. This keeps validation, issue keys, claims and live updates in one place.

### One Rust workspace

| Crate | Job |
|---|---|
| `core` | Domain types, Markdown helpers, `+++` section parsing, issue key parsing, owner question parsing, the shared issue filter |
| `api-types` | Request and response types shared by server and clients |
| `client` | Typed HTTP client used by the CLI and the app |
| `server` | axum + sqlx + SQLite |
| `cli` | clap. Binary name `llmtodo` |
| `ui` | The GPUI desktop app's views and model, as a library |
| `app` | The `llmtodo-app` binary: `main.rs` and start-up |
| `ui-bench` | GPU frame benchmarks for `ui`. See "Performance" |

Rust everywhere means one set of types. The CLI, app and server cannot drift apart.

`ARCHITECTURE.md` states which crate and server layer may depend on which.

The server is a library inside the `server` crate. The `llmtodo` binary runs it with `llmtodo server run`, so one installed binary holds both the CLI and the server. The `server` crate keeps a small `main.rs` for development.

### Dependencies (gap review)

| Need | Crate |
|---|---|
| Async runtime | tokio |
| HTTP server and WebSocket | axum |
| Database | sqlx with the SQLite driver |
| JSON schemas for the route table | schemars |
| HTTP client | reqwest |
| CLI parsing | clap with derive |
| IDs | uuid, version 7 |
| Time | jiff |
| Errors | thiserror in libraries, anyhow in binaries |
| Logging | tracing, with daily log files |

### Data model (gap review)

- **Team**: key such as `ALX`, name, statuses, and the next issue number.
- **Status**: belongs to a team. It has a name, a type, a color and a position.
- **Label** and **label group**: workspace-wide, so every team shares them.
- **Project**: workspace-wide, with a short slug, a state and links to one or more teams. Linked GitHub repos belong to a project.
- **Milestone**: belongs to one project.
- **Issue**: belongs to one team. It has an optional project, milestone and parent. It also has `waiting_on`, which is empty, `agent` or `owner`.
- **Relation**: a typed edge between two issues.
- **Attachment**: a link or a local path on an issue.
- **Comment**: Markdown on an issue. Comments can be edited and deleted, and edits keep revisions.
- **Document**: Markdown inside a project.
- **Actor**, **claim** and **work session**: see the section on actors below.
- **Saved view**: a named issue query. The app and `issue query --view <name>` both use it.
- **Revision**: a full copy of a description, document or comment body at one version.
- **Activity event**: one field change, with the actor, the session and the time.
- **Settings**: workspace name, default team, GitHub poll interval and similar values.

Every record has a UUIDv7 ID. Issues also have a key. Projects, teams and labels also accept their slug, key or name wherever the CLI takes an ID.

### Storage

- SQLite in WAL mode at `~/Library/Application Support/llmtodo/llmtodo.db`. Only the server opens it.
- The database file never lives in a repo. Each git worktree would get its own copy, and parallel agents would not see each other's claims.
- A claim runs in a `BEGIN IMMEDIATE` transaction, so two claimers cannot both win.
- Text search uses FTS5. It covers titles, descriptions, comments and documents. Duplicate search (`issue query --similar`) uses the FTS5 trigram tokenizer and returns matches by score.
- Migrations use `sqlx migrate`, embedded in the binary. Queries are runtime queries with `query_as` and `QueryBuilder`, not the checked `query!` macros. Most issue queries are built from filters at run time, and FTS5 tables confuse the macros. The server integration tests run every query instead. SQL text is built only from constants, through one audited `sql!` macro; values always go through `bind`.
- Imported files live in `files/` in the data folder, named by their content hash.
- Markdown is the record. The server stores a derived copy of any structure that must be queried, and writes it in the same transaction as the Markdown. Nothing edits the copy on its own. Owner questions are the first case.

### Backups and prune

- The server writes a copy of the database to `backups/` in the data folder every day and keeps the last 14. At startup it writes one when the newest copy is older than 24 hours, because the server may not run every day.
- It also writes a copy before every migration, every prune, every import and every `sync pull`.
- Deleted issues go to a trash. Their keys are never reused. Nothing is deleted on a timer.
- `llmtodo prune --before <date>` deletes trashed issues and all their revisions. It also removes their relations and attachments.
- `llmtodo prune --revisions-before <date>` trims old revisions of live issues and keeps the newest one.
- Both prune commands show what they will delete and ask for confirmation unless `-y` is set.
- Agents may do anything else, including delete and restore. Prune is the only permanent delete, so the server rejects it from an agent actor. Only the owner runs prune.
- Dates without a time mean midnight in the local time zone.

### Sharing between laptops

The local database is the primary copy. Only one machine is the primary at a time.

- `llmtodo sync push` writes a copy with `VACUUM INTO`, compresses it, and uploads it with a manifest. The manifest holds the owner host, schema version, a generation counter that goes up on every push, and a timestamp.
- `llmtodo sync pull` downloads the copy and makes this machine the primary. It refuses while another machine holds the lock, unless `--take` is set.
- Pull refuses a copy with a newer schema than the installed binary. An older copy migrates forward after the download.
- The v1 target is a folder path, such as Dropbox or iCloud. Only the copy goes there, never the live file. An S3-compatible target comes later.
- Paths in the work log that belong to another machine show with their hostname, marked as "on another machine".

Gap review additions:

- The lock is the owner host in the manifest. `llmtodo sync push --release` pushes and clears the owner, so another machine can pull without `--take`.
- A machine that is not the primary serves reads but rejects writes with the "not primary" exit code. The app shows a banner.
- Each database stores the generation it last pushed or pulled, plus a flag that says it has writes since then. Pull refuses to replace a database with unpushed writes unless `--force` is set. Push refuses when the remote generation is newer than the local one.
- Sync commands run inside the server. Pull stops writes, makes a backup, swaps the file and reopens the pool. Clients see a reconnect.
- Uploads write to a temporary name and rename at the end. The manifest goes last, so a half-synced folder never looks complete.
- The copy includes the `files/` folder.
- `llmtodo sync status` shows the local generation, the remote manifest and whether this machine has unpushed writes.

### Server

- axum, bound to `127.0.0.1:7878`. `llmtodo server install` writes a launchd file that starts it at login. The CLI starts the server if it is not running.
- REST with JSON. The route table in `api-types` lists every route with its JSON schemas. See decision 27.
- Live updates: the server sends WebSocket events right after each write it makes. This works because there is one server process.
- No tokens. Only the local machine can reach the server. Identity is an actor plus a session ID.

Gap review additions:

- **Browser protection.** A web page in the owner's browser can also reach `127.0.0.1`. The server rejects requests whose `Host` header is not `127.0.0.1` or `localhost`. It also rejects requests that carry an `Origin` header, until the web frontend exists.
- **One server at a time.** The server holds a lock file in the data folder. It writes `server.json` next to it with the port, PID and version. Clients read `server.json` to find the port.
- **Version check.** Every response carries the server version. When the CLI is newer than the running server, it asks the server to stop and starts its own binary. This covers the case where launchd still runs an old binary after an upgrade.
- **Paths.** All endpoints live under `/v1/`.
- **Errors.** Every error returns `{"error": {"code", "message", "details"}}`. The `code` is a stable string, and the CLI maps it to an exit code.
- **Lists.** List endpoints use cursor pagination with a `limit`. The CLI fetches all pages unless `--limit` is set.
- **Time.** The server stores and returns UTC timestamps in RFC 3339 format. Clients show local time.
- **Event catch-up.** Each activity event has a growing sequence number, and WebSocket events carry it. After a reconnect, a client asks for events after its last sequence number. When the gap is too old, the client reloads.

## Actors, claims and the work log

### Actors

An actor has a kind, `human` or `agent`. Agent actors also carry optional `harness`, `model` and `effort` fields, such as `claude-code / opus-5.5 / high` or `ollama / qwen3-coder`.

The CLI picks the actor in this order:

1. `LLMTODO_ACTOR`
2. `.llmtodo.toml`
3. Detection: `AI_AGENT` set means a Claude Code agent. Nothing set means the human owner.

A human actor in `.llmtodo.toml` applies only when no agent is detected. Every caller in the repo shares that file, so it must not turn an agent into the owner. Only `LLMTODO_ACTOR` overrides the detection.

The model comes from the session transcript, and `LLMTODO_MODEL` overrides it. Effort comes from `CLAUDE_EFFORT`.

The server creates an agent actor the first time it sees a new mix of harness, model and effort. `me` in the CLI means the current actor. `owner` always means the human owner.

### Assignee and wanted executor

- **assignee**: the actor who holds the issue now.
- **wanted executor**: an optional requirement, such as "any agent", "opus at high effort" or "a human".

The frontier query filters on the wanted executor, for example `--for model=opus-5.5`.

The wanted executor is a set of `key=value` filters on the actor fields `kind`, `harness`, `model` and `effort`. An empty set means anyone. An actor matches when every filter matches. `kind=agent` is the "any agent" case.

### Claims

- A claim belongs to an actor plus a session. The session ID comes from `LLMTODO_SESSION`, then `CLAUDE_CODE_SESSION_ID`, and the CLI generates one if neither is set.
- `issue claim ALX-12` sets the assignee and In Progress in one transaction. It fails with a stable exit code if another session holds the issue.
- Claims never end on their own. The frontier query and the app show each claim's age.
- `issue release` frees an issue. `issue claim --steal` takes it over and records who stole it.
- A claim stores the host, worktree path, branch and session ID. The CLI reads them from git and the environment, and flags can override them.

Gap review additions:

- "In Progress" means the team's first status of type started. Claims never depend on a status name.
- A claim by the same session again succeeds and changes nothing, so agents can retry safely.
- A claim fails on an issue that is completed or canceled.
- A claim fails on a task with no time estimate or no cost estimate, with exit code 6. Tasks already claimed are not affected. See decision 39.
- A claim succeeds on an issue with open blockers. The response lists those blockers, and the CLI prints a warning.
- A claim ends when the issue moves to a completed or canceled status. It also ends when someone sets a different assignee. The activity log records both cases.
- A generated session ID only lives for one CLI call. The CLI prints a warning on claim, because a later `release` cannot match it. Agents should set `LLMTODO_SESSION`.
- A claim copies its branch, worktree and host to the issue. The issue keeps them after the claim ends, in a git panel with the PR and the commits. Agents update them with `issue update --git-here`, `--branch`, `--worktree` and `--pr`.

### Analyzing (LLM-98)

An agent can mark an issue as Analyzing while it reads the issue or fills in its details without a claim. Examples: an agent writes tickets from the owner's chat, or researches before it plans.

- `issue analyze ALX-12` sets the mark. `issue analyze ALX-12 --done` ends it.
- The server keeps one mark per issue, in the `analyzing` table, with the actor, the session, the start time and the time of the session's last write to the issue. A mark from a new session replaces the old one.
- Analyzing never blocks. Other sessions can write, claim and finish the issue. The app keeps the task editable.
- Every write by the marking session to the issue moves the last-write time. The mark expires 15 minutes after that time, so a crashed agent cannot leave it on. Reads leave out an expired mark, and a later write does not bring it back.
- A claim ends the mark, because Locked replaces Analyzing. A move to a completed or canceled status ends it too, and so does a delete.
- `--done` from another session changes nothing. It does not take the mark away from its owner.
- Issues carry the mark as `analyzing`, with `expires_at`. The app compares `expires_at` with its clock, so the mark goes away without a server event.
- The app list shows both states in one LLM status column: Locked in the lock color, Analyzing in gray with an eye icon.

### Work log

Every write from an agent adds or updates a work-session record on the ticket, so agents never need to remember a flag. A record holds:

- harness and version, model, effort
- session ID and transcript path (`~/.claude/projects/<repo-slug>/<session-id>.jsonl` for Claude Code)
- herdr pane ID, host, worktree, branch
- start time and last-seen time
- commits made on that branch while the session ran

`llmtodo session note` adds free-form notes and links to a session, such as a plan file or a scratchpad path. `issue view` lists the sessions on a ticket. `llmtodo session show <id>` lists the tickets a session touched.

Gap review additions:

- The CLI sends the current `HEAD` commit with every agent write. The server stores the first and the latest `HEAD` per session and ticket. The commit list is the git range between them, read by the CLI from the local repo when `issue view` runs on the same host.
- The herdr pane ID comes from the herdr environment variables, when they are set.
- Reads do not touch the work log. Only writes update the last-seen time.

## Issues and descriptions

- Each team has its own statuses. Each status has a fixed type: backlog, unstarted, started, completed or canceled. A new team starts with Backlog, Todo, In Progress, In Review, Done, Canceled and Duplicate. "Open" means any type other than completed or canceled.
- Priority uses Linear's five levels: none, urgent, high, medium, low.
- An issue takes at most one label from each label group.
- Attachments are links (PRs, commits, URLs) and local file paths.
- The server stores every description and document revision, plus an activity log of field changes. `issue history` lists revisions. `issue restore --revision` brings one back.
- Writes that change a description or document carry the version they were based on. The server rejects a write based on an old version. Status, label and assignee changes skip this check.
- A description patch replaces a section, chosen by heading text or `+++` title, or replaces an exact string that must match once.

Gap review additions:

- A full description replace needs `--base-version`, or `--force` to skip the check. A patch does not need a version, because its section or exact string must still match. A patch that finds no match, or more than one, fails.
- `issue restore <key>` takes an issue out of the trash. `issue restore <key> --revision <n>` brings back an old description.
- Moving an issue to another team gives it a new key. The old key stays as an alias, so old links, commits and PR titles still resolve.
- The server rejects a parent change or a `blocks` relation that would create a cycle.
- `blocked-by` is stored as the reverse of `blocks`. `related` has no direction.
- Setting an issue to the Duplicate status needs a `duplicate` relation to the original. Adding a `duplicate` relation moves the issue to Duplicate.
- The default sort is priority, then the last update time. Issues with no priority come last.
- Issue keys inside Markdown show as links in the app.

## Waiting on and owner questions

An issue can wait on one side. `agent` means it is ready for an agent to pick up, and `owner` means it needs the owner's answer first. The frontier skips issues that wait on the owner. A move to a completed or canceled status clears the field.

Owner questions are how an agent asks the owner for a decision. The description is the record. The questions live as Markdown in a `+++ Owner questions` section, and `core` parses them. The server keeps a derived copy in the `questions` table, written in the same transaction as the description. Issues carry `open_questions`, and `GET /v1/questions` lists questions without loading any description. The table also records which revision answered a question and who wrote it, so undo reads no revisions.

- Each question starts with a `### Q<n>.` heading. `- a)` lines are options, and a question without options takes free text.
- The answer runs from `**Answer:**` to the next question. An empty answer means the question is open.
- `issue ask` adds a question and sets `waiting_on` to `owner`. `issue questions` lists them, and `issue answer` answers one.
- When the owner answers the last open question, `waiting_on` moves to `agent`, so the frontier offers the issue again. An update that sets `waiting_on` itself keeps its own value.
- `issue undo-answer` opens the question again and sets `waiting_on` back to `owner`. Only the actor who answered can undo. It fails when the issue is closed, when the question or answer changed, or when another actor wrote to the issue after the answer.

The inbox in the app lists every open question across issues. It reads them from `GET /v1/questions`, not from descriptions. A choice shows its options as buttons, and every question shows an input. The task view draws the same cards in its Owner questions tab.

## Review rounds (LLM-138)

A ticket with UI work goes to a review status when it lands. A review status is any status with the review flag, such as User review or In Review (LLM-151). The flag decides, never the name. The owner tries it in the app and answers there.

- **The steps and checks are Markdown**, in a `+++ Review` section with the headings `### Steps to reproduce` and `### What to check` (decision 38). `core` parses them.
- **A move to a review status needs both headings with content**, by any path: an update, a new issue, a release or the GitHub sync. The server refuses with exit code 6 and shows the format. The GitHub sync skips the move instead.
- **A move to a review status hands the ticket to the owner.** It ends the live claim with the reason `review`, so the app does not show the ticket as locked. It sets `waiting_on owner`, unless the same update sets it.
- **The answers are the owner's actions**, so they live in a `reviews` table with the issue, the round, the verdict, the feedback, the author and the time. Each answer writes an activity event with the field `review`.
- **Approve** with no open PR moves the ticket to the team's first completed status, which clears `waiting_on` and ends the claim. With an open PR the ticket stays in its review status, waits on an agent, and loses its claim and assignee. An agent merges the PR and marks it Done. The branch field does not count, because it stays set after work lands.
- **Request changes** needs feedback. The ticket moves to the team's first unstarted status, waits on an agent, and loses its claim and assignee, so the frontier offers it again.
- **One answer per round.** A round ends when the ticket enters a review status again.
- **The frontier skips a ticket in a review status**, unless it waits on an agent after an approval with an open PR. A claim on a ticket in a review status moves it to the first work status.
- **Migration 0012 sets the flag** on every status named In Review or User review, in every team. New teams get it on In Review, and a Linear import on either name. At the next start the server ends the live claims on issues in a review status, once, as the `migration` system actor, so the activity log records each one. Setting the flag on a status later ends the claims on its issues the same way.
- **`issue review wait`** blocks on the live events until the next answer, and prints it as JSON with its `seq`.
- **In the app**, the review panel sits at the top of the task view: the steps as a numbered list, the checks as a list, a feedback field, and the Approve and Request changes buttons. Earlier rounds show below it.

## Dependencies (LLM-99)

A dependency is the `blocks` relation. There is no new relation type. "A depends on B" means B blocks A.

- **Depth** is the longest chain of open blockers. Done, canceled and trashed tasks end the chain. A closed or trashed task has depth 0.
- **The server computes depth and sends it with every issue**, with the direct open blockers, so the CLI and the app agree.
- **One recursive query per list page.** It collects the blocker edges the page can reach, and visits each reachable issue once. The longest paths are computed in Rust, so a graph where chains share blockers stays linear. A recursive query that counted lengths in SQL walks every path, which grows exponentially on such graphs.
- **The queries use CROSS JOIN and a covering index.** CROSS JOIN keeps SQLite's join order. The index `relations_blocked_by` serves the walk up the chain. Without them, a page of 1,000 issues took 25 seconds on 50,000 issues.
- **Cycles are refused** when a relation is added, with exit code 6. The error names the cycle in its message and in `details.cycle`. The check follows closed and trashed tasks too, so reopening one cannot complete a cycle.
- **Live updates.** Closing a blocker changes the depth of tasks that get no event of their own. When the app hears a status, title, trash or relation event, it asks for the dependents with `issue query --depends-on` and applies only their depth and open blockers. Rows in the task view read status and title from the app's live copy.
- **`issue deps`** expands each task once per direction and stops at 40 levels. A node that stops early has `truncated: true`. This keeps the output small and within the JSON nesting limit of the client.
- **In the app**, the list has a narrow Dependencies column, so the icon and depth do not take room from the title at narrow widths. It hides after the labels, assignee and waiting columns. The palette commands are `:depends`, `:blocks`, `:undepend` and `:unblock`.

## Order (LLM-206)

An epic's sub-issues can have an order. The order is a list of steps. Each step holds one task, or a group of tasks that can be done in any order.

- **The step lives on the sub-issue.** `issues.order_step` is a number scoped to the parent, 1 first. The steps of one parent stay dense, 1..n, and the server renumbers them on each change. An index on `(parent_id, order_step)` serves the reads.
- **The order works as dependencies.** Each task of step N blocks each task of step N+1. These edges are real `blocks` relations with `source = order`. Every other relation has `source = manual`. The edges link neighbouring steps only, because the chain is transitive.
- **The server rewrites the order edges** in the same transaction as each order change: a new order, a step change, a parent change, and a trash or restore. It deletes the parent's order edges and writes them again. So the depth, open blockers, frontier, dependency tree, cycle check and claim warnings follow the order with no change to their queries, and the 50,000-issue plan of LLM-99 stays.
- **Cycles are refused.** A manual `blocks` edge from a later step to an earlier one would make a cycle. The order write checks each edge against the other `blocks` edges and refuses the whole write with exit code 6 and `details.reason = order_cycle`.
- **Manual edges win.** A manual relation on the same two tasks takes over the order edge, so it stays when the order changes. `relation remove` refuses an order edge with exit code 6, and its message points to `--order-step none` and `issue order set`.
- **Trashed tasks drop out of the order**, and a restored task comes back with no step. A new parent takes a task out of the old order. Closed tasks keep their step, so the line still shows done steps.
- **The API.** `Issue.order_step` is the step. Relations carry `source`. `GET /v1/issues/{key}/order` returns the steps, each with a `done` flag, and the live sub-issues with no step. `PUT /v1/issues/{key}/order` takes `steps: Vec<Vec<IssueKey>>`. Every key must be a live, direct sub-issue of the parent, or the write fails with exit code 6. Sub-issues left out lose their step. `UpdateIssue.order_step` moves one task to an existing step, a new last step, or out of the order.
- **Live updates.** One order change writes one activity event with the field `order` on the parent. It holds the old and new steps as keys. The order edges write no relation events.
- **The CLI.** `issue order set <PARENT> <step>...`, where a step is a key or a comma list, and bare numbers take the parent's team. `issue order show <PARENT>` and `issue order clear <PARENT>`. `issue update <KEY> --order-step <n|none>` moves one task.
- **Agents order an epic** with the "Order an epic" section of the llmtodo skill guide. The owner asks in chat, or sets `waiting_on agent` on the epic with the request.
- **In the app** (LLM-208, LLM-209), the task list groups by parent and draws the order as a vertical line with a dot per step. Palette commands change the order.

## GitHub sync

The server polls the GitHub API every 5 minutes while it runs, using the token from `gh auth token`. `llmtodo github sync` runs a poll on demand. `llmtodo project link-repo <owner/name>` links a repo to a project. After a gap, the server catches up on PRs updated since the last poll.

- A PR gets attached to an issue when its branch matches a stored claim branch, or when its branch name or title holds an issue key.
- When the PR opens or is ready for review, the issue moves to In Review.
- When the PR merges, the issue moves to Done.
- A closed, unmerged PR leaves the status alone and keeps the attachment.
- Commits that mention a key get attached as commit links.

The CLI keeps `llmtodo issue attach` for links the poll misses.

Gap review additions:

- Each team setting names the status for "PR open" and "PR merged". The defaults are In Review and Done. A team can turn either move off.
- The sync never moves an issue out of a completed or canceled status.
- When an issue has several PRs, it moves to Done when the first one merges, like Linear.
- A draft PR attaches but does not move the issue.
- Status moves from the sync appear in the activity log with the actor `github`.
- The poll uses conditional requests with ETags, so unchanged repos do not count against the rate limit. When the limit runs out, the poll waits until the reset time.
- A missing or expired `gh` token pauses the sync. The app and `llmtodo github sync` show why.

## CLI design

The CLI mirrors the `linear` CLI (schpet/linear-cli) that both projects use now.

- `llmtodo issue view|query|create|update|delete|claim|release|history|restore|frontier`
- `llmtodo issue comment add`, `issue relation add|remove`, `issue attach`
- `llmtodo issue ask|questions|answer|undo-answer`
- `llmtodo session show|note`
- `llmtodo label|status|project|milestone|doc|team|view ...`
- `llmtodo sync push|pull|status`, `llmtodo prune`, `llmtodo github sync`, `llmtodo server install|run|stop|status`
- `llmtodo import linear`
- `llmtodo api` sends a raw request, as an escape hatch

Rules for every command:

- Output is JSON when stdout is not a terminal, and a table when it is. `--json` and `--human` force either format.
- Writes return the changed object.
- `--description-file` and `--body-file` accept `-` for stdin.
- `--no-input` and `-y` behave as they do in the `linear` CLI.
- Exit codes are stable and documented. Errors print JSON when the output is JSON.
- `llmtodo schema` prints every command and flag as JSON. It reads the clap definitions, and adds request and response shapes from the route table.
- A test checks that every API route has a CLI command, using the route table.

Gap review additions:

- `--fields key,title,status` limits the JSON output to those fields, so agents spend fewer tokens.
- `issue frontier` takes a map issue, or `--project` for every open, unblocked, unassigned issue in a project. It includes direct sub-issues only, unless `--depth` is set. It sorts by priority, then by key.
- A blocker counts as open while its status is open. A blocker in the trash does not count.

### Exit codes (gap review)

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Unknown error |
| 2 | Bad flags or arguments |
| 3 | Not found |
| 4 | Claim held by another session |
| 5 | Stale version on a description or document write |
| 6 | Validation failed, such as a cycle or a second label from one group |
| 7 | Server unreachable and could not start |
| 8 | Input needed, but `--no-input` is set |
| 9 | This machine is not the primary |
| 10 | Patch matched zero times or more than once |

### Config

- Global config: `~/.config/llmtodo/config.toml`.
- Per-repo defaults (team, project, actor): `.llmtodo.toml`. The CLI looks for it in the current folder and every parent up to the git root.
- Data folder: `~/Library/Application Support/llmtodo/`, holding the database, backups, files, logs and `drafts.json`, the description text the server has not accepted yet.
- `LLMTODO_DATA_DIR` overrides the data folder. Tests and a second local install use it.
- The CLI and the app pick the data folder with one function, `llmtodo_client::data_dir::resolve`. The order is the `--data-dir` flag, then `LLMTODO_DATA_DIR`, then `data_dir` in the global config, then the standard folder.

### Agent instructions

The repo ships the `llmtodo` Claude Code skill in `.claude/skills/llmtodo`. It teaches agents the CLI: setup, claim, frontier, the work log and notes. The binary embeds the skill, and `llmtodo init` or `llmtodo skill install` copies it to `~/.claude/skills/llmtodo`, so every repo gets the same one. `llmtodo init` writes `.llmtodo.toml` in a new repo. `docs/agents/issue-tracker.md` keeps the steps for this repo only, and `docs/agents/cli-reference.md` is generated from `llmtodo schema` (LLM-136).

## Desktop app

GPUI with GPUI Kit (longbridge, formerly `gpui-component`). It has a Markdown viewer, a virtual list, a data table, a code editor, menus, dialogs and dock panels.

### Design language

Zed is the visual target. The app should feel like a Zed window that shows issues instead of code.

- Dense layout with docked panels, thin borders and no heavy shadows.
- A title bar, a sidebar panel, a main pane and a status bar, like Zed's workspace.
- Dark theme by default, with a light theme. Colors come from theme tokens, so a Zed theme file can map onto them later.
- Zed's text sizes and spacing. Monospace font for issue keys, branches and paths.
- Small info elements, such as hover cards, captions and badges, use a smaller font than body text, the same size as their label, and a line height near 1.3 (LLM-198).
- Icons come from Lucide, bundled as SVG files. Each status type, priority level and relation type gets one fixed icon.
- Keyboard first. Every action has a key or a palette command, and the mouse is optional.

### Views in v1

- Sidebar with teams and projects
- Issue list with filters and saved views
- Issue detail with a Markdown preview, the activity log and the work log
- Project documents
- Command palette
- Claims list: which agent holds which ticket, in which worktree and herdr pane
- Inbox: every open owner question, answered in place
- Trash, with restore

### Keys

- `c` creates an issue. The form has a title and a Markdown description. Linear's other shortcuts are left out.
- Vim keys handle navigation: `j`/`k` to move, `gg`/`G` for the top and bottom, `h`/`l` between panes, `Enter` to open, `Esc` to go back, `/` to filter.
- `:` opens the command palette, which takes commands such as `:status done` and `:assign me`.
- `x` selects issues for a bulk action.
- A right-click on any column heading opens a checklist of the columns. The choice is saved in the UI settings. The title column cannot be hidden. The width rule then hides columns from those that remain (LLM-121).
- In the palette, Enter runs a command that writes to a task only when its first word is typed in full, so `a` cannot run `assign me`. The first match stays selected, and ctrl-n or a click still picks any suggestion (LLM-120).

GPUI Kit's key contexts, such as `mode == normal`, carry the vim modes.

### Markdown editing

- Descriptions and docs open as a rendered preview. `+++` sections show as collapsible blocks.
- `e` opens the built-in editor with raw Markdown and syntax highlighting. It folds `+++` sections, like code folding in Zed. `Esc` or `Cmd+S` saves and goes back to the preview.
- `E` (`Shift+E`) opens the text in `$EDITOR`, such as nvim, in a new herdr pane or a terminal. The app reloads and saves when the file is written.
- When a save fails because an agent changed the text first, the app keeps the owner's text and shows both versions side by side. The owner picks one or merges by hand.
- The app starts the server if it is not running, the same way the CLI does.

### Previews (owner decision)

`scripts/preview` lets the owner try the app without risk to real data. By default it builds fresh demo data with `scripts/seed-demo`, which shows every feature and UI state; `real`, `empty` or a folder path pick other data. It puts the data in a fresh temporary folder, starts a server there on a free port, opens the app against it, and waits until the app exits. The owner then reports issues. `LLMTODO_DATA_DIR` and `server.json` make this work, so the port is not fixed at 7878. 7878 is only the default.

A preview can never write real data (LLM-137). It always opens on its temporary folder. The sidebar still lists the real tracker, so the owner can look up a real ticket. The real tracker is read-only whenever the app opened on another folder: the HTTP client refuses every write, and the app does not start the real server. To look up a real ticket, the real server must already run. Preview UI settings live in `~/Library/Application Support/llmtodo-preview`.

Risk: GPUI has no stable release, and its API changes with Zed. We pin a git revision and upgrade on purpose. Local reference clones of Zed and GPUI Kit, described in `CLAUDE.md`, stay at the pinned revision.

## Linear import and cutover

- `llmtodo import linear` reads the Linear GraphQL API with the owner's Linear token. It imports issues, comments, relations, attachments, labels, projects, documents and milestones.
- The import keeps `ALX-123` keys. The next new issue continues the numbering.
- The import is idempotent: running it again updates records by their Linear ID instead of creating duplicates.
- Cutover waits for the owner (decision 23). When it happens, it is one-shot. Import, check the result, and re-run as needed. Then stop writing to Linear and switch `docs/agents/issue-tracker.md` in both repos to the `llmtodo` CLI, with one commit per repo.

Gap review additions:

- The token comes from `LINEAR_API_KEY`. The owner sets it when the import is ready to test.
- `--dry-run` prints counts and every mapping problem without writing anything.
- Linear users become actors. The owner's Linear user maps to the owner. Other users become human actors with their Linear name.
- Linear statuses map by their type. Names stay as they are.
- Archived issues import into the trash. Linear's history of field changes is not imported. Each issue gets one activity event that says it came from Linear.
- Linear-hosted images in descriptions and comments need a Linear login and stop working after cutover. The import downloads them into `files/` and rewrites the links to the local server.
- The import waits and retries when it hits the Linear rate limit.

## Build order

1. `core`, `server` and `cli`, with tests.
2. Dogfooding: llmtodo tracks its own build with its own CLI. No other repo changes.
3. The Linear import, tested read-only against the real workspace. Imported data is test data.
4. The `app`, previewed by the owner on throwaway data.
5. Cutover of the other projects, only when the owner decides. See decision 23.

A GPUI spike runs in parallel with step 1. It proves the `+++` folding and the vim key contexts, because GPUI is the biggest risk.

GitHub sync and laptop sharing are not needed for the cutover. They can land after step 2, before or during step 3.

## Performance

The app targets 120 frames a second, so one frame has 8.3 ms. It must hold that with 50,000 issues in the list.

Two tools measure it:

- `cargo bench -p llmtodo-ui-bench` draws real frames through GPUI's headless Metal renderer. It shapes and rasterizes text, builds the scene and submits it to the GPU. The package is separate from `llmtodo-ui`, because GPUI's benchmarks refuse the `test-support` feature that the app's tests turn on. CI fails when the package pulls that feature in.
- The ignored `perf_tests` in `llmtodo-ui` time the main thread in a test window. They leave out rasterizing and presenting.

The benchmark app has no asset source, so icons draw nothing. The real frames cost a little more.

### Task list frame times (LLM-113)

I measured these on 2026-11-11 on an Apple Silicon MacBook Pro, at commit `e9d0c2d` plus the benchmark. The window is 1400 by 900 points. Three other agents built code at the same time, so the worst frames are noisy. Each row comes from 140 to 1,400 drawn frames.

| Benchmark | Issues | Time per step | Draw p50 | Draw p99 | Draw max |
|---|---|---|---|---|---|
| Scroll 40 points | 1,000 | 3.7 ms | 3.4 ms | 5.3 ms | 8.3 ms |
| Scroll 40 points | 10,000 | 4.0 ms | 3.7 ms | 5.8 ms | 22.9 ms |
| Scroll 40 points | 50,000 | 4.9 ms | 4.4 ms | 7.9 ms | 25.8 ms |
| Filter keystroke | 1,000 | 7.3 ms | 5.3 ms | 21.1 ms | 82.9 ms |
| Filter keystroke | 10,000 | 5.4 ms | 4.2 ms | 15.7 ms | 27.7 ms |
| Filter keystroke | 50,000 | 6.5 ms | 5.3 ms | 12.0 ms | 16.5 ms |
| First draw | 1,000 | 16.2 ms | 6.7 ms | 18.0 ms | 26.7 ms |
| First draw | 10,000 | 17.0 ms | 5.7 ms | 17.1 ms | 28.4 ms |
| First draw | 50,000 | 21.6 ms | 8.4 ms | 28.2 ms | 49.7 ms |

"Time per step" is Criterion's mean. For scroll and the filter it covers one input and the frame after it. For the first draw it covers every frame until the new window is clean, with a cold glyph atlas. "Draw" is one frame, from layout to present.

What the numbers say:

- Scrolling stays inside the frame budget at every size. The median frame grows by 1 ms from 1,000 to 50,000 issues.
- A filter keystroke on a small list builds the rows on the main thread, so 1,000 issues has the slowest worst frame. From 2,000 issues the rows build on a background task, and the keystroke frame stays near 5 ms.
- A new window draws about two to three frames before it is clean. At 50,000 issues the median frame is still inside the budget.

## Tests

- Server integration tests run against a temporary SQLite file.
- CLI tests run against a server started inside the test process.
- A concurrency test starts 20 claimers on one frontier at once and checks that each issue gets exactly one owner.
- The coverage test checks that every API endpoint has a CLI command.

Gap review additions:

- A sync test pushes from one data folder and pulls into another. It checks the lock, the unpushed-writes refusal and the schema check.
- The import runs against recorded Linear API responses, so tests need no network. A second run checks that nothing changes.
- A WebSocket test drops a client, makes writes, and checks that the client catches up from its last sequence number.
- Description patch tests cover headings, `+++` titles, exact strings, zero matches and two matches.
- The GitHub sync runs against recorded API responses.

## Decision log

1. **Single owner.** There is no sign-up, no roles and no sharing. Actors stay, so agents and the owner can hold tickets.
2. **Keep issue keys.** The Linear import keeps `ALX-123`, and keys are never reused.
3. **Local first.** SQLite on the owner's Mac. Postgres, Neon and Render are dropped. Remote access is a handoff, not a live connection.
4. **Markdown editing.** Preview by default, the built-in editor on `e`, and `$EDITOR` on `Shift+E`, which is `E`.
5. **GitHub sync in v1**, by polling, because the local server has no public URL.
6. **Vim keys** in the app instead of Linear's shortcuts, except `c`.

Gap review, 2026-11-10. These stand unless the owner vetoes them. The owner answered the open questions the same day, in decisions 17 to 21.

7. **macOS only in v1.** Linux comes later.
8. **One binary.** `llmtodo` holds the CLI and the server.
9. **Browser requests are rejected** by `Host` and `Origin` checks, because a web page can reach localhost.
10. **Claims end on completion** or on a change of assignee. Claims still never time out.
11. **Labels and projects are workspace-wide.** Statuses stay per team.
12. **Moving teams gives a new key** and keeps the old key as an alias.
13. **A full description replace needs a base version.** A patch does not.
14. **Non-primary machines are read-only.** Pull refuses to drop unpushed writes without `--force`.
15. **The import copies Linear images** into a local file store, because Linear links break after cutover.
16. **First PR merge moves the issue to Done.** The sync never reopens a finished issue.

17. **Claims on blocked issues warn** and do not fail. The response lists the open blockers.
18. **The import skips Linear's field history.** Nothing in the two projects reads it.
19. **Agents may do anything except prune.** Prune is the only permanent delete, and only the owner runs it.
20. **Zed is the design target, and Lucide supplies the icons.**
21. **Agents decide technical details.** The owner directs the work and reviews it.

Owner answers, 2026-11-10:

22. **Agents use git freely in this repo.** Commit, push and branch without asking.
23. **Stay in this repo.** The other projects keep Linear. They migrate only after llmtodo tracks its own build, and only when the owner says so.
24. **The owner supplies the Linear key** when asked, as `LINEAR_API_KEY`.
25. **The owner reviews the app in live previews** on throwaway data, not through screenshots.

Build decisions, 2026-11-10:

26. **Runtime SQL queries** instead of `query!` macros, covered by integration tests. See Storage.
27. **The route table replaces utoipa.** `api-types` lists every route with JSON schemas from `schemars`. The server asserts at startup that it serves exactly those routes. `llmtodo schema` prints the table, and a CLI test checks that each route names a real command.
28. **Exit code 11** means forbidden, such as prune by an agent.
29. **Release** clears the assignee and moves a started issue back to the team's first unstarted status, unless `--status` names another.
30. **Delete, prune and label delete ask first.** Without a terminal they need `-y`, or they exit with code 8.
31. **Token usage per ticket comes from transcripts.** Agent writes carry the transcript's cumulative usage by model; the server gives the new part to the first ticket the write touches. Prices sit in settings, seeded with list prices. See `docs/agents/issue-tracker.md`.
32. **Analyzing is a soft mark**, separate from claims. It never blocks, and it expires 15 minutes after the session's last write. See "Analyzing".
33. **Dependencies reuse the `blocks` relation.** Depth is computed by the server, in one query per page. See "Dependencies".
34. **Issue lists carry the token cost** as `cost_usd`, from one grouped query per page. The app shows it in a Cost column that hides first, and in the task view's meta line, with a dash for a missing value.
35. **`ARCHITECTURE.md` holds the crate and layer rules.** It marks each rule as true today or as a target.
36. **CI runs on macOS only.** The app needs GPUI's macOS backend, and v1 is macOS only (decision 7). One job runs the format check, clippy with warnings as errors, and the tests.
37. **Server rules live in `ops`, and SQL lives in `store`.** Reads with no rule may call `store` from `routes`, and the GitHub sync and Linear import sit at the `ops` level. `crates/server/tests/layers.rs` enforces the layer rules. See `ARCHITECTURE.md`.
38. **Markdown is the record, and queried structure is a derived copy.** See Storage. The `questions` table is the first case. SQL cannot parse Markdown, so the server fills a new derived table in Rust when it next opens the database, inside one transaction.
39. **Tasks carry frozen estimates, and active time is measured (LLM-133).** Every task needs an estimated working time and an estimated token cost before a claim, and a claim without them exits with code 6. Tasks already claimed are not affected. A set estimate changes only with a reason that names a spec change, and the activity log records the old value, the new value and the reason. Active time comes from transcript timestamps and reaches the server with each write, the same way as token usage. A gap that ends at an owner prompt, or that is over 10 minutes, does not count. Sessions add up. Past work gets no invented time. See `docs/agents/issue-tracker.md`.
40. **Review rounds (LLM-138).** A move to a review status needs a `+++ Review` section with steps and checks. The owner's answers live in a `reviews` table. Approve closes a ticket with no open PR at once. Request changes needs feedback and hands the ticket back to the agents. See "Review rounds".
41. **The app holds every issue row, up to about 50,000 issues.** The owner chose this on LLM-122 (Q1), so the list has no server-side paging. The snapshot carries the rows without descriptions, plus the open owner questions. The app fetches a description when a task opens. A change to teams, statuses, labels, projects, views or documents sends the catalog alone, and the app maps the issues it holds to it.
42. **One issue filter.** `IssueFilter` in `core` holds the statuses, labels, assignee and LLM status filters. The server turns it into SQL, and the app's list runs `IssueFilter::matches`, so a saved view keeps what the list shows. Free text stays apart: the list matches what a row shows, and the server searches with FTS5.
43. **Task view layout (LLM-142).** The title row has a meta line under it: the labels, the cost, time and estimate, and the dependencies as chips. The line wraps on a narrow task view. Each panel below, such as Review or Git, starts with a section title and ends with a hairline. The Edit and Preview buttons sit in the description panel's header, beside its tabs.
44. **Trackers are a development tool (LLM-126).** The owner answered Q2 on LLM-122 on 2026-11-11. The app can switch between data folders through environment variables only, so a preview can look up a real ticket. One installation stays one workspace. Real tracker settings are possible future work, so no code may assume one data folder for the life of a process: the client takes the folder as an input and resolves it in one place.
45. **Shared client logic lives in `client` (LLM-126).** The live event stream, the data folder choice and the caller's identity are modules of the client crate. The app and the CLI use them, so a later client such as an MCP server gets them for free. A `Harness` trait reads an agent's session, transcript, usage and active time. Claude Code is the first harness. An agent from another harness names itself with `LLMTODO_ACTOR` and sends no usage.
46. **A status flag marks review, not a name (LLM-151).** The owner's Astro team uses In Review for the step that llmtodo's own team calls User review. A status has a `review` flag, and every review rule reads it: the claim end, the review section gate, the frontier, the review panel, the inbox list and review answers.
47. **Workflow states are fields, not labels (LLM-161).** Ready for an agent and needs the owner are `waiting_on`. Blocked is a `blocked-by` relation, and in review is a review status. `label create` and `label update --name` refuse a name with a workflow word, such as ready, owner or blocked, with exit code 6. `--force` creates it anyway. The check lives in the CLI, so the owner can still make any label in the app.
48. **Tabs that do not fit go under More (LLM-197).** The task view's tab bar shows the tabs that fit, from the left. The rest go under a "More (N)" button at the right end of the tabs, which opens a GPUI Kit dropdown menu. The bar never scrolls sideways. It measures each label with the text system as it renders, and a canvas records the room the tabs had. Keyboard tab switching still reaches every tab.
49. **An epic's order is stored as real `blocks` edges (LLM-206).** A sub-issue has a step number scoped to its parent. The server turns the steps into `blocks` relations with `source = order` and rewrites them on each order change, in the same transaction. Six queries read `blocks` edges, so real edges keep all of them, the cycle check and the 50,000-issue plan unchanged. A view that computed the edges on the fly would touch every one of them. See "Order (LLM-206)".
