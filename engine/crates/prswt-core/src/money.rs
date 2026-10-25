//! Exact monetary amounts (single project currency).

use std::fmt;
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

/// Exact amount in the project currency (`Decimal`, never floating point).
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Money(pub Decimal);

impl Money {
    /// Zero.
    pub const ZERO: Money = Money(Decimal::ZERO);

    /// Amount from whole cents.
    pub fn from_cents(cents: i64) -> Self {
        Self(Decimal::new(cents, 2))
    }

    /// Rounded to cents, banker's rounding (the accounting convention).
    pub fn to_cents_rounded(self) -> Self {
        Self(
            self.0
                .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven),
        )
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}", self.to_cents_rounded().0)
    }
}

impl Add for Money {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl Sub for Money {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl Neg for Money {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}
impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}
impl SubAssign for Money {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}
impl Mul<Decimal> for Money {
    type Output = Self;
    fn mul(self, rhs: Decimal) -> Self {
        Self(self.0 * rhs)
    }
}
impl std::iter::Sum for Money {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn core_03_exact_arithmetic() {
        // 0.1 + 0.2 is exact in decimal, unlike f64.
        let a = Money(Decimal::from_str("0.1").unwrap());
        let b = Money(Decimal::from_str("0.2").unwrap());
        assert_eq!(a + b, Money(Decimal::from_str("0.3").unwrap()));
        assert_eq!(Money::from_cents(12_345).to_string(), "123.45");
        // rate 37.50/h * 7.5h
        let cost = Money(Decimal::from_str("37.50").unwrap()) * Decimal::from_str("7.5").unwrap();
        assert_eq!(cost.to_string(), "281.25");
        // banker's rounding at the cent
        assert_eq!(
            Money(Decimal::from_str("0.125").unwrap()).to_string(),
            "0.12"
        );
        assert_eq!(
            Money(Decimal::from_str("0.135").unwrap()).to_string(),
            "0.14"
        );
        let total: Money = [a, b, -a].into_iter().sum();
        assert_eq!(total, b);
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"0.1\"");
    }
}
