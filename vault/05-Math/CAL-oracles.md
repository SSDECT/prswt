---
type: oracle
scorecard: CAL-01..06
---
# Calendar arithmetic — definitions & hand-worked oracle

## Definitions
Working time W = a union of half-open minute intervals `[s, e)`. Let `cum(t) = |W ∩ [origin, t)|`, which is monotone and non-decreasing.

| Function | Definition |
|---|---|
| `work_between(a, b)` | `cum(b) − cum(a)` (signed) |
| `add_work(t, d>0)` | **smallest** t′ with `cum(t′) = cum(t)+d`, so it lands on a *finish* (inside `(s, e]`) |
| `add_work(t, d<0)` | **largest** t′ with `cum(t′) = cum(t)+d`, so it lands on a *start* (inside `[s, e)`) |
| `add_work(t, 0)` | t (no snapping) |
| `sub_work(t, d)` | `add_work(t, −d)` |
| `snap_start(t)` | t if t ∈ `[s, e)`, else the next s |
| `snap_finish(t)` | t if t ∈ `(s, e]`, else the previous e |

Adjacent intervals (e.g. 22:00–24:00 and next day 00:00–06:00) are merged, so a night shift is one interval.

## Oracle — Standard 5×8 (08–12, 13–17, Mon–Fri)
Reference week: **Fri 2026-10-23** (day of year 296; 2026-01-01 is a Thursday, so it falls on a Friday).

| # | Expression | Expected | Reasoning |
|---|---|---|---|
| a | add_work(Fri 08:00, 1d) | Fri 17:00 | 4h morning + 4h afternoon |
| b | add_work(Fri 08:00, 2d) | Mon 26 17:00 | weekend skipped |
| c | add_work(Fri 16:00, 2h) | Mon 09:00 | 1h Fri + 1h Mon |
| d | add_work(Fri 12:30, 1h) | Fri 14:00 | lunch gap: work starts 13:00 |
| e1 | snap_start(Fri 17:00) | Mon 08:00 | 17:00 is not a working minute |
| e2 | snap_start(Sat 10:00) | Mon 08:00 | |
| e3 | snap_start(Fri 12:00) | Fri 13:00 | |
| f1 | snap_finish(Mon 08:00) | Fri 17:00 | |
| f2 | snap_finish(Fri 12:30) | Fri 12:00 | |
| f3 | snap_finish(Fri 12:00) | Fri 12:00 | 12:00 ∈ (08:00, 12:00] |
| g1 | sub_work(Mon 17:00, 1d) | Mon 08:00 | |
| g2 | sub_work(Mon 09:00, 2h) | Fri 16:00 | |
| h | holiday Mon 26: add_work(Fri 08:00, 2d) | Tue 27 17:00 | |
| i | Sat 24 worked 08–12: add_work(Fri 16:00, 2h) | Sat 09:00 | |
| j | work_between(Fri 08:00, Mon 17:00) | 960 (16h) ; reversed −960 | |
| k | child calendar (base 5×8, own holiday Mon 26, week inherited) | as h | inheritance |
| l | 24h: add_work(Fri 16:00, 2h) | Fri 18:00 | continuous fast path |
| m | night shift Mon–Fri 22–24 + next day 00–06: add_work(Mon 22:00, 4h) | Tue 02:00 | merged interval |
