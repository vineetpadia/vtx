use std::convert::TryInto;

use smallvec::{smallvec, SmallVec};
use std::mem::swap;

use crate::VecF;

#[derive(Debug, Clone)]
pub struct KnotVector {
    /// Knot positions
    U: VecF,

    /// Degree of the knot vector
    p: usize,
}

impl KnotVector {
    /// Constructs a new knot vector of over
    pub fn from_multiplicities(p: usize, knots: &[f64], multiplicities: &[usize]) -> Self {
        assert!(knots.len() == multiplicities.len());
        let U = knots.iter().zip(multiplicities.iter())
            .flat_map(|(k, m)| std::iter::repeat(*k).take(*m))
            .collect();
        Self { U, p }
    }

    /// For basis functions of order `p + 1`, finds the span in the knot vector
    /// that is relevant for position `u`.
    ///
    /// ALGORITHM A2.1
    pub fn find_span(&self, u: f64) -> usize {
        // U is [u_0, u_1, ... u_m]
        let m = self.len() - 1;
        let n = m - (self.p + 1); // max basis index

        if u >= self[n + 1] {
            return n;
        } else if u <= self[self.p] {
            return self.p;
        }
        let mut low = self.p;
        let mut high = n + 1;
        let mut mid = (low + high) / 2;
        while u < self[mid] || u >= self[mid + 1] {
            if u < self[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }
        mid
    }

    pub fn degree(&self) -> usize {
        self.p
    }
    pub fn len(&self) -> usize {
        self.U.len()
    }
    pub fn min_t(&self) -> f64 {
        self[self.p]
    }
    pub fn max_t(&self) -> f64 {
        self[self.len() - 1 - self.p]
    }

    /// Computes non-vanishing basis functions of order `p + 1` at point `u`.
    ///
    /// ALGORITHM A2.2
    pub fn basis_funs(&self, u: f64) -> VecF {
        let i = self.find_span(u);
        self.basis_funs_for_span(i, u)
    }

    // Inner implementation of basis_funs
    pub fn basis_funs_for_span(&self, i: usize, u: f64) -> VecF {
        let mut N: VecF = smallvec![0.0; self.p + 1];

        let mut left: VecF = smallvec![0.0; self.p + 1];
        let mut right: VecF = smallvec![0.0; self.p + 1];
        N[0] = 1.0;
        for j in 1..=self.p {
            left[j] = u - self[i + 1 - j];
            right[j] = self[i + j] - u;
            let mut saved = 0.0;
            for r in 0..j {
                let temp: f64 = N[r] / (right[r + 1] + left[j - r]);
                N[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            N[j] = saved;
        }
        N
    }

    /// Computes the derivatives (up to and including the `nth` derivative) of non-vanishing
    /// basis functions of order `p + 1` at point `u`.
    ///
    /// ALGORITHM A2.3
    /// if ders = basis_funs_derivs_(), then ders[k][j] is the `kth` derivative
    /// of the function `N_{i-p+j, p}` at `u`
    pub fn basis_funs_derivs(&self, u: f64, n: usize) -> Vec<Vec<f64>> {
        let i = self.find_span(u);
        self.basis_funs_derivs_for_span(i, u, n)
    }

    pub fn basis_funs_derivs_for_span(&self, i: usize, u: f64, n: usize) -> Vec<Vec<f64>> {
        self.basis_derivatives(i, u, n)
            .into_iter()
            .map(VecF::into_vec)
            .collect()
    }

    pub(crate) fn basis_derivatives(&self, i: usize, u: f64, n: usize) -> SmallVec<[VecF; 3]> {
        let width = self.p + 1;
        let mut ndu: SmallVec<[f64; 64]> = smallvec![0.0; width * width];
        let mut a: [VecF; 2] = std::array::from_fn(|_| smallvec![0.0; width]);
        let mut left: VecF = smallvec![0.0; width];
        let mut right: VecF = smallvec![0.0; width];
        let mut ders: SmallVec<[VecF; 3]> = smallvec![smallvec![0.0; width]; n + 1];

        ndu[0] = 1.0;
        for j in 1..=self.p {
            left[j] = u - self[i + 1 - j];
            right[j] = self[i + j] - u;
            let mut saved = 0.0;
            for r in 0..j {
                ndu[j * width + r] = right[r + 1] + left[j - r];
                let temp = ndu[r * width + j - 1] / ndu[j * width + r];

                ndu[r * width + j] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            ndu[j * width + j] = saved;
        }
        for j in 0..=self.p {
            ders[0][j] = ndu[j * width + self.p];
        }
        for r in 0..=self.p {
            let mut s1 = 0;
            let mut s2 = 1;
            a[0][0] = 1.0;
            for k in 1..=n {
                let aus = |i: i32| -> usize {
                    i.try_into().expect("Could not convert to usize")
                };
                let mut d = 0.0;
                let rk = (r as i32) - (k as i32);
                let pk = (self.p as i32) - (k as i32);
                if r >= k {
                    a[s2][0] = a[s1][0] / ndu[aus(pk + 1) * width + rk as usize];
                    d = a[s2][0] * ndu[aus(rk) * width + aus(pk)];
                }
                let j1 = aus(if rk >= -1 { 1 } else { -rk });
                let j2 = aus(if r as i32 - 1 <= pk as i32 {
                    k as i32 - 1
                } else {
                    self.p as i32 - r as i32
                });

                for j in j1..=j2 {
                    a[s2][j] = (a[s1][j] - a[s1][j - 1]) / ndu[aus(pk + 1) * width + aus(rk + j as i32)];
                    d += a[s2][j] * ndu[aus(rk + j as i32) * width + aus(pk)];
                }
                if r as i32 <= pk {
                    a[s2][k] = -a[s1][k - 1] / ndu[aus(pk + 1) * width + r];
                    d += a[s2][k] * ndu[r * width + aus(pk)];
                }
                ders[k][r] = d;
                swap(&mut s1, &mut s2);
            }
        }

        let mut r = self.p;
        for k in 1..=n {
            for j in 0..=self.p {
                ders[k][j] *= r as f64;
            }
            r *= self.p - k;
        }
        ders
    }
}

impl std::ops::Index<usize> for KnotVector {
    type Output = f64;
    fn index(&self, i: usize) -> &Self::Output {
        &self.U[i]
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    /*
    #[test]
    fn test_find_span() {
        let k = KnotVector {
            U: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        };
        assert!(k.find_span(0, 0.0) == 2);
        assert!(k.find_span(0, 0.99) == 2);
        assert!(k.find_span(1, 0.99) == 2);
        assert!(k.find_span(2, 0.99) == 2);
    }
    */
}
