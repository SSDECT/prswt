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
