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

impl SvfSimper {
    pub fn new(cutoff: f32, resonance: f32, sample_rate: f32) -> Self {
        // tan(pi * cutoff / sample_rate)
        let g = fastmath::tan_cycles(0.5 * cutoff / sample_rate);
        let k = 2f32 - (1.9f32 * resonance.clamp(0.0, 1.0));

        let a1 = 1.0 / (1.0 + (g * (g + k)));
        let a2 = g * a1;
        let a3 = g * a2;

        SvfSimper {
            c1: 2.0 * a1 - 1.0,
            c2: 2.0 * a2,
            c3: 2.0 * a3,

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

    use super::{LaneValues, SvfSimper, BANDPASS, CHUNK, LOWPASS, SVF_LANES};

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
}

#[cfg(target_arch = "x86_64")]
mod avx {
    use std::arch::x86_64::*;

    use super::{LaneValues, SvfSimper, BANDPASS, LOWPASS, SVF_LANES};

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

    /// Transposes eight vectors of eight values: `output[j][i] == rows[i][j]`.
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn transpose8(rows: [__m256; 8]) -> [__m256; 8] {
        // Pairs of rows interleaved, then pairs of those, then the 128-bit halves swapped over
        let t0 = _mm256_unpacklo_ps(rows[0], rows[1]);
        let t1 = _mm256_unpackhi_ps(rows[0], rows[1]);
        let t2 = _mm256_unpacklo_ps(rows[2], rows[3]);
        let t3 = _mm256_unpackhi_ps(rows[2], rows[3]);
        let t4 = _mm256_unpacklo_ps(rows[4], rows[5]);
        let t5 = _mm256_unpackhi_ps(rows[4], rows[5]);
        let t6 = _mm256_unpacklo_ps(rows[6], rows[7]);
        let t7 = _mm256_unpackhi_ps(rows[6], rows[7]);
        let s0 = _mm256_shuffle_ps::<0x44>(t0, t2);
        let s1 = _mm256_shuffle_ps::<0xEE>(t0, t2);
        let s2 = _mm256_shuffle_ps::<0x44>(t1, t3);
        let s3 = _mm256_shuffle_ps::<0xEE>(t1, t3);
        let s4 = _mm256_shuffle_ps::<0x44>(t4, t6);
        let s5 = _mm256_shuffle_ps::<0xEE>(t4, t6);
        let s6 = _mm256_shuffle_ps::<0x44>(t5, t7);
        let s7 = _mm256_shuffle_ps::<0xEE>(t5, t7);
        [
            _mm256_permute2f128_ps::<0x20>(s0, s4),
            _mm256_permute2f128_ps::<0x20>(s1, s5),
            _mm256_permute2f128_ps::<0x20>(s2, s6),
            _mm256_permute2f128_ps::<0x20>(s3, s7),
            _mm256_permute2f128_ps::<0x31>(s0, s4),
            _mm256_permute2f128_ps::<0x31>(s1, s5),
            _mm256_permute2f128_ps::<0x31>(s2, s6),
            _mm256_permute2f128_ps::<0x31>(s3, s7),
        ]
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
