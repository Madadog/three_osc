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
use fastrand::Rng;

use super::Filter;

// pade 3/2 approximant for tanh
#[inline]
pub fn tanh_pade32(x: f64) -> f64 {
    let x = x.clamp(-3.0, 3.0);
    // return approximant
    x * (15.0 + x.powi(2)) / (15.0 + 6.0 * x.powi(2))
}
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

#[derive(Debug, Clone)]
pub struct LadderFilter {
    cutoff_frequency: f64,
    pub resonance: f64,
    pub ladder_filter_mode: FilterType,
    sample_rate: f64,
    dt: f64,
    pub ladder_integration_method: IntegrationMethod,
    pub oversampling_factor: i32,
    decimator_order: usize,

    // filter state
    p0: f64,
    p1: f64,
    p2: f64,
    p3: f64,
    ut_1: f64,

    // filter output
    out: f64,

    // IIR downsampling filter
    iir_lowpass: IirFilter,
    /// Interpolates the input up to the oversampled rate: the input, zero-stuffed (and scaled
    /// by the oversampling factor), through a lowpass with the same design as the decimator.
    /// Without it the input would be a staircase, whose images around multiples of the sample
    /// rate the tanh stages mix down into the audio band.
    iir_upsampler: IirFilter,

    /// Source of the tiny amount of noise added to the input to keep the filter alive.
    rng: Rng,
}

impl Default for LadderFilter {
    fn default() -> Self {
        Self {
            cutoff_frequency: 0.25,
            resonance: 0.5,
            ladder_filter_mode: FilterType::Lowpass,
            sample_rate: 44100.0,
            dt: integration_rate(44100.0, 3, 0.25),
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
            rng: Rng::new(),
        }
    }
}
/// Number of filters [`LadderFilter::process_block_x4()`] runs in lockstep.
pub const LADDER_LANES: usize = 4;

#[allow(dead_code)]
impl LadderFilter {
    pub fn new() -> Self {
        Self::default()
    }
    /// A filter whose dither noise is reproducible, for tests.
    #[cfg(test)]
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: Rng::with_seed(seed),
            ..Self::default()
        }
    }
    /// Whether [`process_block_x4()`][Self::process_block_x4()] supports this filter's settings.
    pub fn supports_lockstep(&self) -> bool {
        matches!(
            self.ladder_integration_method,
            IntegrationMethod::PredictorCorrectorFullTanh
        )
    }
    /// Processes a block of samples, in place, for up to 4 filters at once, in lockstep with AVX.
    /// Each present lane produces exactly the same output as calling
    /// [`process()`][Filter::process()] on its filter for each sample. The filters must all use
    /// the same integration method (see [`supports_lockstep()`][Self::supports_lockstep()]),
    /// oversampling factor and filter mode; the first present lane's settings are used.
    ///
    /// # Safety
    ///
    /// The CPU must support AVX.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx")]
    pub unsafe fn process_block_x4(mut lanes: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES]) {
        use std::arch::x86_64::*;

        let Some(first) = lanes.iter().find_map(|lane| lane.as_ref().map(|(filter, _)| &**filter))
        else {
            return;
        };
        debug_assert!(first.supports_lockstep());
        let oversampling_factor = first.oversampling_factor;
        let mode = first.ladder_filter_mode;
        let decimate = oversampling_factor > 1;
        let biquads = first.iir_lowpass.decimator_order / 2;
        // The decimator's coefficients, broadcast once: `k`, `a1`, `a2`, `2 - a1`, `1 - a2`
        // (see `IirFilter::filter()`)
        let iir = &first.iir_lowpass;
        let iir_k: [__m256d; IIR_MAX_ORDER / 2] = std::array::from_fn(|i| _mm256_set1_pd(iir.k[i]));
        let iir_a1: [__m256d; IIR_MAX_ORDER / 2] =
            std::array::from_fn(|i| _mm256_set1_pd(iir.a1[i]));
        let iir_a2: [__m256d; IIR_MAX_ORDER / 2] =
            std::array::from_fn(|i| _mm256_set1_pd(iir.a2[i]));
        let iir_b1: [__m256d; IIR_MAX_ORDER / 2] =
            std::array::from_fn(|i| _mm256_set1_pd(2.0 - iir.a1[i]));
        let iir_b2: [__m256d; IIR_MAX_ORDER / 2] =
            std::array::from_fn(|i| _mm256_set1_pd(1.0 - iir.a2[i]));
        let block_len = lanes
            .iter()
            .flatten()
            .map(|(_, buffer)| buffer.len())
            .min()
            .unwrap_or(0);

        // Gather the state of each lane
        let mut p0 = [0.0_f64; LADDER_LANES];
        let mut p1 = [0.0_f64; LADDER_LANES];
        let mut p2 = [0.0_f64; LADDER_LANES];
        let mut p3 = [0.0_f64; LADDER_LANES];
        let mut ut_1 = [0.0_f64; LADDER_LANES];
        let mut dt = [0.0_f64; LADDER_LANES];
        let mut feedback = [0.0_f64; LADDER_LANES];
        let mut z = [[0.0_f64; LADDER_LANES]; IIR_MAX_ORDER];
        let mut zu = [[0.0_f64; LADDER_LANES]; IIR_MAX_ORDER];
        for (lane, entry) in lanes.iter().enumerate() {
            if let Some((filter, _)) = entry {
                p0[lane] = filter.p0;
                p1[lane] = filter.p1;
                p2[lane] = filter.p2;
                p3[lane] = filter.p3;
                ut_1[lane] = filter.ut_1;
                dt[lane] = filter.dt;
                feedback[lane] = 8.0 * filter.resonance;
                for (z, filter_z) in z.iter_mut().zip(filter.iir_lowpass.z.iter()) {
                    z[lane] = *filter_z;
                }
                for (z, filter_z) in zu.iter_mut().zip(filter.iir_upsampler.z.iter()) {
                    z[lane] = *filter_z;
                }
            }
        }
        let load = |values: &[f64; LADDER_LANES]| _mm256_loadu_pd(values.as_ptr());
        let (mut p0, mut p1, mut p2, mut p3) = (load(&p0), load(&p1), load(&p2), load(&p3));
        let mut ut_1 = load(&ut_1);
        let dt = load(&dt);
        let feedback = load(&feedback);
        let half_dt = _mm256_mul_pd(_mm256_set1_pd(0.5), dt);
        // The last output, before the output gain, per lane
        let mut out = {
            let mut values = [0.0_f64; LADDER_LANES];
            for (value, lane) in values.iter_mut().zip(lanes.iter()) {
                if let Some((filter, _)) = lane {
                    *value = filter.out;
                }
            }
            load(&values)
        };
        let mut z: [__m256d; IIR_MAX_ORDER] = std::array::from_fn(|i| load(&z[i]));
        let mut zu: [__m256d; IIR_MAX_ORDER] = std::array::from_fn(|i| load(&zu[i]));
        let interpolate = first.interpolates();
        let upsampling_gain = _mm256_set1_pd(oversampling_factor as f64);

        // One sample through a resampling filter's cascade, with the same operations as
        // `IirFilter::filter()`
        let cascade = |z: &mut [__m256d; IIR_MAX_ORDER], mut out: __m256d| {
            for biquad in 0..biquads {
                let (z0, z1) = (z[2 * biquad], z[2 * biquad + 1]);
                let recursive = _mm256_add_pd(
                    _mm256_mul_pd(iir_a1[biquad], z0),
                    _mm256_mul_pd(iir_a2[biquad], z1),
                );
                let feedforward = _mm256_add_pd(
                    _mm256_mul_pd(iir_b1[biquad], z0),
                    _mm256_mul_pd(iir_b2[biquad], z1),
                );
                let scaled = _mm256_mul_pd(iir_k[biquad], out);
                let biquad_in = _mm256_sub_pd(scaled, recursive);
                out = _mm256_add_pd(scaled, feedforward);
                z[2 * biquad + 1] = z0;
                z[2 * biquad] = biquad_in;
            }
            out
        };

        // Padé 3/2 approximant of tanh, the same operations as `tanh_pade32()`
        let tanh = |x: __m256d| {
            let x = _mm256_max_pd(_mm256_min_pd(x, _mm256_set1_pd(3.0)), _mm256_set1_pd(-3.0));
            let x2 = _mm256_mul_pd(x, x);
            _mm256_div_pd(
                _mm256_mul_pd(x, _mm256_add_pd(_mm256_set1_pd(15.0), x2)),
                _mm256_add_pd(_mm256_set1_pd(15.0), _mm256_mul_pd(_mm256_set1_pd(6.0), x2)),
            )
        };

        for sample in 0..block_len {
            // The input, with the tiny amount of noise that keeps the filter alive
            let mut input = [0.0_f64; LADDER_LANES];
            for (lane, entry) in lanes.iter_mut().enumerate() {
                if let Some((filter, buffer)) = entry {
                    let noise = 1.0e-6 * 2.0 * (filter.rng.f64() - 0.5);
                    input[lane] = buffer[sample] as f64 + noise;
                }
            }
            let input_sample = load(&input);

            // Predictor-corrector integration with full tanh stages, see `process_sample()`
            for step in 0..oversampling_factor {
                // The input at this oversampled step: interpolated from the zero-stuffed input,
                // or held
                let input = if interpolate {
                    let stuffed = if step == 0 {
                        _mm256_mul_pd(input_sample, upsampling_gain)
                    } else {
                        _mm256_setzero_pd()
                    };
                    cascade(&mut zu, stuffed)
                } else {
                    input_sample
                };

                let tanh_in = tanh(_mm256_sub_pd(ut_1, _mm256_mul_pd(feedback, p3)));
                let tanh_p0 = tanh(p0);
                let tanh_p1 = tanh(p1);
                let tanh_p2 = tanh(p2);
                let tanh_p3 = tanh(p3);

                let p0_prime = _mm256_add_pd(p0, _mm256_mul_pd(dt, _mm256_sub_pd(tanh_in, tanh_p0)));
                let p1_prime = _mm256_add_pd(p1, _mm256_mul_pd(dt, _mm256_sub_pd(tanh_p0, tanh_p1)));
                let p2_prime = _mm256_add_pd(p2, _mm256_mul_pd(dt, _mm256_sub_pd(tanh_p1, tanh_p2)));
                let p3_prime = _mm256_add_pd(p3, _mm256_mul_pd(dt, _mm256_sub_pd(tanh_p2, tanh_p3)));
                let tanh_p0_prime = tanh(p0_prime);
                let tanh_p1_prime = tanh(p1_prime);
                let tanh_p2_prime = tanh(p2_prime);
                let tanh_p3_prime = tanh(p3_prime);

                let correct = |p: __m256d, a: __m256d, b: __m256d, a_prime: __m256d, b_prime: __m256d| {
                    _mm256_add_pd(
                        p,
                        _mm256_mul_pd(
                            half_dt,
                            _mm256_add_pd(_mm256_sub_pd(a, b), _mm256_sub_pd(a_prime, b_prime)),
                        ),
                    )
                };
                p3 = correct(p3, tanh_p2, tanh_p3, tanh_p2_prime, tanh_p3_prime);
                p2 = correct(p2, tanh_p1, tanh_p2, tanh_p1_prime, tanh_p2_prime);
                p1 = correct(p1, tanh_p0, tanh_p1, tanh_p0_prime, tanh_p1_prime);
                let tanh_new_in = tanh(_mm256_sub_pd(input, _mm256_mul_pd(feedback, p3)));
                p0 = correct(p0, tanh_in, tanh_p0, tanh_new_in, tanh_p0_prime);

                // The input at the previous oversampled step
                ut_1 = input;

                out = match mode {
                    FilterType::Lowpass => p3,
                    FilterType::Bandpass => _mm256_sub_pd(p1, p3),
                    FilterType::Highpass => tanh(_mm256_sub_pd(
                        _mm256_sub_pd(input, p0),
                        _mm256_mul_pd(feedback, p3),
                    )),
                };

                if decimate {
                    // Every oversampled output goes through the decimation filter
                    out = cascade(&mut z, out);
                }
            }

            let mut output = [0.0_f32; LADDER_LANES];
            _mm_storeu_ps(
                output.as_mut_ptr(),
                _mm256_cvtpd_ps(_mm256_mul_pd(out, _mm256_set1_pd(1.8))),
            );
            for (lane, entry) in lanes.iter_mut().enumerate() {
                if let Some((_, buffer)) = entry {
                    buffer[sample] = output[lane];
                }
            }
        }

        // Scatter the state back
        let store = |vector: __m256d| {
            let mut values = [0.0_f64; LADDER_LANES];
            _mm256_storeu_pd(values.as_mut_ptr(), vector);
            values
        };
        let (p0, p1, p2, p3, ut_1) = (store(p0), store(p1), store(p2), store(p3), store(ut_1));
        let out = store(out);
        let z: Vec<[f64; LADDER_LANES]> = z.iter().map(|z| store(*z)).collect();
        let zu: Vec<[f64; LADDER_LANES]> = zu.iter().map(|z| store(*z)).collect();
        for (lane, entry) in lanes.iter_mut().enumerate() {
            if let Some((filter, _)) = entry {
                filter.p0 = p0[lane];
                filter.p1 = p1[lane];
                filter.p2 = p2[lane];
                filter.p3 = p3[lane];
                filter.ut_1 = ut_1[lane];
                filter.out = out[lane];
                for (filter_z, z) in filter.iir_lowpass.z.iter_mut().zip(z.iter()) {
                    *filter_z = z[lane];
                }
                for (filter_z, z) in filter.iir_upsampler.z.iter_mut().zip(zu.iter()) {
                    *filter_z = z[lane];
                }
            }
        }
    }
    pub fn process_sample(&mut self, input: f64) {
        let feedback = 8.0 * self.resonance;
        let noise = 1.0e-6 * 2.0 * (self.rng.f64() - 0.5);
        let input_sample = input + noise;
        let interpolate = self.interpolates();
        let upsampling_gain = self.oversampling_factor as f64;

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
                            * (tanh_pade32(input - feedback * self.p3) - tanh_pade32(self.p0));
                    self.p1 = self.p1 + self.dt * (tanh_pade32(self.p0) - tanh_pade32(self.p1));
                    self.p2 = self.p2 + self.dt * (tanh_pade32(self.p1) - tanh_pade32(self.p2));
                    self.p3 = self.p3 + self.dt * (tanh_pade32(self.p2) - tanh_pade32(self.p3));
                }
                // predictor-corrector integration
                // with full tanh stages
                IntegrationMethod::PredictorCorrectorFullTanh => {
                    // The stages' tanh values are shared by the predictor and the corrector,
                    // which only needs the predicted stages' values on top of them.
                    let tanh_in = tanh_pade32(self.ut_1 - feedback * self.p3);
                    let tanh_p0 = tanh_pade32(self.p0);
                    let tanh_p1 = tanh_pade32(self.p1);
                    let tanh_p2 = tanh_pade32(self.p2);
                    let tanh_p3 = tanh_pade32(self.p3);

                    // predictor
                    let p0_prime = self.p0 + self.dt * (tanh_in - tanh_p0);
                    let p1_prime = self.p1 + self.dt * (tanh_p0 - tanh_p1);
                    let p2_prime = self.p2 + self.dt * (tanh_p1 - tanh_p2);
                    let p3_prime = self.p3 + self.dt * (tanh_p2 - tanh_p3);
                    let tanh_p0_prime = tanh_pade32(p0_prime);
                    let tanh_p1_prime = tanh_pade32(p1_prime);
                    let tanh_p2_prime = tanh_pade32(p2_prime);
                    let tanh_p3_prime = tanh_pade32(p3_prime);

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
                                + (tanh_pade32(input - feedback * self.p3) - tanh_p0_prime));
                }
                // predictor-corrector integration
                // with feedback tanh stage only
                IntegrationMethod::PredictorCorrectorFeedbackTanh => {
                    let tanh_in = tanh_pade32(self.ut_1 - feedback * self.p3);

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
                                + (tanh_pade32(input - feedback * self.p3) - p0_prime));
                }
                // implicit trapezoidal integration
                // with feedback tanh stage only
                IntegrationMethod::TrapezoidalFeedbackTanh => {
                    let ut = tanh_pade32(self.ut_1 - feedback * self.p3);
                    let b = (0.5 * self.dt) / (1.0 + 0.5 * self.dt);
                    let c = (1.0 - 0.5 * self.dt) / (1.0 + 0.5 * self.dt);
                    let g = -feedback * b.powi(4);
                    let mut x_k = ut;
                    let d_t = c * self.p3
                        + (b + c * b) * self.p2
                        + (b.powi(2) + b.powi(2) * c) * self.p1
                        + (b.powi(3) + b.powi(3) * c) * self.p0
                        + b.powi(4) * ut;
                    let c_t = tanh_pade32(input - feedback * d_t);

                    for _ in 0..LADDER_MAX_NEWTON_STEPS {
                        let tanh_g_xk = tanh_pade32(g * x_k);
                        let tanh_g_xk2 = g * (1.0 - tanh_pade32(g * x_k) * tanh_pade32(g * x_k));

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
                FilterType::Highpass => tanh_pade32(input - self.p0 - feedback * self.p3),
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
    pub fn output(&self) -> f64 {
        self.out
    }
    fn set_integration_rate(&mut self) {
        self.dt = integration_rate(
            self.sample_rate,
            self.oversampling_factor,
            self.cutoff_frequency,
        );
    }
    pub fn set_cutoff(&mut self, cutoff: f64) {
        self.cutoff_frequency = cutoff;
        self.set_integration_rate()
    }
    pub fn set_resonance(&mut self, resonance: f64) {
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
    /// it doesn't alias. Redesigning clears their state, so it is only done when something
    /// changed (this is called for every control rate block).
    fn update_decimator(&mut self) {
        let rate = self.sample_rate * self.oversampling_factor as f64;
        let cutoff = IIR_DOWNSAMPLING_BANDWIDTH * self.sample_rate / 2.0;
        let iir = &self.iir_lowpass;
        if iir.sample_rate != rate
            || iir.cutoff_frequency != cutoff
            || iir.decimator_order != self.decimator_order
        {
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
        self.process_sample(input as f64);
        (self.output() * 1.8) as f32
    }

    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        // The original module expected a cutoff frequency from 0.001 -> 2.25.
        // 22000 / 2.25 = 9800.0
        // ... which is consistently out of tune for some reason.
        // rough retune: 9800.0 / 1.4 = 7000.00
        // Retune makes pitch accurate when resonance == 0.5
        self.cutoff_frequency = (cutoff as f64 / 7000.0) + 0.0001;
        // Self-resonance starts at resonance >= 0.5
        self.resonance = (resonance / 16.6) as f64;
        self.set_sample_rate(sample_rate as f64);
    }

    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.set_filtermode(filter_type);
    }
}

use self::iir::IIR_MAX_ORDER;

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

        // cascaded biquad buffers
        pub z: [f64; IIR_MAX_ORDER],
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

                z: [0.0; IIR_MAX_ORDER],
            };
            iir.compute_coefficients();
            iir
        }
        /// Runs one sample through the cascade.
        ///
        /// Each biquad is `w = k x - a1 z0 - a2 z1` and `y = w + 2 z0 + z1`, rearranged as
        /// `y = k x + ((2 - a1) z0 + (1 - a2) z1)` so that only a multiplication and an
        /// addition stand between a stage's input and its output: the sum of the delay terms
        /// doesn't depend on the input, so it is computed while the previous stage is still
        /// busy. The lockstep ladder (see `LadderFilter::process_block_x4()`) uses the same
        /// operations in the same order.
        pub fn filter(&mut self, input: f64) -> f64 {
            let mut out = input;

            for ((k, a1, a2), z) in izip!(&self.k, &self.a1, &self.a2)
                .zip(self.z.chunks_exact_mut(2))
                .take(self.decimator_order / 2)
            {
                let (z0, z1) = (z[0], z[1]);
                let recursive = a1 * z0 + a2 * z1;
                let feedforward = (2.0 - a1) * z0 + (1.0 - a2) * z1;
                let scaled = k * out;
                let biquad_in = scaled - recursive;
                out = scaled + feedforward;

                // update delays
                z[1] = z0;
                z[0] = biquad_in;
            }

            out
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
                gain_db(oversampled, freq, |x| iir.filter(x))
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
                    unsafe { LadderFilter::process_block_x4(lanes) };
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

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn lockstep_matches_scalar_processing_exactly() {
        if !is_x86_feature_detected!("avx") {
            return;
        }
        for (lanes, mode) in [
            (4, FilterType::Lowpass),
            (4, FilterType::Bandpass),
            (4, FilterType::Highpass),
            (2, FilterType::Lowpass),
            (1, FilterType::Lowpass),
        ] {
            let input = signal(300);
            let cutoffs = [300.0, 2500.0, 9000.0, 18000.0];
            let resonances = [0.5, 3.0, 7.0, 9.5];
            // (Cloning a `fastrand::Rng` forks it, so both sets are seeded explicitly.)
            let mut scalar: Vec<LadderFilter> = (0..lanes).map(|i| LadderFilter::with_seed(i as u64)).collect();
            let mut lockstep: Vec<LadderFilter> = (0..lanes).map(|i| LadderFilter::with_seed(i as u64)).collect();
            for (i, filter) in scalar.iter_mut().chain(lockstep.iter_mut()).enumerate() {
                let i = i % lanes;
                filter.set_filter_type(mode);
                filter.set_params(44100.0, cutoffs[i], resonances[i]);
            }

            let expected: Vec<Vec<f32>> = scalar
                .iter_mut()
                .map(|filter| input.iter().map(|sample| filter.process(*sample)).collect())
                .collect();

            // Process in two blocks to cover the state being carried across calls
            let mut buffers: Vec<Vec<f32>> = (0..lanes).map(|_| input.clone()).collect();
            for (start, end) in [(0, 128), (128, 300)] {
                let mut entries: Vec<Option<(&mut LadderFilter, &mut [f32])>> = lockstep
                    .iter_mut()
                    .zip(buffers.iter_mut())
                    .map(|(filter, buffer)| Some((filter, &mut buffer[start..end])))
                    .collect();
                entries.resize_with(LADDER_LANES, || None);
                let entries: [Option<(&mut LadderFilter, &mut [f32])>; LADDER_LANES] =
                    entries.try_into().ok().unwrap();
                unsafe { LadderFilter::process_block_x4(entries) };
            }

            for lane in 0..lanes {
                if let Some(first) = (0..input.len()).find(|&i| buffers[lane][i] != expected[lane][i]) {
                    let max_diff = (0..input.len())
                        .map(|i| (buffers[lane][i] - expected[lane][i]).abs())
                        .fold(0.0_f32, f32::max);
                    panic!(
                        "{mode:?} lane {lane} of {lanes}: first difference at sample {first}: lockstep {} vs scalar {} (max difference {max_diff:e}, {} of {} samples differ)",
                        buffers[lane][first],
                        expected[lane][first],
                        (0..input.len()).filter(|&i| buffers[lane][i] != expected[lane][i]).count(),
                        input.len()
                    );
                }
                assert_eq!(buffers[lane], expected[lane], "{mode:?} lane {lane} of {lanes}");
                assert!(expected[lane].iter().any(|sample| sample.abs() > 1e-3));
            }
            // and the state is carried over identically
            for lane in 0..lanes {
                assert_eq!(scalar[lane].process(0.25), lockstep[lane].process(0.25));
            }
        }
    }
}
