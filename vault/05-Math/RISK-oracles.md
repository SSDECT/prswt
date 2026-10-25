---
type: oracle
scorecard: RSK-01..15
---
# Risk engine — conventions & analytic oracles

## Conventions
- **Slots and streams:** every uncertain quantity (activity duration, risk occurrence, each risk impact) has a slot keyed by a stable FNV-1a hash of its entity id. Its uniform for iteration n = ChaCha8(seed ‖ key ‖ "PRSWTmc1") on stream n, first u64, mapped to (0,1). This gives (a) thread-count invariance and (b) **common random numbers** across branches that share an entity.
- **Correlation:** Spearman → Pearson with 2·sin(πρ/6); Higham projections when not PSD (Jacobi eigendecomposition); Cholesky; z = Φ⁻¹(u), x = Lz, u′ = Φ(x).
- **PERT:** α = 1+λ(m−o)/(p−o), β = 1+λ(p−m)/(p−o). The inverse CDF is tabulated at 4097 points by bisection on an exact incomplete beta (Lentz continued fraction + Lanczos ln Γ), then linearly interpolated.
- **Risk order:** topological over secondary triggers (cycles rejected). The probability of a triggered risk = max(own, conditional) in that iteration.
- **Contingent branches:** dormant nodes/edges, activated when their risk occurs; `replaces` edges are switched off.
- **Cost per iteration:** Σ (time-dependent × sampled/base duration + fixed) × cost factors + cost adds.
- **Quantiles:** nearest rank. Criticality = TF ≤ threshold in the iteration. Sensitivity = Spearman(duration, finish). Cruciality = CI × ρ.

## Analytic oracles (24 h calendar, days)
| Test | Model | Analytic | Tolerance (n) |
|---|---|---|---|
| serial sum | 3 × Tri(2,4,9) in series | mean 15, σ = √6.5 = 2.55 | ±0.1, ±0.08 (20k) |
| parallel max | max(U(0,10), U(0,10)) | E = 20/3, P50 = 10/√2 = 7.071 | ±0.08, ±0.1 (20k) |
| Bernoulli risk | 10d + (p = 0.3) × 10d | rate 0.3, P50 10, P80 20, mean 13 | ±0.015, exact, ±0.2 |
| copula | U, U with ρs = 0.8 | empirical Spearman 0.8 | ±0.03 (8k) |
| degenerate | o=m=p, p ∈ {0,1} | every iteration = CPM | exact |
| CRN | sandbox = live + 1d lag | every paired Δ = +1d | exact |
| special functions | Γ(5) = 24, I₀.₅(2,2) = ½, Beta⁻¹(½;2,3) = 0.385728, Φ⁻¹(0.975) = 1.959964 | | 1e-6…1e-12 |

## Deterministic realization oracle
A(5) → B(5) alongside D(9). Realizing "D +Tri(1,3,6)" at most-likely gives D = 12: finish +2d, D newly critical, A and B no longer critical, D float consumed 1d, driving path [A,B] → [D]. Pessimistic gives D = 15.

## Performance
5k activities × 5k iterations ≈ 1.5 s (release, 8+ threads); target < 10 s.
