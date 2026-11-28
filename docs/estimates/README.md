# Estimate data

`tickets.json` holds every closed ticket that has an estimate and measured time or cost. `scripts/estimate-report` writes it and prints the comparison. Run the script again to refresh both.

The repo is public. So the file keeps titles and descriptions only for the `llmtodo` project. Other projects have numeric fields only, and their team and project names are anonymized (for example `EXT` and `proj-a`). Run `scripts/estimate-report --full <file>` with a path outside the repo to get every title and description.

## Fields

- `estimate_minutes` and `estimate_cost_usd` are the frozen estimates.
- `active_minutes` and `cost_usd` are the measured time and cost.
- `unmeasured_sessions` counts sessions that stayed on the ticket for 3 minutes or more but reported no usage.
- `interleaved` is true when a session worked on another ticket at the same time. The server gives a session's usage to the ticket it writes next, so the numbers of such a ticket are not reliable.
- A ticket is measured in full when it has no unmeasured session, is not interleaved, and has at least one active minute. The report uses only those tickets after its first lines.
- `kind`, `ui`, `bug`, `visual` and `server_and_app` come from words in the title and description. `crates` lists the `crates/<name>` paths the description names.
- `review_rounds` and `changes_requested` count the owner's review answers.
- `working_sessions` counts sessions with measured usage.

## Other ways to estimate

`scripts/estimate-fit` tests other estimates against the measured tickets. LLM-303 ran it on 2026-11-18. The best method is the agent's own estimate times one fitted scale, the "multiply by 0.4" rule of the llmtodo skill. A fit on counted fields did not beat it, so the CLI has no estimate step.

- `at-estimate.json` holds each description as it was when the agent estimated the ticket. The final description is about 5 times longer, because agents add Agent notes during the work. Its length predicts the real time too well, so every fit uses the earlier text.
