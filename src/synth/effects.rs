//! The effects after the voices, on the synth's stereo output: distortion, chorus, delay and
//! reverb, in that order, each of which can be switched off.
//!
//! The effects process all the voices together, so their controls can't follow each voice's
//! modulation sources the way the voices' own controls do: they follow the newest voice's
//! (see [`ModTarget::is_effect()`]). Their settings change from one control rate block to the
//! next, and are smoothed sample by sample so that neither automation nor modulation steps.
//!
//! Every buffer is allocated up front, for the sample rate the synth is created with.

use nih_plug::prelude::Enum;

use super::fastmath;
use super::modulation::{ModTarget, Targets};

/// The distortion's highest drive, in decibels.
pub const DRIVE_MAX_DB: f32 = 36.0;
/// The delay's highest feedback.
pub const FEEDBACK_MAX: f32 = 0.95;
/// The delay's shortest and longest times, in seconds.
pub const DELAY_MIN_SECONDS: f32 = 0.001;
pub const DELAY_MAX_SECONDS: f32 = 2.0;
/// The chorus's slowest and fastest rates, in Hz.
pub const CHORUS_MIN_RATE: f32 = 0.05;
pub const CHORUS_MAX_RATE: f32 = 8.0;

/// The time the settings take to settle after a change, in seconds (the smoothing's time
/// constant).
const SMOOTHING_SECONDS: f32 = 0.01;
/// The chorus's delay at the middle of its sweep, and how far a full depth sweeps it either
/// way, in seconds.
const CHORUS_DELAY_SECONDS: f32 = 0.012;
const CHORUS_SWEEP_SECONDS: f32 = 0.006;

/// The effects' settings, in the units of their controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectSettings {
    pub distortion: DistortionSettings,
    pub chorus: ChorusSettings,
    pub delay: DelaySettings,
    pub reverb: ReverbSettings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistortionSettings {
    pub enabled: bool,
    /// The gain into the clipper, from 0 to [`DRIVE_MAX_DB`] decibels.
    pub drive_db: f32,
    /// From the dry signal at 0 to only the distorted one at 1.
    pub mix: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChorusSettings {
    pub enabled: bool,
    /// How fast the delays sweep, in Hz.
    pub rate: f32,
    /// How far they sweep, from 0 to 1.
    pub depth: f32,
    pub mix: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DelaySettings {
    pub enabled: bool,
    /// In seconds.
    pub time: f32,
    /// How much of each echo comes back as the next, up to [`FEEDBACK_MAX`].
    pub feedback: f32,
    pub mix: f32,
    /// The echoes bounce between the channels: the input goes into the left channel's delay,
    /// whose echoes go into the right's, whose echoes go back into the left's.
    pub ping_pong: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    pub enabled: bool,
    /// How long the reverb rings, from 0 to 1.
    pub size: f32,
    /// How quickly its highs die away, from 0 to 1.
    pub damping: f32,
    pub mix: f32,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            distortion: DistortionSettings {
                enabled: false,
                drive_db: 12.0,
                mix: 1.0,
            },
            chorus: ChorusSettings {
                enabled: false,
                rate: 0.8,
                depth: 0.5,
                mix: 0.5,
            },
            delay: DelaySettings {
                enabled: false,
                time: 0.375,
                feedback: 0.4,
                mix: 0.3,
                ping_pong: false,
            },
            reverb: ReverbSettings {
                enabled: false,
                size: 0.6,
                damping: 0.5,
                mix: 0.3,
            },
        }
    }
}

impl EffectSettings {
    /// The settings moved by the newest voice's modulation of the effects' targets, and clamped
    /// to the controls' ranges.
    pub fn modulated(&self, modulation: &Targets) -> Self {
        let moved = |target: ModTarget, value: f32, min: f32, max: f32| {
            target.scaling().apply(value, modulation[target.to_index()]).clamp(min, max)
        };
        let mut settings = *self;
        let distortion = &mut settings.distortion;
        distortion.drive_db = moved(ModTarget::DistortionDrive, distortion.drive_db, 0.0, DRIVE_MAX_DB);
        distortion.mix = moved(ModTarget::DistortionMix, distortion.mix, 0.0, 1.0);
        let chorus = &mut settings.chorus;
        chorus.rate = moved(ModTarget::ChorusRate, chorus.rate, CHORUS_MIN_RATE, CHORUS_MAX_RATE);
        chorus.depth = moved(ModTarget::ChorusDepth, chorus.depth, 0.0, 1.0);
        chorus.mix = moved(ModTarget::ChorusMix, chorus.mix, 0.0, 1.0);
        let delay = &mut settings.delay;
        delay.time = moved(ModTarget::DelayTime, delay.time, DELAY_MIN_SECONDS, DELAY_MAX_SECONDS);
        delay.feedback = moved(ModTarget::DelayFeedback, delay.feedback, 0.0, FEEDBACK_MAX);
        delay.mix = moved(ModTarget::DelayMix, delay.mix, 0.0, 1.0);
        let reverb = &mut settings.reverb;
        reverb.size = moved(ModTarget::ReverbSize, reverb.size, 0.0, 1.0);
        reverb.damping = moved(ModTarget::ReverbDamping, reverb.damping, 0.0, 1.0);
        reverb.mix = moved(ModTarget::ReverbMix, reverb.mix, 0.0, 1.0);
        settings
    }

    /// Whether any effect is switched on.
    pub fn any_enabled(&self) -> bool {
        self.distortion.enabled || self.chorus.enabled || self.delay.enabled || self.reverb.enabled
    }
}

/// A setting that glides towards its target with a one-pole lowpass.
#[derive(Debug, Clone, Copy)]
struct Smoothed {
    value: f32,
    /// The share of the remaining distance covered per sample.
    coefficient: f32,
}

impl Smoothed {
    fn new(sample_rate: f32) -> Self {
        Self {
            value: 0.0,
            coefficient: 1.0 - (-1.0 / (SMOOTHING_SECONDS * sample_rate)).exp(),
        }
    }
    #[inline]
    fn next(&mut self, target: f32) -> f32 {
        self.value += (target - self.value) * self.coefficient;
        self.value
    }
}

/// Cubic interpolation between `y1` and `y2` (`t` from 0 to 1), with their neighbours `y0` and
/// `y3`: the Catmull-Rom spline the oscillators also use.
#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// A delay line whose length is a power of two, read at fractional delays.
struct DelayLine {
    buffer: Vec<f32>,
    mask: usize,
    /// Where the next sample is written.
    write: usize,
}

impl DelayLine {
    /// A line that holds at least `samples` samples of history (plus the interpolation's).
    fn new(samples: usize) -> Self {
        let len = (samples + 4).next_power_of_two();
        Self {
            buffer: vec![0.0; len],
            mask: len - 1,
            write: 0,
        }
    }
    fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
    #[inline]
    fn push(&mut self, sample: f32) {
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) & self.mask;
    }
    /// The sample pushed `delay` samples ago (counting the last one pushed as 1), interpolated.
    /// The interpolation needs a newer neighbour, so the delay is at least 2.
    #[inline]
    fn read(&self, delay: f32) -> f32 {
        // (`max()` turns a NaN into the minimum)
        let delay = delay.max(2.0).min((self.buffer.len() - 4) as f32);
        // SAFETY: the delay is finite and within the buffer, so it fits a `u32`
        let whole = unsafe { delay.to_int_unchecked::<u32>() } as usize;
        let fraction = delay - whole as f32;
        // `at(k)` is the sample pushed `k` samples ago, counting the last one as 1 (the mask
        // keeps the index within the slice, so it needs no bounds check)
        let buffer = &self.buffer[..=self.mask];
        let at = |k: usize| buffer[self.write.wrapping_sub(k) & self.mask];
        // Going back in time from `whole` to `whole + 1` as the fraction rises
        hermite(at(whole - 1), at(whole), at(whole + 1), at(whole + 2), fraction)
    }
}

/// `tanh()` soft clipping, with first order antiderivative anti-aliasing: the output is the
/// average of `tanh` over the straight line from the last input to this one, which is the
/// difference of its antiderivative `ln(cosh(x))` divided by the inputs' difference. That
/// takes most of the aliasing out of the clipping's harmonics, for a delay of half a sample.
struct Distortion {
    drive: Smoothed,
    mix: Smoothed,
    /// Each channel's last input, and its antiderivative.
    last: [(f64, f64); 2],
}

impl Distortion {
    fn new(sample_rate: f32) -> Self {
        Self {
            drive: Smoothed::new(sample_rate),
            mix: Smoothed::new(sample_rate),
            last: [(0.0, 0.0); 2],
        }
    }
    fn reset(&mut self, settings: &DistortionSettings) {
        self.drive.value = db_to_gain(settings.drive_db);
        self.mix.value = settings.mix;
        self.last = [(0.0, 0.0); 2];
    }
    /// `ln(cosh(x))`, without overflowing for large inputs.
    fn log_cosh(x: f64) -> f64 {
        let x = x.abs();
        x + (-2.0 * x).exp().ln_1p() - std::f64::consts::LN_2
    }
    #[inline]
    fn clip(&mut self, channel: usize, input: f64) -> f32 {
        let (last, last_antiderivative) = self.last[channel];
        let antiderivative = Self::log_cosh(input);
        self.last[channel] = (input, antiderivative);
        let difference = input - last;
        // Too close together for the division: the line's midpoint instead
        let output = if difference.abs() < 1e-6 {
            (0.5 * (input + last)).tanh()
        } else {
            (antiderivative - last_antiderivative) / difference
        };
        output as f32
    }
    fn process(&mut self, settings: &DistortionSettings, left: &mut [f32], right: &mut [f32]) {
        let gain = db_to_gain(settings.drive_db);
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let gain = self.drive.next(gain);
            let mix = self.mix.next(settings.mix);
            let wet = self.clip(0, (*left * gain) as f64);
            *left += (wet - *left) * mix;
            let wet = self.clip(1, (*right * gain) as f64);
            *right += (wet - *right) * mix;
        }
    }
}

/// Two delay lines swept by an LFO, a quarter of a cycle apart in the two channels.
struct Chorus {
    sample_rate: f32,
    lines: [DelayLine; 2],
    /// The LFO's phase, in cycles.
    phase: f64,
    rate: Smoothed,
    depth: Smoothed,
    mix: Smoothed,
}

impl Chorus {
    fn new(sample_rate: f32) -> Self {
        let longest = ((CHORUS_DELAY_SECONDS + CHORUS_SWEEP_SECONDS) * sample_rate).ceil() as usize + 4;
        Self {
            sample_rate,
            lines: [DelayLine::new(longest), DelayLine::new(longest)],
            phase: 0.0,
            rate: Smoothed::new(sample_rate),
            depth: Smoothed::new(sample_rate),
            mix: Smoothed::new(sample_rate),
        }
    }
    fn reset(&mut self, settings: &ChorusSettings) {
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.phase = 0.0;
        self.rate.value = settings.rate;
        self.depth.value = settings.depth;
        self.mix.value = settings.mix;
    }
    fn process(&mut self, settings: &ChorusSettings, left: &mut [f32], right: &mut [f32]) {
        let centre = CHORUS_DELAY_SECONDS * self.sample_rate;
        let sweep = CHORUS_SWEEP_SECONDS * self.sample_rate;
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let rate = self.rate.next(settings.rate);
            let depth = self.depth.next(settings.depth) * sweep;
            let mix = self.mix.next(settings.mix);
            self.lines[0].push(*left);
            self.lines[1].push(*right);
            let phase = self.phase as f32;
            let wet_left = self.lines[0].read(centre + depth * fastmath::sin_cycles(phase));
            let wet_right = self.lines[1].read(centre + depth * fastmath::cos_cycles(phase));
            *left += (wet_left - *left) * mix;
            *right += (wet_right - *right) * mix;
            self.phase += (rate / self.sample_rate) as f64;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }
}

/// An echo in each channel, fed back into itself (or, ping-ponging, into the other channel's).
struct Delay {
    sample_rate: f32,
    lines: [DelayLine; 2],
    /// The delay in samples, smoothed, so that changing it glides the echoes' pitch like tape.
    time: Smoothed,
    feedback: Smoothed,
    mix: Smoothed,
}

impl Delay {
    fn new(sample_rate: f32) -> Self {
        let longest = (DELAY_MAX_SECONDS * sample_rate).ceil() as usize + 4;
        Self {
            sample_rate,
            lines: [DelayLine::new(longest), DelayLine::new(longest)],
            time: Smoothed::new(sample_rate),
            feedback: Smoothed::new(sample_rate),
            mix: Smoothed::new(sample_rate),
        }
    }
    fn reset(&mut self, settings: &DelaySettings) {
        self.lines.iter_mut().for_each(DelayLine::clear);
        self.time.value = settings.time * self.sample_rate;
        self.feedback.value = settings.feedback;
        self.mix.value = settings.mix;
    }
    fn process(&mut self, settings: &DelaySettings, left: &mut [f32], right: &mut [f32]) {
        let time = settings.time * self.sample_rate;
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let delay = self.time.next(time);
            let feedback = self.feedback.next(settings.feedback);
            let mix = self.mix.next(settings.mix);
            let echo_left = self.lines[0].read(delay);
            let echo_right = self.lines[1].read(delay);
            if settings.ping_pong {
                self.lines[0].push(0.5 * (*left + *right) + echo_right * feedback);
                self.lines[1].push(echo_left * feedback);
            } else {
                self.lines[0].push(*left + echo_left * feedback);
                self.lines[1].push(*right + echo_right * feedback);
            }
            *left += (echo_left - *left) * mix;
            *right += (echo_right - *right) * mix;
        }
    }
}

/// A feedback comb filter with a lowpass in its loop, for the reverb.
struct Comb {
    buffer: Vec<f32>,
    index: usize,
    /// The loop's lowpass state.
    stored: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
            stored: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let output = self.buffer[self.index];
        self.stored = output + (self.stored - output) * damping;
        self.buffer[self.index] = input + self.stored * feedback;
        self.index += 1;
        if self.index == self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

/// A Schroeder allpass filter, for the reverb.
struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            index: 0,
        }
    }
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.index];
        self.buffer[self.index] = input + delayed * 0.5;
        self.index += 1;
        if self.index == self.buffer.len() {
            self.index = 0;
        }
        delayed - input
    }
}

/// Jezar's Freeverb: eight combs in parallel into four allpasses in series, in each channel,
/// with the right channel's a little longer than the left's for width.
struct Reverb {
    combs: [[Comb; 8]; 2],
    allpasses: [[Allpass; 4]; 2],
    size: Smoothed,
    damping: Smoothed,
    mix: Smoothed,
}

impl Reverb {
    /// The comb and allpass lengths at 44.1 kHz, and how much longer the right channel's are.
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
    const SPREAD: usize = 23;
    /// The input's gain into the combs, and the output's back out.
    const INPUT_GAIN: f32 = 0.015;
    const OUTPUT_GAIN: f32 = 3.0;

    fn new(sample_rate: f32) -> Self {
        let scaled = |len: usize| (len as f32 * sample_rate / 44100.0).round() as usize;
        let combs = |spread: usize| std::array::from_fn(|i| Comb::new(scaled(Self::COMBS[i] + spread)));
        let allpasses = |spread: usize| std::array::from_fn(|i| Allpass::new(scaled(Self::ALLPASSES[i] + spread)));
        Self {
            combs: [combs(0), combs(Self::SPREAD)],
            allpasses: [allpasses(0), allpasses(Self::SPREAD)],
            size: Smoothed::new(sample_rate),
            damping: Smoothed::new(sample_rate),
            mix: Smoothed::new(sample_rate),
        }
    }
    fn reset(&mut self, settings: &ReverbSettings) {
        for comb in self.combs.iter_mut().flatten() {
            comb.buffer.fill(0.0);
            comb.stored = 0.0;
        }
        for allpass in self.allpasses.iter_mut().flatten() {
            allpass.buffer.fill(0.0);
        }
        self.size.value = settings.size;
        self.damping.value = settings.damping;
        self.mix.value = settings.mix;
    }
    fn process(&mut self, settings: &ReverbSettings, left: &mut [f32], right: &mut [f32]) {
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            // Freeverb's ranges: a feedback of 0.7 to 0.98 and a damping of 0 to 0.4
            let feedback = 0.7 + 0.28 * self.size.next(settings.size);
            let damping = 0.4 * self.damping.next(settings.damping);
            let mix = self.mix.next(settings.mix);
            let input = (*left + *right) * Self::INPUT_GAIN;
            let mut wet = [0.0_f32; 2];
            for (channel, wet) in wet.iter_mut().enumerate() {
                let mut sum = 0.0;
                for comb in &mut self.combs[channel] {
                    sum += comb.process(input, feedback, damping);
                }
                for allpass in &mut self.allpasses[channel] {
                    sum = allpass.process(sum);
                }
                *wet = sum * Self::OUTPUT_GAIN;
            }
            *left += (wet[0] - *left) * mix;
            *right += (wet[1] - *right) * mix;
        }
    }
}

fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db * 0.05)
}

/// The effects chain, with its state.
pub struct Effects {
    distortion: Distortion,
    chorus: Chorus,
    delay: Delay,
    reverb: Reverb,
    /// Which effects were on for the last block: one that is switched on starts afresh.
    enabled: [bool; 4],
}

impl Effects {
    /// The effects for a sample rate, with their buffers.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            distortion: Distortion::new(sample_rate),
            chorus: Chorus::new(sample_rate),
            delay: Delay::new(sample_rate),
            reverb: Reverb::new(sample_rate),
            enabled: [false; 4],
        }
    }

    /// Forgets the effects' state (echoes, reverb tails), so that they start afresh.
    pub fn reset(&mut self) {
        self.enabled = [false; 4];
    }

    /// Runs a block of the synth's output through the effects that are switched on, in place.
    pub fn process(&mut self, settings: &EffectSettings, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        let (left, right) = (&mut left[..len], &mut right[..len]);
        let enabled = [
            settings.distortion.enabled,
            settings.chorus.enabled,
            settings.delay.enabled,
            settings.reverb.enabled,
        ];
        // An effect that was off (or reset) starts from silence, at its current settings
        let starting = |effect: usize| enabled[effect] && !self.enabled[effect];
        if starting(0) {
            self.distortion.reset(&settings.distortion);
        }
        if starting(1) {
            self.chorus.reset(&settings.chorus);
        }
        if starting(2) {
            self.delay.reset(&settings.delay);
        }
        if starting(3) {
            self.reverb.reset(&settings.reverb);
        }
        self.enabled = enabled;

        if enabled[0] {
            self.distortion.process(&settings.distortion, left, right);
        }
        if enabled[1] {
            self.chorus.process(&settings.chorus, left, right);
        }
        if enabled[2] {
            self.delay.process(&settings.delay, left, right);
        }
        if enabled[3] {
            self.reverb.process(&settings.reverb, left, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::modulation::N_TARGETS;

    const RATE: f32 = 44100.0;

    /// The settings with only the given effects on.
    fn only(enable: impl Fn(&mut EffectSettings)) -> EffectSettings {
        let mut settings = EffectSettings::default();
        enable(&mut settings);
        settings
    }

    /// Runs a signal through the effects in blocks of 32 samples, as the synth does.
    fn run(effects: &mut Effects, settings: &EffectSettings, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (left.to_vec(), right.to_vec());
        for (left, right) in left.chunks_mut(32).zip(right.chunks_mut(32)) {
            effects.process(settings, left, right);
        }
        (left, right)
    }

    fn sine(len: usize, hz: f32, level: f32) -> Vec<f32> {
        (0..len).map(|n| level * (std::f32::consts::TAU * hz * n as f32 / RATE).sin()).collect()
    }

    fn impulse(len: usize) -> Vec<f32> {
        let mut signal = vec![0.0; len];
        signal[0] = 1.0;
        signal
    }

    /// With everything off the signal goes through untouched.
    #[test]
    fn effects_that_are_off_leave_the_signal_alone() {
        let mut effects = Effects::new(RATE);
        let signal = sine(1000, 440.0, 0.8);
        let settings = EffectSettings::default();
        assert!(!settings.any_enabled());
        let (left, right) = run(&mut effects, &settings, &signal, &signal);
        assert_eq!(left, signal);
        assert_eq!(right, signal);
        // An effect at zero mix doesn't change it either (once the smoothing has settled)
        for enable in [
            |s: &mut EffectSettings| s.distortion = DistortionSettings { enabled: true, drive_db: 30.0, mix: 0.0 },
            |s: &mut EffectSettings| s.chorus = ChorusSettings { enabled: true, rate: 2.0, depth: 1.0, mix: 0.0 },
            |s: &mut EffectSettings| s.delay = DelaySettings { enabled: true, time: 0.01, feedback: 0.5, mix: 0.0, ping_pong: false },
            |s: &mut EffectSettings| s.reverb = ReverbSettings { enabled: true, size: 0.9, damping: 0.2, mix: 0.0 },
        ] {
            let mut effects = Effects::new(RATE);
            let (left, _) = run(&mut effects, &only(enable), &signal, &signal);
            assert!(left.iter().zip(&signal).all(|(a, b)| (a - b).abs() < 1e-6));
        }
    }

    /// The distortion clips loud signals to within ±1, leaves quiet ones nearly alone at no
    /// drive, and its anti-aliasing keeps the output close to the plain clipping's.
    #[test]
    fn distortion_soft_clips() {
        let mut effects = Effects::new(RATE);
        let loud = only(|s| s.distortion = DistortionSettings { enabled: true, drive_db: DRIVE_MAX_DB, mix: 1.0 });
        let signal = sine(4410, 100.0, 1.0);
        let (left, right) = run(&mut effects, &loud, &signal, &signal);
        assert!(left.iter().all(|sample| sample.abs() <= 1.0 + 1e-6));
        assert_eq!(left, right);
        // Heavily driven, a sine is nearly a square
        let high = left.iter().filter(|sample| sample.abs() > 0.95).count();
        assert!(high > left.len() * 3 / 4, "{high}");

        let mut effects = Effects::new(RATE);
        let clean = only(|s| s.distortion = DistortionSettings { enabled: true, drive_db: 0.0, mix: 1.0 });
        let quiet = sine(4410, 100.0, 0.05);
        let (left, _) = run(&mut effects, &clean, &quiet, &quiet);
        // tanh(x) is x for small x (the anti-aliasing delays it by half a sample)
        for n in 1..quiet.len() {
            let want = 0.5 * (quiet[n] + quiet[n - 1]);
            assert!((left[n] - want).abs() < 1e-4, "{n}: {} vs {want}", left[n]);
        }
    }

    /// The chorus mixes in a copy delayed by the centre delay plus the sweep, differently in
    /// the two channels.
    #[test]
    fn chorus_sweeps_a_delayed_copy() {
        let mut effects = Effects::new(RATE);
        let settings = only(|s| s.chorus = ChorusSettings { enabled: true, rate: 1.0, depth: 1.0, mix: 1.0 });
        let signal = sine(8820, 300.0, 0.5);
        let (left, right) = run(&mut effects, &settings, &signal, &signal);
        assert!(left.iter().chain(&right).all(|sample| sample.is_finite() && sample.abs() < 1.0));
        // Fully wet, it's silent until the shortest delay has passed
        let shortest = ((CHORUS_DELAY_SECONDS - CHORUS_SWEEP_SECONDS) * RATE) as usize;
        assert!(left[..shortest - 2].iter().all(|sample| sample.abs() < 1e-6));
        assert!(left.iter().zip(&right).any(|(l, r)| (l - r).abs() > 0.05));
    }

    /// An impulse comes back after the delay time, smaller by the feedback each time; ping-pong
    /// alternates the channels.
    #[test]
    fn delay_echoes_at_its_time() {
        let time = 0.01;
        let samples = (time * RATE) as usize;
        let mut effects = Effects::new(RATE);
        let settings = only(|s| s.delay = DelaySettings { enabled: true, time, feedback: 0.5, mix: 1.0, ping_pong: false });
        let (left, right) = run(&mut effects, &settings, &impulse(4 * samples), &vec![0.0; 4 * samples]);
        let peak = |signal: &[f32], around: usize| signal[around - 3..around + 4].iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        assert!((peak(&left, samples) - 1.0).abs() < 0.05, "{}", peak(&left, samples));
        assert!((peak(&left, 2 * samples) - 0.5).abs() < 0.05, "{}", peak(&left, 2 * samples));
        assert!((peak(&left, 3 * samples) - 0.25).abs() < 0.05, "{}", peak(&left, 3 * samples));
        assert!(right.iter().all(|sample| *sample == 0.0));

        let mut effects = Effects::new(RATE);
        let settings = only(|s| s.delay = DelaySettings { enabled: true, time, feedback: 0.5, mix: 1.0, ping_pong: true });
        let (left, right) = run(&mut effects, &settings, &impulse(4 * samples), &vec![0.0; 4 * samples]);
        // The mono input goes left first, then bounces right, then left again
        assert!(peak(&left, samples) > 0.4 && peak(&right, samples) < 1e-3);
        assert!(peak(&right, 2 * samples) > 0.2 && peak(&left, 2 * samples) < 1e-3);
        assert!(peak(&left, 3 * samples) > 0.1 && peak(&right, 3 * samples) < 1e-3);
    }

    /// The reverb rings on after an impulse, longer for a bigger size, and dies away.
    #[test]
    fn reverb_rings_and_decays() {
        let len = 4 * RATE as usize;
        let energy = |signal: &[f32]| signal.iter().map(|sample| sample * sample).sum::<f32>();
        let tail = |size: f32| {
            let mut effects = Effects::new(RATE);
            let settings = only(|s| s.reverb = ReverbSettings { enabled: true, size, damping: 0.3, mix: 1.0 });
            let (left, right) = run(&mut effects, &settings, &impulse(len), &impulse(len));
            assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
            assert!(left.iter().zip(&right).any(|(l, r)| (l - r).abs() > 1e-4));
            (energy(&left[len / 4..len / 2]), energy(&left[len - len / 8..]))
        };
        let (small_mid, small_end) = tail(0.2);
        let (big_mid, big_end) = tail(1.0);
        assert!(big_mid > small_mid * 2.0, "{big_mid} vs {small_mid}");
        assert!(small_end < small_mid * 1e-3, "{small_end} vs {small_mid}");
        assert!(big_end < big_mid, "{big_end} vs {big_mid}");
    }

    /// Modulation moves the settings by their targets' scalings and keeps them in range.
    #[test]
    fn modulation_moves_the_settings() {
        let settings = EffectSettings::default();
        let mut modulation = [0.0; N_TARGETS];
        assert_eq!(settings.modulated(&modulation), settings);
        modulation[ModTarget::DelayTime.to_index()] = 0.5;
        modulation[ModTarget::ReverbMix.to_index()] = -1.0;
        modulation[ModTarget::DistortionDrive.to_index()] = 2.0;
        let moved = settings.modulated(&modulation);
        assert!((moved.delay.time - settings.delay.time * 2.0).abs() < 1e-6);
        assert_eq!(moved.reverb.mix, 0.0);
        assert_eq!(moved.distortion.drive_db, DRIVE_MAX_DB);
    }

    /// Switching an effect on starts it from silence, and switching settings mid-signal stays
    /// finite and quiet enough, at high sample rates too.
    #[test]
    fn effects_restart_cleanly() {
        for rate in [44100.0, 192000.0] {
            let mut effects = Effects::new(rate);
            let all = only(|s| {
                s.distortion.enabled = true;
                s.chorus.enabled = true;
                s.delay.enabled = true;
                s.reverb.enabled = true;
            });
            let signal: Vec<f32> = (0..20000).map(|n| (n as f32 * 0.37).sin() * 0.7).collect();
            let (left, _) = run(&mut effects, &all, &signal, &signal);
            assert!(left.iter().all(|sample| sample.is_finite() && sample.abs() < 4.0));
            // Off, then on again: no echoes of what came before
            run(&mut effects, &EffectSettings::default(), &signal, &signal);
            let silence = vec![0.0; 2000];
            let (left, right) = run(&mut effects, &all, &silence, &silence);
            assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
        }
    }
}
