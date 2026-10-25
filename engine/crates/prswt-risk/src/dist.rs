//! RSK-01: distributions sampled by inverse CDF, plus the normal CDF/quantile
//! used by the Gaussian copula. All sampling works in `f64` minutes (or
//! dimensionless factors); reporting rounds back to integers/decimals.

use std::collections::HashMap;
use std::sync::Arc;

use prswt_core::model::{Estimate, Quantity};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

/// Points in a tabulated PERT (Beta) inverse CDF.
const BETA_TABLE_STEPS: usize = 4096;
const BISECTION_STEPS: u32 = 64;

/// A sampleable distribution (values in minutes or as factors).
#[derive(Debug, Clone, PartialEq)]
pub enum Dist {
    /// Always this value.
    Fixed(f64),
    /// Triangular.
    Triangular {
        /// Minimum.
        o: f64,
        /// Mode.
        m: f64,
        /// Maximum.
        p: f64,
    },
    /// Beta-PERT over `[o, p]`; `table[k]` = Beta⁻¹(k / steps).
    Pert {
        /// Minimum.
        o: f64,
        /// Maximum.
        p: f64,
        /// Tabulated Beta inverse CDF on [0, 1].
        table: Arc<Vec<f64>>,
    },
    /// Uniform.
    Uniform {
        /// Minimum.
        a: f64,
        /// Maximum.
        b: f64,
    },
    /// Values with cumulative probabilities (last = 1).
    Discrete {
        /// Outcome values.
        values: Vec<f64>,
        /// Cumulative probabilities (last = 1).
        cum: Vec<f64>,
    },
}

/// Builds distributions, caching Beta tables by shape.
#[derive(Debug, Default)]
pub struct DistBuilder {
    beta_tables: HashMap<(u64, u64), Arc<Vec<f64>>>,
}

impl DistBuilder {
    /// Distribution of a working-time estimate (minutes).
    pub fn estimate(&mut self, e: &Estimate) -> Dist {
        let f = |w: prswt_core::WorkMinutes| int_f64(w.0);
        match e {
            Estimate::Deterministic { value } => Dist::Fixed(f(*value)),
            Estimate::Triangular { o, m, p } => tri(f(*o), f(*m), f(*p)),
            Estimate::Pert { o, m, p, lambda } => {
                self.pert(f(*o), f(*m), f(*p), f64::from(*lambda))
            }
            Estimate::Uniform { min, max } => uni(f(*min), f(*max)),
            Estimate::Discrete { outcomes } => {
                let values = outcomes.iter().map(|(v, _)| f(*v)).collect();
                discrete(
                    values,
                    &outcomes
                        .iter()
                        .map(|(_, p)| dec_f64(*p))
                        .collect::<Vec<_>>(),
                )
            }
        }
    }

    /// Distribution of a dimensionless quantity (factor or money).
    pub fn quantity(&mut self, q: &Quantity) -> Dist {
        match q {
            Quantity::Fixed { value } => Dist::Fixed(dec_f64(*value)),
            Quantity::Triangular { o, m, p } => tri(dec_f64(*o), dec_f64(*m), dec_f64(*p)),
            Quantity::Pert { o, m, p, lambda } => {
                self.pert(dec_f64(*o), dec_f64(*m), dec_f64(*p), f64::from(*lambda))
            }
            Quantity::Uniform { min, max } => uni(dec_f64(*min), dec_f64(*max)),
        }
    }

    fn pert(&mut self, o: f64, m: f64, p: f64, lambda: f64) -> Dist {
        if p <= o {
            return Dist::Fixed(o);
        }
        let alpha = 1.0 + lambda * (m - o) / (p - o);
        let beta = 1.0 + lambda * (p - m) / (p - o);
        let table = self
            .beta_tables
            .entry((alpha.to_bits(), beta.to_bits()))
            .or_insert_with(|| Arc::new(beta_table(alpha, beta)))
            .clone();
        Dist::Pert { o, p, table }
    }
}

impl Dist {
    /// Inverse CDF at `u ∈ (0, 1)`.
    pub fn sample(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Self::Fixed(v) => *v,
            Self::Triangular { o, m, p } => {
                let (o, m, p) = (*o, *m, *p);
                if p <= o {
                    return o;
                }
                let fc = (m - o) / (p - o);
                if u < fc {
                    o + (u * (p - o) * (m - o)).sqrt()
                } else {
                    p - ((1.0 - u) * (p - o) * (p - m)).sqrt()
                }
            }
            Self::Pert { o, p, table } => o + (p - o) * interpolate(table, u),
            Self::Uniform { a, b } => a + (b - a) * u,
            Self::Discrete { values, cum } => {
                let k = cum
                    .partition_point(|&c| c < u)
                    .min(values.len().saturating_sub(1));
                values.get(k).copied().unwrap_or(0.0)
            }
        }
    }

    /// Expected value.
    pub fn mean(&self) -> f64 {
        match self {
            Self::Fixed(v) => *v,
            Self::Triangular { o, m, p } => (o + m + p) / 3.0,
            Self::Pert { o, p, table } => {
                let n = int_f64(i64::try_from(table.len()).unwrap_or(1) - 1).max(1.0);
                let avg = table
                    .windows(2)
                    .map(|w| f64::midpoint(w[0], w[1]))
                    .sum::<f64>()
                    / n;
                o + (p - o) * avg
            }
            Self::Uniform { a, b } => (a + b) / 2.0,
            Self::Discrete { values, cum } => {
                let mut prev = 0.0;
                values
                    .iter()
                    .zip(cum)
                    .map(|(v, c)| {
                        let w = c - prev;
                        prev = *c;
                        v * w
                    })
                    .sum()
            }
        }
    }

    /// True when every sample would be the same.
    pub fn is_fixed(&self) -> bool {
        match self {
            Self::Fixed(_) => true,
            Self::Triangular { o, p, .. } | Self::Pert { o, p, .. } => p <= o,
            Self::Uniform { a, b } => b <= a,
            Self::Discrete { values, .. } => values.len() <= 1,
        }
    }
}

fn tri(o: f64, m: f64, p: f64) -> Dist {
    if p <= o {
        Dist::Fixed(o)
    } else {
        Dist::Triangular { o, m, p }
    }
}

fn uni(a: f64, b: f64) -> Dist {
    if b <= a {
        Dist::Fixed(a)
    } else {
        Dist::Uniform { a, b }
    }
}

fn discrete(values: Vec<f64>, probs: &[f64]) -> Dist {
    let total: f64 = probs.iter().sum();
    let mut acc = 0.0;
    let mut cum: Vec<f64> = probs
        .iter()
        .map(|p| {
            acc += p / total.max(f64::MIN_POSITIVE);
            acc
        })
        .collect();
    if let Some(last) = cum.last_mut() {
        *last = 1.0;
    }
    Dist::Discrete { values, cum }
}

fn interpolate(table: &[f64], u: f64) -> f64 {
    let steps = table.len().saturating_sub(1);
    if steps == 0 {
        return table.first().copied().unwrap_or(0.0);
    }
    let x = u * int_f64(i64::try_from(steps).unwrap_or(0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let k = (x.floor() as usize).min(steps - 1);
    let frac = x - int_f64(i64::try_from(k).unwrap_or(0));
    table[k] + (table[k + 1] - table[k]) * frac
}

fn beta_table(a: f64, b: f64) -> Vec<f64> {
    let steps = int_f64(i64::try_from(BETA_TABLE_STEPS).unwrap_or(1));
    (0..=BETA_TABLE_STEPS)
        .map(|k| beta_inv(int_f64(i64::try_from(k).unwrap_or(0)) / steps, a, b))
        .collect()
}

/// Inverse regularized incomplete beta by bisection (monotone, robust).
pub fn beta_inv(u: f64, a: f64, b: f64) -> f64 {
    if u <= 0.0 {
        return 0.0;
    }
    if u >= 1.0 {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..BISECTION_STEPS {
        let mid = f64::midpoint(lo, hi);
        if reg_inc_beta(mid, a, b) < u {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    f64::midpoint(lo, hi)
}

/// Regularized incomplete beta `I_x(a, b)` (continued fraction, Lentz).
pub fn reg_inc_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let ln_front = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    let front = ln_front.exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_cf(x, a, b) / a
    } else {
        1.0 - front * beta_cf(1.0 - x, b, a) / b
    }
}

fn beta_cf(x: f64, a: f64, b: f64) -> f64 {
    const TINY: f64 = 1e-300;
    const EPS: f64 = 1e-15;
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..300 {
        let m = f64::from(m);
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// Lanczos approximation (g = 7, n = 9), |error| < 1e-13 for x > 0.
pub fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // reflection
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + int_f64(i64::try_from(i).unwrap_or(0)));
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Standard normal CDF via erfc (W. J. Cody–style rational approximation
/// through the complementary error function; |error| < 1.2e-7).
pub fn norm_cdf(x: f64) -> f64 {
    0.5 * erfc(-x / std::f64::consts::SQRT_2)
}

fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// Standard normal quantile (Acklam, relative error < 1.2e-9).
pub fn norm_inv(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let p = p.clamp(1e-300, 1.0 - 1e-16);
    let lo = 0.024_25;
    if p < lo {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - lo {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

/// Exact for |x| < 2^53, which every working-minute value is.
#[allow(clippy::cast_precision_loss)]
pub fn int_f64(x: i64) -> f64 {
    x as f64
}

/// Decimal → f64 (0 when unrepresentable).
pub fn dec_f64(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}
