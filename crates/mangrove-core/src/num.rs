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
    // Both branches below compute `10^|scale|`. Bound it so a crafted exponent
    // (e.g. `1e1000000000`) cannot exhaust memory, and so the `as u32` casts
    // below cannot truncate (which would silently collide distinct values onto
    // one hash). Far beyond any real unit literal; past it, "not an exact int".
    const MAX_SCALE: u64 = 10_000;
    if scale.unsigned_abs() > MAX_SCALE {
        return None;
    }
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

    #[test]
    fn huge_exponent_is_rejected_not_expanded() {
        // Was: 10^scale expanded (DoS) and `as u32` truncated (hash collision).
        // Now: rejected as non-integer, instantly.
        assert_eq!(exact_bigint(&bd("1e1000000000")), None);
        assert_eq!(exact_bigint(&bd("1e4294967296")), None); // 2^32: would truncate to 10^0=1
        assert_eq!(exact_bigint(&bd("1e-1000000000")), None);
    }
}
