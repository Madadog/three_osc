/*
 * Original (C) 2021 Janne Heikkarainen <janne808\at\radiofreerobotron.net>
 * at https://github.com/janne808/kocmoc-rack-modules
 *
 * Ported to Rust by Adam Godwin (evilspamalt/at/gmail.com)
 *
 * This program is free software; you can redistribute it and/or
 * modify it under the terms of the GNU General Public
 * License as published by the Free Software Foundation; either
 * version 3 of the License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * General Public License for more details.
 *
 * You should have received a copy of the GNU General Public
 * License along with this program (see COPYING); if not, write to the
 * Free Software Foundation, Inc., 51 Franklin Street, Fifth Floor,
 * Boston, MA 02110-1301 USA.
 *
 */

use crate::synth::filter::ladder::iir::IirFilter;
use crate::synth::filter::FilterType;

use super::Filter;

// pade 3/2 approximant for tanh
#[inline]
pub fn tanh_pade32_f32(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    // return approximant
    x * (15.0 + x.powi(2)) / (15.0 + 6.0 * x.powi(2))
}

// steepness of downsample filter response
const IIR_DOWNSAMPLE_ORDER: usize = 16;

// downsampling passthrough bandwidth
const IIR_DOWNSAMPLING_BANDWIDTH: f64 = 0.9;

// maximum newton-raphson iteration steps
const LADDER_MAX_NEWTON_STEPS: i32 = 8;

// check for newton-raphson breaking limit
const LADDER_NEWTON_BREAKING_LIMIT: i32 = 1;

/// Length of the [`NOISE`] table, a power of two.
const NOISE_LEN: usize = 4096;

/// The tiny amount of noise added to the input to keep the filter alive: uniformly distributed
/// pseudo-random values in [-1e-6, 1e-6), worked out at compile time. Each filter reads it from
/// its own position onwards, wrapping around. The table is padded with copies of its first
/// [`LADDER_LANES`] values, so that the lockstep kernel can read that many values in a row from
/// any position.
static NOISE: [f32; NOISE_LEN + LADDER_LANES] = noise_table();

const fn noise_table() -> [f32; NOISE_LEN + LADDER_LANES] {
    let mut table = [0.0; NOISE_LEN + LADDER_LANES];
    let mut state: u32 = 0x9e37_79b9;
    let mut i = 0;
    while i < NOISE_LEN {
        // xorshift32
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        // The top 24 bits as a number in [0, 1)
        let unit = (state >> 8) as f32 / (1 << 24) as f32;
        table[i] = 1.0e-6 * 2.0 * (unit - 0.5);
        i += 1;
    }
    while i < NOISE_LEN + LADDER_LANES {
        table[i] = table[i - NOISE_LEN];
        i += 1;
    }
    table
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum IntegrationMethod {
    EulerFullTanh,
    PredictorCorrectorFullTanh,
    PredictorCorrectorFeedbackTanh,
    TrapezoidalFeedbackTanh,
}

fn integration_rate(sample_rate: f64, oversampling_factor: i32, cutoff_frequency: f64) -> f64 {
    let dt = 44100.0 / (sample_rate * oversampling_factor as f64) * cutoff_frequency;
    dt.clamp(0.0, 0.6)
}

/// The Kocmoc ladder filter, 3x oversampled.
///
/// The filter runs in single precision, like the original's 32-bit build for the MetaModule:
/// that is twice as many filters per SIMD vector as double precision, and the divisions in its
/// tanh approximations are much cheaper. Its parameters and the resampling filters' design are
/// worked out in double precision.
#[derive(Debug, Clone)]
pub struct LadderFilter {
    cutoff_frequency: f64,
    pub resonance: f32,
    pub ladder_filter_mode: FilterType,
    sample_rate: f64,
    dt: f32,
    pub ladder_integration_method: IntegrationMethod,
    pub oversampling_factor: i32,
    decimator_order: usize,

    // filter state
    p0: f32,
    p1: f32,
    p2: f32,
    p3: f32,
    ut_1: f32,

    // filter output
    out: f32,

    // IIR downsampling filter
    iir_lowpass: IirFilter,
    /// Interpolates the input up to the oversampled rate: the input, zero-stuffed (and scaled
    /// by the oversampling factor), through a lowpass with the same design as the decimator.
    /// Without it the input would be a staircase, whose images around multiples of the sample
    /// rate the tanh stages mix down into the audio band.
    iir_upsampler: IirFilter,

    /// Where the filter reads the [`NOISE`] table next.
    noise_position: usize,
}

impl Default for LadderFilter {
    fn default() -> Self {
        Self {
            cutoff_frequency: 0.25,
            resonance: 0.5,
            ladder_filter_mode: FilterType::Lowpass,
            sample_rate: 44100.0,
            dt: integration_rate(44100.0, 3, 0.25) as f32,
            ladder_integration_method: IntegrationMethod::PredictorCorrectorFullTanh,
            oversampling_factor: 3,
            decimator_order: IIR_DOWNSAMPLE_ORDER,
            p0: 0.0,
            p1: 0.0,
            p2: 0.0,
            p3: 0.0,
            ut_1: 0.0,
            out: 0.0,
            iir_lowpass: IirFilter::new_lowpass(
                44100.0 * 3.0,
                IIR_DOWNSAMPLING_BANDWIDTH * 44100.0 / 2.0,
                IIR_DOWNSAMPLE_ORDER,
            ),
            iir_upsampler: IirFilter::new_lowpass(
                44100.0 * 3.0,
                IIR_DOWNSAMPLING_BANDWIDTH * 44100.0 / 2.0,
                IIR_DOWNSAMPLE_ORDER,
            ),
            // A random start in the noise table, so that the voices' noise isn't the same
            noise_position: fastrand::usize(..NOISE_LEN),
        }
    }
}
/// Number of filters [`LadderFilter::process_block_x8()`] runs in lockstep.
pub const LADDER_LANES: usize = 8;

/// The integration rate and resonance (as the filter keeps them, see
/// [`LadderFilter::coefficients8()`]) of every sample of a block, for running a filter whose
/// cutoff or resonance changes at every sample (see [`LadderFilter::process_samples()`]).
#[derive(Debug, Clone, Copy)]
pub struct LadderSamples<'a> {
    pub dt: &'a [f32],
    pub resonance: &'a [f32],
}

#[allow(dead_code)]
impl LadderFilter {
    pub fn new() -> Self {
        Self::default()
    }
    /// A filter whose dither noise is reproducible, for tests.
    #[cfg(test)]
    pub fn with_seed(seed: u64) -> Self {
        Self {
            noise_position: fastrand::Rng::with_seed(seed).usize(..NOISE_LEN),
            ..Self::default()
        }
    }
    /// The integration rate and the resonance the filter keeps for eight cutoffs and
    /// resonances, as [`set_params()`][Filter::set_params()] works them out (in single rather
    /// than double precision, which makes a difference of a few parts in ten million).
    pub fn coefficients8(&self, cutoff: wide::f32x8, resonance: wide::f32x8, sample_rate: f32) -> [wide::f32x8; 2] {
        use wide::f32x8;
        let cutoff_frequency = cutoff / f32x8::splat(7000.0) + f32x8::splat(0.0001);
        let scale = 44100.0 / (sample_rate * self.oversampling_factor as f32);
        let dt = (f32x8::splat(scale) * cutoff_frequency).fast_max(f32x8::ZERO).fast_min(f32x8::splat(0.6));
        [dt, resonance / f32x8::splat(16.6)]
    }
    /// Sets the integration rate and resonance directly, as
    /// [`process_samples()`][Self::process_samples()] takes them.
    pub fn set_coefficients(&mut self, dt: f32, resonance: f32) {
        self.dt = dt;
        self.resonance = resonance;
    }
    /// Runs the filter over a block of samples in place, with the integration rate and
    /// resonance given for every sample. The filter keeps the last sample's.
    pub fn process_samples(&mut self, samples: &mut [f32], coefficients: LadderSamples) {
        for (n, sample) in samples.iter_mut().enumerate() {
            self.dt = coefficients.dt[n];
            self.resonance = coefficients.resonance[n];
            *sample = self.process(*sample);
        }
    }
    /// Whether [`process_block_x8()`][Self::process_block_x8()] supports this filter's settings.
    pub fn supports_lockstep(&self) -> bool {
        matches!(
            self.ladder_integration_method,
            IntegrationMethod::PredictorCorrectorFullTanh
        )
    }
    /// Processes a block of samples, in place, for up to 8 filters at once, in lockstep with AVX:
    /// each lane of the SIMD vectors is a filter. Each present lane produces exactly the same
    /// output as calling [`process()`][Filter::process()] on its filter for each sample. The
    /// filters must all use the same integration method (see
    /// [`supports_lockstep()`][Self::supports_lockstep()]), oversampling factor, resampling
    /// filter design and filter mode; the first present lane's settings are used.
    ///
    /// # Safety
    ///
    /// The CPU must support AVX.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    pub unsafe fn process_block_x8(lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES]) {
        Self::process_lanes::<Separate, false>(lanes, &[None; LADDER_LANES])
    }
    /// [`process_block_x8()`][Self::process_block_x8()] with the integration rate and
    /// resonance of every sample given for each lane, with exactly the same results as
    /// [`process_samples()`][Self::process_samples()].
    ///
    /// # Safety
    ///
    /// The CPU must support AVX.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    pub unsafe fn process_block_x8_samples(
        lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES],
        coefficients: &[Option<LadderSamples>; LADDER_LANES],
    ) {
        Self::process_lanes::<Separate, true>(lanes, coefficients)
    }
    /// [`process_block_x8()`][Self::process_block_x8()] with fused multiply-adds, which are
    /// fewer instructions and shorten the chains of dependent operations. Rounding once rather
    /// than twice, its output differs from the one-filter path's by rounding errors.
    ///
    /// # Safety
    ///
    /// The CPU must support AVX2 and FMA.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn process_block_x8_fma(lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES]) {
        Self::process_lanes::<Fused, false>(lanes, &[None; LADDER_LANES])
    }
    /// [`process_block_x8_samples()`][Self::process_block_x8_samples()] with fused
    /// multiply-adds, see [`process_block_x8_fma()`][Self::process_block_x8_fma()].
    ///
    /// # Safety
    ///
    /// The CPU must support AVX2 and FMA.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn process_block_x8_samples_fma(
        lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES],
        coefficients: &[Option<LadderSamples>; LADDER_LANES],
    ) {
        Self::process_lanes::<Fused, true>(lanes, coefficients)
    }
    /// The lockstep kernel, with its multiply-adds done by `M`, and with the integration rate
    /// and resonance of every sample given in `samples` with `PER_SAMPLE` (otherwise the
    /// filters' own). Inlined into the functions above, which enable the instructions it needs.
    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    unsafe fn process_lanes<M: MulAdd, const PER_SAMPLE: bool>(
        mut lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES],
        samples: &[Option<LadderSamples>; LADDER_LANES],
    ) {
        use std::arch::x86_64::*;

        use super::simd::transpose8;

        let Some(first) = lanes.iter().find_map(|lane| lane.as_ref().map(|(filter, _)| &**filter))
        else {
            return;
        };
        debug_assert!(first.supports_lockstep());
        let oversampling_factor = first.oversampling_factor;
        let mode = first.ladder_filter_mode;
        let decimate = oversampling_factor > 1;
        let interpolate = first.interpolates();
        let biquads = first.iir_lowpass.decimator_order / 2;
        let coefficients = CascadeCoefficients::new(&first.iir_lowpass);
        let block_len = lanes
            .iter()
            .flatten()
            .map(|(_, buffer)| buffer.len())
            .min()
            .unwrap_or(0);

        // The state of each lane, transposed into one vector per value. Empty lanes are zero.
        let rows = std::array::from_fn(|lane| match &lanes[lane] {
            Some((filter, _)) => {
                let (f, resonance) = (filter, 8.0 * filter.resonance);
                _mm256_setr_ps(f.p0, f.p1, f.p2, f.p3, f.ut_1, f.dt, resonance, f.out)
            }
            None => _mm256_setzero_ps(),
        });
        // (`out` is the last output, before the output gain)
        let [mut p0, mut p1, mut p2, mut p3, mut ut_1, mut dt, mut feedback, mut out] = transpose8(rows);
        // The resampling filters' delays the cascades use, transposed eight at a time
        let delay_groups = (2 * biquads).div_ceil(LADDER_LANES);
        let mut z = [_mm256_setzero_ps(); IIR_MAX_ORDER];
        let mut zu = [_mm256_setzero_ps(); IIR_MAX_ORDER];
        for group in 0..delay_groups {
            let at = group * LADDER_LANES;
            let rows = |delays: fn(&LadderFilter) -> &[f32; IIR_MAX_ORDER]| {
                std::array::from_fn(|lane| match &lanes[lane] {
                    Some((filter, _)) => _mm256_loadu_ps(delays(filter)[at..at + LADDER_LANES].as_ptr()),
                    None => _mm256_setzero_ps(),
                })
            };
            z[at..at + LADDER_LANES].copy_from_slice(&transpose8(rows(|filter| &filter.iir_lowpass.z)));
            zu[at..at + LADDER_LANES].copy_from_slice(&transpose8(rows(|filter| &filter.iir_upsampler.z)));
        }
        let mut half_dt = _mm256_mul_ps(_mm256_set1_ps(0.5), dt);
        let upsampling_gain = _mm256_set1_ps(oversampling_factor as f32);

        // Eight values of every lane from `start` on (fewer at the end of the block, padded
        // with zeros), transposed into one vector per sample, for the coefficients given per
        // sample. Missing lanes are zero.
        let transposed = |resonance: bool, start: usize, len: usize| {
            let rows = std::array::from_fn(|lane| match &samples[lane] {
                Some(samples) => {
                    let values = if resonance { samples.resonance } else { samples.dt };
                    let values = &values[start..start + len];
                    let mut padded = [0.0_f32; LADDER_LANES];
                    padded[..len].copy_from_slice(values);
                    _mm256_loadu_ps(padded.as_ptr())
                }
                None => _mm256_setzero_ps(),
            });
            transpose8(rows)
        };

        // One sample through a resampling filter's cascade. The synth's design has a version
        // of its own, unrolled, which saves the loop's bookkeeping in every biquad.
        let cascade = |z: &mut [__m256; IIR_MAX_ORDER], input: __m256| {
            if biquads == IIR_DOWNSAMPLE_ORDER / 2 {
                cascade_x8::<{ IIR_DOWNSAMPLE_ORDER / 2 }, M>(z, &coefficients, input, biquads)
            } else {
                cascade_x8::<0, M>(z, &coefficients, input, biquads)
            }
        };

        // Padé 3/2 approximant of tanh, with the same results as `tanh_pade32_f32()`. The
        // square of the clamped input is the square of the input clamped to 9, which is
        // exactly the same number but available sooner: this is on the filter's critical path.
        let tanh = |x: __m256| {
            let clamped = _mm256_max_ps(_mm256_min_ps(x, _mm256_set1_ps(3.0)), _mm256_set1_ps(-3.0));
            let x2 = _mm256_min_ps(_mm256_mul_ps(x, x), _mm256_set1_ps(9.0));
            _mm256_div_ps(
                _mm256_mul_ps(clamped, _mm256_add_ps(_mm256_set1_ps(15.0), x2)),
                M::mul_add(_mm256_set1_ps(6.0), x2, _mm256_set1_ps(15.0)),
            )
        };
        // The predictor's step `p + dt (a - b)` and the corrector's
        let predict = |dt: __m256, p: __m256, a: __m256, b: __m256| M::mul_add(dt, _mm256_sub_ps(a, b), p);
        let correct = |half_dt: __m256, p: __m256, a: __m256, b: __m256, a_prime: __m256, b_prime: __m256| {
            M::mul_add(
                half_dt,
                _mm256_add_ps(_mm256_sub_ps(a, b), _mm256_sub_ps(a_prime, b_prime)),
                p,
            )
        };

        let mut start = 0;
        while start < block_len {
            // Eight samples of every lane (fewer at the end of the block, padded with zeros),
            // with the tiny amount of noise that keeps the filter alive, transposed into one
            // vector per sample. Empty lanes are zero.
            let len = LADDER_LANES.min(block_len - start);
            let mut rows = [_mm256_setzero_ps(); LADDER_LANES];
            for (row, entry) in rows.iter_mut().zip(lanes.iter_mut()) {
                if let Some((filter, buffer)) = entry {
                    let samples = &buffer[start..start + len];
                    let input = if len == LADDER_LANES {
                        _mm256_loadu_ps(samples.as_ptr())
                    } else {
                        let mut padded = [0.0_f32; LADDER_LANES];
                        padded[..len].copy_from_slice(samples);
                        _mm256_loadu_ps(padded.as_ptr())
                    };
                    let position = filter.noise_position;
                    let noise = &NOISE[position..position + LADDER_LANES];
                    *row = _mm256_add_ps(input, _mm256_loadu_ps(noise.as_ptr()));
                    filter.noise_position = (position + len) % NOISE_LEN;
                }
            }
            let mut frames = transpose8(rows);
            // The integration rate and feedback (eight times the resonance, as the one-filter
            // path works it out) of each sample, when they are given per sample
            let (dts, resonances) = if PER_SAMPLE {
                (transposed(false, start, len), transposed(true, start, len))
            } else {
                ([dt; LADDER_LANES], [feedback; LADDER_LANES])
            };

            for (n, frame) in frames[..len].iter_mut().enumerate() {
                let input_sample = *frame;
                if PER_SAMPLE {
                    dt = dts[n];
                    half_dt = _mm256_mul_ps(_mm256_set1_ps(0.5), dt);
                    feedback = _mm256_mul_ps(_mm256_set1_ps(8.0), resonances[n]);
                }

                // Predictor-corrector integration with full tanh stages, see `process_sample()`
                for step in 0..oversampling_factor {
                    // The input at this oversampled step: interpolated from the zero-stuffed input,
                    // or held
                    let input = if interpolate {
                        let stuffed = if step == 0 {
                            _mm256_mul_ps(input_sample, upsampling_gain)
                        } else {
                            _mm256_setzero_ps()
                        };
                        cascade(&mut zu, stuffed)
                    } else {
                        input_sample
                    };

                    let tanh_in = tanh(M::neg_mul_add(feedback, p3, ut_1));
                    let tanh_p0 = tanh(p0);
                    let tanh_p1 = tanh(p1);
                    let tanh_p2 = tanh(p2);
                    let tanh_p3 = tanh(p3);

                    let p0_prime = predict(dt, p0, tanh_in, tanh_p0);
                    let p1_prime = predict(dt, p1, tanh_p0, tanh_p1);
                    let p2_prime = predict(dt, p2, tanh_p1, tanh_p2);
                    let p3_prime = predict(dt, p3, tanh_p2, tanh_p3);
                    let tanh_p0_prime = tanh(p0_prime);
                    let tanh_p1_prime = tanh(p1_prime);
                    let tanh_p2_prime = tanh(p2_prime);
                    let tanh_p3_prime = tanh(p3_prime);

                    p3 = correct(half_dt, p3, tanh_p2, tanh_p3, tanh_p2_prime, tanh_p3_prime);
                    p2 = correct(half_dt, p2, tanh_p1, tanh_p2, tanh_p1_prime, tanh_p2_prime);
                    p1 = correct(half_dt, p1, tanh_p0, tanh_p1, tanh_p0_prime, tanh_p1_prime);
                    let tanh_new_in = tanh(M::neg_mul_add(feedback, p3, input));
                    p0 = correct(half_dt, p0, tanh_in, tanh_p0, tanh_new_in, tanh_p0_prime);

                    // The input at the previous oversampled step
                    ut_1 = input;

                    out = match mode {
                        FilterType::Lowpass => p3,
                        FilterType::Bandpass => _mm256_sub_ps(p1, p3),
                        FilterType::Highpass => tanh(M::neg_mul_add(feedback, p3, _mm256_sub_ps(input, p0))),
                    };

                    if decimate {
                        // Every oversampled output goes through the decimation filter
                        out = cascade(&mut z, out);
                    }
                }

                *frame = _mm256_mul_ps(out, _mm256_set1_ps(1.8));
            }

            let rows = transpose8(frames);
            for (row, entry) in rows.iter().zip(lanes.iter_mut()) {
                if let Some((_, buffer)) = entry {
                    let samples = &mut buffer[start..start + len];
                    if len == LADDER_LANES {
                        _mm256_storeu_ps(samples.as_mut_ptr(), *row);
                    } else {
                        let mut padded = [0.0_f32; LADDER_LANES];
                        _mm256_storeu_ps(padded.as_mut_ptr(), *row);
                        samples.copy_from_slice(&padded[..len]);
                    }
                }
            }
            start += len;
        }

        // Transpose the state back into the filters
        let rows = transpose8([p0, p1, p2, p3, ut_1, dt, feedback, out]);
        for (row, entry) in rows.iter().zip(lanes.iter_mut()) {
            if let Some((filter, _)) = entry {
                let mut values = [0.0_f32; LADDER_LANES];
                _mm256_storeu_ps(values.as_mut_ptr(), *row);
                [filter.p0, filter.p1, filter.p2, filter.p3, filter.ut_1] =
                    [values[0], values[1], values[2], values[3], values[4]];
                filter.out = values[7];
            }
        }
        if PER_SAMPLE && block_len > 0 {
            // The filters keep the last sample's integration rate and resonance
            for (entry, samples) in lanes.iter_mut().zip(samples) {
                if let (Some((filter, _)), Some(samples)) = (entry, samples) {
                    filter.dt = samples.dt[block_len - 1];
                    filter.resonance = samples.resonance[block_len - 1];
                }
            }
        }
        for group in 0..delay_groups {
            let at = group * LADDER_LANES;
            let lowpass = transpose8(z[at..at + LADDER_LANES].try_into().unwrap());
            let upsampler = transpose8(zu[at..at + LADDER_LANES].try_into().unwrap());
            for (lane, entry) in lanes.iter_mut().enumerate() {
                if let Some((filter, _)) = entry {
                    let delays = &mut filter.iir_lowpass.z[at..at + LADDER_LANES];
                    _mm256_storeu_ps(delays.as_mut_ptr(), lowpass[lane]);
                    let delays = &mut filter.iir_upsampler.z[at..at + LADDER_LANES];
                    _mm256_storeu_ps(delays.as_mut_ptr(), upsampler[lane]);
                }
            }
        }
    }
    pub fn process_sample(&mut self, input: f32) {
        let feedback = 8.0 * self.resonance;
        let noise = NOISE[self.noise_position];
        self.noise_position = (self.noise_position + 1) % NOISE_LEN;
        let input_sample = input + noise;
        let interpolate = self.interpolates();
        let upsampling_gain = self.oversampling_factor as f32;

        // integrate filter state
        // with oversampling
        for step in 0..self.oversampling_factor {
            // The input at this oversampled step: interpolated from the zero-stuffed input,
            // or held
            let input = if interpolate {
                let stuffed = if step == 0 {
                    input_sample * upsampling_gain
                } else {
                    0.0
                };
                self.iir_upsampler.filter(stuffed)
            } else {
                input_sample
            };

            match self.ladder_integration_method {
                // semi-implicit euler integration
                // with full tanh stages
                IntegrationMethod::EulerFullTanh => {
                    self.p0 = self.p0
                        + self.dt
                            * (tanh_pade32_f32(input - feedback * self.p3) - tanh_pade32_f32(self.p0));
                    self.p1 = self.p1 + self.dt * (tanh_pade32_f32(self.p0) - tanh_pade32_f32(self.p1));
                    self.p2 = self.p2 + self.dt * (tanh_pade32_f32(self.p1) - tanh_pade32_f32(self.p2));
                    self.p3 = self.p3 + self.dt * (tanh_pade32_f32(self.p2) - tanh_pade32_f32(self.p3));
                }
                // predictor-corrector integration
                // with full tanh stages
                IntegrationMethod::PredictorCorrectorFullTanh => {
                    // The stages' tanh values are shared by the predictor and the corrector,
                    // which only needs the predicted stages' values on top of them.
                    let tanh_in = tanh_pade32_f32(self.ut_1 - feedback * self.p3);
                    let tanh_p0 = tanh_pade32_f32(self.p0);
                    let tanh_p1 = tanh_pade32_f32(self.p1);
                    let tanh_p2 = tanh_pade32_f32(self.p2);
                    let tanh_p3 = tanh_pade32_f32(self.p3);

                    // predictor
                    let p0_prime = self.p0 + self.dt * (tanh_in - tanh_p0);
                    let p1_prime = self.p1 + self.dt * (tanh_p0 - tanh_p1);
                    let p2_prime = self.p2 + self.dt * (tanh_p1 - tanh_p2);
                    let p3_prime = self.p3 + self.dt * (tanh_p2 - tanh_p3);
                    let tanh_p0_prime = tanh_pade32_f32(p0_prime);
                    let tanh_p1_prime = tanh_pade32_f32(p1_prime);
                    let tanh_p2_prime = tanh_pade32_f32(p2_prime);
                    let tanh_p3_prime = tanh_pade32_f32(p3_prime);

                    // corrector
                    self.p3 = self.p3
                        + 0.5 * self.dt * ((tanh_p2 - tanh_p3) + (tanh_p2_prime - tanh_p3_prime));
                    self.p2 = self.p2
                        + 0.5 * self.dt * ((tanh_p1 - tanh_p2) + (tanh_p1_prime - tanh_p2_prime));
                    self.p1 = self.p1
                        + 0.5 * self.dt * ((tanh_p0 - tanh_p1) + (tanh_p0_prime - tanh_p1_prime));
                    self.p0 = self.p0
                        + 0.5
                            * self.dt
                            * ((tanh_in - tanh_p0)
                                + (tanh_pade32_f32(input - feedback * self.p3) - tanh_p0_prime));
                }
                // predictor-corrector integration
                // with feedback tanh stage only
                IntegrationMethod::PredictorCorrectorFeedbackTanh => {
                    let tanh_in = tanh_pade32_f32(self.ut_1 - feedback * self.p3);

                    // predictor
                    let p0_prime = self.p0 + self.dt * (tanh_in - self.p0);
                    let p1_prime = self.p1 + self.dt * (self.p0 - self.p1);
                    let p2_prime = self.p2 + self.dt * (self.p1 - self.p2);
                    let p3_prime = self.p3 + self.dt * (self.p2 - self.p3);

                    // corrector
                    self.p3 =
                        self.p3 + 0.5 * self.dt * ((self.p2 - self.p3) + (p2_prime - p3_prime));
                    self.p2 =
                        self.p2 + 0.5 * self.dt * ((self.p1 - self.p2) + (p1_prime - p2_prime));
                    self.p1 =
                        self.p1 + 0.5 * self.dt * ((self.p0 - self.p1) + (p0_prime - p1_prime));
                    self.p0 = self.p0
                        + 0.5
                            * self.dt
                            * ((tanh_in - self.p0)
                                + (tanh_pade32_f32(input - feedback * self.p3) - p0_prime));
                }
                // implicit trapezoidal integration
                // with feedback tanh stage only
                IntegrationMethod::TrapezoidalFeedbackTanh => {
                    let ut = tanh_pade32_f32(self.ut_1 - feedback * self.p3);
                    let b = (0.5 * self.dt) / (1.0 + 0.5 * self.dt);
                    let c = (1.0 - 0.5 * self.dt) / (1.0 + 0.5 * self.dt);
                    let g = -feedback * b.powi(4);
                    let mut x_k = ut;
                    let d_t = c * self.p3
                        + (b + c * b) * self.p2
                        + (b.powi(2) + b.powi(2) * c) * self.p1
                        + (b.powi(3) + b.powi(3) * c) * self.p0
                        + b.powi(4) * ut;
                    let c_t = tanh_pade32_f32(input - feedback * d_t);

                    for _ in 0..LADDER_MAX_NEWTON_STEPS {
                        let tanh_g_xk = tanh_pade32_f32(g * x_k);
                        let tanh_g_xk2 = g * (1.0 - tanh_pade32_f32(g * x_k) * tanh_pade32_f32(g * x_k));

                        let x_k2 = x_k
                            - (x_k + x_k * tanh_g_xk * c_t - tanh_g_xk - c_t)
                                / (1.0 + c_t * (tanh_g_xk + x_k * tanh_g_xk2) - tanh_g_xk2);

                        if LADDER_NEWTON_BREAKING_LIMIT > 0 && (x_k2 - x_k).abs() < 1.0e-9 {
                            x_k = x_k2;
                            break;
                        }
                        x_k = x_k2;
                    }

                    let ut_2 = x_k;

                    let p0_prime = self.p0;
                    let p1_prime = self.p1;
                    let p2_prime = self.p2;
                    let p3_prime = self.p3;

                    self.p0 = c * p0_prime + b * (ut + ut_2);
                    self.p1 = c * p1_prime + b * (p0_prime + self.p0);
                    self.p2 = c * p2_prime + b * (p1_prime + self.p1);
                    self.p3 = c * p3_prime + b * (p2_prime + self.p2);
                }
            }

            // The input at the previous oversampled step
            self.ut_1 = input;

            self.out = match self.ladder_filter_mode {
                FilterType::Lowpass => self.p3,
                FilterType::Bandpass => self.p1 - self.p3,
                FilterType::Highpass => tanh_pade32_f32(input - self.p0 - feedback * self.p3),
                #[allow(unreachable_patterns)]
                _ => 0.0,
            };

            // Every oversampled output goes through the decimation filter, and the last one
            // is kept: decimation.
            if self.oversampling_factor > 1 {
                self.out = self.iir_lowpass.filter(self.out);
            }
        }
    }
    pub fn output(&self) -> f32 {
        self.out
    }
    fn set_integration_rate(&mut self) {
        self.dt = integration_rate(
            self.sample_rate,
            self.oversampling_factor,
            self.cutoff_frequency,
        ) as f32;
    }
    pub fn set_cutoff(&mut self, cutoff: f64) {
        self.cutoff_frequency = cutoff;
        self.set_integration_rate()
    }
    pub fn set_resonance(&mut self, resonance: f32) {
        self.resonance = resonance;
    }
    pub fn set_filtermode(&mut self, mode: FilterType) {
        self.ladder_filter_mode = mode;
    }
    /// Sets the order of the decimation filter (0 disables it, so the oversampled signal is
    /// decimated without filtering).
    pub fn set_decimator_order(&mut self, order: usize) {
        self.decimator_order = order.min(IIR_MAX_ORDER);
        self.update_decimator();
    }
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.update_decimator();
        self.set_integration_rate();
    }
    /// Designs the resampling filters for the current sample rate, oversampling factor and
    /// order, if any of them changed: lowpasses running at the oversampled rate that remove
    /// everything above 90% of the output's Nyquist frequency. The upsampler smooths the input
    /// into the oversampled rate, and the decimator removes the distortion the filter
    /// generates above the output's Nyquist frequency before the signal is decimated, so that
    /// it doesn't alias. Redesigning clears their state, so it is only done when the design
    /// changed (this is called for every control rate block): at another rate, the same
    /// design (relative to the rate) keeps its coefficients and its state.
    fn update_decimator(&mut self) {
        let rate = self.sample_rate * self.oversampling_factor as f64;
        let cutoff = IIR_DOWNSAMPLING_BANDWIDTH * self.sample_rate / 2.0;
        let iir = &self.iir_lowpass;
        if iir.sample_rate == rate && iir.cutoff_frequency == cutoff && iir.decimator_order == self.decimator_order {
            return;
        }
        if iir.decimator_order == self.decimator_order && iir.cutoff_frequency / iir.sample_rate == cutoff / rate {
            // The same filters at another rate (when the voices' oversampling changes): the
            // coefficients are the same, so they carry on as they are
            for iir in [&mut self.iir_lowpass, &mut self.iir_upsampler] {
                iir.sample_rate = rate;
                iir.cutoff_frequency = cutoff;
            }
        } else {
            self.iir_lowpass.redesign(rate, cutoff, self.decimator_order);
            self.iir_upsampler.redesign(rate, cutoff, self.decimator_order);
        }
    }
    /// Whether the input is interpolated to the oversampled rate (see `iir_upsampler`). It is
    /// held at each sample instead when there is no oversampling, or no resampling filter.
    fn interpolates(&self) -> bool {
        self.oversampling_factor > 1 && self.iir_upsampler.decimator_order >= 2
    }

}

impl Filter for LadderFilter {
    fn process(&mut self, input: f32) -> f32 {
        self.process_sample(input);
        self.output() * 1.8
    }

    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        // The original module expected a cutoff frequency from 0.001 -> 2.25.
        // 22000 / 2.25 = 9800.0
        // ... which is consistently out of tune for some reason.
        // rough retune: 9800.0 / 1.4 = 7000.00
        // Retune makes pitch accurate when resonance == 0.5
        self.cutoff_frequency = (cutoff as f64 / 7000.0) + 0.0001;
        // Self-resonance starts at resonance >= 0.5
        self.resonance = resonance / 16.6;
        self.set_sample_rate(sample_rate as f64);
    }

    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.set_filtermode(filter_type);
    }
}

use self::iir::IIR_MAX_ORDER;

/// The multiplications followed by additions in the lockstep kernel: either rounded after
/// each operation, exactly like the one-filter path, or fused (FMA), which is fewer
/// instructions and rounds once.
#[cfg(target_arch = "x86_64")]
trait MulAdd {
    /// `a * b + c`
    unsafe fn mul_add(a: std::arch::x86_64::__m256, b: std::arch::x86_64::__m256, c: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256;
    /// `c - a * b`
    unsafe fn neg_mul_add(a: std::arch::x86_64::__m256, b: std::arch::x86_64::__m256, c: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256;
}

/// [`MulAdd`] as a multiplication and an addition (AVX).
#[cfg(target_arch = "x86_64")]
struct Separate;

#[cfg(target_arch = "x86_64")]
impl MulAdd for Separate {
    #[inline(always)]
    unsafe fn mul_add(a: std::arch::x86_64::__m256, b: std::arch::x86_64::__m256, c: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256 {
        use std::arch::x86_64::*;
        _mm256_add_ps(_mm256_mul_ps(a, b), c)
    }
    #[inline(always)]
    unsafe fn neg_mul_add(a: std::arch::x86_64::__m256, b: std::arch::x86_64::__m256, c: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256 {
        use std::arch::x86_64::*;
        _mm256_sub_ps(c, _mm256_mul_ps(a, b))
    }
}

/// [`MulAdd`] as fused multiply-adds (FMA).
#[cfg(target_arch = "x86_64")]
struct Fused;

#[cfg(target_arch = "x86_64")]
impl MulAdd for Fused {
    #[inline(always)]
    unsafe fn mul_add(a: std::arch::x86_64::__m256, b: std::arch::x86_64::__m256, c: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256 {
        std::arch::x86_64::_mm256_fmadd_ps(a, b, c)
    }
    #[inline(always)]
    unsafe fn neg_mul_add(a: std::arch::x86_64::__m256, b: std::arch::x86_64::__m256, c: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256 {
        std::arch::x86_64::_mm256_fnmadd_ps(a, b, c)
    }
}

/// A resampling filter's coefficients for [`cascade_x8()`], each broadcast to a vector.
#[cfg(target_arch = "x86_64")]
struct CascadeCoefficients {
    a1: [std::arch::x86_64::__m256; IIR_MAX_ORDER / 2],
    a2: [std::arch::x86_64::__m256; IIR_MAX_ORDER / 2],
    b1: [std::arch::x86_64::__m256; IIR_MAX_ORDER / 2],
    b2: [std::arch::x86_64::__m256; IIR_MAX_ORDER / 2],
    gain: std::arch::x86_64::__m256,
}

#[cfg(target_arch = "x86_64")]
impl CascadeCoefficients {
    #[target_feature(enable = "avx")]
    unsafe fn new(iir: &IirFilter) -> Self {
        use std::arch::x86_64::*;
        let broadcast = |index: usize| std::array::from_fn(|i| _mm256_set1_ps(iir.run[i][index]));
        Self {
            a1: broadcast(0),
            a2: broadcast(1),
            b1: broadcast(2),
            b2: broadcast(3),
            gain: _mm256_set1_ps(iir.gain),
        }
    }
}

/// One sample of eight filters through the first `biquads` biquads of a resampling filter's
/// cascade, with the operations of [`IirFilter::filter()`] (exactly, with [`Separate`]). A
/// `FIXED` above zero is the number of biquads instead, known at compile time, so that the loop
/// unrolls.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn cascade_x8<const FIXED: usize, M: MulAdd>(
    z: &mut [std::arch::x86_64::__m256; IIR_MAX_ORDER],
    coefficients: &CascadeCoefficients,
    mut out: std::arch::x86_64::__m256,
    biquads: usize,
) -> std::arch::x86_64::__m256 {
    use std::arch::x86_64::*;
    let biquads = if FIXED > 0 { FIXED } else { biquads };
    for biquad in 0..biquads {
        let (z0, z1) = (z[2 * biquad], z[2 * biquad + 1]);
        let recursive = M::mul_add(coefficients.a1[biquad], z0, _mm256_mul_ps(coefficients.a2[biquad], z1));
        let feedforward = M::mul_add(coefficients.b1[biquad], z0, _mm256_mul_ps(coefficients.b2[biquad], z1));
        let biquad_in = _mm256_sub_ps(out, recursive);
        out = _mm256_add_ps(out, feedforward);
        z[2 * biquad + 1] = z0;
        z[2 * biquad] = biquad_in;
    }
    _mm256_mul_ps(out, coefficients.gain)
}

mod iir {
    use std::f64::consts::PI;

    use itertools::izip;

    pub const IIR_MAX_ORDER: usize = 32;

    #[derive(Debug, Clone)]
    pub struct IirFilter {
        // filter design variables
        pub cutoff_frequency: f64,
        pub sample_rate: f64,
        pub decimator_order: usize,

        // dsp variables
        pub a1: [f64; IIR_MAX_ORDER / 2],
        pub a2: [f64; IIR_MAX_ORDER / 2],
        pub k: [f64; IIR_MAX_ORDER / 2],
        pa_real: [f64; IIR_MAX_ORDER / 2],
        pa_imag: [f64; IIR_MAX_ORDER / 2],
        p_real: [f64; IIR_MAX_ORDER / 2],
        p_imag: [f64; IIR_MAX_ORDER / 2],

        /// The coefficients the cascade runs with, in single precision, per biquad: `a1`, `a2`,
        /// `2 - a1` and `1 - a2` (see [`filter()`][Self::filter()]).
        pub run: [[f32; 4]; IIR_MAX_ORDER / 2],
        /// The product of the biquads' gains `k`, applied once at the end of the cascade.
        pub gain: f32,

        // cascaded biquad buffers
        pub z: [f32; IIR_MAX_ORDER],
    }
    #[allow(dead_code)]
    impl IirFilter {
        pub fn new_lowpass(
            sample_rate: f64,
            cutoff_frequency: f64,
            decimator_order: usize,
        ) -> Self {
            let mut iir = Self {
                cutoff_frequency,
                sample_rate,
                decimator_order,

                a1: [0.0; IIR_MAX_ORDER / 2],
                a2: [0.0; IIR_MAX_ORDER / 2],
                k: [0.0; IIR_MAX_ORDER / 2],
                pa_real: [0.0; IIR_MAX_ORDER / 2],
                pa_imag: [0.0; IIR_MAX_ORDER / 2],
                p_real: [0.0; IIR_MAX_ORDER / 2],
                p_imag: [0.0; IIR_MAX_ORDER / 2],

                run: [[0.0; 4]; IIR_MAX_ORDER / 2],
                gain: 1.0,
                z: [0.0; IIR_MAX_ORDER],
            };
            iir.compute_coefficients();
            iir
        }
        /// Runs one sample through the cascade.
        ///
        /// Each biquad is `w = k x - a1 z0 - a2 z1` and `y = w + 2 z0 + z1`. The gains `k` are
        /// taken out of the biquads and applied together at the end, which scales each biquad's
        /// state by the gains before it but gives the same output, and `y` is rearranged as
        /// `y = x + ((2 - a1) z0 + (1 - a2) z1)`, so that only an addition stands between a
        /// stage's input and its output: the sum of the delay terms doesn't depend on the input,
        /// so it is computed while the previous stage is still busy. The lockstep ladder (see
        /// `LadderFilter::process_block_x8()`) uses the same operations in the same order.
        pub fn filter(&mut self, input: f32) -> f32 {
            let mut out = input;

            for (&[a1, a2, b1, b2], z) in self
                .run
                .iter()
                .zip(self.z.chunks_exact_mut(2))
                .take(self.decimator_order / 2)
            {
                let (z0, z1) = (z[0], z[1]);
                let recursive = a1 * z0 + a2 * z1;
                let feedforward = b1 * z0 + b2 * z1;
                let biquad_in = out - recursive;
                out += feedforward;

                // update delays
                z[1] = z0;
                z[0] = biquad_in;
            }

            out * self.gain
        }

        pub fn set_order(&mut self, order: usize) {
            self.decimator_order = order.clamp(0, IIR_MAX_ORDER);
            self.initialize_biquad_cascade();
            self.compute_coefficients();
        }
        /// Sets the sample rate, cutoff and order at once, with a single recomputation of the
        /// coefficients. Clears the filter's state.
        pub fn redesign(&mut self, sample_rate: f64, cutoff_frequency: f64, order: usize) {
            self.sample_rate = sample_rate;
            self.cutoff_frequency = cutoff_frequency;
            self.decimator_order = order.clamp(0, IIR_MAX_ORDER);
            self.initialize_biquad_cascade();
            self.compute_coefficients();
        }
        pub fn set_sample_rate(&mut self, sample_rate: f64) {
            self.sample_rate = sample_rate;
            self.initialize_biquad_cascade();
            self.compute_coefficients();
        }
        pub fn set_cutoff(&mut self, cutoff: f64) {
            self.cutoff_frequency = cutoff;
            self.initialize_biquad_cascade();
            self.compute_coefficients();
        }

        pub fn initialize_biquad_cascade(&mut self) {
            for i in self.z.iter_mut() {
                *i = 0.0;
            }
        }

        fn compute_coefficients(&mut self) {
            // place butterworth style analog filter poles

            for ii in 0..self.decimator_order / 2 {
                let k = self.decimator_order / 2 - ii;
                let theta = (2.0 * k as f64 - 1.0) * PI / (2.0 * self.decimator_order as f64);

                self.pa_real[ii] = -1.0 * theta.sin();
                self.pa_imag[ii] = theta.cos();
            }

            // prewarp and scale poles
            let fc = self.sample_rate / PI * (PI * self.cutoff_frequency / self.sample_rate).tan();
            for (pa_real, pa_imag) in self
                .pa_real
                .iter_mut()
                .zip(self.pa_imag.iter_mut())
                .take(self.decimator_order / 2)
            {
                *pa_real *= 2.0 * PI * fc;
                *pa_imag *= 2.0 * PI * fc;
            }

            // bilinear transform to z-plane
            for ((pa_real, pa_imag), (p_real, p_imag)) in self
                .pa_real
                .iter_mut()
                .zip(self.pa_imag.iter_mut())
                .take(self.decimator_order / 2)
                .zip(self.p_real.iter_mut().zip(self.p_imag.iter_mut()))
            {
                // complex division
                let u = (2.0 * self.sample_rate + *pa_real) / (2.0 * self.sample_rate);
                let v = *pa_imag / (2.0 * self.sample_rate);
                let x = (2.0 * self.sample_rate - *pa_real) / (2.0 * self.sample_rate);
                let y = -1.0 * *pa_imag / (2.0 * self.sample_rate);

                let c = 1.0 / (x * x + y * y);

                *p_real = c * (u * x + v * y);
                *p_imag = c * (v * x - u * y);
            }

            // compute cascade coefficients
            for (((a1, a2), (p_real, p_imag)), k) in self
                .a1
                .iter_mut()
                .zip(self.a2.iter_mut())
                .take(self.decimator_order / 2)
                .zip(self.p_real.iter_mut().zip(self.p_imag.iter_mut()))
                .zip(self.k.iter_mut())
            {
                *a1 = -2.0 * *p_real;
                *a2 = p_real.powi(2) + p_imag.powi(2);
                *k = (1.0 + *a1 + *a2) / 4.0;
            }

            for (run, (a1, a2)) in self
                .run
                .iter_mut()
                .zip(izip!(&self.a1, &self.a2))
                .take(self.decimator_order / 2)
            {
                *run = [*a1 as f32, *a2 as f32, (2.0 - a1) as f32, (1.0 - a2) as f32];
            }
            self.gain = self.k.iter().take(self.decimator_order / 2).product::<f64>() as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain in dB of a sine at `freq` Hz through `process`, measured after it settles.
    fn gain_db(sample_rate: f64, freq: f64, mut process: impl FnMut(f64) -> f64) -> f64 {
        let len = sample_rate as usize;
        let (mut energy_in, mut energy_out) = (0.0, 0.0);
        for i in 0..len {
            let x = 0.1 * (2.0 * std::f64::consts::PI * freq * i as f64 / sample_rate).sin();
            let y = process(x);
            if i >= len / 4 {
                energy_in += x * x;
                energy_out += y * y;
            }
        }
        10.0 * (energy_out / energy_in).log10()
    }

    /// The decimator is designed for the oversampled rate of whatever rate the filter runs at,
    /// and keeps its state when the parameters are set again at the same rate (which happens
    /// every control rate block).
    #[test]
    fn decimator_follows_the_sample_rate() {
        let design = |filter: &LadderFilter| {
            let iir = &filter.iir_lowpass;
            (iir.sample_rate, iir.cutoff_frequency, iir.decimator_order)
        };
        let expected =
            |rate: f64| (rate * 3.0, IIR_DOWNSAMPLING_BANDWIDTH * rate / 2.0, IIR_DOWNSAMPLE_ORDER);
        let mut filter = LadderFilter::with_seed(1);
        assert_eq!(design(&filter), expected(44100.0));
        for rate in [48000.0, 96000.0, 22050.0, 44100.0] {
            filter.set_params(rate as f32, 1000.0, 1.0);
            assert_eq!(design(&filter), expected(rate));
        }

        for _ in 0..64 {
            filter.process(0.5);
        }
        let state = filter.iir_lowpass.z;
        assert!(state.iter().any(|z| *z != 0.0));
        filter.set_params(44100.0, 2000.0, 2.0);
        assert_eq!(filter.iir_lowpass.z, state);

        filter.set_decimator_order(8);
        assert_eq!(design(&filter).2, 8);
        filter.set_decimator_order(1000);
        assert_eq!(design(&filter).2, IIR_MAX_ORDER);
    }

    /// At the oversampled rate, the decimator passes the audio band and removes what would
    /// alias into it when only every third sample is kept.
    #[test]
    fn decimator_rejects_what_would_alias() {
        for rate in [44100.0, 96000.0] {
            let mut filter = LadderFilter::default();
            filter.set_params(rate as f32, 1000.0, 1.0);
            let oversampled = rate * 3.0;
            let gain = |freq: f64| {
                let mut iir = filter.iir_lowpass.clone();
                gain_db(oversampled, freq, |x| iir.filter(x as f32) as f64)
            };
            // Flat up to a third of the output rate (15 kHz at 44.1 kHz)...
            for freq in [0.02, 0.2, 0.34] {
                let gain = gain(freq * rate);
                assert!(gain.abs() < 0.5, "{rate} Hz: {} Hz at {gain} dB", freq * rate);
            }
            // ...and at least 60 dB down from two thirds of it (what lands on 14 kHz and
            // below at 44.1 kHz once decimated) up to the oversampled Nyquist frequency
            for freq in [0.68, 0.9, 1.2] {
                let gain = gain(freq * rate);
                assert!(gain < -60.0, "{rate} Hz: {} Hz at {gain} dB", freq * rate);
            }
        }
    }

    /// Wide open, the ladder keeps the top of the audio band. With the decimator running at the
    /// output rate (as it used to) it cut everything above 10 kHz at 44.1 kHz: 12 kHz came out
    /// 51 dB below 1 kHz, and 15 kHz 120 dB below.
    #[test]
    fn wide_open_ladder_keeps_the_top_octave() {
        let rate = 44100.0;
        let response = |freq: f64| {
            let mut filter = LadderFilter::with_seed(1);
            filter.set_params(rate as f32, 22000.0, 0.7);
            gain_db(rate, freq, |x| filter.process(x as f32) as f64)
        };
        let reference = response(1000.0);
        for (freq, floor) in [(5000.0, -3.0), (12000.0, -12.0), (15000.0, -18.0)] {
            let relative = response(freq) - reference;
            assert!(relative > floor, "{freq} Hz is {relative} dB below 1 kHz");
        }
    }

    /// The power of `signal` at `freq` Hz (a single DFT bin, via the Goertzel algorithm).
    fn power_at(signal: &[f32], freq: f64, sample_rate: f64) -> f64 {
        let coefficient = 2.0 * (2.0 * std::f64::consts::PI * freq / sample_rate).cos();
        let (mut s1, mut s2) = (0.0_f64, 0.0_f64);
        for &x in signal {
            let s0 = x as f64 + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coefficient * s1 * s2) / (signal.len() as f64).powi(2)
    }

    /// Driving the filter hard with a 7 kHz tone makes odd harmonics. The 5th and 7th (35 and
    /// 49 kHz) are above the output's Nyquist frequency, and would fold back to 9.1 and 4.9 kHz
    /// at 44.1 kHz. With the input held at each sample, its images around 44.1 kHz mixed with
    /// the harmonics in the tanh stages and put products there at about -38 dB, which the
    /// decimator can't remove; interpolating the input keeps them out of the audio band.
    #[test]
    fn interpolated_input_keeps_distortion_from_folding_back() {
        let rate = 44100.0;
        let len = rate as usize;
        for lockstep in [false, true] {
            let mut filter = LadderFilter::with_seed(1);
            filter.set_params(rate as f32, 22000.0, 0.7);
            let mut signal: Vec<f32> = (0..2 * len)
                .map(|i| 1.5 * (2.0 * std::f64::consts::PI * 7000.0 * i as f64 / rate).sin() as f32)
                .collect();
            if lockstep && is_x86_feature_detected!("avx") {
                for block in signal.chunks_mut(32) {
                    let mut lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES] =
                        std::array::from_fn(|_| None);
                    lanes[0] = Some((&mut filter, block));
                    unsafe { LadderFilter::process_block_x8(lanes) };
                }
            } else {
                for sample in signal.iter_mut() {
                    *sample = filter.process(*sample);
                }
            }
            let settled = &signal[len..];
            let tone = power_at(settled, 7000.0, rate);
            for fold in [9100.0, 4900.0] {
                let relative = 10.0 * (power_at(settled, fold, rate) / tone).log10();
                assert!(relative < -100.0, "{fold} Hz at {relative} dB (lockstep {lockstep})");
            }
        }
    }

    /// A test signal: a detuned pair of saws
    fn signal(len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| ((i % 97) as f32 / 48.5 - 1.0) * 0.5 + ((i % 61) as f32 / 30.5 - 1.0) * 0.3)
            .collect()
    }

    /// The FMA version of the lockstep kernel rounds differently, but stays within rounding
    /// errors of the one-filter path, resonating or not.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn fused_lockstep_stays_close_to_scalar_processing() {
        if !(is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")) {
            return;
        }
        for mode in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
            let input = signal(2048);
            let cutoffs = [300.0, 2500.0, 9000.0, 18000.0, 22000.0, 50.0, 1200.0, 6000.0];
            let resonances = [0.5, 3.0, 7.0, 9.5, 0.1, 10.0, 5.0, 8.5];
            let make = || -> Vec<LadderFilter> {
                (0..LADDER_LANES)
                    .map(|i| {
                        let mut filter = LadderFilter::with_seed(i as u64);
                        filter.set_filter_type(mode);
                        filter.set_params(44100.0, cutoffs[i], resonances[i]);
                        filter
                    })
                    .collect()
            };
            let (mut scalar, mut fused) = (make(), make());
            let expected: Vec<Vec<f32>> = scalar
                .iter_mut()
                .map(|filter| input.iter().map(|sample| filter.process(*sample)).collect())
                .collect();
            let mut buffers: Vec<Vec<f32>> = (0..LADDER_LANES).map(|_| input.clone()).collect();
            for start in (0..input.len()).step_by(32) {
                let mut filters = fused.iter_mut().zip(buffers.iter_mut());
                let lanes = std::array::from_fn(|_| {
                    filters.next().map(|(filter, buffer)| (filter, &mut buffer[start..start + 32]))
                });
                unsafe { LadderFilter::process_block_x8_fma(lanes) };
            }
            for lane in 0..LADDER_LANES {
                let peak = expected[lane].iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                let worst = (0..input.len())
                    .map(|i| (buffers[lane][i] - expected[lane][i]).abs())
                    .fold(0.0_f32, f32::max);
                assert!(peak > 1e-3, "{mode:?} lane {lane}");
                assert!(worst <= 1e-4 * peak, "{mode:?} lane {lane}: {worst:e} of {peak}");
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    /// With the integration rate and resonance given for every sample, the lockstep kernel
    /// (without fused multiply-adds) produces exactly what the one-filter path does, and the
    /// fused one stays close.
    #[test]
    fn lockstep_with_sample_coefficients_matches_scalar_processing() {
        if !is_x86_feature_detected!("avx2") || !is_x86_feature_detected!("fma") {
            return;
        }
        let len = 300;
        let input = signal(len);
        let lanes = [0, 1, 2, 4, 7];
        let template = LadderFilter::default();
        // Each filter's cutoff sweeps differently, and its resonance wobbles
        let coefficients: Vec<[Vec<f32>; 2]> = (0..LADDER_LANES)
            .map(|lane| {
                let mut dt = vec![0.0; len];
                let mut resonance = vec![0.0; len];
                for start in (0..len).step_by(8) {
                    let cutoffs = wide::f32x8::new(std::array::from_fn(|i| {
                        let n = (start + i) as f32;
                        300.0 * (1.0 + lane as f32) * (1.0 + 6.0 * (n * 0.03 * (lane + 1) as f32).sin().abs())
                    }));
                    let resonances = wide::f32x8::new(std::array::from_fn(|i| 4.0 + 4.0 * ((start + i) as f32 * 0.07).cos()));
                    let [d, r] = template.coefficients8(cutoffs, resonances, 44100.0);
                    let end = (start + 8).min(len);
                    dt[start..end].copy_from_slice(&d.to_array()[..end - start]);
                    resonance[start..end].copy_from_slice(&r.to_array()[..end - start]);
                }
                [dt, resonance]
            })
            .collect();
        type Kernel = unsafe fn([Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES], &[Option<LadderSamples>; LADDER_LANES]);
        let kernels: [(&str, Kernel, f32); 2] = [
            ("separate", LadderFilter::process_block_x8_samples, 0.0),
            ("fused", LadderFilter::process_block_x8_samples_fma, 1e-4),
        ];
        for (name, kernel, tolerance) in kernels {
            for block in [32, 1, 13] {
                let make = |lane: usize| {
                    let mut filter = LadderFilter::with_seed(lane as u64);
                    filter.set_params(44100.0, 1000.0, 2.0);
                    filter
                };
                let mut scalar: Vec<LadderFilter> = (0..LADDER_LANES).map(make).collect();
                let mut lockstep: Vec<LadderFilter> = (0..LADDER_LANES).map(make).collect();
                let mut expected = vec![input.clone(); LADDER_LANES];
                let mut buffers = vec![input.clone(); LADDER_LANES];
                let samples = |lane: usize, start: usize, end: usize| LadderSamples {
                    dt: &coefficients[lane][0][start..end],
                    resonance: &coefficients[lane][1][start..end],
                };
                for start in (0..len).step_by(block) {
                    let end = (start + block).min(len);
                    for &lane in &lanes {
                        scalar[lane].process_samples(&mut expected[lane][start..end], samples(lane, start, end));
                    }
                    let entries: Vec<Option<(&mut LadderFilter, &mut [f32])>> = lockstep
                        .iter_mut()
                        .zip(buffers.iter_mut())
                        .enumerate()
                        .map(|(lane, (filter, buffer))| lanes.contains(&lane).then(|| (filter, &mut buffer[start..end])))
                        .collect();
                    let lane_coefficients: [Option<LadderSamples>; LADDER_LANES] =
                        std::array::from_fn(|lane| lanes.contains(&lane).then(|| samples(lane, start, end)));
                    unsafe { kernel(entries.try_into().ok().unwrap(), &lane_coefficients) };
                }
                for &lane in &lanes {
                    let case = format!("{name}, lane {lane}, blocks of {block}");
                    let worst = (0..len).map(|i| (buffers[lane][i] - expected[lane][i]).abs()).fold(0.0_f32, f32::max);
                    assert!(worst <= tolerance, "{case}: differs by up to {worst}");
                    assert!(expected[lane].iter().any(|sample| sample.abs() > 1e-3), "{case}");
                    assert_eq!(scalar[lane].dt, lockstep[lane].dt, "{case}");
                    assert_eq!(scalar[lane].resonance, lockstep[lane].resonance, "{case}");
                    if tolerance == 0.0 {
                        assert_eq!(scalar[lane].process(0.25), lockstep[lane].process(0.25), "{case}");
                    }
                }
            }
        }
    }

    #[test]
    fn lockstep_matches_scalar_processing_exactly() {
        if !is_x86_feature_detected!("avx") {
            return;
        }
        // The filters, in these lanes
        let all: &[usize] = &[0, 1, 2, 3, 4, 5, 6, 7];
        for (positions, mode) in [
            (all, FilterType::Lowpass),
            (all, FilterType::Bandpass),
            (all, FilterType::Highpass),
            (&all[..5], FilterType::Lowpass),
            (&all[..2], FilterType::Lowpass),
            (&all[..1], FilterType::Lowpass),
            (&[1, 4, 7][..], FilterType::Lowpass),
        ] {
            let lanes = positions.len();
            let input = signal(300);
            let cutoffs = [300.0, 2500.0, 9000.0, 18000.0, 22000.0, 50.0, 1200.0, 6000.0];
            let resonances = [0.5, 3.0, 7.0, 9.5, 0.1, 10.0, 5.0, 8.5];
            let mut scalar: Vec<LadderFilter> = (0..lanes).map(|i| LadderFilter::with_seed(i as u64)).collect();
            let mut lockstep: Vec<LadderFilter> = (0..lanes).map(|i| LadderFilter::with_seed(i as u64)).collect();
            for (i, filter) in scalar.iter_mut().chain(lockstep.iter_mut()).enumerate() {
                let i = i % lanes;
                filter.set_filter_type(mode);
                filter.set_params(44100.0, cutoffs[i], resonances[i]);
                // The first filter's noise wraps around the end of the table within a group
                if i == 0 {
                    filter.noise_position = NOISE_LEN - 5;
                }
            }

            let expected: Vec<Vec<f32>> = scalar
                .iter_mut()
                .map(|filter| input.iter().map(|sample| filter.process(*sample)).collect())
                .collect();

            // Process in two blocks to cover the state being carried across calls, the second
            // not a multiple of eight samples long
            let mut buffers: Vec<Vec<f32>> = (0..lanes).map(|_| input.clone()).collect();
            for (start, end) in [(0, 128), (128, 300)] {
                let mut filters = lockstep.iter_mut().zip(buffers.iter_mut());
                let entries: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES] =
                    std::array::from_fn(|lane| {
                        if positions.contains(&lane) {
                            filters.next().map(|(filter, buffer)| (filter, &mut buffer[start..end]))
                        } else {
                            None
                        }
                    });
                unsafe { LadderFilter::process_block_x8(entries) };
            }

            for lane in 0..lanes {
                let case = format!("{mode:?} filter {lane} in lanes {positions:?}");
                if let Some(first) = (0..input.len()).find(|&i| buffers[lane][i] != expected[lane][i]) {
                    let max_diff = (0..input.len())
                        .map(|i| (buffers[lane][i] - expected[lane][i]).abs())
                        .fold(0.0_f32, f32::max);
                    panic!(
                        "{case}: first difference at sample {first}: lockstep {} vs scalar {} (max difference {max_diff:e}, {} of {} samples differ)",
                        buffers[lane][first],
                        expected[lane][first],
                        (0..input.len()).filter(|&i| buffers[lane][i] != expected[lane][i]).count(),
                        input.len()
                    );
                }
                assert!(expected[lane].iter().any(|sample| sample.abs() > 1e-3), "{case}");
            }
            // and the state is carried over identically
            for lane in 0..lanes {
                assert_eq!(scalar[lane].noise_position, lockstep[lane].noise_position);
                assert_eq!(scalar[lane].process(0.25), lockstep[lane].process(0.25));
            }
        }
    }
}
