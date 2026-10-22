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
// 

// implemented from https://cytomic.com/files/dsp/SvfLinearTrapOptimised2.pdf
// thanks, andy!

use wide::f32x8;

use super::super::fastmath;
use super::{Filter, FilterType};

/// Number of filters [`SvfSimper::process_block_x8()`] runs in lockstep.
pub const SVF_LANES: usize = 8;

#[derive(Debug, Clone)]
pub struct SvfSimper {
    pub a1: f32,
    pub a2: f32,
    pub a3: f32,

    pub ic1eq: f32,
    pub ic2eq: f32,

    k: f32,

    /// Per sample change of the coefficients while ramping towards new parameters, see
    /// `set_params_ramped()`.
    a1_step: f32,
    a2_step: f32,
    a3_step: f32,
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
            a1,
            a2,
            a3,

            ic1eq: 0.0,
            ic2eq: 0.0,

            k,

            a1_step: 0.0,
            a2_step: 0.0,
            a3_step: 0.0,
            k_step: 0.0,

            filter_type: FilterType::Lowpass,
        }
    }

    pub fn set(&mut self, cutoff: f32, resonance: f32, sample_rate: f32) {
        let new = Self::new(cutoff, resonance, sample_rate);

        self.a1 = new.a1;
        self.a2 = new.a2;
        self.a3 = new.a3;
        self.k = new.k;
        self.a1_step = 0.0;
        self.a2_step = 0.0;
        self.a3_step = 0.0;
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
    pub fn process_block_x8(mut lanes: [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES]) {
        let Some(first) = lanes
            .iter()
            .find_map(|lane| lane.as_ref().map(|(filter, _)| &**filter))
        else {
            return;
        };
        let filter_type = first.filter_type;
        let block_len = lanes
            .iter()
            .flatten()
            .map(|(_, buffer)| buffer.len())
            .min()
            .unwrap_or(0);

        // Gather the state of each lane. Empty lanes stay at zero and produce zeros.
        let gather = |get: fn(&SvfSimper) -> f32| {
            let mut values = [0.0_f32; SVF_LANES];
            for (value, lane) in values.iter_mut().zip(lanes.iter()) {
                if let Some((filter, _)) = lane {
                    *value = get(filter);
                }
            }
            f32x8::new(values)
        };
        let mut a1 = gather(|filter| filter.a1);
        let mut a2 = gather(|filter| filter.a2);
        let mut a3 = gather(|filter| filter.a3);
        let mut k = gather(|filter| filter.k);
        let a1_step = gather(|filter| filter.a1_step);
        let a2_step = gather(|filter| filter.a2_step);
        let a3_step = gather(|filter| filter.a3_step);
        let k_step = gather(|filter| filter.k_step);
        let mut ic1eq = gather(|filter| filter.ic1eq);
        let mut ic2eq = gather(|filter| filter.ic2eq);
        let two = f32x8::splat(2.0);

        let mut input = [0.0_f32; SVF_LANES];
        for sample in 0..block_len {
            for (value, lane) in input.iter_mut().zip(lanes.iter()) {
                if let Some((_, buffer)) = lane {
                    *value = buffer[sample];
                }
            }
            let input = f32x8::new(input);

            // The same operations, in the same order, as `process()`
            a1 += a1_step;
            a2 += a2_step;
            a3 += a3_step;
            k += k_step;

            let v3 = input - ic2eq;
            let v1 = (a1 * ic1eq) + (a2 * v3);
            let v2 = ic2eq + (a2 * ic1eq) + (a3 * v3);

            ic1eq = (two * v1) - ic1eq;
            ic2eq = (two * v2) - ic2eq;

            let output = match filter_type {
                FilterType::Lowpass => v2,
                FilterType::Bandpass => v1,
                FilterType::Highpass => input - k * v1 - v2,
            };
            for (value, lane) in output.to_array().iter().zip(lanes.iter_mut()) {
                if let Some((_, buffer)) = lane {
                    buffer[sample] = *value;
                }
            }
        }

        // Scatter the state back
        let (a1, a2, a3, k) = (a1.to_array(), a2.to_array(), a3.to_array(), k.to_array());
        let (ic1eq, ic2eq) = (ic1eq.to_array(), ic2eq.to_array());
        for (lane, entry) in lanes.iter_mut().enumerate() {
            if let Some((filter, _)) = entry {
                filter.a1 = a1[lane];
                filter.a2 = a2[lane];
                filter.a3 = a3[lane];
                filter.k = k[lane];
                filter.ic1eq = ic1eq[lane];
                filter.ic2eq = ic2eq[lane];
            }
        }
    }
}
impl Filter for SvfSimper {
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        self.a1 += self.a1_step;
        self.a2 += self.a2_step;
        self.a3 += self.a3_step;
        self.k += self.k_step;

        let v3 = input - self.ic2eq;
        let v1 = (self.a1 * self.ic1eq) + (self.a2 * v3);
        let v2 = self.ic2eq + (self.a2 * self.ic1eq) + (self.a3 * v3);

        self.ic1eq = (2.0 * v1) - self.ic1eq;
        self.ic2eq = (2.0 * v2) - self.ic2eq;

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
        self.a1_step = (new.a1 - self.a1) * scale;
        self.a2_step = (new.a2 - self.a2) * scale;
        self.a3_step = (new.a3 - self.a3) * scale;
        self.k_step = (new.k - self.k) * scale;
    }

    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.filter_type = filter_type;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lockstep block processing produces exactly the same samples and state as processing
    /// each filter on its own, for every filter type and any number of present lanes, while
    /// the coefficients ramp.
    #[test]
    fn lockstep_matches_scalar_processing_exactly() {
        let input: Vec<f32> = (0..256)
            .map(|i| ((i % 97) as f32 / 48.5 - 1.0) * 0.5 + ((i % 61) as f32 / 30.5 - 1.0) * 0.3)
            .collect();
        for filter_type in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
            for lanes in [1, 3, 8] {
                let make = |lane: usize| {
                    let mut filter = SvfSimper::new(300.0 + 500.0 * lane as f32, 0.3, 44100.0);
                    filter.filter_type = filter_type;
                    filter
                };
                let mut scalar: Vec<SvfSimper> = (0..lanes).map(make).collect();
                let mut lockstep: Vec<SvfSimper> = (0..lanes).map(make).collect();
                let mut expected = vec![input.clone(); lanes];
                let mut buffers = vec![input.clone(); lanes];

                for start in (0..input.len()).step_by(32) {
                    let end = (start + 32).min(input.len());
                    for lane in 0..lanes {
                        let cutoff = 400.0 + 900.0 * lane as f32 + start as f32;
                        scalar[lane].set_params_ramped(44100.0, cutoff, 2.0, end - start);
                        lockstep[lane].set_params_ramped(44100.0, cutoff, 2.0, end - start);
                        for sample in &mut expected[lane][start..end] {
                            *sample = scalar[lane].process(*sample);
                        }
                    }
                    let mut entries: Vec<Option<(&mut SvfSimper, &mut [f32])>> = lockstep
                        .iter_mut()
                        .zip(buffers.iter_mut())
                        .map(|(filter, buffer)| Some((filter, &mut buffer[start..end])))
                        .collect();
                    entries.resize_with(SVF_LANES, || None);
                    let entries: [Option<(&mut SvfSimper, &mut [f32])>; SVF_LANES] =
                        entries.try_into().ok().unwrap();
                    SvfSimper::process_block_x8(entries);
                }

                for lane in 0..lanes {
                    assert_eq!(buffers[lane], expected[lane], "{filter_type:?} lane {lane} of {lanes}");
                    assert!(expected[lane].iter().any(|sample| sample.abs() > 1e-3));
                    // and the state is carried over identically
                    assert_eq!(scalar[lane].process(0.25), lockstep[lane].process(0.25));
                }
            }
        }
    }
}
