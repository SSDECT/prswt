---
type: oracle
scorecard: RES-01..05, COST-01, EVM-01..04
---
# Resources, cost & EVM — conventions & oracle

## Spreading
- Cumulative curves F(x), x = elapsed share of working time: Uniform `x`; Front `1−(1−x)²`; Back `x²`; Bell `2x²` for x ≤ ½, else `1−2(1−x)²`. (The densities are 1, 2(1−x), 2x and the triangle 4x / 4(1−x).)
- Bucket amount = `A_k − A_{k−1}`, with `A_k = round₁₀(U·F(x_k))` and `A_last = U` exactly, so buckets **sum to U exactly**. Found by testing: multiplying `U·(F₁−F₀)` per bucket drifts by 1e-27 under Decimal division by 1/3.
- Remaining work is spread over `[max(ES, DD), EF]`; completed activities have no remaining usage.
- Capacity per day = max units/h (effective-dated) × working hours on the resource calendar.

## Oracle (5×8, Mon 2026-10-26 08:00, crew 1 u/h, 50/h)
| Case | Expected |
|---|---|
| 24 units over 3 days, uniform | 8 / 8 / 8 |
| front-loaded | 13.33 / 8 / 2.67 (24·5/9, 24·3/9, 24·1/9) |
| A, B parallel 2d × 16 units | 16/day vs 8 → overallocated Mon–Tue |
| leveling (TF, ES, code) | B delayed 2d → Wed 28 08:00; finish Thu 29 17:00 |
| leveling with preserve_float | B unresolved (TF 0) |
| cost 16h × 50 + expense 100 | 900; by day Mon 400 |
| rate 50 → 80 from Tue | 8·50 + 8·80 = 1040 |
| resource-dependent 24 u at 0.5 u/h | 48 h |

## EVM oracle (A 4d → B 4d, each 32 h × 25 = 800; BAC 1600)
As of Thu 29 00:00 (3 working days), A at 50 %, AC 500:

| PV | EV | AC | CV | SV | CPI | SPI | EAC (BAC/CPI) | ETC | VAC | TCPI | EAC (AC+BAC−EV) | EAC (CPI·SPI) | ES | SPI(t) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 600 | 400 | 500 | −100 | −200 | 0.8 | 0.6667 | 2000 | 1500 | −400 | 1.0909 | 1700 | 2750 | 2d | 0.6667 |

PV within a day = the worked share of that day's budget on the project calendar (Mon 12:00 → 100).

## Actual cost entry (`activity.progress actual_cost`)
The cost to date is split over the activity's assignments and expenses in proportion to their planned cost (equally if none has one); the last item takes the rounding remainder, so AC adds back exactly. A 2d with 16 h × 100 = 1600 labour and a 400 expense; 1000 to date → 800 / 200; at 50 %: EV 1000, AC 1000, CPI 1. An activity with neither is rejected.
