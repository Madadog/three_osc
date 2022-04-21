use super::lerp;

use super::envelopes::AdsrEnvelope;

use std::f32::consts::PI;

#[derive(Debug, Default, Clone)]
/// Reproduced from https://ccrma.stanford.edu/~jos/filters/Direct_Form_II.html
/// 
pub(crate) struct TestFilter {
    pub(crate) stage0: f32, pub(crate) // internal feedback storage
    stage1: f32,
    pub(crate) a0: f32, // gain compensation
    pub(crate) a1: f32, // [n-1] feedback
    pub(crate) a2: f32, // [n-2] feedback
    pub(crate) b0: f32, // [n] out
    pub(crate) b1: f32, // [n-1] out
    pub(crate) b2: f32, pub(crate) // [n-2] out
    target_a: (f32, f32, f32), pub(crate) // smoothing
    target_b: (f32, f32, f32),
}

impl Filter for TestFilter {
    fn process(&mut self, input: f32) -> f32 {
        if !(self.stage0.is_finite() && self.stage1.is_finite()) {
            println!(
                "Warning: filters were unstable, {} and {}",
                self.stage0, self.stage1
            );
            self.stage0 = 0.0;
            self.stage1 = 0.0;
        }

        let previous_previous_sample = self.stage1;
        let previous_sample = self.stage0;
        let current_sample = (input - self.a1 * self.stage0 - self.a2 * self.stage1) / self.a0;
        //let current_sample = -self.stage0.mul_add(self.a1,  -self.stage1.mul_add(self.a2, input));

        // Propogate
        self.stage0 = current_sample;
        self.stage1 = previous_sample;

        (self.b0 * current_sample + self.b1 * previous_sample + self.b2 * previous_previous_sample)
            / self.a0
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        // Coefficients and formulas from https://www.w3.org/TR/audio-eq-cookbook/

        // "This software or document includes material copied from or derived from Audio Eq Cookbook (https://www.w3.org/TR/audio-eq-cookbook/). Copyright © 2021 W3C® (MIT, ERCIM, Keio, Beihang)." 
    
        // [This notice should be placed within redistributed or derivative software code or text when appropriate. This particular formulation became active on May 13, 2015, and edited for clarity 7 April, 2021, superseding the 2002 version.]
        // Audio Eq Cookbook: https://www.w3.org/TR/audio-eq-cookbook/
        // Copyright © 2021 World Wide Web Consortium, (Massachusetts Institute of Technology, European Research Consortium for Informatics and Mathematics, Keio University, Beihang). All Rights Reserved. This work is distributed under the W3C® Software and Document License [1] in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
        // [1] http://www.w3.org/Consortium/Legal/copyright-software

        let phase_change = 2.0 * PI * cutoff / sample_rate;
        let (sin, cos) = phase_change.sin_cos();
        let a = sin / (2.0 * resonance);

        self.b0 = (1.0 - cos) / 2.0;
        self.b1 = 1.0 - cos;
        self.b2 = (1.0 - cos) / 2.0;

        // self.a0 = 1.0 + a;
        self.a0 = 1.0 + a;
        self.a1 = -2.0 * cos;
        self.a2 = 1.0 - a;
    }
}

#[derive(Debug, Default)]
/// Filter in series
pub(crate) struct CascadeFilter {
    pub(crate) filter_1: TestFilter,
    pub(crate) filter_2: TestFilter,
}

impl Filter for CascadeFilter {
    fn process(&mut self, input: f32) -> f32 {
        self.filter_2.process(self.filter_1.process(input))
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        self.filter_1.set_params(sample_rate, cutoff, resonance);
        self.filter_2.set_params(sample_rate, cutoff, resonance);
    }
}

pub(crate) trait Filter {
    fn process(&mut self, input: f32) -> f32;
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32);
}

#[derive(Debug)]
/// Filter in series
pub(crate) struct FilterController {
    pub(crate) cutoff_envelope: AdsrEnvelope,
    pub(crate) envelope_amount: f32,
    pub(crate) cutoff: f32,
    pub(crate) resonance: f32,
    pub(crate) keytrack: f32,
}

impl FilterController {
    pub(crate) fn new() -> Self {
        Self {
            cutoff_envelope: AdsrEnvelope::new(0.0, 0.0, 0.0, 1.0, 1.0),
            envelope_amount: 0.0,
            cutoff: 100.0,
            resonance: 0.1,
            keytrack: 0.0,
        }
    }
    pub(crate) fn process_envelope_held(&mut self, filter: &mut impl Filter, keytrack_freq: f32, input: f32, envelope_index: f32, sample_rate: f32) -> f32 {
        filter.set_params(sample_rate,
            (self.cutoff + keytrack_freq + self.cutoff_envelope.sample_held(envelope_index) * self.envelope_amount).clamp(1.0, 22000.0),
            self.resonance
        );
        let out = filter.process(input);
        filter.set_params(sample_rate, self.cutoff, self.resonance);
        out
    }
    pub(crate) fn process_envelope_released(&mut self, filter: &mut impl Filter, keytrack_freq: f32, input: f32, envelope_index: f32, release_index: f32, sample_rate: f32) -> f32 {
        filter.set_params(sample_rate,
            (self.cutoff + keytrack_freq + self.cutoff_envelope.sample_released(release_index, envelope_index) * self.envelope_amount).clamp(1.0, 22000.0),
            self.resonance
        );
        let out = filter.process(input);
        filter.set_params(sample_rate, self.cutoff, self.resonance);
        out
    }

    pub(crate) fn lerp_controls(&mut self, filter: &mut TestFilter, sample_rate: f32, target_cutoff: f32, target_resonance: f32) {
        self.cutoff = lerp(self.cutoff, target_cutoff, 500.0 / sample_rate);
        self.resonance = lerp(self.cutoff, target_resonance, 500.0 / sample_rate);
        filter.set_params(sample_rate, self.cutoff, self.resonance);
    }
}

pub enum FilterType {
    Lowpass,
    Bandpass,
    Highpass,
}

pub enum FilterOrder {
    _12dB,
    _24dB,
}

/// Port of the LMMS RC filters (originally from https://github.com/LMMS/lmms/blob/master/include/BasicFilters.h)
pub struct RcFilter {
    rca: f32,
    rcb: f32,
    rcc: f32,
    rcq: f32,

    last: f32,
    bp: f32,
    lp: f32,
    hp: f32,

    order: FilterOrder,
    filter_type: FilterType,
}
impl Filter for RcFilter {
    // Original notice:
    // 4-times oversampled simulation of an active RC-Bandpass,-Lowpass,-Highpass-
    // Filter-Network as it was used in nearly all modern analog synthesizers. This
    // can be driven up to self-oscillation (BTW: do not remove the limits!!!).
    // (C) 1998 ... 2009 S.Fendt. Released under the GPL v2.0  or any later version.
    fn process(&mut self, input: f32) -> f32 {
        match self.order {
            FilterOrder::_12dB => {
                match self.filter_type {
                    FilterType::Lowpass => {
                        for _ in 0..4 {
                            let old_last = self.last;
                            self.last = (input + self.bp * self.rcq).clamp(-1.0, 1.0);
                            
                            self.lp = (self.last * self.rcb + self.lp * self.rca).clamp(-1.0, 1.0);

                            self.hp = (self.rcc * (self.hp + self.last - old_last)).clamp(-1.0, 1.0);

                            self.bp = (self.hp * self.rcb + self.bp * self.rca).clamp(-1.0, 1.0);
                        }
                        self.lp
                    },
                    FilterType::Bandpass => todo!(),
                    FilterType::Highpass => todo!(),
                }
            },
            FilterOrder::_24dB => todo!(),
        }
    }

    fn set_params(&mut self, sample_rate: f32, mut cutoff: f32, resonance: f32) {
        cutoff = cutoff.clamp(20.0, 21000.0);
        let delta = (1.0 / sample_rate) * 0.25;
        let freq = 1.0 / (cutoff * PI * 2.0);

        self.rca = 1.0 - delta / (freq + delta);
        self.rcb = 1.0 - self.rca;
        self.rcb = freq / (freq + delta);

        self.rcq = resonance * 0.25;
    }
}