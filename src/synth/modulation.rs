//! The modulation matrix: sources (LFOs, envelopes and MIDI controls) are routed to targets
//! (the synth's controls) with an amount, and the routes are evaluated per voice once per
//! control rate block.
//!
//! An amount of 100% maps the source's full swing onto the target's whole range: a slider
//! target moves across its whole length, the pitch targets by [`ModTarget::PITCH_SEMITONES`],
//! and the frequency-like targets (cutoff, resonance, drive, LFO rates) by a number of octaves.
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
}

/// Number of [`ModTarget`] variants.
pub const N_TARGETS: usize = 28;

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
