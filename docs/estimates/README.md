# Estimate data

`tickets.json` holds every closed ticket that has an estimate and measured time or cost. `scripts/estimate-report` writes it and prints the comparison. Run the script again to refresh both.

The repo is public. So the file keeps titles and descriptions only for the llmtodo project. Other projects have numeric fields only, and the Fueled team's projects are named `AST-a` and `AST-b`. Run `scripts/estimate-report --full <file>` with a path outside the repo to get every title and description.

## Fields

- `estimate_minutes` and `estimate_cost_usd` are the frozen estimates.
- `active_minutes` and `cost_usd` are the measured time and cost.
- `unmeasured_sessions` counts sessions that stayed on the ticket for 3 minutes or more but reported no usage.
- `interleaved` is true when a session worked on another ticket at the same time. The server gives a session's usage to the ticket it writes next, so the numbers of such a ticket are not reliable.
- A ticket is measured in full when it has no unmeasured session, is not interleaved, and has at least one active minute. The report uses only those tickets after its first lines.
- `kind`, `ui`, `bug`, `visual` and `server_and_app` come from words in the title and description. `crates` lists the `crates/<name>` paths the description names.
- `review_rounds` and `changes_requested` count the owner's review answers.
- `working_sessions` counts sessions with measured usage.
