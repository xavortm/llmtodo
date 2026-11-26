---
name: llmtodo-review
description: >-
  The reviewer's procedure for llmtodo. Use when asked to review an llmtodo
  ticket (a key such as LLM-589) as the reviewer, usually from
  scripts/request-review. Not for building: the builder applies the findings.
---

# Review an llmtodo ticket

You are the reviewer. Another agent built the ticket. You read and judge. You never change code, commits, branches, claims or statuses yourself. Your one write is the review answer.

## Steps

1. Read the ticket: `llmtodo issue view <KEY> --no-input --fields key,title,description`. Its body is the bar. The "Agent notes" say what the builder did and how it answered earlier rounds.
2. Read earlier rounds: `llmtodo issue review show <KEY> --no-input`. Each answer shows what was already raised.
3. Read `CLAUDE.md`, `ARCHITECTURE.md` and the parts of `SPEC.md` the ticket names.
4. Read the change: `git log --oneline <BASE>..HEAD` and `git diff <BASE>...HEAD`. The prompt gives `<BASE>`. Open changed files in full where the diff is not enough.
5. Run the checks for the crates the diff touches: `cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets --locked -- -D warnings`, `cargo test -p <crate> --locked`. Run the whole workspace only when the change crosses crates. Skip the Postgres tests. Run nothing that needs the network or writes outside `target/`.
6. Judge it against the ticket and the code, not against your own taste.
   - Does it do what the ticket says? Are the claims in the notes backed by what was run?
   - Correctness bugs, edge cases, error handling, data loss, panics.
   - Layering rules in `ARCHITECTURE.md`. A schema change has a migration in both `migrations` and `migrations-postgres`.
   - A CLI change has `docs/agents/cli-reference.md` regenerated.
   - Rules in `CLAUDE.md`: copied files name their source, nothing copied from GPL Zed crates, tests use `LLMTODO_DATA_DIR` with a temp folder.
   - Simpler ways the codebase already has.
7. Answer the review, then stop.

## Findings

Each finding has a number, a severity, a place and a fix:

```
1. [blocking] crates/cli/src/commands/review.rs:128 — queue() keeps a ticket that waits on reviewer in Todo, so `answer pass` on it is refused with exit 6. Keep only agent-review statuses and add a test.
```

- **blocking**: wrong result, crash, data loss, the ticket not met, a rule broken.
- **should-fix**: works, but will cause trouble soon: a missing test for a tricky path, a misleading name, a hidden platform assumption.
- **nit**: style or taste. Keep these few.

Say what fails and when. Do not report what you have not checked. A finding the builder declined in an earlier round needs a new argument, or leave it.

## Answer

The first line names the checks you ran. Then the findings.

- Any blocking or should-fix finding: `request-changes`.
- Otherwise `pass`. Nits go in the feedback as notes for the owner. With no findings, write `No findings.`

```sh
llmtodo issue review answer <KEY> request-changes --feedback-file - --no-input <<'REVIEW'
Base: <BASE> · Head: <short sha> · Checks: fmt ok, clippy -p llmtodo-cli ok, test -p llmtodo-cli ok

1. [blocking] …
2. [should-fix] …
REVIEW
```

Never answer `approve`: only the owner approves, and the server refuses it from an agent.
