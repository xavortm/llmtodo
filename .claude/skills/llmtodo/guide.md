# llmtodo guide

The full guide to the `llmtodo` CLI for agents. `SKILL.md` in this folder has the short version. The examples use the team key `ABC`. Use the key of your own team.

`llmtodo <command> --help` lists the flags of one command. `llmtodo schema` prints every command, flag, exit code and API shape as JSON.

## Rules

- **Output is JSON** when stdout is not a terminal. Pass `--fields key,title,status.name` to save tokens.
- **Set a session ID** before you claim: `export LLMTODO_SESSION=$CLAUDE_CODE_SESSION_ID`. The CLI reads Claude Code's session ID on its own, but a release must match the session that claimed.
- **Give each subagent its own session.** Subagents can inherit the parent's session ID, and a claim by the same session succeeds again with `changed: false`. Set `LLMTODO_SESSION=$CLAUDE_CODE_SESSION_ID-<subagent-name>` in each subagent's brief. A repeated claim prints a warning on stderr.
- **Never prompt.** Pass `--no-input`. Commands that delete ask first, so pass `-y` when you mean it.
- **Exit codes are stable.** 3 not found, 4 claim held by another session, 5 stale version, 6 validation, 10 patch matched zero or several times. The full list is in the reference.
- **Agents may do anything except prune.** Trash and restore are fine. Prune is the owner's.
- `.llmtodo.toml` at the git root sets the default team and project, so most commands need neither flag. `llmtodo init` writes it. See "Set up a repo" in `SKILL.md`.

## Pick and claim work

```sh
llmtodo issue frontier ABC-1 --fields key,title,priority     # open, unblocked, unassigned sub-issues
llmtodo issue frontier --project llmtodo --for kind=agent     # the whole project, for agents
llmtodo issue view ABC-12                                     # description, comments, relations, work log
llmtodo issue claim ABC-12                                    # assignee + In Progress; exit 4 if held
```

A claim refuses a task that lacks a time estimate or a cost estimate, with exit code 6. See "Estimate before you claim", below. A claim by the same session again changes nothing, so retries are safe. A claim on a blocked issue works, but prints a warning with the open blockers. Claims never time out. `issue release ABC-12` frees one and moves it back to Todo. `--steal` takes one over and records who held it.

## Estimate before you claim

Every task needs an estimated working time and an estimated token cost before work starts. Read the task, estimate both, and set them. The owner can create tasks without estimates, so the agent that picks up a task adds them.

```sh
llmtodo issue create -t "Title" --estimate-minutes 45 --estimate-cost 2.50
llmtodo issue update ABC-12 --estimate-minutes 45 --estimate-cost 2.50
llmtodo issue claim ABC-12
```

- The time is minutes of active agent work. The cost is the token cost in USD.
- Filling in an empty estimate needs no reason.
- A set estimate stays frozen. It can change only when the spec changes. The server refuses any other change with exit code 6.
- To change one after a spec change, name the change: `llmtodo issue update ABC-12 --estimate-minutes 90 --estimate-reason "The spec added CSV export."`. The activity log records the old value, the new value and the reason.
- A refused claim has `details.reason` set to `estimate_missing`, and `details.missing` names the fields. A refused change has `estimate_frozen`.
- Tasks that a session already holds are not affected. Their holder can claim again, and `--steal` still works.

Estimates let us compare the plan with the real time and cost later. So estimate honestly once, and do not adjust an estimate to match the work.

### How to estimate

Estimate from past tasks, not from how big the task feels. The first estimates in this tracker were 3 to 10 times too high for time, and 2 to 8 times too high for cost. Agents work faster than a person, and the time counts only active work.

1. Find two or three closed tasks like this one, and read what they really took:

   ```sh
   llmtodo issue query -q "<words from the title>" --fields key,title,estimate_minutes,estimate_cost_usd,active_seconds,cost_usd --limit 10
   ```

2. Start from their real time and cost. `active_seconds` is the real time, and `cost_usd` is the real cost.
3. If you find no similar task, start from this table. It comes from measured tasks on 2026-11-11.

   | Task | Time | Cost |
   |---|---|---|
   | Small fix in one place, with a test | 5 to 15 min | $1 to $3 |
   | UI change across a few files, with tests | 10 to 25 min | $2 to $5 |
   | Feature across the server, the CLI and the app | 20 to 60 min | $4 to $12 |
   | Refactor across every crate | 60 to 180 min | $10 to $30 |

4. Cost grows with time. Measured tasks cost about $0.15 to $0.45 per active minute. A cost far outside that range for your time estimate is probably wrong.
5. Add time only for a known risk, such as a bug with no known cause or an API you have not used. Say the risk in your Agent notes.

## Mark a ticket as Analyzing

Mark a ticket when you read it or fill in its details without a claim. Examples: you write tickets from the owner's chat, or you research before you plan. The app shows the mark in the LLM status column, so the owner sees that an agent is on it.

```sh
llmtodo issue analyze ABC-12          # set the mark for this session
llmtodo issue analyze ABC-12 --done   # end it when you are done
```

- The mark blocks nobody. Other agents can still write, claim and finish the ticket, and the owner can still edit it.
- Each write you make to the ticket keeps the mark alive. It ends 15 minutes after your last write, so a crashed agent cannot leave it on.
- A claim ends it, because Locked replaces Analyzing. Finishing or deleting the ticket ends it too.
- Use a claim, not this mark, when you will change code for the ticket.

## Write to a ticket

The description is the main record. Put notes there, not in comments.

```sh
# Add or replace a collapsible section at the end. No version needed.
llmtodo issue update ABC-12 --upsert-section "Agent notes" --body-file notes.md

# Replace one section, chosen by heading text or `+++` title.
llmtodo issue update ABC-12 --section "Plan" --body $'\n1. Do this\n'

# Replace one exact string. It must occur exactly once.
llmtodo issue update ABC-12 --replace "TODO: pick a crate" --with "Use comrak."

# A full replace needs the version you read, from `issue view`.
llmtodo issue update ABC-12 --description-file desc.md --base-version 4
```

`--body-file -` and `--description-file -` read stdin.

Other writes:

```sh
llmtodo issue update ABC-12 --status "In Review" --add-label Area/cli --remove-label needs-triage
llmtodo issue attach ABC-12 https://github.com/owner/repo/pull/7
llmtodo issue relation add ABC-12 blocked-by ABC-9
llmtodo issue update ABC-12 --status Done                     # ends your claim
```

When a ticket has UI work the owner can review, do not mark it Done after it lands. Write its review section first, then move it to a review status. The next section explains both.

## Send a ticket to a review status

A review status is any status with the review flag, such as User review or In Review. `llmtodo status list` shows `review: true` on it, and `llmtodo status update <ID> --review true` sets the flag. A ticket moves to a review status only with a `+++ Review` section in its description. The section has two headings, and each needs at least one line. The server refuses the move with exit code 6 otherwise, and the error shows the format. `details.reason` is `review_section_missing`, and `details.missing` names the empty headings.

```sh
llmtodo issue review set ABC-12 --step "Open the inbox." --step "Hover an option button." \
  --check "The text turns black on the white background."
llmtodo issue update ABC-12 --status "User review" -w owner
```

`review set` writes this section, or replaces it:

```md
+++ Review
### Steps to reproduce
1. Open the inbox.
2. Hover an option button.

### What to check
- The text turns black on the white background.
+++
```

Write it for the owner, who reads it once and acts on it:

- Use very simple words.
- Put one action on each line.
- Give no reasons. The rest of the description holds the context.
- The owner must not need to read anything twice.

A move to a review status hands the ticket to the owner. It ends your claim, so the app no longer shows the ticket as locked, and the activity log records the reason `review`. The frontier skips the ticket while it waits for the owner. A claim on it moves it back to the first work status. It also sets `waiting_on owner` when the update does not set it.

## Act on the owner's answer

The owner answers in the app with Approve or Request changes. Each answer is one round. The server stores every round, and `issue review show ABC-12` lists them with the section.

- **Approve, no open PR.** The server moves the ticket to Done at once. Nothing is left to do.
- **Approve, with an open PR.** The ticket stays in its review status and waits on an agent, so the frontier offers it. Merge the PR, update anything else that is left, then run `llmtodo issue update ABC-12 --status Done`.
- **Request changes.** The feedback says what to change. The ticket moves to Todo and waits on an agent, and any claim ends. Claim it, make the changes, write the review section again, and move it back to the review status.

```sh
llmtodo issue review wait                    # blocks until the next answer, then prints it as JSON
llmtodo issue review wait ABC-12             # only answers on ABC-12
llmtodo issue review wait --after-seq 812    # answers after the last one you handled
llmtodo issue review answer ABC-12 approve   # answer as the owner does in the app
llmtodo issue review answer ABC-12 request-changes -m "The text is too small."
```

`review wait` listens on the live events, so it does not poll. The answer it prints has `seq`, `key`, `round`, `verdict`, `feedback`, `author`, `status` and `waiting_on`. Pass that `seq` to `--after-seq` on the next wait, so you miss no answer between two waits. `--timeout <seconds>` gives up with exit code 1.

## Dependencies

A dependency is a `blocked-by` relation: "ABC-12 depends on ABC-9" means ABC-12 is blocked by ABC-9. The server refuses a relation that would make a cycle, with exit code 6, and names the cycle in the error.

```sh
llmtodo issue relation add ABC-12 blocked-by ABC-9            # ABC-12 depends on ABC-9
llmtodo issue create -t "Title" --blocked-by ABC-9
llmtodo issue deps ABC-12                                     # the tree, both directions
llmtodo issue deps ABC-12 --open                              # hides closed tasks
llmtodo issue query --depends-on ABC-9 --fields key,depth     # open tasks that wait for ABC-9
```

`issue view` and every list include `depth` and `open_blockers`, so you rarely need another call. Depth is the longest chain of open blockers. If A depends on B and B depends on C, A has depth 2 and B has depth 1. Done, canceled and trashed tasks end the chain. A closed task has depth 0.

In the tree, each node has `key`, `title`, `status` and `depth`. The root has `depends_on` and `blocks`. A node with `truncated: true` has more behind it that the tree leaves out, because the node shows earlier in the tree or the chain is over 40 levels. A claim on a blocked task still works, and its warning lists the longest chain. The frontier skips tasks with open blockers.

## Keep the git panel current

Each issue shows a git panel with its branch, worktree, PR and commits. A claim fills the branch and worktree from your checkout. The panel keeps them after the claim ends.

Update the panel whenever one of them changes: you switch branch or worktree, or you open a PR.

```sh
llmtodo issue update ABC-12 --git-here                        # branch and worktree from the current checkout
llmtodo issue update ABC-12 --branch fix/thing --worktree /path/to/worktree
llmtodo issue update ABC-12 --pr https://github.com/owner/repo/pull/7
llmtodo issue update ABC-12 --branch none --worktree none --pr none   # clear them
```

`--pr` also attaches the PR, so the GitHub sync follows it. The commits come from the work log: `issue view` lists them when the worktree, or the current repo, has them. A session lists only the commits made in its checkout. Commits it pulled in with a fast-forward or a rebase belong to other sessions.

## Waiting on an agent or the owner

Each issue can wait on one side: `agent` means it is ready for an agent to pick up, `owner` means it needs the owner's answer first. The frontier skips issues that wait on the owner. Finishing an issue clears it.

```sh
llmtodo issue update ABC-12 -w owner        # you need an answer: prefer `issue ask`, below
llmtodo issue update ABC-12 -w agent        # ready to pick up
llmtodo issue update ABC-12 -w none
llmtodo issue query --waiting-on owner --open
```

## Ask the owner

When you need a decision from the owner, ask it as an owner question. Do not write the question in prose in your notes. The owner answers every question in one place, the inbox in the app.

```sh
llmtodo issue ask ABC-12 "Fork the crate or send an upstream PR?" -o "a=Fork" -o "b=Upstream PR"
llmtodo issue ask ABC-12 "What should the binary be called?" --context "It shows in the Dock."
llmtodo issue questions ABC-12 --all                          # every question on ABC-12, with answers
llmtodo issue questions                                       # open questions on every open issue
llmtodo issue answer ABC-12 Q1 b                              # answer, as the owner does in the app
llmtodo issue undo-answer ABC-12 Q1                           # take the answer back
```

`issue ask` adds the question to the `+++ Owner questions` section of the description and sets `waiting_on owner`. An `-o` without a letter takes the next free one. Leave out `-o` for a free-text question. The owner can also answer a choice with their own text, so do not assume the answer is an option letter. When the owner answers the last open question, `waiting_on` moves to `agent`, so the frontier offers the issue again. Read the answers with `issue questions` or `issue view`. Issues carry `open_questions`, the number of questions with no answer, so a list shows it without the description.

`issue undo-answer` opens the question again and sets `waiting_on owner`. Only the actor who answered can undo. It fails with exit code 6 when another actor wrote to the issue after the answer, the question or answer changed, or the issue is closed. `details.reason` in the error names the rule.

The section is plain Markdown, so you can also write it by hand:

```md
+++ Owner questions
### Q1. Fork the crate or send an upstream PR?
Optional context.

- a) Fork
- b) Upstream PR

**Answer:** b) Upstream PR

### Q2. What should the binary be called?

**Answer:**
+++
```

Each question starts with a `### Q<n>.` heading. `- a)` lines are options. The answer runs from `**Answer:**` to the next question, and an empty answer means the question is open.

## Create tickets

Search for duplicates first. `--similar` searches every project. Other queries use the default project from `.llmtodo.toml`; pass `--all-projects` to search them all.

```sh
llmtodo issue query --similar "websocket catch up test" --fields key,title,score --limit 5
llmtodo issue create -t "Title" --parent ABC-1 -l Area/server --description-file body.md --for kind=agent
```

`--for` sets the wanted executor, such as `kind=agent`, `model=opus-5.5,effort=high` or `kind=human`.

## The work log

Every write you make adds your session to the ticket's work log: harness, model, effort, branch, worktree, herdr pane and commits. You do not need a flag for it. Add a note or a link to your session:

```sh
llmtodo session note "Plan: /path/to/plan.md" --issue ABC-12
llmtodo session show                                          # tickets this session touched
```

## Token usage

The CLI reads your Claude Code transcript and sends your token usage with every write. The server gives what is new since your last ticket write to the ticket you write now. You do not need a flag for it.

Tokens you spend after your last write are not counted until your next one. So make your final status change, such as `--status "In Review"`, after the work is done. `llmtodo usage` shows tokens and cost per ticket; `issue view` shows them per session. Issue lists carry the cost as `cost_usd`. The field is missing when the issue has no usage or no priced model.

## Active time

The CLI also sends your active working time with every write, the same way as tokens. The server gives what is new to the ticket you write now. Sessions add up, including parallel agents on one ticket.

Active time is the sum of the gaps between the user and assistant lines of your transcript and its subagent transcripts. Two kinds of gap do not count. A gap that ends at a prompt from the owner is waiting time. A gap over 10 minutes is idle, because 10 minutes is the longest foreground tool call Claude Code allows.

`llmtodo usage` shows time per ticket, and `issue view` shows it per session. Issue lists carry it as `active_seconds`. The field is missing when the task has no time data. Past work gets no invented time: a session that reported tokens before time tracking existed counts time only from its next write on.

## Search

```sh
llmtodo issue query -q "claim conflict"                       # full text: titles, descriptions, comments
llmtodo issue query --open --assignee me
llmtodo issue query --label needs-owner --open
```
