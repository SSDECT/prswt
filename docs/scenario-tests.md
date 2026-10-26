# Scenario tests

> **Last run:** 2026-10-26 · 89/89 checks · release `prswt-cli api` · `python tools/scenario_test.py`

`tools/scenario_test.py` builds projects from engine commands over the local HTTP API. It uses no outside files, which keeps it clean-room. Each scenario checks two things:

- dates worked out by hand;
- general invariants: links hold, floats agree with dates, round trips keep the schedule, the engine survives hostile input.

Set `PRSWT_BIN` to test another build.

| # | Scenario | What it checks |
|---|---|---|
| S01 | Chain on 5×8 | Finish, weekend skip, critical path, zero float |
| S02 | SS / FF / SF / lead on 7×24 | Dates of each link type; every link holds |
| S03 | Holidays | A non-working Wednesday and a working Saturday morning move the finish |
| S04 | Constraints | Must-finish-by gives negative float. SNET creates float. A mandatory start overrides logic. The health check flags both. |
| S05 | Progress | Remaining duration, expected finish, suspend/resume, out-of-sequence report, finished activity cannot be suspended |
| S06 | Leveling | Overallocation found and removed; back-to-back finish; no stacking on re-level; `clear`; cost |
| S07 | EVM | BAC, PV, EV, AC by hand; CV, SV, CPI and SPI identities |
| S08 | Risk / Monte Carlo | Certain delay moves P50; tornado effect; probability 0; delay inside float; P10 ≤ P50 ≤ P80 ≤ P90; seeds; finish-by probability |
| S09 | Sandbox, undo | Diff, live untouched, discard, commit, undo/redo, history kept through save and open |
| S10 | Round trips | XER and MSPDI export → import keep every activity's dates (S01, S02, S05, S06) |
| S11 | Hostile input | Loops, self links, bad durations, absurd lags, bad dates, probability 7, 10⁹ iterations, blank codes, huge names; delete with reconnect |
| S12 | Activity kinds | ALAP chain, ALAP milestone, finish milestone, level of effort |
| S13 | Scale | 1,500 activities with random SS/FF/SF/lag links. Every link holds; finish = latest early finish; float = late − early; Monte Carlo runs. |

## Defects the scenarios found (fixed 2026-10-26)

| Defect | Fix | Test |
|---|---|---|
| Every command copied the whole event history (`Vec<Event>`) for panic recovery. A write took 21 ms at 4,000 activities; it now takes 0.4 ms. | Branch events are an `imbl::Vector` (shared structure) | Benchmark in `docs/scenario-tests.md`; type-level |
| `localhost` took 2 s per request. The API listened only on 127.0.0.1, and `localhost` tries `::1` first. | The API also listens on `::1` | `api_01_serves_on_both_loopback_addresses` |
| The leveling report put a finish milestone at the next morning, and `finish_after` disagreed with the schedule after applying | The report comes from the rescheduled network | `res_04_leveled_start_is_kept_apart_from_constraints` |
| The tornado showed 0 for a risk that occurs in every iteration | The same iterations are re-run without that risk | `rsk_03_certain_risk_effect_in_tornado` |
| MSPDI export → import replaced activity codes with task IDs | Our own files take the code from WBS | `fmt_02_round_trip_keeps_schedule_calendar_and_structure` |
| Blank activity, resource, risk and WBS codes were accepted | Codes must be non-blank, at most 100 characters, without control characters | `app_02_unknown_and_bad_input_errors` |
| `as_late_as_possible` (as reported) was refused as input | Reported names are accepted | same |
| The table in the desktop app and the Gantt showed only the first 1,000 activities | The UI pages through all rows (up to 50,000) | `dsk_09_views_load_every_row_past_one_page` |
| Activity rows with `links` had no lag | `lag` added | scenario S02 |

## Write latency (HTTP API, kept-alive connection)

| Activities | `activity.add` before | after |
|---|---|---|
| 500 | 1.2 ms | 0.29 ms |
| 2,000 | 8.7 ms | 0.34 ms |
| 4,000 | 21.2 ms | 0.43 ms |
