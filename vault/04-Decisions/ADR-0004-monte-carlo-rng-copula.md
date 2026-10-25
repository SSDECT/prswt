# ADR-0004 — Monte Carlo RNG & correlation
**Status:** accepted
**Context:** Results must be reproducible across thread counts. Correlated inputs must not require N×K pre-sampled matrices.
**Decision:** ChaCha8Rng(seed) with stream = iteration index. Gaussian copula per correlation group (Spearman→Pearson via 2·sin(πρ/6), Higham PSD repair, Cholesky). Inverse-CDF sampling. Common random numbers for branch comparisons.
**Rejected:** Iman-Conover (needs the full sample matrix in memory, breaks streaming).
**Consequences:** PERT needs an inverse regularized incomplete beta function (implemented and tested against reference values).
