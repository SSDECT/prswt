# PRSWT MCP server

`prswt-cli mcp` speaks newline-delimited JSON-RPC 2.0 over stdio (protocol `2025-06-18`; `2025-03-26` and `2024-11-05` clients are answered in their own version). Logs never go to stdout.

| Mode | Command |
|---|---|
| In-memory with a sample | `prswt-cli mcp --sample bridge` |
| Persisted (PostgreSQL) | `prswt-cli mcp --db` |
| Drive the open desktop app | `prswt --control 7979`, then `prswt-cli mcp --connect 7979` |

Claude Code: `.mcp.json` in this repo registers `prswt` and `prswt-db`. Elsewhere: `claude mcp add prswt -- <path>\prswt-cli.exe mcp --sample bridge`.

## Safety
Clients are agents. They may read everything and write **only in sandboxes**; `commit_sandbox_to_baseline` returns `LIVE_WRITE_FORBIDDEN` unless a human commits (or the host runs with `--allow-agent-commit`). Every change is an event attributed to the client's name.

## Tools
39 curated tools plus 2 generic ones (see `tools/list`), grouped as:
- **Read:** `list_projects`, `get_schedule_summary`, `query_activities`, `explain_activity_dates`, `get_float_paths`, `schedule_health_check`, `detect_overallocation`, `resource_histogram`, `cost_summary`, `compute_evm`, `risk_register`, `run_impact_simulation`, `get_simulation_result`, `generate_mitigation_subgraph`, `diff_branches`, `list_sandboxes`
- **Edit:** `create_project`, `load_sample`, `add_activity`, `update_activity`, `delete_activity`, `link_activities`, `unlink_activities`, `update_progress`, `add_resource`, `assign_resource`, `auto_level_resources`, `capture_baseline`, `upsert_risk`, `inject_risk_event`, `realize_risk`, `link_secondary_risk`, `set_correlation`, `attach_mitigation`, `undo`, `redo`
- **Sandboxes:** `create_sandbox`, `commit_sandbox_to_baseline`, `discard_sandbox`
- **Generic:** `execute_command` (any of the 52 catalog commands, e.g. `file.export_xer`, `file.import_mspdi`) and `list_commands`

Durations are strings like `"5d"`, `"-2h"` or three-point objects `{"o":"5d","m":"8d","p":"15d","dist":"pert"}`.

## Resources
`prswt://projects`, `prswt://commands`, and per project and branch `prswt://projects/{project}/b/{branch}/{summary|critical-path|activities|health|contention|evm|risks|cost|events}`, plus `…/activities/{code}`, `prswt://projects/{project}/diff/{sandbox}` and `prswt://simulations/{run}`.

## Prompts
`risk_auditor`, `supply_chain_correlator`, `executive_briefing`, `mitigation_optimizer`, `schedule_repair`.
