use self::ladder::{tanh_pade32_f32, LadderFilter};
use self::svf_simper::SvfSimper;

use super::lerp;

use super::envelopes::AdsrEnvelope;

use std::f32::consts::PI;

#[derive(Debug, Default, Clone)]
/// Reproduced from https://ccrma.stanford.edu/~jos/filters/Direct_Form_II.html
///
pub struct BiquadFilter {
    pub(crate) stage0: f32,
    pub(crate) stage1: f32,
    pub(crate) a0: f32, // gain compensation
    pub(crate) a1: f32, // [n-1] feedback
    pub(crate) a2: f32, // [n-2] feedback
    pub(crate) b0: f32, // [n] out
    pub(crate) b1: f32, // [n-1] out
    pub(crate) b2: f32,
    // targets for coefficient interpolation:
    pub(crate) target_a: (f32, f32, f32),
    pub(crate) target_b: (f32, f32, f32),
    /// Default coefficient interpolation rate 
    pub(crate) lerp_amount: f32,
    pub(crate) filter_type: FilterType,
}
impl BiquadFilter {
    fn lerp_params(&mut self, amount: f32) {
        self.a0 = lerp(self.a0, self.target_a.0, amount);
        self.a1 = lerp(self.a1, self.target_a.1, amount);
        self.a2 = lerp(self.a2, self.target_a.2, amount);
        self.b0 = lerp(self.b0, self.target_b.0, amount);
        self.b1 = lerp(self.b1, self.target_b.1, amount);
        self.b2 = lerp(self.b2, self.target_b.2, amount);
    }
    pub fn with_params(
        cutoff: f32,
        resonance: f32,
        sample_rate: f32,
        filter_type: FilterType,
    ) -> BiquadFilter {
        let mut filter = BiquadFilter::default();
        let coeffs = BiquadFilter::calc_coef(cutoff, resonance, sample_rate, &filter_type);

        filter.a0 = coeffs.0;
        filter.a1 = coeffs.1;
        filter.a2 = coeffs.2;
        filter.b0 = coeffs.3;
        filter.b1 = coeffs.4;
        filter.b2 = coeffs.5;

        filter.target_a = (filter.a0, filter.a1, filter.a2);
        filter.target_b = (filter.b0, filter.b1, filter.b2);

        filter.lerp_amount = 705.6 / sample_rate;

        filter
    }
    /// Outputs biquad coefficients in the format (a0, a1, a2, b0, b1, b2)
    pub fn calc_coef(
        cutoff: f32,
        resonance: f32,
        sample_rate: f32,
        filter_type: &FilterType,
    ) -> (f32, f32, f32, f32, f32, f32) {
        // Biquad is less stable than other filters at low frequencies, clamp to 30 Hz minimum.
        let cutoff = cutoff.max(30.0);

        // Coefficients and formulas from https://www.w3.org/TR/audio-eq-cookbook/

        // "This software or document includes material copied from or derived from Audio Eq Cookbook (https://www.w3.org/TR/audio-eq-cookbook/). Copyright © 2021 W3C® (MIT, ERCIM, Keio, Beihang)."

        // [This notice should be placed within redistributed or derivative software code or text when appropriate. This particular formulation became active on May 13, 2015, and edited for clarity 7 April, 2021, superseding the 2002 version.]
        // Audio Eq Cookbook: https://www.w3.org/TR/audio-eq-cookbook/
        // Copyright © 2021 World Wide Web Consortium, (Massachusetts Institute of Technology, European Research Consortium for Informatics and Mathematics, Keio University, Beihang). All Rights Reserved. This work is distributed under the W3C® Software and Document License [1] in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
        // [1] http://www.w3.org/Consortium/Legal/copyright-software

        let phase_change = 2.0 * PI * cutoff / sample_rate;
        let (sin, cos) = phase_change.sin_cos();
        let alpha = sin / (2.0 * resonance);

        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos / a0;
        let a2 = (1.0 - alpha) / a0;

        let (b0, b1, b2) = match filter_type {
            FilterType::Lowpass => {
                let b1 = (1.0 - cos) / a0;
                let b0 = b1 / 2.0;
                (b0, b1, b0)
            }
            FilterType::Bandpass => {
                let b0 = sin / 2.0 / a0;
                (b0, 0.0, -b0)
            }
            FilterType::Highpass => {
                let b1 = (-1.0 - cos) / a0;
                let b0 = -b1 / 2.0;
                (b0, b1, b0)
            }
        };

        (a0, a1, a2, b0, b1, b2)
    }
}

impl Filter for BiquadFilter {
    fn process(&mut self, input: f32) -> f32 {
        self.lerp_params(self.lerp_amount);

        let previous_previous_sample = self.stage1;
        let previous_sample = self.stage0;
        let current_sample = input - self.a1 * self.stage0 - self.a2 * self.stage1;

        // Propogate
        self.stage0 = current_sample;
        self.stage1 = previous_sample;

        if !(self.stage1.is_finite() && self.stage0.is_finite()) {
            println!(
                "Warning: filters were unstable, {} and {}",
                self.stage0, self.stage1
            );
            self.stage0 = 0.0;
            self.stage1 = 0.0;
        }

        self.b0 * self.stage0 + self.b1 * self.stage1 + self.b2 * previous_previous_sample
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        let coeffs = BiquadFilter::calc_coef(cutoff, resonance, sample_rate, &self.filter_type);

        self.target_a.0 = coeffs.0;
        self.target_a.1 = coeffs.1;
        self.target_a.2 = coeffs.2;

        self.target_b.0 = coeffs.3;
        self.target_b.1 = coeffs.4;
        self.target_b.2 = coeffs.5;
    }
    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.filter_type = filter_type;
    }
}

#[derive(Debug, Default)]
/// Filter in series
pub(crate) struct CascadeFilter {
    pub(crate) filter_1: BiquadFilter,
    pub(crate) filter_2: BiquadFilter,
}

impl Filter for CascadeFilter {
    fn process(&mut self, input: f32) -> f32 {
        self.filter_2.process(self.filter_1.process(input))
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        self.filter_1.set_params(sample_rate, cutoff, resonance);
        self.filter_2.set_params(sample_rate, cutoff, resonance);
    }
    fn set_filter_type(&mut self, filter_type: FilterType) {
        self.filter_1.set_filter_type(filter_type);
        self.filter_2.set_filter_type(filter_type);
    }
}

pub(crate) trait Filter {
    fn process(&mut self, input: f32) -> f32;
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32);
    fn set_filter_type(&mut self, filter_type: FilterType);
}

#[derive(Debug)]
/// Applies an envelope to something that implements the `Filter` trait.
/// Also handles keytrack.
pub(crate) struct FilterController {
    pub(crate) cutoff_envelope: AdsrEnvelope,
    pub(crate) envelope_amount: f32,
    pub(crate) cutoff: f32,
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
    pub fn get_cutoff(
        &self,
        cutoff_mult: f32,
        envelope_index: f32,
        release_index: Option<u32>,
        sample_rate: f32,
    ) -> f32 {
        let envelope = if let Some(release_index) = release_index {
            let release_time = release_index as f32 / sample_rate as f32;
            self.cutoff_envelope
                .sample_released(release_time, envelope_index)
        } else {
            self.cutoff_envelope.sample_held(envelope_index)
        };
        let envelope = envelope * self.envelope_amount * (440.0 + self.cutoff * cutoff_mult) * 50.0;
        (self.cutoff * cutoff_mult + envelope).clamp(10.0, 22000.0)
    }
}

#[derive(Debug, Clone)]
/// A filter that can be switched between multiple filter modes.
// TODO: There must be a better way to do this.
pub enum FilterContainer {
    None,
    RcFilter(RcFilter),
    LadderFilter(LadderFilter),
    BiquadFilter(BiquadFilter),
    SvfSimperFilter(SvfSimper),
}
impl FilterContainer {
    pub fn set(
        &mut self,
        filter_model: FilterModel,
        cutoff: f32,
        resonance: f32,
        sample_rate: f32,
        filter_type: FilterType,
    ) {
        match (self, filter_model) {
            (FilterContainer::RcFilter(_), FilterModel::RcFilter) => {}
            (FilterContainer::LadderFilter(_), FilterModel::LadderFilter) => {}
            (FilterContainer::BiquadFilter(_), FilterModel::BiquadFilter) => {}
            (FilterContainer::SvfSimperFilter(_), FilterModel::SvfSimperFilter) => {}
            (x, FilterModel::RcFilter) => {
                let mut filter = RcFilter::new(sample_rate, cutoff, resonance);
                filter.set_filter_type(filter_type);
                filter.order = FilterOrder::_24dB;
                *x = FilterContainer::RcFilter(filter)
            },
            (x, FilterModel::LadderFilter) => {
                *x = FilterContainer::LadderFilter(LadderFilter::default())
            }
            (x, FilterModel::BiquadFilter) => {
                *x = FilterContainer::BiquadFilter(BiquadFilter::with_params(
                    cutoff,
                    resonance,
                    sample_rate,
                    filter_type,
                ))
            }
            (x, FilterModel::SvfSimperFilter) => {
                let mut filter = SvfSimper::new(cutoff, resonance, sample_rate);
                filter.filter_type = filter_type;
                *x = FilterContainer::SvfSimperFilter(filter)
            }
            (x, FilterModel::None) => *x = FilterContainer::None,
            #[allow(unreachable_patterns)]
            (_, _) => {
                unreachable!("Forgot to set a FilterModel for FilterContainer")
            }
        }
    }
}
impl Filter for FilterContainer {
    fn process(&mut self, input: f32) -> f32 {
        match self {
            FilterContainer::RcFilter(x) => x.process(input),
            FilterContainer::LadderFilter(x) => x.process(input),
            FilterContainer::BiquadFilter(x) => x.process(input),
            FilterContainer::SvfSimperFilter(x) => x.process(input),
            _ => input,
        }
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        match self {
            FilterContainer::RcFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            FilterContainer::LadderFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            FilterContainer::BiquadFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            FilterContainer::SvfSimperFilter(x) => x.set_params(sample_rate, cutoff, resonance),
            _ => {}
        }
    }
    fn set_filter_type(&mut self, filter_type: FilterType) {
        match self {
            FilterContainer::RcFilter(x) => x.set_filter_type(filter_type),
            FilterContainer::LadderFilter(x) => x.set_filter_type(filter_type),
            FilterContainer::BiquadFilter(x) => x.set_filter_type(filter_type),
            FilterContainer::SvfSimperFilter(x) => x.set_filter_type(filter_type),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
/// Used to select FilterContainer without creating a filter.
pub enum FilterModel {
    None,
    RcFilter,
    LadderFilter,
    BiquadFilter,
    SvfSimperFilter,
}

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum FilterType {
    Lowpass,
    Bandpass,
    Highpass,
}
impl Default for FilterType {
    fn default() -> Self {
        Self::Lowpass
    }
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
#[allow(dead_code)]
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
        let tanh_scale = 4.0;
        let temp_in = tanh_pade32_f32((input + self.bp0 * self.rcq) / tanh_scale) * tanh_scale;
        let lp =
            tanh_pade32_f32((temp_in * self.rcb + self.lp0 * self.rca) / tanh_scale) * tanh_scale;
        let hp = tanh_pade32_f32((self.rcc * (self.hp0 + temp_in - self.last0)) / tanh_scale)
            * tanh_scale;
        let bp = tanh_pade32_f32((hp * self.rcb + self.bp0 * self.rca) / tanh_scale) * tanh_scale;

        self.last0 = temp_in;
        self.lp0 = lp;
        self.hp0 = hp;
        self.bp0 = bp;
    }
    /// Filters a single sample through the RC filter's second stage using tanh instead of hard clipping
    #[inline]
    fn step_second_stage_tanh(&mut self, input: f32) {
        let tanh_scale = 4.0;
        let temp_in = tanh_pade32_f32((input + self.bp1 * self.rcq) / tanh_scale) * tanh_scale;
        let lp =
            tanh_pade32_f32((temp_in * self.rcb + self.lp1 * self.rca) / tanh_scale) * tanh_scale;
        let hp = tanh_pade32_f32((self.rcc * (self.hp1 + temp_in - self.last1)) / tanh_scale)
            * tanh_scale;
        let bp = tanh_pade32_f32((hp * self.rcb + self.bp1 * self.rca) / tanh_scale) * tanh_scale;

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
    pub fn new(sample_rate: f32, mut cutoff: f32, resonance: f32) -> Self {
        cutoff = cutoff.clamp(5.0, 22000.0);
        let delta = (1.0 / sample_rate) / 4.0; // division by 4.0 occurs because of oversampling when processing...
        let freq = 1.0 / (cutoff * PI * 2.0);

        let rca = 1.0 - delta / (freq + delta);

        Self {
            rca,
            rcb: 1.0 - rca,
            rcc: freq / (freq + delta),
            rcq: resonance * 0.245,
            last0: 0.0,
            bp0: 0.0,
            lp0: 0.0,
            hp0: 0.0,
            last1: 0.0,
            bp1: 0.0,
            lp1: 0.0,
            hp1: 0.0,
            order: FilterOrder::_12dB,
            filter_type: FilterType::Lowpass,
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
    }

    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        let filter = Self::new(sample_rate, cutoff, resonance);

        self.rca = filter.rca;
        self.rcb = filter.rcb;
        self.rcc = filter.rcc;

        self.rcq = filter.rcq;
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
mod svf_simper;