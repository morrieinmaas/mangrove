//! Numeric helpers shared across layers.

use bigdecimal::BigDecimal;
use num_bigint::BigInt;
use std::cmp::Ordering;

/// Returns `Some(n)` if `d` is an exact integer, else `None`.
/// Used to enforce "fractional unit literals are legal iff exact in the base
/// unit" (§4.5): `0.5btc` resolves, `0.5sat` does not.
pub fn exact_bigint(d: &BigDecimal) -> Option<BigInt> {
    // `d == digits * 10^(-scale)`
    let (digits, scale) = d.as_bigint_and_exponent();
    match scale.cmp(&0) {
        Ordering::Less | Ordering::Equal => {
            let pow = BigInt::from(10).pow((-scale) as u32);
            Some(digits * pow)
        }
        Ordering::Greater => {
            let pow = BigInt::from(10).pow(scale as u32);
            if (&digits % &pow) == BigInt::from(0) {
                Some(digits / pow)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn bd(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).unwrap()
    }

    #[test]
    fn integers_and_fractions() {
        assert_eq!(exact_bigint(&bd("1024")), Some(BigInt::from(1024)));
        assert_eq!(exact_bigint(&bd("0.50")), None);
        assert_eq!(
            exact_bigint(&bd("100000000")),
            Some(BigInt::from(100_000_000))
        );
        // 0.5 * 100_000_000 = 50_000_000 (exact) is checked at the resolve site;
        // here 0.5 itself is not an integer:
        assert_eq!(exact_bigint(&bd("0.5")), None);
        assert_eq!(exact_bigint(&bd("2.0")), Some(BigInt::from(2)));
    }
}
