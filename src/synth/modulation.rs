//! The modulation matrix: sources (LFOs, envelopes and MIDI controls) are routed to targets
//! (the synth's controls) with an amount, and the routes are evaluated per voice once per
//! control rate block (or at every sample, see [`super::ModulationRate`]). The effects' targets
//! follow the newest voice, since the effects process all the voices together.
//!
//! An amount of 100% maps the source's full swing onto the target's whole range: a slider
//! target moves across its whole length, the pitch targets by [`ModTarget::PITCH_SEMITONES`],
//! and the frequency-like targets (cutoff, resonance, drive, LFO rates, the chorus rate and the
//! delay time) by a number of octaves, see [`ModTarget::scaling()`].
//! A target's modulation is the sum of its routes, and the result is clamped to the control's
//! range.
//!
//! On its way to the amount, a route can reshape its source (see [`Remap`]): swing it from 0 to
//! 1 or from -1 to 1 whatever the source's own range, bend it, step it, and scale it by another
//! source.

use nih_plug::prelude::Enum;
use wide::f32x8;

use super::fastmath;
use super::oscillator::{MULTIPLIER_RANGE, OCTAVE_RANGE};

/// Something that can drive a modulation route. The LFOs, the key, the random source and the
/// oscillators are bipolar (-1 to 1), everything else goes from 0 to 1.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModSource {
    #[id = "none"]
    #[name = "Off"]
    None,
    #[id = "lfo1"]
    #[name = "LFO 1"]
    Lfo1,
    #[id = "lfo2"]
    #[name = "LFO 2"]
    Lfo2,
    #[id = "lfo3"]
    #[name = "LFO 3"]
    Lfo3,
    #[id = "env1"]
    #[name = "Env 1"]
    Env1,
    #[id = "env2"]
    #[name = "Env 2"]
    Env2,
    #[id = "vol_env"]
    #[name = "Volume Env."]
    VolumeEnv,
    #[id = "filter_env"]
    #[name = "Filter Env."]
    FilterEnv,
    #[id = "velocity"]
    Velocity,
    #[id = "key"]
    #[name = "Key Track"]
    Key,
    #[id = "mod_wheel"]
    #[name = "Mod Wheel"]
    ModWheel,
    #[id = "aftertouch"]
    Aftertouch,
    #[id = "random"]
    Random,
    #[id = "osc1"]
    #[name = "Osc 1"]
    Osc1,
    #[id = "osc2"]
    #[name = "Osc 2"]
    Osc2,
    #[id = "osc3"]
    #[name = "Osc 3"]
    Osc3,
}

/// Number of [`ModSource`] variants.
pub const N_SOURCES: usize = 16;
/// Number of modulation routes (slots in the matrix).
pub const N_SLOTS: usize = 16;
/// Number of LFOs.
pub const N_LFOS: usize = 3;
/// Number of modulation envelopes (the ones without a fixed job).
pub const N_MOD_ENVELOPES: usize = 2;

impl ModSource {
    /// The sources that come from MIDI, and the random one, which have no controls of their own.
    pub const MIDI: [Self; 5] = [Self::Velocity, Self::Key, Self::ModWheel, Self::Aftertouch, Self::Random];

    pub fn lfo(index: usize) -> Self {
        [Self::Lfo1, Self::Lfo2, Self::Lfo3][index]
    }
    pub fn envelope(index: usize) -> Self {
        [Self::Env1, Self::Env2][index]
    }
    pub fn osc(index: usize) -> Self {
        [Self::Osc1, Self::Osc2, Self::Osc3][index]
    }
    /// Whether the source swings from -1 to 1, rather than from 0 to 1.
    pub fn is_bipolar(self) -> bool {
        matches!(
            self,
            Self::Lfo1 | Self::Lfo2 | Self::Lfo3 | Self::Key | Self::Random | Self::Osc1 | Self::Osc2 | Self::Osc3
        )
    }
    /// A short description for the GUI.
    pub fn help(self) -> &'static str {
        match self {
            Self::None => "No source.",
            Self::Lfo1 | Self::Lfo2 | Self::Lfo3 => {
                "The LFO's output, from -1 to 1. Each note has its own copy of the LFO, started \
                 when the note starts."
            }
            Self::Env1 | Self::Env2 => "The envelope's level, from 0 to 1.",
            Self::VolumeEnv => "The volume envelope's level, from 0 to 1.",
            Self::FilterEnv => "The filter envelope's level, from 0 to 1.",
            Self::Velocity => "How hard the note was played, from 0 to 1.",
            Self::Key => {
                "The note's position on the keyboard: 0 at middle C (C4), -1 five octaves below \
                 and 1 five octaves above it."
            }
            Self::ModWheel => "The MIDI modulation wheel (controller 1), from 0 to 1.",
            Self::Aftertouch => "MIDI channel or polyphonic aftertouch, from 0 to 1.",
            Self::Random => "A random value from -1 to 1, drawn when the note starts.",
            Self::Osc1 | Self::Osc2 | Self::Osc3 => {
                "The oscillator's wave, from -1 to 1 (its unison voices averaged), before its \
                 Amplitude, so a silent oscillator can modulate too. It reaches the routes a \
                 control rate block (32 samples) late, since the oscillators play after the \
                 modulation is worked out, so an oscillator modulating its own pitch is fed \
                 back through that delay. At the Block modulation rate it is only read once \
                 per block: modulation at audio rates wants the Sample rate (Advanced tab)."
            }
        }
    }
}

/// Something a modulation route can control.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModTarget {
    #[id = "none"]
    #[name = "Off"]
    None,
    #[id = "pitch"]
    #[name = "Pitch (all)"]
    Pitch,
    #[id = "fine_pitch"]
    #[name = "Fine Pitch (all)"]
    FinePitch,
    #[id = "volume"]
    #[name = "Volume (all)"]
    Volume,
    #[id = "osc1_pitch"]
    #[name = "Osc 1 Pitch"]
    Osc1Pitch,
    #[id = "osc1_octave"]
    #[name = "Osc 1 Octave"]
    Osc1Octave,
    #[id = "osc1_multiplier"]
    #[name = "Osc 1 Freq. Mult"]
    Osc1Multiplier,
    #[id = "osc1_position"]
    #[name = "Osc 1 Position"]
    Osc1Position,
    #[id = "osc1_amp"]
    #[name = "Osc 1 Amplitude"]
    Osc1Amp,
    #[id = "osc1_detune"]
    #[name = "Osc 1 Unison Detune"]
    Osc1Detune,
    #[id = "osc1_pan"]
    #[name = "Osc 1 Pan"]
    Osc1Pan,
    #[id = "osc1_pm"]
    #[name = "Osc 1 PM"]
    Osc1Pm,
    #[id = "osc1_fm"]
    #[name = "Osc 1 FM"]
    Osc1Fm,
    #[id = "osc1_am"]
    #[name = "Osc 1 AM"]
    Osc1Am,
    #[id = "osc2_pitch"]
    #[name = "Osc 2 Pitch"]
    Osc2Pitch,
    #[id = "osc2_octave"]
    #[name = "Osc 2 Octave"]
    Osc2Octave,
    #[id = "osc2_multiplier"]
    #[name = "Osc 2 Freq. Mult"]
    Osc2Multiplier,
    #[id = "osc2_position"]
    #[name = "Osc 2 Position"]
    Osc2Position,
    #[id = "osc2_amp"]
    #[name = "Osc 2 Amplitude"]
    Osc2Amp,
    #[id = "osc2_detune"]
    #[name = "Osc 2 Unison Detune"]
    Osc2Detune,
    #[id = "osc2_pan"]
    #[name = "Osc 2 Pan"]
    Osc2Pan,
    #[id = "osc2_pm"]
    #[name = "Osc 2 PM"]
    Osc2Pm,
    #[id = "osc2_fm"]
    #[name = "Osc 2 FM"]
    Osc2Fm,
    #[id = "osc2_am"]
    #[name = "Osc 2 AM"]
    Osc2Am,
    #[id = "osc3_pitch"]
    #[name = "Osc 3 Pitch"]
    Osc3Pitch,
    #[id = "osc3_octave"]
    #[name = "Osc 3 Octave"]
    Osc3Octave,
    #[id = "osc3_multiplier"]
    #[name = "Osc 3 Freq. Mult"]
    Osc3Multiplier,
    #[id = "osc3_position"]
    #[name = "Osc 3 Position"]
    Osc3Position,
    #[id = "osc3_amp"]
    #[name = "Osc 3 Amplitude"]
    Osc3Amp,
    #[id = "osc3_detune"]
    #[name = "Osc 3 Unison Detune"]
    Osc3Detune,
    #[id = "osc3_pan"]
    #[name = "Osc 3 Pan"]
    Osc3Pan,
    #[id = "cutoff"]
    #[name = "Filter Cutoff"]
    Cutoff,
    #[id = "resonance"]
    #[name = "Filter Resonance"]
    Resonance,
    #[id = "drive"]
    #[name = "Filter Drive"]
    Drive,
    #[id = "lfo1_rate"]
    #[name = "LFO 1 Rate"]
    Lfo1Rate,
    #[id = "lfo2_rate"]
    #[name = "LFO 2 Rate"]
    Lfo2Rate,
    #[id = "lfo3_rate"]
    #[name = "LFO 3 Rate"]
    Lfo3Rate,
    // The effects, which process all the voices together: their modulation comes from the
    // newest voice (see `ModTarget::is_effect()`)
    #[id = "dist_drive"]
    #[name = "Distortion Drive"]
    DistortionDrive,
    #[id = "dist_mix"]
    #[name = "Distortion Mix"]
    DistortionMix,
    #[id = "chorus_rate"]
    #[name = "Chorus Rate"]
    ChorusRate,
    #[id = "chorus_depth"]
    #[name = "Chorus Depth"]
    ChorusDepth,
    #[id = "chorus_mix"]
    #[name = "Chorus Mix"]
    ChorusMix,
    #[id = "delay_time"]
    #[name = "Delay Time"]
    DelayTime,
    #[id = "delay_feedback"]
    #[name = "Delay Feedback"]
    DelayFeedback,
    #[id = "delay_mix"]
    #[name = "Delay Mix"]
    DelayMix,
    #[id = "reverb_size"]
    #[name = "Reverb Size"]
    ReverbSize,
    #[id = "reverb_damping"]
    #[name = "Reverb Damping"]
    ReverbDamping,
    #[id = "reverb_mix"]
    #[name = "Reverb Mix"]
    ReverbMix,
}

/// Number of [`ModTarget`] variants.
pub const N_TARGETS: usize = 48;

/// How a target's control moves under modulation, in the control's own units: a modulation of
/// 1 is a source at its full value (1, or -1 for the bipolar ones) at an amount of 100%.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scaling {
    /// Adds the modulation times this much: a slider's whole range.
    Linear(f32),
    /// Multiplies by two to the power of the modulation times this many octaves.
    Octaves(f32),
    /// Scales a level in decibels by `1 + modulation`: silent at -1 and doubled at 1.
    Gain,
}

impl Scaling {
    /// A control's value moved by a modulation. The result isn't clamped to the control's
    /// range.
    pub fn apply(self, value: f32, modulation: f32) -> f32 {
        match self {
            Self::Linear(range) => value + modulation * range,
            Self::Octaves(octaves) => value * (modulation * octaves).exp2(),
            Self::Gain => value + 20.0 * (1.0 + modulation).max(1e-9).log10(),
        }
    }
}

impl ModTarget {
    /// Semitones the pitch targets move at a full amount.
    pub const PITCH_SEMITONES: f32 = 12.0;
    /// Semitones the fine pitch target moves at a full amount.
    pub const FINE_PITCH_SEMITONES: f32 = 1.0;
    /// Octaves the filter cutoff moves at a full amount (about the cutoff control's range).
    pub const CUTOFF_OCTAVES: f32 = 10.0;
    /// Octaves the filter resonance moves at a full amount (about the control's range).
    pub const RESONANCE_OCTAVES: f32 = 3.32;
    /// Octaves the filter drive moves at a full amount.
    pub const DRIVE_OCTAVES: f32 = 5.0;
    /// Octaves an LFO's rate moves at a full amount.
    pub const LFO_RATE_OCTAVES: f32 = 4.0;
    /// How far a full amount moves an oscillator's pan: its whole range, from left to right.
    pub const PAN_RANGE: f32 = 2.0;
    /// Octaves the octave controls move at a full amount: their whole range.
    pub const OCTAVE_SPAN: f32 = (OCTAVE_RANGE.1 - OCTAVE_RANGE.0) as f32;
    /// Steps the frequency multipliers move at a full amount: their whole range.
    pub const MULTIPLIER_SPAN: f32 = (MULTIPLIER_RANGE.1 - MULTIPLIER_RANGE.0) as f32;
    /// Octaves the chorus rate moves at a full amount.
    pub const CHORUS_RATE_OCTAVES: f32 = 4.0;
    /// Octaves the delay time moves at a full amount.
    pub const DELAY_TIME_OCTAVES: f32 = 2.0;

    pub fn osc_pitch(osc: usize) -> Self {
        [Self::Osc1Pitch, Self::Osc2Pitch, Self::Osc3Pitch][osc]
    }
    pub fn osc_octave(osc: usize) -> Self {
        [Self::Osc1Octave, Self::Osc2Octave, Self::Osc3Octave][osc]
    }
    pub fn osc_multiplier(osc: usize) -> Self {
        [Self::Osc1Multiplier, Self::Osc2Multiplier, Self::Osc3Multiplier][osc]
    }
    pub fn osc_position(osc: usize) -> Self {
        [Self::Osc1Position, Self::Osc2Position, Self::Osc3Position][osc]
    }
    pub fn osc_amp(osc: usize) -> Self {
        [Self::Osc1Amp, Self::Osc2Amp, Self::Osc3Amp][osc]
    }
    pub fn osc_detune(osc: usize) -> Self {
        [Self::Osc1Detune, Self::Osc2Detune, Self::Osc3Detune][osc]
    }
    pub fn osc_pan(osc: usize) -> Self {
        [Self::Osc1Pan, Self::Osc2Pan, Self::Osc3Pan][osc]
    }
    /// The PM, FM and AM targets of an oscillator, which only the first two have.
    pub fn osc_pm(osc: usize) -> Option<Self> {
        [Self::Osc1Pm, Self::Osc2Pm].get(osc).copied()
    }
    pub fn osc_fm(osc: usize) -> Option<Self> {
        [Self::Osc1Fm, Self::Osc2Fm].get(osc).copied()
    }
    pub fn osc_am(osc: usize) -> Option<Self> {
        [Self::Osc1Am, Self::Osc2Am].get(osc).copied()
    }
    pub fn lfo_rate(lfo: usize) -> Self {
        [Self::Lfo1Rate, Self::Lfo2Rate, Self::Lfo3Rate][lfo]
    }
    /// Whether this is an effect's control. The effects process all the voices together, so
    /// these are modulated by the newest voice's sources.
    pub fn is_effect(self) -> bool {
        self.to_index() >= Self::DistortionDrive.to_index()
    }
    /// How a full amount moves this target's control, see [`Scaling`]. The effects' controls
    /// are modulated this way; for the others it says how the synth's own code moves them, for
    /// drawing.
    pub fn scaling(self) -> Scaling {
        match self {
            Self::Pitch | Self::Osc1Pitch | Self::Osc2Pitch | Self::Osc3Pitch => Scaling::Linear(Self::PITCH_SEMITONES),
            Self::FinePitch => Scaling::Linear(Self::FINE_PITCH_SEMITONES),
            Self::Osc1Octave | Self::Osc2Octave | Self::Osc3Octave => Scaling::Linear(Self::OCTAVE_SPAN),
            Self::Osc1Multiplier | Self::Osc2Multiplier | Self::Osc3Multiplier => Scaling::Linear(Self::MULTIPLIER_SPAN),
            Self::Volume => Scaling::Gain,
            // The amplitude and unison detune sliders are in percent
            Self::Osc1Amp | Self::Osc2Amp | Self::Osc3Amp => Scaling::Linear(100.0),
            Self::Osc1Detune | Self::Osc2Detune | Self::Osc3Detune => Scaling::Linear(100.0),
            Self::Osc1Pan | Self::Osc2Pan | Self::Osc3Pan => Scaling::Linear(Self::PAN_RANGE),
            Self::Cutoff => Scaling::Octaves(Self::CUTOFF_OCTAVES),
            Self::Resonance => Scaling::Octaves(Self::RESONANCE_OCTAVES),
            Self::Drive => Scaling::Octaves(Self::DRIVE_OCTAVES),
            Self::Lfo1Rate | Self::Lfo2Rate | Self::Lfo3Rate => Scaling::Octaves(Self::LFO_RATE_OCTAVES),
            Self::DistortionDrive => Scaling::Linear(super::effects::DRIVE_MAX_DB),
            Self::ChorusRate => Scaling::Octaves(Self::CHORUS_RATE_OCTAVES),
            Self::DelayTime => Scaling::Octaves(Self::DELAY_TIME_OCTAVES),
            Self::DelayFeedback => Scaling::Linear(super::effects::FEEDBACK_MAX),
            // The rest go from 0 to 1
            _ => Scaling::Linear(1.0),
        }
    }
    /// What a full amount does to this target, for the GUI.
    pub fn help(self) -> &'static str {
        match self {
            Self::None => "No target.",
            Self::Pitch | Self::Osc1Pitch | Self::Osc2Pitch | Self::Osc3Pitch => {
                "Pitch, by up to an octave (12 semitones) at 100%."
            }
            Self::FinePitch => "Pitch, by up to a semitone at 100%.",
            Self::Osc1Octave | Self::Osc2Octave | Self::Osc3Octave => {
                "The oscillator's octave, by the slider's whole range (16 octaves) at 100%, so \
                 6.25% for an octave. It moves in whole octaves, within the slider's range."
            }
            Self::Osc1Multiplier | Self::Osc2Multiplier | Self::Osc3Multiplier => {
                "The frequency multiplier, by the slider's whole range (128 steps) at 100%, so \
                 about 0.8% a step. It moves in whole steps (x2, x3... or /2, /3...), within \
                 the slider's range."
            }
            Self::Volume => "The note's volume, scaled from 0 at -100% to double at 100%.",
            Self::Cutoff => "The filter cutoff, by up to 10 octaves at 100%.",
            Self::Resonance => "The filter resonance, by up to 10 times at 100%.",
            Self::Drive => "The filter drive, by up to 32 times at 100%.",
            Self::Lfo1Rate | Self::Lfo2Rate | Self::Lfo3Rate => {
                "The LFO's rate, by up to 4 octaves (16 times) at 100%."
            }
            Self::Osc1Pan | Self::Osc2Pan | Self::Osc3Pan => {
                "The oscillator's pan, by its whole range (from left to right) at 100%. It \
                 changes once per control rate block."
            }
            Self::ChorusRate => {
                "The chorus rate, by up to 4 octaves at 100%, following the newest note's \
                 sources."
            }
            Self::DelayTime => {
                "The delay time, by up to 2 octaves (4 times) at 100%, following the newest \
                 note's sources."
            }
            target if target.is_effect() => {
                "The effect's slider, by its whole length at 100%, following the newest note's \
                 sources (the effects process all the notes together)."
            }
            _ => "The control's slider, by its whole length at 100%.",
        }
    }
}

/// How a route swings: as its source does, or from 0 to 1 or from -1 to 1 whatever the source.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Polarity {
    /// From -1 to 1 for the bipolar sources (see [`ModSource::is_bipolar()`]), and from 0 to 1
    /// for the others.
    #[id = "source"]
    #[name = "As Source"]
    #[default]
    Source,
    #[id = "unipolar"]
    Unipolar,
    #[id = "bipolar"]
    Bipolar,
}

impl Polarity {
    /// Whether a route from `source` swings from -1 to 1.
    pub fn is_bipolar(self, source: ModSource) -> bool {
        match self {
            Self::Source => source.is_bipolar(),
            Self::Unipolar => false,
            Self::Bipolar => true,
        }
    }
}

/// How a route reshapes its source on the way to the amount. The source's position in its range
/// (from 0 to 1) is bent by the curve, then stepped, then swung over the route's polarity, then
/// scaled by the other source (from 0 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Remap {
    pub polarity: Polarity,
    /// From -1 to 1: 0 leaves the source straight, more than 0 keeps it low for longer (raising
    /// its position to a power of up to [`Remap::MAX_EXPONENT`]), less than 0 high.
    pub curve: f32,
    /// How many levels the source is stepped to, or 1 (or 0) to leave it smooth.
    pub steps: u32,
    /// A source that scales the route, from 0 at its bottom to 1 at its top (a bipolar one over
    /// its whole swing, from -1), or [`ModSource::None`].
    pub via: ModSource,
}

impl Default for Remap {
    fn default() -> Self {
        Self {
            polarity: Polarity::Source,
            curve: 0.0,
            steps: 1,
            via: ModSource::None,
        }
    }
}

impl Remap {
    /// The power a full curve raises the source's position to (and the root a full negative
    /// curve takes).
    pub const MAX_EXPONENT: f32 = 4.0;

    /// The power the curve raises the source's position to.
    pub fn exponent(&self) -> f32 {
        Self::MAX_EXPONENT.powf(self.curve)
    }
}

/// An active modulation route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    pub source: ModSource,
    pub target: ModTarget,
    /// From -1 to 1.
    pub amount: f32,
    pub remap: Remap,
    /// Whether the source goes to the amount as it comes, which spares the reshaping.
    plain: bool,
    /// Whether the route swings from -1 to 1.
    bipolar: bool,
    /// [`Remap::exponent()`].
    exponent: f32,
}

impl Route {
    /// A route that applies its source as it comes.
    pub fn new(source: ModSource, target: ModTarget, amount: f32) -> Self {
        Self::remapped(source, target, amount, Remap::default())
    }

    /// A route that reshapes its source as `remap` says.
    pub fn remapped(source: ModSource, target: ModTarget, amount: f32, remap: Remap) -> Self {
        let bipolar = remap.polarity.is_bipolar(source);
        Self {
            source,
            target,
            amount,
            remap,
            plain: bipolar == source.is_bipolar() && remap.curve == 0.0 && remap.steps <= 1 && remap.via == ModSource::None,
            bipolar,
            exponent: remap.exponent(),
        }
    }

    /// Whether the route applies its source as it comes.
    pub fn is_plain(&self) -> bool {
        self.plain
    }

    /// The route's contribution to its target, given the sources' values.
    #[inline]
    pub fn apply(&self, sources: &Sources) -> f32 {
        let value = sources[self.source.to_index()];
        if self.plain {
            return value * self.amount;
        }
        self.reshape(value, sources[self.remap.via.to_index()]) * self.amount
    }

    /// A value of the source reshaped as the route's [`Remap`] says, with the scaling source at
    /// `via`, before the amount.
    pub fn reshape(&self, value: f32, via: f32) -> f32 {
        if self.plain {
            return value;
        }
        let remap = &self.remap;
        let mut position = if self.source.is_bipolar() { (value + 1.0) * 0.5 } else { value };
        if remap.curve != 0.0 || remap.steps > 1 {
            position = position.clamp(0.0, 1.0);
        }
        if remap.curve != 0.0 {
            position = fastmath::pow(position, self.exponent);
        }
        if remap.steps > 1 {
            let steps = remap.steps as f32;
            position = (position * steps).floor().min(steps - 1.0) / (steps - 1.0);
        }
        let mut shaped = if self.bipolar { position * 2.0 - 1.0 } else { position };
        if remap.via != ModSource::None {
            shaped *= scale(remap.via, via);
        }
        shaped
    }

    /// [`reshape()`][Self::reshape()] of eight values at once, with exactly the same results.
    pub fn reshape8(&self, value: f32x8, via: f32x8) -> f32x8 {
        if self.plain {
            return value;
        }
        let splat = f32x8::splat;
        let remap = &self.remap;
        let mut position = if self.source.is_bipolar() { (value + splat(1.0)) * splat(0.5) } else { value };
        if remap.curve != 0.0 || remap.steps > 1 {
            position = position.fast_max(f32x8::ZERO).fast_min(splat(1.0));
        }
        if remap.curve != 0.0 {
            position = fastmath::x8::pow(position, splat(self.exponent));
        }
        if remap.steps > 1 {
            let steps = splat(remap.steps as f32);
            position = (position * steps).floor().fast_min(steps - splat(1.0)) / (steps - splat(1.0));
        }
        let mut shaped = if self.bipolar { position * splat(2.0) - splat(1.0) } else { position };
        if remap.via != ModSource::None {
            shaped *= scale8(remap.via, via);
        }
        shaped
    }

    /// The sources the route reads: its own, and the one that scales it.
    pub fn sources(&self) -> impl Iterator<Item = ModSource> {
        [self.source, self.remap.via].into_iter().filter(|source| *source != ModSource::None)
    }
}

/// A scaling source's value as a scale from 0 to 1: a bipolar source's whole swing, from -1.
fn scale(source: ModSource, value: f32) -> f32 {
    if source.is_bipolar() {
        (value + 1.0) * 0.5
    } else {
        value
    }
}

/// [`scale()`] of eight values.
fn scale8(source: ModSource, value: f32x8) -> f32x8 {
    if source.is_bipolar() {
        (value + f32x8::splat(1.0)) * f32x8::splat(0.5)
    } else {
        value
    }
}

/// The value of every source for one voice at one point in time.
pub type Sources = [f32; N_SOURCES];
/// The summed modulation of every target.
pub type Targets = [f32; N_TARGETS];

/// Sums the routes' contributions to each target.
pub fn apply_routes(routes: &[Route], sources: &Sources) -> Targets {
    let mut targets = [0.0; N_TARGETS];
    for route in routes {
        targets[route.target.to_index()] += route.apply(sources);
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_counts_match() {
        assert_eq!(ModSource::variants().len(), N_SOURCES);
        assert_eq!(ModTarget::variants().len(), N_TARGETS);
        for i in 0..N_LFOS {
            assert_eq!(ModSource::lfo(i).to_index(), i + 1);
            assert_eq!(ModTarget::lfo_rate(i), ModTarget::from_index(ModTarget::Lfo1Rate.to_index() + i));
        }
        // The effects' targets are the last ones
        let effects: Vec<&str> = (0..N_TARGETS)
            .map(ModTarget::from_index)
            .filter(|target| target.is_effect())
            .map(|target| ModTarget::variants()[target.to_index()])
            .collect();
        assert_eq!(effects.len(), 11);
        assert!(effects.iter().all(|name| ["Distortion", "Chorus", "Delay", "Reverb"].iter().any(|fx| name.starts_with(fx))));
        assert!(!ModTarget::Lfo3Rate.is_effect() && !ModTarget::Osc3Pan.is_effect());
    }

    #[test]
    fn scalings_move_controls() {
        assert_eq!(Scaling::Linear(2.0).apply(0.25, -0.5), -0.75);
        assert_eq!(Scaling::Octaves(4.0).apply(100.0, 0.5), 400.0);
        assert!((Scaling::Gain.apply(-6.0, 1.0) - (-6.0 + 20.0 * 2.0_f32.log10())).abs() < 1e-5);
        assert!(Scaling::Gain.apply(-6.0, -1.0) < -150.0);
        assert_eq!(ModTarget::Osc2Pan.scaling(), Scaling::Linear(ModTarget::PAN_RANGE));
        assert_eq!(ModTarget::Cutoff.scaling(), Scaling::Octaves(ModTarget::CUTOFF_OCTAVES));
    }

    #[test]
    fn routes_sum_into_their_targets() {
        let mut sources = [0.0; N_SOURCES];
        sources[ModSource::Lfo1.to_index()] = 0.5;
        sources[ModSource::Velocity.to_index()] = 1.0;
        let routes = [
            Route::new(ModSource::Lfo1, ModTarget::Cutoff, 0.5),
            Route::new(ModSource::Velocity, ModTarget::Cutoff, -0.1),
            Route::new(ModSource::Velocity, ModTarget::Osc2Amp, 1.0),
        ];
        let targets = apply_routes(&routes, &sources);
        assert!((targets[ModTarget::Cutoff.to_index()] - 0.15).abs() < 1e-6);
        assert_eq!(targets[ModTarget::Osc2Amp.to_index()], 1.0);
        assert_eq!(targets[ModTarget::Osc1Amp.to_index()], 0.0);
    }

    /// A route reshapes its source as its remap says, and leaves it alone by default.
    #[test]
    fn routes_reshape_their_sources() {
        let route = |source, remap| Route::remapped(source, ModTarget::Cutoff, 1.0, remap);
        let plain = Remap::default();
        // By default the source goes through as it is
        for source in [ModSource::Lfo1, ModSource::Env1] {
            assert!(route(source, plain).is_plain());
            for value in [-1.0, -0.3, 0.0, 0.42, 1.0] {
                assert_eq!(route(source, plain).reshape(value, 0.0), value);
            }
        }
        // An LFO made unipolar swings from 0 to 1, and an envelope made bipolar from -1 to 1
        let unipolar = route(ModSource::Lfo1, Remap { polarity: Polarity::Unipolar, ..plain });
        assert!(!unipolar.is_plain());
        assert_eq!([-1.0, 0.0, 1.0].map(|value| unipolar.reshape(value, 0.0)), [0.0, 0.5, 1.0]);
        let bipolar = route(ModSource::Env1, Remap { polarity: Polarity::Bipolar, ..plain });
        assert_eq!([0.0, 0.5, 1.0].map(|value| bipolar.reshape(value, 0.0)), [-1.0, 0.0, 1.0]);
        // (and choosing the source's own polarity changes nothing)
        assert!(route(ModSource::Lfo1, Remap { polarity: Polarity::Bipolar, ..plain }).is_plain());
        // A curve bends the range between its ends: up keeps it low for longer
        let curved = route(ModSource::Env1, Remap { curve: 0.5, ..plain });
        assert_eq!(curved.reshape(0.0, 0.0), 0.0);
        assert!((curved.reshape(1.0, 0.0) - 1.0).abs() < 1e-6);
        assert!((curved.reshape(0.5, 0.0) - 0.25).abs() < 1e-5);
        let bent_down = route(ModSource::Env1, Remap { curve: -1.0, ..plain });
        assert!((bent_down.reshape(0.0625, 0.0) - 0.5).abs() < 1e-5);
        // Steps quantise it into levels from the bottom to the top of its range
        let stepped = route(ModSource::Lfo1, Remap { steps: 3, ..plain });
        assert_eq!([-1.0, -0.4, 0.1, 0.5, 1.0].map(|value| stepped.reshape(value, 0.0)), [-1.0, -1.0, 0.0, 1.0, 1.0]);
        // Another source scales it from 0 to 1, a bipolar one from its middle
        let scaled = route(ModSource::Lfo1, Remap { via: ModSource::ModWheel, ..plain });
        assert_eq!(scaled.reshape(-0.8, 0.25), -0.2);
        let by_lfo = route(ModSource::Env1, Remap { via: ModSource::Lfo2, ..plain });
        assert_eq!(by_lfo.reshape(0.8, 0.0), 0.4);
        assert_eq!(by_lfo.sources().collect::<Vec<_>>(), [ModSource::Env1, ModSource::Lfo2]);
        // All of it together, eight values at a time, the same as one at a time
        let everything = route(
            ModSource::Lfo1,
            Remap { polarity: Polarity::Unipolar, curve: -0.3, steps: 5, via: ModSource::Key },
        );
        let values = [-1.0, -0.6, -0.2, 0.0, 0.3, 0.55, 0.9, 1.0];
        let vias = [0.2, -1.0, 1.0, 0.5, -0.5, 0.0, 0.7, 1.0];
        let vector = everything.reshape8(f32x8::new(values), f32x8::new(vias)).to_array();
        for ((value, via), reshaped) in values.into_iter().zip(vias).zip(vector) {
            assert_eq!(reshaped.to_bits(), everything.reshape(value, via).to_bits(), "{value}, {via}");
        }
        // The amount applies on top
        let mut sources = [0.0; N_SOURCES];
        sources[ModSource::Lfo1.to_index()] = -1.0;
        let half = Route::remapped(ModSource::Lfo1, ModTarget::Cutoff, 0.5, Remap { polarity: Polarity::Unipolar, ..plain });
        assert_eq!(half.apply(&sources), 0.0);
        sources[ModSource::Lfo1.to_index()] = 1.0;
        assert_eq!(half.apply(&sources), 0.5);
    }
}
