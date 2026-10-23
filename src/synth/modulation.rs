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

use nih_plug::prelude::Enum;

/// Something that can drive a modulation route. The LFOs and the random source are bipolar
/// (-1 to 1), everything else goes from 0 to 1.
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
}

/// Number of [`ModSource`] variants.
pub const N_SOURCES: usize = 13;
/// Number of modulation routes (slots in the matrix).
pub const N_SLOTS: usize = 16;
/// Number of LFOs.
pub const N_LFOS: usize = 3;
/// Number of modulation envelopes (the ones without a fixed job).
pub const N_MOD_ENVELOPES: usize = 2;

impl ModSource {
    pub fn lfo(index: usize) -> Self {
        [Self::Lfo1, Self::Lfo2, Self::Lfo3][index]
    }
    pub fn envelope(index: usize) -> Self {
        [Self::Env1, Self::Env2][index]
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
pub const N_TARGETS: usize = 42;

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
    /// Octaves the chorus rate moves at a full amount.
    pub const CHORUS_RATE_OCTAVES: f32 = 4.0;
    /// Octaves the delay time moves at a full amount.
    pub const DELAY_TIME_OCTAVES: f32 = 2.0;

    pub fn osc_pitch(osc: usize) -> Self {
        [Self::Osc1Pitch, Self::Osc2Pitch, Self::Osc3Pitch][osc]
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

/// An active modulation route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    pub source: ModSource,
    pub target: ModTarget,
    /// From -1 to 1.
    pub amount: f32,
}

/// The value of every source for one voice at one point in time.
pub type Sources = [f32; N_SOURCES];
/// The summed modulation of every target.
pub type Targets = [f32; N_TARGETS];

/// Sums the routes' contributions to each target.
pub fn apply_routes(routes: &[Route], sources: &Sources) -> Targets {
    let mut targets = [0.0; N_TARGETS];
    for route in routes {
        targets[route.target.to_index()] += sources[route.source.to_index()] * route.amount;
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
            Route { source: ModSource::Lfo1, target: ModTarget::Cutoff, amount: 0.5 },
            Route { source: ModSource::Velocity, target: ModTarget::Cutoff, amount: -0.1 },
            Route { source: ModSource::Velocity, target: ModTarget::Osc2Amp, amount: 1.0 },
        ];
        let targets = apply_routes(&routes, &sources);
        assert!((targets[ModTarget::Cutoff.to_index()] - 0.15).abs() < 1e-6);
        assert_eq!(targets[ModTarget::Osc2Amp.to_index()], 1.0);
        assert_eq!(targets[ModTarget::Osc1Amp.to_index()], 0.0);
    }
}
