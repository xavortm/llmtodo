# Issue tracker: llmtodo

This repo tracks its work in llmtodo, through the `llmtodo` CLI. Use it for every ticket you read, claim, change or create. Do not use Linear here.

The guide to the CLI is in the `llmtodo` skill: [SKILL.md](../../.claude/skills/llmtodo/SKILL.md) and [guide.md](../../.claude/skills/llmtodo/guide.md). The full command and flag list is in [cli-reference.md](cli-reference.md). `scripts/gen-agent-docs` writes that file from `llmtodo schema`.

The map issue of this repo is LLM-1. The rest of this page holds the steps that apply only to this repo.

## Review before anything lands

Every ticket that ends in a commit is reviewed on its branch, by agy first and then by the owner. `CLAUDE.md` ("Build and review") has the steps. In short:

- Work on a branch and a PR. Nothing goes to `main`, and nothing is published, before the owner approves.
- For a ticket with UI work, run `scripts/preview-build <KEY>`, so the owner's **Try it** opens the branch app on demo data without a compile.
- Move the ticket to In Review and run `scripts/request-review <KEY>`. agy reviews in a throwaway clone. When agy is out of quota, the script has Claude Code review instead. A pass moves the ticket to User review, with the reviewer's notes kept.
- After the owner approves: merge the PR, publish as below, and mark the ticket Done.

## Publish after an approved merge

The owner runs the installed app, not a build from this repo. Publish only after the owner approved a ticket and its PR is merged to `main`, never from a branch. Publish from the main checkout. From a worktree, `git worktree list` prints that folder first.

```sh
main=$(git worktree list --porcelain | sed -n '1s/^worktree //p')
git -C "$main" merge --ff-only origin/main
"$main/scripts/publish"
```

- `scripts/publish` builds the release app, swaps it into `~/Applications/llmtodo.app` in one step, and installs the CLI from the same commit into `$CARGO_HOME/bin` (`~/.cargo/bin` when `CARGO_HOME` is unset). It also runs `scripts/gen-agent-docs`.
- It never stops a server. The owner's app sees the new build within a few seconds and shows "Restart to update" in the title bar. The owner's click stops the server and opens the new app.
- Publish once per round, not after every commit. A release build takes minutes.
- `llmtodo app` opens the installed app from any folder.
- Tests pass two folders, `scripts/publish <apps folder> <cli folder>`, so they never touch the real install.
