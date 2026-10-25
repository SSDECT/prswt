//! RSK-07: small dense symmetric linear algebra for correlation matrices
//! (row-major `n × n`): Jacobi eigendecomposition, Higham nearest
//! correlation matrix, Cholesky.

const JACOBI_SWEEPS: usize = 100;
const HIGHAM_ITERS: usize = 200;
const TOL: f64 = 1e-12;
/// Smallest eigenvalue kept after repair so Cholesky stays well-conditioned.
const MIN_EIGEN: f64 = 1e-10;

/// Spearman rank correlation → Pearson correlation of the underlying normals.
pub fn spearman_to_pearson(rho_s: f64) -> f64 {
    2.0 * (std::f64::consts::PI * rho_s / 6.0).sin()
}

/// Eigenvalues and eigenvectors (columns of `v`) of symmetric `a`.
pub fn jacobi_eigen(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = a.to_vec();
    let mut v = identity(n);
    for _ in 0..JACOBI_SWEEPS {
        let off: f64 = (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| m[i * n + j] * m[i * n + j])
            .sum();
        if off < TOL * TOL {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = m[p * n + q];
                if apq.abs() < f64::MIN_POSITIVE {
                    continue;
                }
                let theta = (m[q * n + q] - m[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (mkp, mkq) = (m[k * n + p], m[k * n + q]);
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let (mpk, mqk) = (m[p * n + k], m[q * n + k]);
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| m[i * n + i]).collect(), v)
}

/// `V · diag(max(λ, floor)) · Vᵀ`.
fn clip_psd(a: &[f64], n: usize, floor: f64) -> Vec<f64> {
    let (vals, v) = jacobi_eigen(a, n);
    let mut out = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            out[i * n + j] = (0..n)
                .map(|k| v[i * n + k] * vals[k].max(floor) * v[j * n + k])
                .sum();
        }
    }
    out
}

/// Higham (2002) alternating projections onto {PSD} ∩ {unit diagonal}.
/// Returns the repaired matrix and whether any change was needed.
pub fn nearest_correlation(a: &[f64], n: usize) -> (Vec<f64>, bool) {
    let (vals, _) = jacobi_eigen(a, n);
    if vals.iter().all(|&l| l >= MIN_EIGEN) {
        return (a.to_vec(), false);
    }
    let mut y = a.to_vec();
    let mut ds = vec![0.0; n * n];
    for _ in 0..HIGHAM_ITERS {
        let r: Vec<f64> = y.iter().zip(&ds).map(|(y, d)| y - d).collect();
        let x = clip_psd(&r, n, 0.0);
        ds = x.iter().zip(&r).map(|(x, r)| x - r).collect();
        let mut next = x.clone();
        for i in 0..n {
            next[i * n + i] = 1.0;
        }
        let diff: f64 = next.iter().zip(&x).map(|(a, b)| (a - b) * (a - b)).sum();
        y = next;
        if diff.sqrt() < 1e-10 {
            break;
        }
    }
    // Make strictly positive definite and renormalise the diagonal.
    let pd = clip_psd(&y, n, MIN_EIGEN);
    let mut out = pd.clone();
    for i in 0..n {
        for j in 0..n {
            let s = (pd[i * n + i] * pd[j * n + j]).sqrt();
            out[i * n + j] = if s > 0.0 { pd[i * n + j] / s } else { 0.0 };
        }
    }
    (out, true)
}

/// Lower-triangular `L` with `L·Lᵀ = a`; `None` if not positive definite.
pub fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let s: f64 = (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum();
            if i == j {
                let d = a[i * n + i] - s;
                if d <= 0.0 {
                    return None;
                }
                l[i * n + i] = d.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - s) / l[j * n + j];
            }
        }
    }
    Some(l)
}

fn identity(n: usize) -> Vec<f64> {
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    v
}
