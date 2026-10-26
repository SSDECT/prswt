# System test report

> Run: 2026-10-26 21:35 · 64/64 checks passed

| Check | Result | Detail |
|---|---|---|
| S01 release binaries exist | ✅ |  |
| S02 command catalog has 64 commands | ✅ | 64 commands |
| S03 CLI analysis run exits 0 | ✅ |  |
| S03 deterministic finish matches oracle | ✅ | 2027-07-14T17:00 |
| S03 critical path starts at notice to proceed | ✅ |  |
| S03 DCMA reports 14 checks | ✅ |  |
| S03 P10 <= P50 <= P80 <= P90 | ✅ | P50 2027-08-03T15:50 P80 2027-08-17T09:10 |
| S03 risk tornado lists the 4 bridge risks | ✅ |  |
| S03 EVM report against BL1 | ✅ |  |
| S04 same seed in a new process gives identical P-values | ✅ |  |
| S04 a different seed gives different samples | ✅ |  |
| S05 XER export writes a file | ✅ |  |
| S05 XER import in a fresh process | ✅ | {'activities': 14, 'name': 'Bridge Rehabilitation', 'project': '01a17946-f6d5-7161-b9dc-def5c477efac', 'relationships': 14} |
| S05 XER round trip keeps the finish | ✅ | 2027-07-14T17:00 |
| S05 MSPDI export writes a file | ✅ |  |
| S05 MSPDI import in a fresh process | ✅ | {'activities': 14, 'name': 'Bridge Rehabilitation', 'project': '01a17946-f6f7-7629-a67c-2eb3cb204b36', 'relationships': 14} |
| S05 MSPDI round trip keeps the finish | ✅ | 2027-07-14T17:00 |
| S06 MCP initialize | ✅ |  |
| S06 MCP lists 41 tools | ✅ | 41 |
| S06 agent cannot write live | ✅ |  |
| S06 agent creates a sandbox | ✅ |  |
| S06 realize R1 shifts finish 20d | ✅ | {'applied': True, 'branch_activities_added': [], 'delta': {'driving_path_after': ['M0', 'D10', 'D20', 'D30', 'C10', 'C20', 'C30', 'C40', 'C50', 'C60', |
| S06 paired comparison shows a later finish | ✅ |  |
| S06 diff lists changes | ✅ |  |
| S06 agent commit needs a human | ✅ |  |
| S06 resource read on a sandbox | ✅ |  |
| S06 prompt renders | ✅ |  |
| S07 malformed messages get JSON-RPC errors | ✅ |  |
| S07 oversized message rejected | ✅ |  |
| S07 server still alive afterwards | ✅ |  |
| S08 MCP --db session writes | ✅ | {'fork_version': 1, 'id': '01a17946-f7e3-7408-ab34-dd60372b9873', 'sandbox': 'sys-1793036056'} |
| S08 a new process sees stored projects | ✅ |  |
| S08 sandboxes survive a restart | ✅ |  |
| S08 first writer succeeds | ✅ | {'applied': True, 'branch_activities_added': [], 'delta': {'driving_path_after': ['S10', 'S20', 'M10', 'M30', 'M50', 'U1 |
| S08 stale second writer is rejected | ✅ | {'error': {'code': 'VERSION_CONFLICT', 'message': 'branch race-1793036056 diverged from the database at version 1; reload first'}} |
| S09 HTTP API starts | ✅ |  |
| S09 no token → 401 | ✅ |  |
| S09 GET /v1/commands | ✅ |  |
| S09 POST schedule.run | ✅ |  |
| S09 agent header → 403 | ✅ |  |
| S09 bad input → 422 | ✅ |  |
| S09 SSE delivers the change event | ✅ | [{'author': {'name': 'http', 'type': 'human'}, 'branch': '01a17946-fa06-75f1-a929-e8d57d0d1418', 'changes': 1, 'project': '01a17946-fa03-758a-a735-626 |
| S10 desktop app starts its control channel | ✅ |  |
| S10 MCP --connect reads the app's project | ✅ |  |
| S10 remote agent still cannot write live | ✅ |  |
| S10 desktop app still running | ✅ |  |
| S11 20k-iteration simulation (incl. process start) < 15 s | ✅ | 0.1 s |
| S11 benchmark 10k/50k CPM < 10 ms | ✅ |  |
| S11 benchmark 5k×5k Monte Carlo < 10 s | ✅ |  |
| S12 server starts | ✅ |  |
| S12 signed-out visitor is sent to /login | ✅ |  |
| S12 login page renders | ✅ |  |
| S12 sign-in sets an HttpOnly session cookie | ✅ |  |
| S12 signed-in home goes to the account page | ✅ |  |
| S12 account page renders with a no-script CSP | ✅ | 200 |
| S12 no web UI is served | ✅ | 404 |
| S12 cookie write without Origin refused (CSRF) | ✅ |  |
| S12 human can load a project via API | ✅ | {"error":{"code":"VALIDATION","message":"a project named `Unit 2 Turnaround` is already open"}} |
| S12 token created and shown once | ✅ |  |
| S12 remote MCP initialize over HTTPS-style POST | ✅ |  |
| S12 remote MCP lists 41 tools | ✅ |  |
| S12 token acts as an agent (live write refused) | ✅ |  |
| S12 agent runs a simulation remotely | ✅ |  |
| S12 revoked token is refused | ✅ |  |
