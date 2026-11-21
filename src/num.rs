//! Numeric conversions that std has no `From` for, written so that no value narrows or wraps unnoticed.

use num_traits::ToPrimitive;

/// `n` as the nearest f64, the rounding an `as` cast gives.
pub fn f64_of(n: u64) -> f64 {
    n.to_f64().unwrap_or(f64::MAX)
}

pub fn f64_of_usize(n: usize) -> f64 {
    f64_of(u64::try_from(n).unwrap_or(u64::MAX))
}

/// Saturates at `u64::MAX`, which only a nanosecond count of centuries reaches.
pub fn f64_of_u128(n: u128) -> f64 {
    f64_of(u64::try_from(n).unwrap_or(u64::MAX))
}

/// `f` truncated toward zero and clamped into the type: NaN and anything at or below zero is 0, anything past the top
/// is the maximum.
pub fn sat_u64(f: f64) -> u64 {
    f.to_u64().unwrap_or(if f > 0.0 { u64::MAX } else { 0 })
}

pub fn sat_u32(f: f64) -> u32 {
    f.to_u32().unwrap_or(if f > 0.0 { u32::MAX } else { 0 })
}

pub fn sat_usize(f: f64) -> usize {
    f.to_usize().unwrap_or(if f > 0.0 { usize::MAX } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_float_conversions_follow_as_cast_semantics() {
        assert_eq!(sat_u32(f64::NAN), 0);
        assert_eq!(sat_u32(-3.0), 0);
        assert_eq!(sat_u32(2.9), 2);
        assert_eq!(sat_u32(1e30), u32::MAX);
        assert_eq!(sat_u64(-0.5), 0);
        assert_eq!(sat_u64(1e30), u64::MAX);
        assert_eq!(sat_usize(7.7), 7);
    }
}
