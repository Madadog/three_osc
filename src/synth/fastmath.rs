//! Cheap approximations of the transcendental functions the control rate code leans on.
//!
//! Every voice recalculates its pitch, envelopes, LFO and filter coefficients once per control
//! rate block, which with the standard library's `powf`, `log2`, `sin` and `tan` costs more than
//! rendering the samples of the block. These versions are accurate to within a few parts in ten
//! million (around single precision), which is far below anything audible, and run in a handful
//! of multiplications each. None of them handle NaN or infinite inputs specially.

/// 2 raised to `x`. Accurate to about 1e-7 relative. Inputs below -126 return 0 (the result
/// would be subnormal) and inputs above 127 are clamped.
#[inline]
pub fn exp2(x: f32) -> f32 {
    if x < -126.0 {
        return 0.0;
    }
    let x = x.min(127.0);
    // Split into an integer and a fraction in [-0.5, 0.5]. Adding 1.5 * 2^23 rounds to the
    // nearest integer, and leaves that integer in the low mantissa bits.
    let shifted = x + ROUND_MAGIC;
    let integer = shifted.to_bits().wrapping_sub(ROUND_MAGIC_BITS) as i32;
    let fraction = x - (shifted - ROUND_MAGIC);
    // 2^fraction as a polynomial, then the integer goes straight into the exponent bits.
    let poly = exp2_fraction(fraction);
    f32::from_bits(poly.to_bits().wrapping_add((integer << 23) as u32))
}

/// Base 2 logarithm of `x`, which must be positive. Accurate to about 2e-7 absolute.
#[inline]
pub fn log2(x: f32) -> f32 {
    // x = m * 2^e with m in [1, 2), then m is brought into [sqrt(1/2), sqrt(2)) so the series
    // below converges quickly.
    let bits = x.to_bits();
    let mut exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let mut mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if mantissa > std::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }
    // log2(m) = 2 / ln(2) * atanh(t) with t = (m - 1) / (m + 1), as a series in t
    let t = (mantissa - 1.0) / (mantissa + 1.0);
    let t2 = t * t;
    let series = t
        * (LOG2_C1 + t2 * (LOG2_C3 + t2 * (LOG2_C5 + t2 * (LOG2_C7 + t2 * LOG2_C9))));
    exponent as f32 + series
}

/// `base` raised to `exponent`, for a positive `base`; a `base` of zero or less returns 0.
/// Accurate to about 1e-6 relative for results within the normal `f32` range.
#[inline]
pub fn pow(base: f32, exponent: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    exp2(exponent * log2(base))
}

/// Sine of `phase` cycles (so a phase of 1 is a full turn). Accurate to about 1e-7.
#[inline]
pub fn sin_cycles(phase: f32) -> f32 {
    // Bring the phase into [-0.5, 0.5] and fold the outer quarters onto the inner ones, where
    // the odd polynomial applies.
    let shifted = phase + ROUND_MAGIC;
    let mut x = phase - (shifted - ROUND_MAGIC);
    if x > 0.25 {
        x = 0.5 - x;
    } else if x < -0.25 {
        x = -0.5 - x;
    }
    let x2 = x * x;
    x * (SIN_C1 + x2 * (SIN_C3 + x2 * (SIN_C5 + x2 * (SIN_C7 + x2 * SIN_C9))))
}

/// Cosine of `phase` cycles.
#[inline]
pub fn cos_cycles(phase: f32) -> f32 {
    // Reduced before the quarter turn is added, which would otherwise lose precision for
    // phases of many turns.
    let shifted = phase + ROUND_MAGIC;
    sin_cycles(phase - (shifted - ROUND_MAGIC) + 0.25)
}

/// Tangent of `phase` cycles. Accurate to about 3e-6 relative for phases up to 0.245 (just
/// below a quarter turn, where the tangent blows up).
#[inline]
pub fn tan_cycles(phase: f32) -> f32 {
    sin_cycles(phase) / cos_cycles(phase)
}

/// 1.5 * 2^23: adding it to a float of smaller magnitude rounds the float to the nearest
/// integer, with that integer in the low bits of the result.
const ROUND_MAGIC: f32 = 12_582_912.0;
const ROUND_MAGIC_BITS: u32 = 0x4b40_0000;

#[inline]
fn exp2_fraction(f: f32) -> f32 {
    let f2 = f * f;
    (f * EXP2_C1 + f2 * (EXP2_C2 + f * EXP2_C3) + f2 * f2 * (EXP2_C4 + f * EXP2_C5)) + EXP2_C0
}

// Minimax fit of 2^f on [-0.5, 0.5] with the constant term pinned to 1 (so that whole powers
// of two come out exact), relative error 9.1e-8. The first coefficient lands near ln(2), as
// the Taylor series would have it, but it is a fit, not the constant.
const EXP2_C0: f32 = 1.0;
#[allow(clippy::approx_constant)]
const EXP2_C1: f32 = 0.693_147;
const EXP2_C2: f32 = 0.240_222_42;
const EXP2_C3: f32 = 0.055_507_337;
const EXP2_C4: f32 = 0.009_671_513;
const EXP2_C5: f32 = 0.001_326_472_6;

// 2 / ln(2) * t^n / n, the atanh series
const LOG2_C1: f32 = 2.885_39;
const LOG2_C3: f32 = 0.961_796_7;
const LOG2_C5: f32 = 0.577_078_04;
const LOG2_C7: f32 = 0.412_198_57;
const LOG2_C9: f32 = 0.320_598_9;

// Minimax fit of sin(2 pi x) on [-0.25, 0.25], absolute error 3.3e-9. Like the 2^f fit, the
// first coefficient only happens to land near 2 pi.
#[allow(clippy::approx_constant)]
const SIN_C1: f32 = 6.283_185;
const SIN_C3: f32 = -41.341_655;
const SIN_C5: f32 = 81.601_004;
const SIN_C7: f32 = -76.549_78;
const SIN_C9: f32 = 39.536_706;

/// The same approximations for eight values at a time. They do the same operations in the same
/// order (with no fused multiply-adds), so every lane comes out exactly as the one-value
/// version would.
pub mod x8 {
    use wide::{bytemuck::cast, f32x8, i32x8, u32x8};

    use super::{
        EXP2_C0, EXP2_C1, EXP2_C2, EXP2_C3, EXP2_C4, EXP2_C5, LOG2_C1, LOG2_C3, LOG2_C5, LOG2_C7,
        LOG2_C9, ROUND_MAGIC, ROUND_MAGIC_BITS, SIN_C1, SIN_C3, SIN_C5, SIN_C7, SIN_C9,
    };

    #[inline(always)]
    fn splat(value: f32) -> f32x8 {
        f32x8::splat(value)
    }

    /// [`exp2()`][super::exp2()] of each lane.
    #[inline]
    pub fn exp2(x: f32x8) -> f32x8 {
        let underflow = x.simd_lt(splat(-126.0));
        // (`min()` only adds a check for a NaN bound)
        let x = x.fast_min(splat(127.0));
        let shifted = x + splat(ROUND_MAGIC);
        let integer = cast::<f32x8, u32x8>(shifted) - u32x8::splat(ROUND_MAGIC_BITS);
        let fraction = x - (shifted - splat(ROUND_MAGIC));
        let poly = exp2_fraction(fraction);
        let result = cast::<u32x8, f32x8>(cast::<f32x8, u32x8>(poly) + (integer << 23));
        underflow.select(f32x8::ZERO, result)
    }

    /// [`log2()`][super::log2()] of each lane.
    #[inline]
    pub fn log2(x: f32x8) -> f32x8 {
        let bits = cast::<f32x8, u32x8>(x);
        let exponent =
            cast::<u32x8, i32x8>((bits >> 23) & u32x8::splat(0xff)) - i32x8::splat(127);
        let mantissa =
            cast::<u32x8, f32x8>((bits & u32x8::splat(0x007f_ffff)) | u32x8::splat(0x3f80_0000));
        let big = mantissa.simd_gt(splat(std::f32::consts::SQRT_2));
        let mantissa = big.select(mantissa * splat(0.5), mantissa);
        // The mask's lanes are all ones, -1 as integers
        let exponent = exponent - cast::<f32x8, i32x8>(big);
        let t = (mantissa - splat(1.0)) / (mantissa + splat(1.0));
        let t2 = t * t;
        let series = t
            * (splat(LOG2_C1)
                + t2 * (splat(LOG2_C3)
                    + t2 * (splat(LOG2_C5) + t2 * (splat(LOG2_C7) + t2 * splat(LOG2_C9)))));
        exponent.round_float() + series
    }

    /// [`pow()`][super::pow()] of each lane.
    #[inline]
    pub fn pow(base: f32x8, exponent: f32x8) -> f32x8 {
        base.simd_gt(f32x8::ZERO).select(exp2(exponent * log2(base)), f32x8::ZERO)
    }

    /// [`sin_cycles()`][super::sin_cycles()] of each lane.
    #[inline]
    pub fn sin_cycles(phase: f32x8) -> f32x8 {
        let shifted = phase + splat(ROUND_MAGIC);
        let x = phase - (shifted - splat(ROUND_MAGIC));
        let x = x.simd_gt(splat(0.25)).select(splat(0.5) - x, x);
        let x = x.simd_lt(splat(-0.25)).select(splat(-0.5) - x, x);
        let x2 = x * x;
        x * (splat(SIN_C1)
            + x2 * (splat(SIN_C3) + x2 * (splat(SIN_C5) + x2 * (splat(SIN_C7) + x2 * splat(SIN_C9)))))
    }

    /// [`cos_cycles()`][super::cos_cycles()] of each lane.
    #[inline]
    pub fn cos_cycles(phase: f32x8) -> f32x8 {
        let shifted = phase + splat(ROUND_MAGIC);
        sin_cycles(phase - (shifted - splat(ROUND_MAGIC)) + splat(0.25))
    }

    /// [`tan_cycles()`][super::tan_cycles()] of each lane.
    #[inline]
    pub fn tan_cycles(phase: f32x8) -> f32x8 {
        sin_cycles(phase) / cos_cycles(phase)
    }

    #[inline]
    fn exp2_fraction(f: f32x8) -> f32x8 {
        let f2 = f * f;
        (f * splat(EXP2_C1)
            + f2 * (splat(EXP2_C2) + f * splat(EXP2_C3))
            + f2 * f2 * (splat(EXP2_C4) + f * splat(EXP2_C5)))
            + splat(EXP2_C0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks an eight lane function against its one-value version, to the bit, on the inputs
    /// eight at a time.
    fn assert_lanes_match(inputs: &[f32], lanes: impl Fn(wide::f32x8) -> wide::f32x8, one: impl Fn(f32) -> f32) {
        for chunk in inputs.chunks_exact(8) {
            let got = lanes(wide::f32x8::new(chunk.try_into().unwrap())).to_array();
            for (input, got) in chunk.iter().zip(got) {
                assert_eq!(got.to_bits(), one(*input).to_bits(), "{input}: {got} vs {}", one(*input));
            }
        }
    }

    #[test]
    fn eight_lane_versions_match_exactly() {
        let wide: Vec<f32> = (-13_000..13_000).map(|i| i as f32 / 97.0 + 0.000_31).collect();
        assert_lanes_match(&wide, x8::exp2, exp2);
        assert_lanes_match(&[-300.0, -127.0, -126.0, -125.9, 126.9, 127.0, 128.0, 300.0], x8::exp2, exp2);
        let positive: Vec<f32> = (1..=80_000).map(|i| i as f32 * 0.003_7).collect();
        assert_lanes_match(&positive, x8::log2, log2);
        let tiny: Vec<f32> = (0..64).map(|i| (i as f32 - 120.0).exp2() * 1.37).collect();
        assert_lanes_match(&tiny, x8::log2, log2);
        let phases: Vec<f32> = (-40_000..40_000).map(|i| i as f32 * 0.000_13).collect();
        assert_lanes_match(&phases, x8::sin_cycles, sin_cycles);
        assert_lanes_match(&phases, x8::cos_cycles, cos_cycles);
        let tangents: Vec<f32> = (0..24_496).map(|i| i as f32 * 0.000_01).collect();
        assert_lanes_match(&tangents[..24_496 / 8 * 8], x8::tan_cycles, tan_cycles);
        for exponent in [1.0 / 256.0, 0.3, 1.0, 7.3, 256.0] {
            let bases: Vec<f32> = (-8..1000).map(|i| i as f32 / 997.0).collect();
            let bases = &bases[..bases.len() / 8 * 8];
            assert_lanes_match(bases, |b| x8::pow(b, wide::f32x8::splat(exponent)), |b| pow(b, exponent));
        }
    }

    fn max_relative_error(inputs: impl Iterator<Item = f32>, f: impl Fn(f32) -> f32, g: impl Fn(f32) -> f32) -> f32 {
        inputs
            .map(|x| ((f(x) - g(x)) / g(x)).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn exp2_is_accurate() {
        let inputs = (-12_600..12_700).map(|i| i as f32 / 100.0 + 0.00123);
        let error = max_relative_error(inputs, exp2, f32::exp2);
        assert!(error < 2e-7, "{error}");
        // Exact at the integers, and sane at the edges
        for n in -126..=127 {
            assert_eq!(exp2(n as f32), (n as f32).exp2(), "{n}");
        }
        assert_eq!(exp2(-200.0), 0.0);
        assert_eq!(exp2(200.0), 127.0_f32.exp2());
    }

    #[test]
    fn log2_is_accurate() {
        let inputs = (1..=200_000).map(|i| i as f32 * 0.001);
        // Within about an ulp of the result (the result reaches 7.6)
        let error = inputs
            .map(|x| (log2(x) - x.log2()).abs())
            .fold(0.0, f32::max);
        assert!(error < 6e-7, "{error}");
        for n in -126..=127 {
            let x = (n as f32).exp2();
            assert_eq!(log2(x), n as f32, "{n}");
        }
        assert!((log2(1e-30) - 1e-30_f32.log2()).abs() < 1e-5);
        assert!((log2(1e30) - 1e30_f32.log2()).abs() < 1e-5);
    }

    #[test]
    fn pow_is_accurate() {
        // The envelope slopes raise levels in (0, 1] to powers between 2^-8 and 2^8
        for exponent in [1.0 / 256.0, 0.1, 0.5, 1.0, 2.0, 7.3, 64.0, 256.0] {
            for i in 1..=1000 {
                let base = i as f32 / 1000.0;
                let want = base.powf(exponent);
                let got = pow(base, exponent);
                let error = if want > 1e-30 {
                    ((got - want) / want).abs()
                } else {
                    (got - want).abs()
                };
                assert!(error < 2e-5, "{base}^{exponent}: {got} vs {want}");
            }
        }
        assert_eq!(pow(0.0, 0.5), 0.0);
        assert_eq!(pow(-1.0, 2.0), 0.0);
        assert_eq!(pow(1.0, 123.0), 1.0);
    }

    #[test]
    fn sine_and_tangent_are_accurate() {
        use std::f64::consts::PI;
        // The references are computed in double precision, since a single precision argument
        // to `sin()` is already off by more than the approximation.
        let error = (-40_000..=40_000)
            .map(|i| i as f32 * 0.000_1)
            .map(|phase| (sin_cycles(phase) - (2.0 * PI * phase as f64).sin() as f32).abs())
            .fold(0.0, f32::max);
        assert!(error < 3e-7, "{error}");
        let error = (-40_000..=40_000)
            .map(|i| i as f32 * 0.000_1)
            .map(|phase| (cos_cycles(phase) - (2.0 * PI * phase as f64).cos() as f32).abs())
            .fold(0.0, f32::max);
        assert!(error < 3e-7, "{error}");
        assert_eq!(sin_cycles(0.0), 0.0);
        assert_eq!(sin_cycles(0.5), 0.0);
        assert_eq!(sin_cycles(1.0), 0.0);
        assert!((sin_cycles(0.25) - 1.0).abs() < 1e-7);
        assert!((sin_cycles(-0.25) + 1.0).abs() < 1e-7);

        // The SVF's coefficient: tan(pi * cutoff / sample rate), cutoff up to 0.49 * sample rate
        for i in 1..=24_500 {
            let phase = i as f32 * 0.000_01;
            let want = (2.0 * PI * phase as f64).tan() as f32;
            let error = ((tan_cycles(phase) - want) / want).abs();
            assert!(error < 4e-6, "tan at {phase} cycles: {} vs {want}", tan_cycles(phase));
        }
    }
}
