// Original from: https://github.com/wrl/baseplug, under
// the MIT license:

// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:

// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

// Modifications from the original:
// Removed simd acceleration.
// Since then: the state update works out the new state directly (see `process()`), and eight
// filters run in lockstep in SIMD vectors (see `process_block_x8()`).

// implemented from https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf
// thanks, andy!

use wide::f32x8;

use super::super::fastmath;
use super::{Filter, FilterType};

/// Number of filters [`SvfSimper::process_block_x8()`] runs in lockstep.
pub const SVF_LANES: usize = 8;
/// The samples [`SvfSimper::process_block_x8()`] transposes at a time.
const CHUNK: usize = 32;

#[derive(Debug, Clone)]
pub struct SvfSimper {
    /// `2 a1 - 1`, `2 a2` and `2 a3`, from the paper's coefficients `a1`, `a2` and `a3`: the
    /// coefficients of the state update as `process()` writes it.
    c1: f32,
    c2: f32,
    c3: f32,

    ic1eq: f32,
    ic2eq: f32,

    k: f32,

    /// Per sample change of the coefficients while ramping towards new parameters, see
    /// `set_params_ramped()`.
    c1_step: f32,
    c2_step: f32,
    c3_step: f32,
    k_step: f32,

    pub filter_type: FilterType,
}

/// The coefficients of every sample of a block, `c1`, `c2`, `c3` and `k` as
/// [`SvfSimper::coefficients()`] gives them, for running a filter whose cutoff or resonance
/// changes at every sample (see [`SvfSimper::process_samples()`]).
#[derive(Debug, Clone, Copy)]
pub struct SvfSamples<'a> {
    pub c1: &'a [f32],
    pub c2: &'a [f32],
    pub c3: &'a [f32],
    pub k: &'a [f32],
}

impl SvfSimper {
    pub fn new(cutoff: f32, resonance: f32, sample_rate: f32) -> Self {
        let [c1, c2, c3, k] = Self::coefficients(cutoff, resonance, sample_rate);
        SvfSimper {
            c1,
            c2,
            c3,

            ic1eq: 0.0,
            ic2eq: 0.0,

            k,

            c1_step: 0.0,
            c2_step: 0.0,
            c3_step: 0.0,
            k_step: 0.0,

            filter_type: FilterType::Lowpass,
        }
    }

    /// The coefficients `[c1, c2, c3, k]` for a cutoff, a resonance from 0 to 1 and the sample
    /// rate.
    #[inline]
    pub fn coefficients(cutoff: f32, resonance: f32, sample_rate: f32) -> [f32; 4] {
        // tan(pi * cutoff / sample_rate)
        let g = fastmath::tan_cycles(0.5 * cutoff / sample_rate);
        let k = 2f32 - (1.9f32 * resonance.clamp(0.0, 1.0));

        let a1 = 1.0 / (1.0 + (g * (g + k)));
        let a2 = g * a1;
        let a3 = g * a2;
        [2.0 * a1 - 1.0, 2.0 * a2, 2.0 * a3, k]
    }

    /// [`coefficients()`][Self::coefficients()] for eight cutoffs and resonances at once, with
    /// exactly the same results.
    #[inline]
    pub fn coefficients8(cutoff: f32x8, resonance: f32x8, sample_rate: f32) -> [f32x8; 4] {
        let g = fastmath::x8::tan_cycles(f32x8::splat(0.5) * cutoff / f32x8::splat(sample_rate));
        let k = f32x8::splat(2.0)
            - f32x8::splat(1.9) * resonance.fast_max(f32x8::ZERO).fast_min(f32x8::splat(1.0));

        let one = f32x8::splat(1.0);
        let a1 = one / (one + (g * (g + k)));
        let a2 = g * a1;
        let a3 = g * a2;
        let two = f32x8::splat(2.0);
        [two * a1 - one, two * a2, two * a3, k]
    }

    pub fn set(&mut self, cutoff: f32, resonance: f32, sample_rate: f32) {
        let new = Self::new(cutoff, resonance, sample_rate);

        self.c1 = new.c1;
        self.c2 = new.c2;
        self.c3 = new.c3;
        self.k = new.k;
        self.c1_step = 0.0;
        self.c2_step = 0.0;
        self.c3_step = 0.0;
        self.k_step = 0.0;
    }

    /// Processes a block of samples, in place, for up to 8 filters at once in lockstep: each
    /// lane's samples go through its own filter, with the eight filters' states packed into
    /// SIMD vectors. Each present lane produces exactly the same output as calling
    /// [`process()`][Filter::process()] on its filter for each sample would, since the
    /// operations are the same and in the same order. The filters must all have the same filter
    /// type; the first present lane's is used.
    ///
    /// The filter's recurrence is a chain of dependent operations, so one filter on its own
    /// leaves most of the CPU idle: this runs eight chains side by side for the same latency.
    /// [`process_block_x8_avx()`][Self::process_block_x8_avx()] does the same, faster.
    pub fn process_block_x8(mut lanes: [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) {
        let Some((filter_type, block_len)) = lockstep_shape(&lanes) else {
            return;
        };
        let mut values = LaneValues::gather(&lanes);
        // One version of the loop per filter type, so that there's no choice in the loop
        match filter_type {
            FilterType::Lowpass => portable::process::<LOWPASS>(&mut lanes, &mut values, block_len),
            FilterType::Bandpass => portable::process::<BANDPASS>(&mut lanes, &mut values, block_len),
            FilterType::Highpass => portable::process::<HIGHPASS>(&mut lanes, &mut values, block_len),
        }
        values.scatter(&mut lanes);
    }

    /// Runs the filter over a block of samples in place, with the coefficients given for every
    /// sample instead of ramping. The filter keeps the last sample's coefficients.
    pub fn process_samples(&mut self, samples: &mut [f32], coefficients: SvfSamples) {
        self.clear_ramp();
        for (n, sample) in samples.iter_mut().enumerate() {
            (self.c1, self.c2, self.c3, self.k) =
                (coefficients.c1[n], coefficients.c2[n], coefficients.c3[n], coefficients.k[n]);
            *sample = self.process(*sample);
        }
    }

    /// Sets the coefficients `[c1, c2, c3, k]` (see [`coefficients()`][Self::coefficients()])
    /// directly, without a ramp.
    pub fn set_coefficients(&mut self, [c1, c2, c3, k]: [f32; 4]) {
        (self.c1, self.c2, self.c3, self.k) = (c1, c2, c3, k);
        self.clear_ramp();
    }

    /// Stops the coefficients' ramp, as for coefficients given per sample.
    fn clear_ramp(&mut self) {
        self.c1_step = 0.0;
        self.c2_step = 0.0;
        self.c3_step = 0.0;
        self.k_step = 0.0;
    }

    /// [`process_block_x8()`][Self::process_block_x8()] with the coefficients of every sample
    /// given for each lane, as [`process_samples()`][Self::process_samples()] takes them, with
    /// exactly the same results as that.
    pub fn process_block_x8_samples(
        mut lanes: [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES],
        coefficients: &[Option<SvfSamples>; SVF_LANES],
    ) {
        let Some((filter_type, block_len)) = lockstep_shape(&lanes) else {
            return;
        };
        let mut values = LaneValues::gather(&lanes);
        match filter_type {
            FilterType::Lowpass => portable::process_samples::<LOWPASS>(&mut lanes, coefficients, &mut values, block_len),
            FilterType::Bandpass => portable::process_samples::<BANDPASS>(&mut lanes, coefficients, &mut values, block_len),
            FilterType::Highpass => portable::process_samples::<HIGHPASS>(&mut lanes, coefficients, &mut values, block_len),
        }
        values.scatter_without_ramp(&mut lanes);
    }

    /// [`process_block_x8_samples()`][Self::process_block_x8_samples()] with AVX, with
    /// exactly the same results.
    ///
    /// # Safety
    ///
    /// The CPU must support AVX.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    pub unsafe fn process_block_x8_samples_avx(
        mut lanes: [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES],
        coefficients: &[Option<SvfSamples>; SVF_LANES],
    ) {
        let Some((filter_type, block_len)) = lockstep_shape(&lanes) else {
            return;
        };
        let mut values = LaneValues::gather(&lanes);
        match filter_type {
            FilterType::Lowpass => avx::process_samples::<LOWPASS>(&mut lanes, coefficients, &mut values, block_len),
            FilterType::Bandpass => avx::process_samples::<BANDPASS>(&mut lanes, coefficients, &mut values, block_len),
            FilterType::Highpass => avx::process_samples::<HIGHPASS>(&mut lanes, coefficients, &mut values, block_len),
        }
        values.scatter_without_ramp(&mut lanes);
    }

    /// [`process_block_x8()`][Self::process_block_x8()] with AVX, with exactly the same results:
    /// the eight lanes fill one vector, and the samples are transposed eight at a time with
    /// shuffles.
    ///
    /// # Safety
    ///
    /// The CPU must support AVX.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    pub unsafe fn process_block_x8_avx(mut lanes: [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) {
        let Some((filter_type, block_len)) = lockstep_shape(&lanes) else {
            return;
        };
        let mut values = LaneValues::gather(&lanes);
        match filter_type {
            FilterType::Lowpass => avx::process::<LOWPASS>(&mut lanes, &mut values, block_len),
            FilterType::Bandpass => avx::process::<BANDPASS>(&mut lanes, &mut values, block_len),
            FilterType::Highpass => avx::process::<HIGHPASS>(&mut lanes, &mut values, block_len),
        }
        values.scatter(&mut lanes);
    }
}

/// The filter types, as the `MODE` of the lockstep loops
const LOWPASS: u8 = 0;
const BANDPASS: u8 = 1;
const HIGHPASS: u8 = 2;

mod portable {
    use wide::f32x8;

    use super::{LaneValues, SvfSamples, SvfSimper, BANDPASS, CHUNK, LOWPASS, SVF_LANES};

    /// The coefficients and state of eight filters
    struct Vectors {
        c1: f32x8,
        c2: f32x8,
        c3: f32x8,
        k: f32x8,
        c1_step: f32x8,
        c2_step: f32x8,
        c3_step: f32x8,
        k_step: f32x8,
        ic1eq: f32x8,
        ic2eq: f32x8,
    }

    impl Vectors {
        /// One sample through the eight filters, with the same operations in the same order as
        /// [`process()`][crate::synth::filter::Filter::process()].
        #[inline(always)]
        fn step<const MODE: u8>(&mut self, input: f32x8) -> f32x8 {
            self.c1 += self.c1_step;
            self.c2 += self.c2_step;
            self.c3 += self.c3_step;
            self.k += self.k_step;
            self.update::<MODE>(input)
        }

        /// One sample with the current coefficients.
        #[inline(always)]
        fn update<const MODE: u8>(&mut self, input: f32x8) -> f32x8 {
            let v3 = input - self.ic2eq;
            let ic1eq = (self.c1 * self.ic1eq) + (self.c2 * v3);
            let ic2eq = (self.ic2eq + (self.c2 * self.ic1eq)) + (self.c3 * v3);
            let half = f32x8::splat(0.5);
            let v1 = half * (self.ic1eq + ic1eq);
            let v2 = half * (self.ic2eq + ic2eq);
            self.ic1eq = ic1eq;
            self.ic2eq = ic2eq;

            match MODE {
                LOWPASS => v2,
                BANDPASS => v1,
                _ => input - self.k * v1 - v2,
            }
        }
    }

    /// The loop of [`SvfSimper::process_block_x8()`], for one filter type.
    pub fn process<const MODE: u8>(
        lanes: &mut [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES],
        values: &mut LaneValues,
        block_len: usize,
    ) {
        let mut vectors = Vectors {
            c1: f32x8::new(values.c1),
            c2: f32x8::new(values.c2),
            c3: f32x8::new(values.c3),
            k: f32x8::new(values.k),
            c1_step: f32x8::new(values.c1_step),
            c2_step: f32x8::new(values.c2_step),
            c3_step: f32x8::new(values.c3_step),
            k_step: f32x8::new(values.k_step),
            ic1eq: f32x8::new(values.ic1eq),
            ic2eq: f32x8::new(values.ic2eq),
        };

        // The samples go through in chunks, transposed so that each vector holds one sample of
        // every lane: moving a lane's samples in and out in one go is cheaper than picking out
        // eight lanes for every sample. Empty lanes stay at zero.
        let mut frames = [[0.0_f32; SVF_LANES]; CHUNK];
        let mut start = 0;
        while start < block_len {
            let len = CHUNK.min(block_len - start);
            for (lane, entry) in lanes.iter().enumerate() {
                if let Some((_, buffer)) = entry {
                    for (frame, sample) in frames.iter_mut().zip(&buffer[start..start + len]) {
                        frame[lane] = *sample;
                    }
                }
            }
            for frame in frames[..len].iter_mut() {
                *frame = vectors.step::<MODE>(f32x8::new(*frame)).to_array();
            }
            for (lane, entry) in lanes.iter_mut().enumerate() {
                if let Some((_, buffer)) = entry {
                    for (frame, sample) in frames.iter().zip(&mut buffer[start..start + len]) {
                        *sample = frame[lane];
                    }
                }
            }
            start += len;
        }

        values.c1 = vectors.c1.to_array();
        values.c2 = vectors.c2.to_array();
        values.c3 = vectors.c3.to_array();
        values.k = vectors.k.to_array();
        values.ic1eq = vectors.ic1eq.to_array();
        values.ic2eq = vectors.ic2eq.to_array();
    }

    /// The loop of [`SvfSimper::process_block_x8_samples()`], for one filter type: like
    /// [`process()`], with each sample's coefficients transposed in along with the samples.
    pub fn process_samples<const MODE: u8>(
        lanes: &mut [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES],
        coefficients: &[Option<SvfSamples>; SVF_LANES],
        values: &mut LaneValues,
        block_len: usize,
    ) {
        let zero = f32x8::ZERO;
        let mut vectors = Vectors {
            c1: zero,
            c2: zero,
            c3: zero,
            k: zero,
            c1_step: zero,
            c2_step: zero,
            c3_step: zero,
            k_step: zero,
            ic1eq: f32x8::new(values.ic1eq),
            ic2eq: f32x8::new(values.ic2eq),
        };

        // One sample of every lane at a time, with its coefficients
        for n in 0..block_len {
            let mut frame = [[0.0_f32; SVF_LANES]; 5];
            for (lane, (entry, coefficients)) in lanes.iter().zip(coefficients).enumerate() {
                if let (Some((_, buffer)), Some(c)) = (entry, coefficients) {
                    frame[0][lane] = buffer[n];
                    frame[1][lane] = c.c1[n];
                    frame[2][lane] = c.c2[n];
                    frame[3][lane] = c.c3[n];
                    frame[4][lane] = c.k[n];
                }
            }
            vectors.c1 = f32x8::new(frame[1]);
            vectors.c2 = f32x8::new(frame[2]);
            vectors.c3 = f32x8::new(frame[3]);
            vectors.k = f32x8::new(frame[4]);
            let output = vectors.update::<MODE>(f32x8::new(frame[0])).to_array();
            for (lane, entry) in lanes.iter_mut().enumerate() {
                if let Some((_, buffer)) = entry {
                    buffer[n] = output[lane];
                }
            }
        }

        values.c1 = vectors.c1.to_array();
        values.c2 = vectors.c2.to_array();
        values.c3 = vectors.c3.to_array();
        values.k = vectors.k.to_array();
        values.ic1eq = vectors.ic1eq.to_array();
        values.ic2eq = vectors.ic2eq.to_array();
    }
}

#[cfg(target_arch = "x86_64")]
mod avx {
    use std::arch::x86_64::*;

    use super::super::simd::transpose8;
    use super::{LaneValues, SvfSamples, SvfSimper, BANDPASS, HIGHPASS, LOWPASS, SVF_LANES};

    /// The coefficients and state of eight filters
    struct Vectors {
        c1: __m256,
        c2: __m256,
        c3: __m256,
        k: __m256,
        c1_step: __m256,
        c2_step: __m256,
        c3_step: __m256,
        k_step: __m256,
        ic1eq: __m256,
        ic2eq: __m256,
    }

    impl Vectors {
        /// One sample through the eight filters, with the same operations in the same order as
        /// [`process()`][crate::synth::filter::Filter::process()].
        #[target_feature(enable = "avx")]
        #[inline]
        unsafe fn step<const MODE: u8>(&mut self, input: __m256) -> __m256 {
            self.c1 = _mm256_add_ps(self.c1, self.c1_step);
            self.c2 = _mm256_add_ps(self.c2, self.c2_step);
            self.c3 = _mm256_add_ps(self.c3, self.c3_step);
            self.k = _mm256_add_ps(self.k, self.k_step);
            self.update::<MODE>(input)
        }

        /// One sample with the current coefficients.
        #[target_feature(enable = "avx")]
        #[inline]
        unsafe fn update<const MODE: u8>(&mut self, input: __m256) -> __m256 {
            let v3 = _mm256_sub_ps(input, self.ic2eq);
            let ic1eq = _mm256_add_ps(_mm256_mul_ps(self.c1, self.ic1eq), _mm256_mul_ps(self.c2, v3));
            let ic2eq = _mm256_add_ps(
                _mm256_add_ps(self.ic2eq, _mm256_mul_ps(self.c2, self.ic1eq)),
                _mm256_mul_ps(self.c3, v3),
            );
            let half = _mm256_set1_ps(0.5);
            let v1 = _mm256_mul_ps(half, _mm256_add_ps(self.ic1eq, ic1eq));
            let v2 = _mm256_mul_ps(half, _mm256_add_ps(self.ic2eq, ic2eq));
            self.ic1eq = ic1eq;
            self.ic2eq = ic2eq;

            match MODE {
                LOWPASS => v2,
                BANDPASS => v1,
                _ => _mm256_sub_ps(_mm256_sub_ps(input, _mm256_mul_ps(self.k, v1)), v2),
            }
        }
    }

    /// The loop of [`SvfSimper::process_block_x8_avx()`], for one filter type.
    #[target_feature(enable = "avx")]
    pub unsafe fn process<const MODE: u8>(
        lanes: &mut [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES],
        values: &mut LaneValues,
        block_len: usize,
    ) {
        let load = |values: [f32; SVF_LANES]| _mm256_loadu_ps(values.as_ptr());
        let mut vectors = Vectors {
            c1: load(values.c1),
            c2: load(values.c2),
            c3: load(values.c3),
            k: load(values.k),
            c1_step: load(values.c1_step),
            c2_step: load(values.c2_step),
            c3_step: load(values.c3_step),
            k_step: load(values.k_step),
            ic1eq: load(values.ic1eq),
            ic2eq: load(values.ic2eq),
        };

        let mut start = 0;
        while start < block_len {
            // Eight samples of every lane (fewer at the end of the block, padded with zeros),
            // transposed into one vector per sample. Empty lanes are zero.
            let len = SVF_LANES.min(block_len - start);
            let mut rows = [_mm256_setzero_ps(); SVF_LANES];
            for (row, entry) in rows.iter_mut().zip(lanes.iter()) {
                if let Some((_, buffer)) = entry {
                    let samples = &buffer[start..start + len];
                    *row = if len == SVF_LANES {
                        _mm256_loadu_ps(samples.as_ptr())
                    } else {
                        let mut padded = [0.0_f32; SVF_LANES];
                        padded[..len].copy_from_slice(samples);
                        _mm256_loadu_ps(padded.as_ptr())
                    };
                }
            }
            let mut frames = transpose8(rows);
            if len == SVF_LANES {
                for frame in frames.iter_mut() {
                    *frame = vectors.step::<MODE>(*frame);
                }
            } else {
                for frame in frames[..len].iter_mut() {
                    *frame = vectors.step::<MODE>(*frame);
                }
            }

            let rows = transpose8(frames);
            for (row, entry) in rows.iter().zip(lanes.iter_mut()) {
                if let Some((_, buffer)) = entry {
                    let samples = &mut buffer[start..start + len];
                    if len == SVF_LANES {
                        _mm256_storeu_ps(samples.as_mut_ptr(), *row);
                    } else {
                        let mut padded = [0.0_f32; SVF_LANES];
                        _mm256_storeu_ps(padded.as_mut_ptr(), *row);
                        samples.copy_from_slice(&padded[..len]);
                    }
                }
            }
            start += len;
        }

        let store = |vector: __m256| {
            let mut values = [0.0_f32; SVF_LANES];
            _mm256_storeu_ps(values.as_mut_ptr(), vector);
            values
        };
        values.c1 = store(vectors.c1);
        values.c2 = store(vectors.c2);
        values.c3 = store(vectors.c3);
        values.k = store(vectors.k);
        values.ic1eq = store(vectors.ic1eq);
        values.ic2eq = store(vectors.ic2eq);
    }

    /// Eight values of every lane from `start` on (fewer at the end of the block, padded with
    /// zeros), transposed into one vector per sample. Missing lanes are zero.
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn transposed(rows: [Option<&[f32]>; SVF_LANES], start: usize, len: usize) -> [__m256; SVF_LANES] {
        let mut vectors = [_mm256_setzero_ps(); SVF_LANES];
        for lane in 0..SVF_LANES {
            if let Some(row) = rows[lane] {
                vectors[lane] = if len == SVF_LANES {
                    _mm256_loadu_ps(row[start..start + SVF_LANES].as_ptr())
                } else {
                    padded(&row[start..start + len])
                };
            }
        }
        transpose8(vectors)
    }

    /// Fewer than eight values, padded with zeros.
    #[cold]
    #[target_feature(enable = "avx")]
    unsafe fn padded(values: &[f32]) -> __m256 {
        let mut padded = [0.0_f32; SVF_LANES];
        padded[..values.len()].copy_from_slice(values);
        _mm256_loadu_ps(padded.as_ptr())
    }

    /// The loop of [`SvfSimper::process_block_x8_samples_avx()`], for one filter type: like
    /// [`process()`], with each sample's coefficients transposed in along with the samples.
    #[target_feature(enable = "avx")]
    pub unsafe fn process_samples<const MODE: u8>(
        lanes: &mut [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES],
        coefficients: &[Option<SvfSamples>; SVF_LANES],
        values: &mut LaneValues,
        block_len: usize,
    ) {
        let zero = _mm256_setzero_ps();
        let load = |values: [f32; SVF_LANES]| _mm256_loadu_ps(values.as_ptr());
        let mut vectors = Vectors {
            c1: zero,
            c2: zero,
            c3: zero,
            k: zero,
            c1_step: zero,
            c2_step: zero,
            c3_step: zero,
            k_step: zero,
            ic1eq: load(values.ic1eq),
            ic2eq: load(values.ic2eq),
        };

        let mut start = 0;
        while start < block_len {
            let len = SVF_LANES.min(block_len - start);
            let mut rows = [None; SVF_LANES];
            let mut c1_rows = [None; SVF_LANES];
            let mut c2_rows = [None; SVF_LANES];
            let mut c3_rows = [None; SVF_LANES];
            let mut k_rows = [None; SVF_LANES];
            for lane in 0..SVF_LANES {
                if let (Some((_, buffer)), Some(c)) = (&lanes[lane], &coefficients[lane]) {
                    rows[lane] = Some(&**buffer);
                    (c1_rows[lane], c2_rows[lane], c3_rows[lane], k_rows[lane]) =
                        (Some(c.c1), Some(c.c2), Some(c.c3), Some(c.k));
                }
            }
            let mut frames = transposed(rows, start, len);
            let c1 = transposed(c1_rows, start, len);
            let c2 = transposed(c2_rows, start, len);
            let c3 = transposed(c3_rows, start, len);
            // Only the highpass output needs `k`
            let k = if MODE == HIGHPASS {
                transposed(k_rows, start, len)
            } else {
                [zero; SVF_LANES]
            };
            for (n, frame) in frames[..len].iter_mut().enumerate() {
                (vectors.c1, vectors.c2, vectors.c3, vectors.k) = (c1[n], c2[n], c3[n], k[n]);
                *frame = vectors.update::<MODE>(*frame);
            }

            let rows = transpose8(frames);
            for (row, entry) in rows.iter().zip(lanes.iter_mut()) {
                if let Some((_, buffer)) = entry {
                    let samples = &mut buffer[start..start + len];
                    if len == SVF_LANES {
                        _mm256_storeu_ps(samples.as_mut_ptr(), *row);
                    } else {
                        let mut padded = [0.0_f32; SVF_LANES];
                        _mm256_storeu_ps(padded.as_mut_ptr(), *row);
                        samples.copy_from_slice(&padded[..len]);
                    }
                }
            }
            start += len;
        }

        let store = |vector: __m256| {
            let mut values = [0.0_f32; SVF_LANES];
            _mm256_storeu_ps(values.as_mut_ptr(), vector);
            values
        };
        values.c1 = store(vectors.c1);
        values.c2 = store(vectors.c2);
        values.c3 = store(vectors.c3);
        values.ic1eq = store(vectors.ic1eq);
        values.ic2eq = store(vectors.ic2eq);
        // The filters keep the last sample's `k` too, which only the highpass loop loads
        for (k, coefficients) in values.k.iter_mut().zip(coefficients) {
            if let Some(coefficients) = coefficients {
                *k = coefficients.k[block_len - 1];
            }
        }
    }
}

/// The filter type of a lockstep call's filters (the first present lane's) and how many samples
/// every lane has, or `None` without any lanes.
fn lockstep_shape(lanes: &[Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) -> Option<(FilterType, usize)> {
    let mut present = lanes.iter().flatten();
    let filter_type = present.next()?.0.filter_type;
    let block_len = lanes.iter().flatten().map(|(_, buffer)| buffer.len()).min()?;
    Some((filter_type, block_len))
}

/// The coefficients, ramps and state of the filters of a lockstep call, one lane each. Empty
/// lanes are all zero, which keeps their output at zero.
#[derive(Default)]
struct LaneValues {
    c1: [f32; SVF_LANES],
    c2: [f32; SVF_LANES],
    c3: [f32; SVF_LANES],
    k: [f32; SVF_LANES],
    c1_step: [f32; SVF_LANES],
    c2_step: [f32; SVF_LANES],
    c3_step: [f32; SVF_LANES],
    k_step: [f32; SVF_LANES],
    ic1eq: [f32; SVF_LANES],
    ic2eq: [f32; SVF_LANES],
}

impl LaneValues {
    fn gather(lanes: &[Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) -> Self {
        let mut values = LaneValues::default();
        for (lane, entry) in lanes.iter().enumerate() {
            if let Some((filter, _)) = entry {
                values.c1[lane] = filter.c1;
                values.c2[lane] = filter.c2;
                values.c3[lane] = filter.c3;
                values.k[lane] = filter.k;
                values.c1_step[lane] = filter.c1_step;
                values.c2_step[lane] = filter.c2_step;
                values.c3_step[lane] = filter.c3_step;
                values.k_step[lane] = filter.k_step;
                values.ic1eq[lane] = filter.ic1eq;
                values.ic2eq[lane] = filter.ic2eq;
            }
        }
        values
    }

    /// Writes the coefficients and state back into the filters and stops their ramps, after
    /// coefficients given per sample.
    fn scatter_without_ramp(&self, lanes: &mut [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) {
        self.scatter(lanes);
        for (filter, _) in lanes.iter_mut().flatten() {
            filter.clear_ramp();
        }
    }

    /// Writes the coefficients and state back into the filters (the ramps don't change).
    fn scatter(&self, lanes: &mut [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) {
        for (lane, entry) in lanes.iter_mut().enumerate() {
            if let Some((filter, _)) = entry {
                filter.c1 = self.c1[lane];
                filter.c2 = self.c2[lane];
                filter.c3 = self.c3[lane];
                filter.k = self.k[lane];
                filter.ic1eq = self.ic1eq[lane];
                filter.ic2eq = self.ic2eq[lane];
            }
        }
    }
}

impl Filter for SvfSimper {
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        self.c1 += self.c1_step;
        self.c2 += self.c2_step;
        self.c3 += self.c3_step;
        self.k += self.k_step;

        // The paper's update, `v1 = a1 ic1eq + a2 v3`, `v2 = ic2eq + a2 ic1eq + a3 v3` and then
        // `ic = 2 v - ic`, with the new state worked out directly instead: one sample's state
        // follows from the last one's after three dependent operations rather than five, and
        // that chain is what limits the filter's speed. The `v`s come back from the old and new
        // states.
        let v3 = input - self.ic2eq;
        let ic1eq = (self.c1 * self.ic1eq) + (self.c2 * v3);
        let ic2eq = (self.ic2eq + (self.c2 * self.ic1eq)) + (self.c3 * v3);
        let v1 = 0.5 * (self.ic1eq + ic1eq);
        let v2 = 0.5 * (self.ic2eq + ic2eq);
        self.ic1eq = ic1eq;
        self.ic2eq = ic2eq;

        match self.filter_type {
            FilterType::Lowpass => v2,
            FilterType::Bandpass => v1,
            FilterType::Highpass => input - self.k * v1 - v2,
            // notch: v0 - self.k * v1
            // peak: 2.0 * v2 - v0 + self.k * v1
            // all: v0 - 2.0 * self.k * v1
        }
    }

    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        self.set(cutoff, resonance / 10.0, sample_rate)
    }

    /// Ramps the coefficients linearly to the new parameters over the next `ramp_samples`
    /// samples, so that control rate cutoff changes don't step.
    fn set_params_ramped(
        &mut self,
        sample_rate: f32,
        cutoff: f32,
        resonance: f32,
        ramp_samples: usize,
    ) {
        if ramp_samples == 0 {
            return self.set_params(sample_rate, cutoff, resonance);
        }
        let new = Self::new(cutoff, resonance / 10.0, sample_rate);
        let scale = 1.0 / ramp_samples as f32;
        self.c1_step = (new.c1 - self.c1) * scale;
        self.c2_step = (new.c2 - self.c2) * scale;
        self.c3_step = (new.c3 - self.c3) * scale;
        self.k_step = (new.k - self.k) * scale;
    }

    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.filter_type = filter_type;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direct state update gives the same filter as the paper's equations, here in double
    /// precision, for every filter type.
    #[test]
    fn state_update_matches_the_paper() {
        // A slow square wave (for the low cutoffs) with some noise on top
        let input: Vec<f32> = (0..4000)
            .map(|i| if i % 900 < 450 { 0.5 } else { -0.5 } + ((i * 37 % 101) as f32 / 50.5 - 1.0) * 0.3)
            .collect();
        for filter_type in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
            for (cutoff, resonance) in [(80.0, 0.1), (1000.0, 0.5), (6000.0, 0.95), (18000.0, 0.3)] {
                let mut filter = SvfSimper::new(cutoff, resonance, 44100.0);
                filter.filter_type = filter_type;

                let g = (std::f64::consts::PI * cutoff as f64 / 44100.0).tan();
                let k = 2.0 - 1.9 * resonance as f64;
                let a1 = 1.0 / (1.0 + g * (g + k));
                let (a2, a3) = (g * a1, g * g * a1);
                let (mut ic1eq, mut ic2eq) = (0.0_f64, 0.0_f64);
                let mut peak = 0.0_f64;
                let mut worst = 0.0_f64;
                for &sample in &input {
                    let v0 = sample as f64;
                    let v3 = v0 - ic2eq;
                    let v1 = a1 * ic1eq + a2 * v3;
                    let v2 = ic2eq + a2 * ic1eq + a3 * v3;
                    ic1eq = 2.0 * v1 - ic1eq;
                    ic2eq = 2.0 * v2 - ic2eq;
                    let want = match filter_type {
                        FilterType::Lowpass => v2,
                        FilterType::Bandpass => v1,
                        FilterType::Highpass => v0 - k * v1 - v2,
                    };
                    let got = filter.process(sample) as f64;
                    peak = peak.max(want.abs());
                    worst = worst.max((got - want).abs());
                }
                // The single precision tangent and rounding leave small differences
                assert!(peak > 0.05, "{filter_type:?} {cutoff} Hz: {peak}");
                assert!(worst < 1e-5 * peak, "{filter_type:?} {cutoff} Hz: {worst} of {peak}");
            }
        }
    }

    /// The eight lane coefficients are exactly the one-filter ones.
    #[test]
    fn eight_lane_coefficients_match() {
        for sample_rate in [44100.0, 96000.0] {
            for i in 0..2000 {
                let cutoffs: [f32; 8] = std::array::from_fn(|lane| 10.0 + (i * 8 + lane) as f32 * 1.37);
                let resonances: [f32; 8] = std::array::from_fn(|lane| (lane as f32 - 1.5) * 0.17);
                let got = SvfSimper::coefficients8(f32x8::new(cutoffs), f32x8::new(resonances), sample_rate);
                for lane in 0..8 {
                    let want = SvfSimper::coefficients(cutoffs[lane], resonances[lane], sample_rate);
                    for c in 0..4 {
                        assert_eq!(got[c].to_array()[lane].to_bits(), want[c].to_bits(), "{} Hz, coefficient {c}", cutoffs[lane]);
                    }
                }
            }
        }
    }

    /// The lockstep kernels with coefficients for every sample, portable and AVX where the CPU
    /// has it.
    fn sample_kernels() -> Vec<(&'static str, SampleKernel)> {
        let mut kernels: Vec<(&'static str, SampleKernel)> =
            vec![("portable", SvfSimper::process_block_x8_samples)];
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx") {
            // SAFETY: AVX was just detected.
            kernels.push(("avx", |lanes, coefficients| unsafe {
                SvfSimper::process_block_x8_samples_avx(lanes, coefficients)
            }));
        }
        kernels
    }
    type SampleKernel = fn([Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES], &[Option<SvfSamples>; SVF_LANES]);

    /// With coefficients for every sample too, the lockstep kernels produce exactly the
    /// samples, state and coefficients of the one-filter version, in blocks of any length.
    #[test]
    fn lockstep_with_sample_coefficients_matches_scalar_processing_exactly() {
        let len = 200;
        let input: Vec<f32> = (0..len)
            .map(|i| ((i % 97) as f32 / 48.5 - 1.0) * 0.5 + ((i % 61) as f32 / 30.5 - 1.0) * 0.3)
            .collect();
        // Each lane's cutoff sweeps differently, and its resonance wobbles
        let coefficients: Vec<[Vec<f32>; 4]> = (0..SVF_LANES)
            .map(|lane| {
                let per_sample: Vec<[f32; 4]> = (0..len)
                    .map(|n| {
                        let cutoff = 200.0 * (1.0 + lane as f32) * (1.0 + (n as f32 * 0.05 * (lane + 1) as f32).sin().abs() * 8.0);
                        let resonance = 0.5 + 0.4 * (n as f32 * 0.11).cos();
                        SvfSimper::coefficients(cutoff, resonance, 44100.0)
                    })
                    .collect();
                std::array::from_fn(|c| per_sample.iter().map(|values| values[c]).collect())
            })
            .collect();
        let lane_sets: [&[usize]; 3] = [&[0], &[0, 1, 2, 3, 4, 5, 6, 7], &[1, 4, 7]];
        for (kernel_name, kernel) in sample_kernels() {
            for filter_type in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
                for present in lane_sets {
                    for block in [32, 1, 13] {
                        let make = |lane: usize| {
                            let mut filter = SvfSimper::new(300.0 + 500.0 * lane as f32, 0.3, 44100.0);
                            filter.filter_type = filter_type;
                            filter
                        };
                        let mut scalar: Vec<SvfSimper> = (0..SVF_LANES).map(make).collect();
                        let mut lockstep: Vec<SvfSimper> = (0..SVF_LANES).map(make).collect();
                        let mut expected = vec![input.clone(); SVF_LANES];
                        let mut buffers = vec![input.clone(); SVF_LANES];
                        let samples = |lane: usize, start: usize, end: usize| SvfSamples {
                            c1: &coefficients[lane][0][start..end],
                            c2: &coefficients[lane][1][start..end],
                            c3: &coefficients[lane][2][start..end],
                            k: &coefficients[lane][3][start..end],
                        };

                        for start in (0..len).step_by(block) {
                            let end = (start + block).min(len);
                            for &lane in present {
                                scalar[lane].process_samples(&mut expected[lane][start..end], samples(lane, start, end));
                            }
                            let entries: Vec<Option<(&mut SvfSimper, &mut [f32])>> = lockstep
                                .iter_mut()
                                .zip(buffers.iter_mut())
                                .enumerate()
                                .map(|(lane, (filter, buffer))| {
                                    present.contains(&lane).then(|| (filter, &mut buffer[start..end]))
                                })
                                .collect();
                            let lane_coefficients: [Option<SvfSamples>; SVF_LANES] =
                                std::array::from_fn(|lane| present.contains(&lane).then(|| samples(lane, start, end)));
                            kernel(entries.try_into().ok().unwrap(), &lane_coefficients);
                        }

                        for lane in 0..SVF_LANES {
                            let case = format!("{kernel_name}: {filter_type:?} lane {lane} of {present:?}, blocks of {block}");
                            assert_eq!(buffers[lane], expected[lane], "{case}");
                            if present.contains(&lane) {
                                assert!(expected[lane].iter().any(|sample| sample.abs() > 1e-3), "{case}");
                            }
                            // The state and the last coefficients are carried over identically,
                            // whichever output a later block takes
                            for filter_type in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
                                let (mut scalar, mut lockstep) = (scalar[lane].clone(), lockstep[lane].clone());
                                scalar.filter_type = filter_type;
                                lockstep.filter_type = filter_type;
                                assert_eq!(scalar.process(0.25), lockstep.process(0.25), "{case}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// The lockstep kernels, portable and AVX where the CPU has it.
    fn kernels() -> Vec<(&'static str, Kernel)> {
        let mut kernels: Vec<(&'static str, Kernel)> = vec![("portable", SvfSimper::process_block_x8)];
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx") {
            // SAFETY: AVX was just detected.
            kernels.push(("avx", |lanes| unsafe { SvfSimper::process_block_x8_avx(lanes) }));
        }
        kernels
    }
    type Kernel = fn([Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]);

    /// The lockstep block processing produces exactly the same samples and state as processing
    /// each filter on its own, for every filter type and any set of present lanes, while the
    /// coefficients ramp, in blocks shorter and longer than the transposed chunks and not a
    /// multiple of eight samples long.
    #[test]
    fn lockstep_matches_scalar_processing_exactly() {
        let input: Vec<f32> = (0..256)
            .map(|i| ((i % 97) as f32 / 48.5 - 1.0) * 0.5 + ((i % 61) as f32 / 30.5 - 1.0) * 0.3)
            .collect();
        let lane_sets: [&[usize]; 4] = [&[0], &[0, 1, 2], &[0, 1, 2, 3, 4, 5, 6, 7], &[1, 4, 7]];
        for (kernel_name, kernel) in kernels() {
            for filter_type in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
                for present in lane_sets {
                    for block in [32, 100, 7] {
                        let make = |lane: usize| {
                            let mut filter = SvfSimper::new(300.0 + 500.0 * lane as f32, 0.3, 44100.0);
                            filter.filter_type = filter_type;
                            filter
                        };
                        let mut scalar: Vec<SvfSimper> = (0..SVF_LANES).map(make).collect();
                        let mut lockstep: Vec<SvfSimper> = (0..SVF_LANES).map(make).collect();
                        let mut expected = vec![input.clone(); SVF_LANES];
                        let mut buffers = vec![input.clone(); SVF_LANES];

                        for start in (0..input.len()).step_by(block) {
                            let end = (start + block).min(input.len());
                            for &lane in present {
                                let cutoff = 400.0 + 900.0 * lane as f32 + start as f32;
                                scalar[lane].set_params_ramped(44100.0, cutoff, 2.0, end - start);
                                lockstep[lane].set_params_ramped(44100.0, cutoff, 2.0, end - start);
                                for sample in &mut expected[lane][start..end] {
                                    *sample = scalar[lane].process(*sample);
                                }
                            }
                            let entries: Vec<Option<(&mut SvfSimper, &mut [f32])>> = lockstep
                                .iter_mut()
                                .zip(buffers.iter_mut())
                                .enumerate()
                                .map(|(lane, (filter, buffer))| {
                                    present.contains(&lane).then(|| (filter, &mut buffer[start..end]))
                                })
                                .collect();
                            kernel(entries.try_into().ok().unwrap());
                        }

                        for lane in 0..SVF_LANES {
                            let case = format!("{kernel_name}: {filter_type:?} lane {lane} of {present:?}, blocks of {block}");
                            assert_eq!(buffers[lane], expected[lane], "{case}");
                            if present.contains(&lane) {
                                assert!(expected[lane].iter().any(|sample| sample.abs() > 1e-3), "{case}");
                            }
                            // and the state is carried over identically
                            assert_eq!(scalar[lane].process(0.25), lockstep[lane].process(0.25), "{case}");
                        }
                    }
                }
            }
        }
    }
}
