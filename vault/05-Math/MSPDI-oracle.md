---
type: oracle
scorecard: FMT-02
---
# MSPDI — code mapping & oracle

## Codes (public MSPDI schema)
| Element | Values → PRSWT |
|---|---|
| `PredecessorLink/Type` | 0 FF · 1 FS · 2 SF · 3 SS |
| `LinkLag` | tenths of minutes (4800 = 1 day at 8 h). `LagFormat` 4/6/8/10/12 = elapsed → 24h lag calendar; 19/20 (percent) → 0 |
| `ConstraintType` | 0 ASAP · 1 ALAP · 2 MSO · 3 MFO · 4 SNET · 5 SNLT · 6 FNET · 7 FNLT |
| `WeekDay/DayType` | 1 Sunday … 7 Saturday; 0 = exception (older files, uses `TimePeriod`) |
| `Resource/Type` | 0 material · 1 work (labor) · 2 cost (non-labor) |
| `Duration`, `Work` | ISO-8601 `PT16H0M0S`; days use `MinutesPerDay` |

Summary tasks become WBS elements (parent = outline-number prefix). Task UID 0 (project summary) and resource UID 0 (placeholder) are skipped.

## Oracle
Standard 5×8 with a holiday on Tue 2027-03-02. A 16 h → B 24 h (FS + 1 d) → milestone M.
- A runs Mon 1st and Wed 3rd (the holiday is skipped) and finishes Wed 17:00.
- The 1 d lag consumes Thu 4th.
- B runs Fri 5th, Mon 8th and Tue 9th.
- **The finish is Tue 2027-03-09 17:00.** It is unchanged after two export/import round trips, and the holiday survives.
