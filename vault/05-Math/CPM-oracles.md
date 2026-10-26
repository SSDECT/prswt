---
type: oracle
scorecard: CPM-01..18
---
# CPM — conventions & hand-worked oracle

## Conventions (implemented in `prswt-cpm`)
- The forward pass gathers ES candidates (formulas in [[03-Pipeline]] §A.4), takes their max, then `ES = snap_start(cal, cand)`, except finish milestones, which stay unsnapped.
- The backward pass gathers LF candidates, takes their min, then `LF = snap_finish(cal, cand)`, except start milestones, which stay unsnapped.
- TF and FF are measured in working minutes on the activity's own calendar. FF ≤ TF.
- Open ends (no successors) get LF = project LF = `must_finish_by ?? max EF`.
- LOE activities never drive. Their ES comes from predecessors, and their EF = the max successor-required finish (FS → succ ES, FF → succ EF). TF is not applicable.
- Multiple float paths: seed = the unassigned activity with the smallest TF (ties: latest EF, then code). Walk backward to the unassigned predecessor with the smallest link slack, then the smallest TF.

## Oracle network N1 (24h calendar, 1d = 1440 min, start = day 0)
| Act | Dur | Links in |
|---|---|---|
| A | 3 | — |
| B | 4 | A FS |
| C | 2 | A FS |
| D | 5 | B FS, C FS+2 |
| E | 3 | C SS+1 |
| F | 2 | D FS, E FF+1 |

| Act | ES | EF | LS | LF | TF | FF | Critical |
|---|---|---|---|---|---|---|---|
| A | 0 | 3 | 0 | 3 | 0 | 0 | ✔ |
| B | 3 | 7 | 3 | 7 | 0 | 0 | ✔ |
| C | 3 | 5 | 3 | 5 | 0 | 0 | ✔ |
| D | 7 | 12 | 7 | 12 | 0 | 0 | ✔ |
| E | 4 | 7 | 10 | 13 | 6 | 6 | |
| F | 12 | 14 | 12 | 14 | 0 | 0 | ✔ |

Workings:
- D = max(B.EF 7, C.EF 5 + 2) = 7.
- E = C.ES 3 + 1 = 4.
- F = max(D.EF 12, E.EF 7 + 1 − 2 = 6) = 12.
- Backward: LF_E = LF_F − 1 = 13; LF_C = min(LS_D − 2 = 5, (LS_E − 1) + 2 = 11) = 5.

### N1 variants
- **N1-SNET:** C SNET day 5 → C 5–7; D = max(7, 9) = 9–14; E = 6–9; F = max(14, 10 − 2) = 14–16. Finish 16. Critical: A? No. A's TF = LS_A − ES_A: LS_B = 9 − 4 = 5, LS_C = 5 (constraint pins nothing backward; LF_C = LS_D − 2 = 7, LS_C = 5), so LF_A = 5 and TF_A = 2. B: LF 9, TF 2. C, D, F: TF 0.
- **N1-MFBY:** must_finish_by = day 12 → every TF on the critical path = −2. E's TF = 4.
- **N1-MFO:** F MandatoryFinish day 15 → F ES 13 EF 15. Its predecessors still get LF from F's late dates: LS_F = 13, so the TF of A, B, C, D = 1. F's TF = 0. Finish 15.
- **N1-SF:** add G (1d) with link A SF+2 → G: EF ≥ A.ES + 2 = 2, so G = 1–2.

## Oracle N2 — calendar effects (Std 5×8, Fri 2026-10-23 08:00)
- A 1d → B 1d (FS): A Fri 08–17; B Mon 26 08–17.
- With lag +4h on the 5×8 predecessor calendar: B starts Mon 26 13:00 (after lunch) and finishes Tue 27 12:00.
- With lag 4h on the 24h calendar: Fri 17:00 + 4h = Fri 21:00 → snap → Mon 08:00, finishing Mon 17:00.

## CPM-19: expected finish, suspend/resume, out-of-sequence (24 h days)
- **Expected finish.** A is 10 d, started at day 0, 20 % done, with the data date at day 2.
  - Without an expected finish, 8 d are left and A finishes at day 10.
  - With an expected finish of day 6, the remaining duration becomes the work from day 2 to day 6 (4 d), so EF = 6.
  - Successor B (2 d, FS) then runs from day 6 to day 8, and A's total float is 0.
  - Not-started C (5 d) with an expected finish of day 3 runs from day 2 (the data date) to day 3.
  - When the project option is off, the durations decide instead: A finishes at 10, B runs 10–12 and C runs 2–7.
- **Suspend/resume.** A is 10 d, 40 % done (6 d left), with the data date at day 2.
  - Suspended at day 2 and resumed at day 5: A finishes at 5 + 6 = day 11.
  - Suspended with no resume date: work continues from the data date and A finishes at day 8.
- **Out-of-sequence.** A link is out of sequence when the successor's actual date is earlier than what the link needs from the predecessor. Lags are ignored.
  - FS: the successor started before the predecessor finished.
  - SS: the successor started before the predecessor started.
  - FF: the successor finished before the predecessor finished.
  - SF: the successor finished before the predecessor started.
  - Example: X is not finished and B started at day 1, so X→B (FS) is out of sequence. Y started at day 2, after B started at day 1, so Y→B (SS) is out of sequence. Z finished at day 1, when B started, so Z→B (FS) is in order.
- **App oracle (Mon–Fri, 8 h).** A is 4 d, B is 2 d (FS), A is 25 % done, and the data date is Tue 2027-01-05 08:00.
  - With no suspension, the project finishes Mon 2027-01-11 17:00.
  - Suspended Tue 5 and resumed Mon 11: the 3 days left run Mon–Wed, B runs Thu–Fri, and the project finishes Fri 2027-01-15 17:00.
  - With an expected finish of Fri 2027-01-08 17:00, B runs Mon–Tue and the project finishes Tue 2027-01-12 17:00.

## RES-06: leveled starts
A is 4 d and B is 4 d, both on one-unit resource R, and C (2 d) follows both. The project starts Mon 2027-01-04.
- Without leveling, the project finishes Mon 2027-01-11 17:00.
- Leveling moves B to Fri 2027-01-08 (B runs Fri–Wed and C runs Thu–Fri), so the project finishes Fri 2027-01-15 17:00.
- A second leveling run gives the same finish (no stacking). B's own SNET constraint is kept, and its leveled start is Fri 2027-01-08 08:00.
- Clearing the leveling brings the finish back to Mon 2027-01-11.

## CPM-09 ALAP chain (real-data fix, 2026-10-26)

M (ALAP milestone) → B (ALAP, 3d) → C (2d); X (10d) → C; 24h calendar, start day 0.
C starts on day 10. B slides by its free float, 7 days: ES 7, EF 10.
That slide gives the M → B link 7 days of slack, so M slides to day 7.
After the slides, FF(B) = 0, TF(B) = 0 and TF(M) = 0.
Successors are processed first (reverse topological order).
Test: `cpm_09_alap_chain_slides_together`.
