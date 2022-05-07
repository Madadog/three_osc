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
                44100.0 * 2.0,
                IIR_DOWNSAMPLING_BANDWIDTH * 44100.0 / 2.0,
                IIR_DOWNSAMPLE_ORDER,
            ),
        }
    }
}
#[allow(dead_code)]
impl LadderFilter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn process_sample(&mut self, mut input: f64) {
        let feedback = 8.0 * self.resonance;
        let noise = 1.0e-6 * 2.0 * (Rng::new().f64() - 0.5);

        input += noise;

        // integrate filter state
        // with oversampling
        for _ in 0..self.oversampling_factor {
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
                    // predictor
                    let p0_prime = self.p0
                        + self.dt
                            * (tanh_pade32(self.ut_1 - feedback * self.p3) - tanh_pade32(self.p0));
                    let p1_prime =
                        self.p1 + self.dt * (tanh_pade32(self.p0) - tanh_pade32(self.p1));
                    let p2_prime =
                        self.p2 + self.dt * (tanh_pade32(self.p1) - tanh_pade32(self.p2));
                    let p3_prime =
                        self.p3 + self.dt * (tanh_pade32(self.p2) - tanh_pade32(self.p3));

                    // corrector
                    let p3t_1 = self.p3;
                    self.p3 = self.p3
                        + 0.5
                            * self.dt
                            * ((tanh_pade32(self.p2) - tanh_pade32(self.p3))
                                + (tanh_pade32(p2_prime) - tanh_pade32(p3_prime)));
                    self.p2 = self.p2
                        + 0.5
                            * self.dt
                            * ((tanh_pade32(self.p1) - tanh_pade32(self.p2))
                                + (tanh_pade32(p1_prime) - tanh_pade32(p2_prime)));
                    self.p1 = self.p1
                        + 0.5
                            * self.dt
                            * ((tanh_pade32(self.p0) - tanh_pade32(self.p1))
                                + (tanh_pade32(p0_prime) - tanh_pade32(p1_prime)));
                    self.p0 = self.p0
                        + 0.5
                            * self.dt
                            * ((tanh_pade32(self.ut_1 - feedback * p3t_1) - tanh_pade32(self.p0))
                                + (tanh_pade32(input - feedback * self.p3)
                                    - tanh_pade32(p0_prime)));
                }
                // predictor-corrector integration
                // with feedback tanh stage only
                IntegrationMethod::PredictorCorrectorFeedbackTanh => {
                    // predictor
                    let p0_prime =
                        self.p0 + self.dt * (tanh_pade32(self.ut_1 - feedback * self.p3) - self.p0);
                    let p1_prime = self.p1 + self.dt * (self.p0 - self.p1);
                    let p2_prime = self.p2 + self.dt * (self.p1 - self.p2);
                    let p3_prime = self.p3 + self.dt * (self.p2 - self.p3);

                    // corrector
                    let p3t_1 = self.p3;
                    self.p3 =
                        self.p3 + 0.5 * self.dt * ((self.p2 - self.p3) + (p2_prime - p3_prime));
                    self.p2 =
                        self.p2 + 0.5 * self.dt * ((self.p1 - self.p2) + (p1_prime - p2_prime));
                    self.p1 =
                        self.p1 + 0.5 * self.dt * ((self.p0 - self.p1) + (p0_prime - p1_prime));
                    self.p0 = self.p0
                        + 0.5
                            * self.dt
                            * ((tanh_pade32(self.ut_1 - feedback * p3t_1) - self.p0)
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
        }

        self.ut_1 = input;

        match self.ladder_filter_mode {
            FilterType::Lowpass => {
                self.out = self.p3;
            }
            FilterType::Bandpass => {
                self.out = self.p1 - self.p3;
            }
            FilterType::Highpass => {
                self.out = tanh_pade32(input - self.p0 - feedback * self.p3);
            }
            #[allow(unreachable_patterns)]
            _ => self.out = 0.0,
        }

        if self.oversampling_factor > 1 {
            self.out = self.iir_lowpass.filter(self.out);
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
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.iir_lowpass.sample_rate = sample_rate * self.oversampling_factor as f64;
        self.iir_lowpass.cutoff_frequency = IIR_DOWNSAMPLING_BANDWIDTH * sample_rate / 2.0;
        self.iir_lowpass.decimator_order = self.decimator_order;

        self.set_integration_rate();
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

mod iir {
    use std::f64::consts::PI;

    use itertools::izip;

    const IIR_MAX_ORDER: usize = 32;

    #[derive(Debug, Clone)]
    pub struct IirFilter {
        // filter design variables
        pub cutoff_frequency: f64,
        pub sample_rate: f64,
        pub decimator_order: usize,

        // dsp variables
        a1: [f64; IIR_MAX_ORDER / 2],
        a2: [f64; IIR_MAX_ORDER / 2],
        k: [f64; IIR_MAX_ORDER / 2],
        pa_real: [f64; IIR_MAX_ORDER / 2],
        pa_imag: [f64; IIR_MAX_ORDER / 2],
        p_real: [f64; IIR_MAX_ORDER / 2],
        p_imag: [f64; IIR_MAX_ORDER / 2],

        // cascaded biquad buffers
        z: [f64; IIR_MAX_ORDER],
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
        pub fn filter(&mut self, input: f64) -> f64 {
            let mut out = input;

            for ((k, a1, a2), z) in izip!(self.k, self.a1, self.a2)
                .zip(self.z.chunks_exact_mut(2))
                .take(self.decimator_order / 2)
            {
                // compute biquad input
                let biquad_in = k * out - a1 * z[0] - a2 * z[1];

                // compute biquad output
                out = biquad_in + 2.0 * z[0] + z[1];

                // update delays
                z[1] = z[0];
                z[0] = biquad_in;
            }

            out
        }

        pub fn set_order(&mut self, order: usize) {
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
