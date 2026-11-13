---
name: llmtodo
description: Read, claim, update and create tickets with the llmtodo CLI, and set up llmtodo in a repo. Use whenever a task mentions a ticket key such as ABC-12, asks what to work on next, needs a note, status change or new ticket in the issue tracker, or asks to set up llmtodo in a project.
---

# llmtodo

Read `guide.md` in this skill folder before the first tracker command in a session. It covers claims, the frontier, section patches, the work log and exit codes. `llmtodo <command> --help` lists the flags of one command, and `llmtodo schema` prints all of them as JSON.

## Set up a repo

A repo uses llmtodo when its git root has a `.llmtodo.toml` file. The file names the default team and project, so most commands need neither flag. When the file is missing, set the repo up first:

1. `llmtodo team list --fields key,name` and `llmtodo project list --fields slug,name` show what exists.
2. `llmtodo init --team <KEY> --project <slug> --no-input` writes the file for an existing team and project.
3. Add `--create` when the team or the project does not exist yet. Add `--team-name "<name>"` and `--project-name "<name>"` to name new ones.
4. `llmtodo init` refuses to replace an existing `.llmtodo.toml`. Pass `--force` only when the owner asks for a new team or project.

5. A new project needs an icon. Pick one from `llmtodo project icons <word>` and pass `--project-icon <name>`. See "Give a new project an icon" in the guide.

On a terminal, `llmtodo init` with no flags asks for each value. `llmtodo init` also installs this skill in `~/.claude/skills/llmtodo`. `llmtodo skill install` does only that step.

## The short version

1. `export LLMTODO_SESSION=$CLAUDE_CODE_SESSION_ID`. Give each subagent its own value, such as `$CLAUDE_CODE_SESSION_ID-<name>`.
2. `llmtodo issue frontier <MAP-KEY> --fields key,title,priority` to pick work under a map issue. `llmtodo issue frontier --for kind=agent` covers the whole project.
3. Estimate before you claim: `llmtodo issue update <KEY> --estimate-minutes <minutes> --estimate-cost <usd>`. A claim refuses a task without both, with exit code 6. A set estimate changes only on a spec change, with `--estimate-reason "<what changed>"`.
4. `llmtodo issue claim <KEY>`. Exit code 4 means another session holds it: pick another.
5. Filling in or researching a ticket without claiming it? Run `llmtodo issue analyze <KEY>` first and `llmtodo issue analyze <KEY> --done` after. The app shows it as Analyzing. It blocks nobody and ends by itself 15 minutes after your last write.
6. `llmtodo issue update <KEY> --upsert-section "Agent notes" --body-file -` for notes. The description is the record, not comments.
7. Keep the git panel current. When the branch or worktree changes, run `llmtodo issue update <KEY> --git-here`. When you open a PR, run `llmtodo issue update <KEY> --pr <url>`.
8. Need a decision from the owner? `llmtodo issue ask <KEY> "Question?" -o "a=One" -o "b=Two"`. Leave out `-o` for free text. It sets `waiting_on owner`. Read answers with `llmtodo issue questions <KEY> --all`.
9. Ready for an agent? Set waiting on: `llmtodo issue update <KEY> -w agent`, or `-w agent` on `issue create`. Never use a label for a workflow state. The app shows waiting on only from the field. Waiting on the owner is `-w owner` or `issue ask`. Blocked is a `blocked-by` relation. In review is a review status. `label create` refuses such names without `--force`.
10. Dependencies: `llmtodo issue relation add <KEY> blocked-by <OTHER>` means KEY waits for OTHER. `llmtodo issue deps <KEY>` prints the tree. `issue view` and lists carry `depth` and `open_blockers`. A cycle exits with code 6. See the Dependencies section of the guide.
11. Order an epic's sub-issues in steps: `llmtodo issue order set <EPIC> 18 28 11,23,4 94`. A comma list is one step whose tasks can go in any order, and each task waits for the step before it. `issue order show <EPIC>` prints it. See "Order an epic" in the guide.
12. `llmtodo issue update <KEY> --status Done` when the work is merged. Make this status change last: token usage is counted at each write. If the ticket has UI work the owner can review, do not close it. Write its review section, then move it to the team's review status, such as User review or In Review:
    `llmtodo issue review set <KEY> --step "Open the inbox." --check "The text is black."`
    `llmtodo issue update <KEY> --status "User review" -w owner`
    A review status has the review flag: `llmtodo status list` shows `review: true`. The move hands the ticket to the owner and ends your claim. It fails with exit code 6 without both headings. Use very simple words, one action per line, and no reasons. The owner must not need to read anything twice.
13. The owner answers with Approve or Request changes. `llmtodo issue review wait` blocks until the next answer and prints it. Approve with an open PR: merge it, then mark the ticket Done. Request changes: read the feedback, fix it, write the section again and send it back to the review status. See "Act on the owner's answer" in the guide.

Pass `--no-input` on every call. Never run `llmtodo prune`.

The `llmtodo` binary carries a copy of this folder and installs it with `llmtodo skill install`. Edit the skill only in the llmtodo repo, in `.claude/skills/llmtodo`.
