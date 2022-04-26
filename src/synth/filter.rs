use self::ladder::LadderFilter;

use super::lerp;

use super::envelopes::AdsrEnvelope;

use std::f32::consts::PI;

#[derive(Debug, Default, Clone)]
/// Reproduced from https://ccrma.stanford.edu/~jos/filters/Direct_Form_II.html
///
pub struct TestFilter {
    pub(crate) stage0: f32,
    pub(crate) stage1: f32,
    pub(crate) a0: f32, // gain compensation
    pub(crate) a1: f32, // [n-1] feedback
    pub(crate) a2: f32, // [n-2] feedback
    pub(crate) b0: f32, // [n] out
    pub(crate) b1: f32, // [n-1] out
    pub(crate) b2: f32,
    pub(crate) target_a: (f32, f32, f32),
    pub(crate) target_b: (f32, f32, f32),
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
    fn set_filter_type(&mut self, filter_type: FilterType) {}
}

#[derive(Debug)]
/// Filter in series
pub(crate) struct FilterController {
    pub(crate) cutoff_envelope: AdsrEnvelope,
    pub(crate) envelope_amount: f32,
    cutoff: f32,
    pub(crate) target_cutoff: f32,
    pub(crate) resonance: f32,
    pub(crate) drive: f32,
    pub(crate) keytrack: f32,
    pub(crate) filter_type: FilterType,
    pub(crate) filter_model: FilterModel,
}

impl FilterController {
    pub(crate) fn new() -> Self {
        Self {
            cutoff_envelope: AdsrEnvelope::new(0.0, 0.0, 0.0, 1.0, 1.0),
            envelope_amount: 0.0,
            cutoff: 100.0,
            target_cutoff: 100.0,
            resonance: 0.1,
            drive: 1.0,
            keytrack: 0.0,
            filter_type: FilterType::Lowpass,
            filter_model: FilterModel::RcFilter,
        }
    }
    pub(crate) fn lerp_cutoff(&mut self, amount: f32) {
        self.cutoff = lerp(self.cutoff, self.target_cutoff, amount);
    }
    pub(crate) fn process_envelope_held(
        &mut self,
        filter: &mut impl Filter,
        keytrack_freq: f32,
        input: f32,
        envelope_index: f32,
        sample_rate: f32,
    ) -> f32 {
        filter.set_params(
            sample_rate,
            (self.cutoff
                + keytrack_freq
                + self.cutoff_envelope.sample_held(envelope_index) * self.envelope_amount)
                .clamp(1.0, 22000.0),
            self.resonance,
        );
        let out = filter.process(input * self.drive);
        filter.set_params(sample_rate, self.cutoff, self.resonance);
        out
    }
    pub(crate) fn process_envelope_released(
        &mut self,
        filter: &mut impl Filter,
        keytrack_freq: f32,
        input: f32,
        envelope_index: f32,
        release_index: f32,
        sample_rate: f32,
    ) -> f32 {
        filter.set_params(
            sample_rate,
            (self.cutoff
                + keytrack_freq
                + self
                    .cutoff_envelope
                    .sample_released(release_index, envelope_index)
                    * self.envelope_amount)
                .clamp(1.0, 22000.0),
            self.resonance,
        );
        let out = filter.process(input * self.drive);
        filter.set_params(sample_rate, self.cutoff, self.resonance);
        out
    }

    pub(crate) fn lerp_controls(
        &mut self,
        filter: &mut TestFilter,
        sample_rate: f32,
        target_cutoff: f32,
        target_resonance: f32,
    ) {
        self.cutoff = lerp(self.cutoff, target_cutoff, 500.0 / sample_rate);
        self.resonance = lerp(self.cutoff, target_resonance, 500.0 / sample_rate);
        filter.set_params(sample_rate, self.cutoff, self.resonance);
    }
}

#[derive(Debug, Clone)]
/// A filter that can be switched between multiple filter modes.
// TODO: There must be a better way to do this.
pub enum FilterContainer {
    None,
    RcFilter(RcFilter),
    LadderFilter(LadderFilter),
    BiquadFilter(TestFilter),
}
impl FilterContainer {
    pub fn set(&mut self, filter_model: FilterModel) {
        match (self, filter_model) {
            (FilterContainer::RcFilter(_), FilterModel::RcFilter) => {},
            (FilterContainer::LadderFilter(_), FilterModel::LadderFilter) => {},
            (FilterContainer::BiquadFilter(_), FilterModel::BiquadFilter) => {},
            (x, FilterModel::RcFilter) => {*x = FilterContainer::RcFilter(RcFilter::default())},
            (x, FilterModel::LadderFilter) => {*x = FilterContainer::LadderFilter(LadderFilter::default())},
            (x, FilterModel::BiquadFilter) => {*x = FilterContainer::BiquadFilter(TestFilter::default())},
            (x, FilterModel::None) => {*x = FilterContainer::None},
            (_, _) => {unreachable!("Forgot to set a FilterModel for FilterContainer")}
        }
    }
}
impl Filter for FilterContainer {
    fn process(&mut self, input: f32) -> f32 {
        match self {
            FilterContainer::RcFilter(x) => x.process(input),
            FilterContainer::LadderFilter(x) => x.process(input),
            FilterContainer::BiquadFilter(x) => x.process(input),
            _ => {input}
        }
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        match self {
            FilterContainer::RcFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            FilterContainer::LadderFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            FilterContainer::BiquadFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            _ => {}
        }
    }
    fn set_filter_type(&mut self, filter_type: FilterType) {
        match self {
            FilterContainer::RcFilter(x) => x.set_filter_type(filter_type),
            FilterContainer::LadderFilter(x) => x.set_filter_type(filter_type),
            FilterContainer::BiquadFilter(x) => x.set_filter_type(filter_type),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Used to select FilterContainer without creating a filter.
pub enum FilterModel {
    None,
    RcFilter,
    LadderFilter,
    BiquadFilter,
}

#[derive(Debug, Clone, Copy)]
pub enum FilterType {
    Lowpass,
    Bandpass,
    Highpass,
}

#[derive(Debug, Clone, Copy)]
pub enum FilterOrder {
    _12dB,
    _24dB,
}

/// Port of the LMMS RC filters (originally from https://github.com/LMMS/lmms/blob/master/include/BasicFilters.h)
#[derive(Debug, Clone)]
pub struct RcFilter {
    rca: f32,
    rcb: f32,
    rcc: f32,
    rcq: f32,

    // First stage
    last0: f32,
    bp0: f32,
    lp0: f32,
    hp0: f32,

    // Second stage
    last1: f32,
    bp1: f32,
    lp1: f32,
    hp1: f32,

    order: FilterOrder,
    filter_type: FilterType,
}
impl RcFilter {
    /// Filters a single sample through the RC filter's first stage
    #[inline]
    fn step_first_stage(&mut self, input: f32) {
        let temp_in = (input + self.bp0 * self.rcq).clamp(-1.0, 1.0);
        let lp = (temp_in * self.rcb + self.lp0 * self.rca).clamp(-1.0, 1.0);
        let hp = (self.rcc * (self.hp0 + temp_in - self.last0)).clamp(-1.0, 1.0);
        let bp = (hp * self.rcb + self.bp0 * self.rca).clamp(-1.0, 1.0);

        self.last0 = temp_in;
        self.lp0 = lp;
        self.hp0 = hp;
        self.bp0 = bp;
    }
    /// Filters a single sample through the RC filter's second stage
    #[inline]
    fn step_second_stage(&mut self, input: f32) {
        let temp_in = (input + self.bp1 * self.rcq).clamp(-1.0, 1.0);
        let lp = (temp_in * self.rcb + self.lp1 * self.rca).clamp(-1.0, 1.0);
        let hp = (self.rcc * (self.hp1 + temp_in - self.last1)).clamp(-1.0, 1.0);
        let bp = (hp * self.rcb + self.bp1 * self.rca).clamp(-1.0, 1.0);
        
        self.last1 = temp_in;
        self.lp1 = lp;
        self.hp1 = hp;
        self.bp1 = bp;
    }
    /// Filters a single sample through the RC filter's first stage using tanh instead of hard clipping
    #[inline]
    fn step_first_stage_tanh(&mut self, input: f32) {
        let temp_in = (input + self.bp0 * self.rcq).tanh();
        let lp = (temp_in * self.rcb + self.lp0 * self.rca).tanh();
        let hp = (self.rcc * (self.hp0 + temp_in - self.last0)).tanh();
        let bp = (hp * self.rcb + self.bp0 * self.rca).tanh();

        self.last0 = temp_in;
        self.lp0 = lp;
        self.hp0 = hp;
        self.bp0 = bp;
    }
    /// Filters a single sample through the RC filter's second stage using tanh instead of hard clipping
    #[inline]
    fn step_second_stage_tanh(&mut self, input: f32) {
        let temp_in = (input + self.bp1 * self.rcq).tanh();
        let lp = (temp_in * self.rcb + self.lp1 * self.rca).tanh();
        let hp = (self.rcc * (self.hp1 + temp_in - self.last1)).tanh();
        let bp = (hp * self.rcb + self.bp1 * self.rca).tanh();
        
        self.last1 = temp_in;
        self.lp1 = lp;
        self.hp1 = hp;
        self.bp1 = bp;
    }
    /// First-order-filters a single sample and returns a tuple of (lp, hp, bp)
    #[inline]
    pub fn filter_all(&mut self, input: f32) -> (f32, f32, f32) {
        for _ in 0..4 {
            self.step_first_stage(input);
        }
        (self.lp0, self.hp0, self.bp0)
    }
    #[inline]
    pub fn filter_2nd_order(&mut self, input: f32) -> f32 {
        for _ in 0..4 {
            self.step_first_stage(input);
            match self.filter_type {
                FilterType::Lowpass => self.step_second_stage(self.lp0),
                FilterType::Bandpass => self.step_second_stage(self.bp0),
                FilterType::Highpass => self.step_second_stage(self.hp0),
            }
        }
        match self.filter_type {
            FilterType::Lowpass => self.lp1,
            FilterType::Bandpass => self.bp1,
            FilterType::Highpass => self.hp1,
        }
    }
    #[inline]
    pub fn filter_all_tanh(&mut self, input: f32) -> (f32, f32, f32) {
        for _ in 0..4 {
            self.step_first_stage_tanh(input);
        }
        (self.lp0, self.hp0, self.bp0)
    }
    #[inline]
    pub fn filter_2nd_order_tanh(&mut self, input: f32) -> f32 {
        for _ in 0..4 {
            self.step_first_stage_tanh(input);
            match self.filter_type {
                FilterType::Lowpass => self.step_second_stage_tanh(self.lp0),
                FilterType::Bandpass => self.step_second_stage_tanh(self.bp0),
                FilterType::Highpass => self.step_second_stage_tanh(self.hp0),
            }
        }
        match self.filter_type {
            FilterType::Lowpass => self.lp1,
            FilterType::Bandpass => self.bp1,
            FilterType::Highpass => self.hp1,
        }
    }
}
impl Filter for RcFilter {
    // Original notice:
    // 4-times oversampled simulation of an active RC-Bandpass,-Lowpass,-Highpass-
    // Filter-Network as it was used in nearly all modern analog synthesizers. This
    // can be driven up to self-oscillation (BTW: do not remove the limits!!!).
    // (C) 1998 ... 2009 S.Fendt. Released under the GPL v2.0  or any later version.
    fn process(&mut self, input: f32) -> f32 {
        match &self.order {
            FilterOrder::_12dB => match &self.filter_type {
                FilterType::Lowpass => self.filter_all(input).0,
                FilterType::Bandpass => self.filter_all(input).1,
                FilterType::Highpass => self.filter_all(input).2,
            },
            FilterOrder::_24dB => self.filter_2nd_order(input),
        }
        // match &self.order {
        //     FilterOrder::_12dB => match &self.filter_type {
        //         FilterType::Lowpass => self.filter_all_tanh(input).0,
        //         FilterType::Bandpass => self.filter_all_tanh(input).1,
        //         FilterType::Highpass => self.filter_all_tanh(input).2,
        //     },
        //     FilterOrder::_24dB => self.filter_2nd_order_tanh(input),
        // }
    }

    fn set_params(&mut self, sample_rate: f32, mut cutoff: f32, resonance: f32) {
        cutoff = cutoff.clamp(5.0, 22000.0);
        let delta = (1.0 / sample_rate) / 4.0; // division by 4.0 occurs because of oversampling when processing... 
        let freq = 1.0 / (cutoff * PI * 2.0);

        self.rca = 1.0 - delta / (freq + delta);
        self.rcb = 1.0 - self.rca;
        self.rcc = freq / (freq + delta);

        self.rcq = resonance * 0.25;
        // println!("rcq: {}", self.rcq);
    }
    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.filter_type = filter_type;
    }
}
impl Default for RcFilter {
    fn default() -> Self {
        Self {
            rca: Default::default(),
            rcb: Default::default(),
            rcc: Default::default(),
            rcq: Default::default(),
            last0: Default::default(),
            bp0: Default::default(),
            lp0: Default::default(),
            hp0: Default::default(),
            last1: Default::default(),
            bp1: Default::default(),
            lp1: Default::default(),
            hp1: Default::default(),
            order: FilterOrder::_24dB,
            filter_type: FilterType::Lowpass,
        }
    }
}

mod ladder;