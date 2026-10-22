//! Plugin parameters.
//!
//! These replace the LV2 control ports (and the `build.rs` script that used to generate the `.ttl`
//! metadata for them). Each parameter's name, range, default value and display formatting is
//! declared right here, and nih-plug exposes them to the host and to the GUI.
//!
//! The parameter IDs (e.g. `osc1_wave`, `fil1_cutoff`, `vol_attack`) intentionally match the old
//! LV2 port symbols.

use std::f32::consts::PI;
use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;

use crate::synth::filter::{FilterModel, FilterType};
use crate::synth::oscillator::OscWave;
use crate::synth::Polyphony;

/// Waveforms available to the oscillators and the LFO.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    #[id = "sine"]
    Sine,
    #[id = "triangle"]
    Triangle,
    #[id = "saw"]
    Saw,
    #[id = "exponential"]
    Exponential,
    #[id = "square"]
    Square,
}

impl Wave {
    pub fn to_osc_wave(self) -> OscWave {
        match self {
            Wave::Sine => OscWave::Sine,
            Wave::Triangle => OscWave::Tri,
            Wave::Saw => OscWave::Saw,
            Wave::Exponential => OscWave::Exp,
            Wave::Square => OscWave::Square,
        }
    }

    /// Like [`to_osc_wave()`][Self::to_osc_wave()], but the square wave becomes a pulse wave with
    /// an adjustable width. Used by Osc 3.
    pub fn to_osc_wave_pulse(self) -> OscWave {
        match self {
            Wave::Square => OscWave::Pulse { width: PI },
            wave => wave.to_osc_wave(),
        }
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterModelParam {
    #[id = "none"]
    None,
    #[id = "rc"]
    #[name = "RC"]
    Rc,
    #[id = "ladder"]
    Ladder,
    #[id = "digital"]
    Digital,
}

impl FilterModelParam {
    pub fn to_filter_model(self) -> FilterModel {
        match self {
            FilterModelParam::None => FilterModel::None,
            FilterModelParam::Rc => FilterModel::RcFilter,
            FilterModelParam::Ladder => FilterModel::LadderFilter,
            FilterModelParam::Digital => FilterModel::SvfSimperFilter,
        }
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterTypeParam {
    #[id = "lowpass"]
    Lowpass,
    #[id = "bandpass"]
    Bandpass,
    #[id = "highpass"]
    Highpass,
}

impl FilterTypeParam {
    pub fn to_filter_type(self) -> FilterType {
        match self {
            FilterTypeParam::Lowpass => FilterType::Lowpass,
            FilterTypeParam::Bandpass => FilterType::Bandpass,
            FilterTypeParam::Highpass => FilterType::Highpass,
        }
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoTarget {
    #[id = "all"]
    #[name = "All Oscs"]
    All,
    #[id = "osc1"]
    #[name = "Osc. 1"]
    Osc1,
    #[id = "osc2"]
    #[name = "Osc. 2"]
    Osc2,
    #[id = "osc3"]
    #[name = "Osc. 3"]
    Osc3,
}

impl LfoTarget {
    /// The index of the targeted oscillator, or `None` when the LFO targets all oscillators.
    pub fn to_osc_index(self) -> Option<usize> {
        match self {
            LfoTarget::All => None,
            LfoTarget::Osc1 => Some(0),
            LfoTarget::Osc2 => Some(1),
            LfoTarget::Osc3 => Some(2),
        }
    }
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyphonyParam {
    #[id = "polyphonic"]
    Polyphonic,
    #[id = "monophonic"]
    Monophonic,
    #[id = "legato"]
    Legato,
}

impl PolyphonyParam {
    pub fn to_polyphony(self) -> Polyphony {
        match self {
            PolyphonyParam::Polyphonic => Polyphony::Polyphonic,
            PolyphonyParam::Monophonic => Polyphony::Monophonic,
            PolyphonyParam::Legato => Polyphony::Legato,
        }
    }
}

/// All of the synth's parameters.
#[derive(Params)]
pub struct ThreeOscParams {
    /// The editor's window size, saved together with the parameters so it is restored when the
    /// plugin is reloaded.
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[nested(id_prefix = "osc1", group = "Osc 1")]
    pub osc1: OscParams,
    #[nested(id_prefix = "osc1", group = "Osc 1")]
    pub osc1_mod: ModParams,

    #[nested(id_prefix = "osc2", group = "Osc 2")]
    pub osc2: OscParams,
    #[nested(id_prefix = "osc2", group = "Osc 2")]
    pub osc2_mod: ModParams,

    #[nested(id_prefix = "osc3", group = "Osc 3")]
    pub osc3: OscParams,
    #[nested(id_prefix = "osc3", group = "Osc 3")]
    pub osc3_pulse: PulseParams,

    #[nested(id_prefix = "fil1", group = "Filter 1")]
    pub filter: FilterParams,
    #[nested(id_prefix = "fil1", group = "Filter 1 Env.")]
    pub filter_env: FilterEnvParams,

    #[nested(id_prefix = "vol", group = "Volume Env.")]
    pub vol_env: EnvelopeParams,

    #[nested(id_prefix = "lfo", group = "LFO 1")]
    pub lfo: LfoParams,

    #[nested(group = "Global")]
    pub global: GlobalParams,
}

impl Default for ThreeOscParams {
    fn default() -> Self {
        Self {
            editor_state: crate::editor::default_state(),

            // The first oscillator defaults to an audible saw wave, the others are silent.
            osc1: OscParams::new(100.0, Wave::Saw),
            osc1_mod: ModParams::new("Osc 2"),
            osc2: OscParams::new(0.0, Wave::Sine),
            osc2_mod: ModParams::new("Osc 3"),
            osc3: OscParams::new(0.0, Wave::Sine),
            osc3_pulse: PulseParams::new(),

            filter: FilterParams::new(),
            filter_env: FilterEnvParams::new(),

            vol_env: EnvelopeParams::new(0.002, 0.25, 1.0, 0.005),

            lfo: LfoParams::new(),

            global: GlobalParams::new(),
        }
    }
}

/// Controls shared by all three oscillators.
#[derive(Params)]
pub struct OscParams {
    #[id = "wave"]
    pub wave: EnumParam<Wave>,
    /// Oscillator output volume, in percent.
    #[id = "amp"]
    pub amp: FloatParam,
    /// Detune in semitones.
    #[id = "semitone"]
    pub semitone: FloatParam,
    /// Detune in octaves.
    #[id = "octave"]
    pub octave: IntParam,
    /// Pitch multiplier (positive) or divider (negative), 0 is bypassed.
    #[id = "multiplier"]
    pub multiplier: IntParam,
    /// Number of unison voices.
    #[id = "voices"]
    pub voices: IntParam,
    /// Unison detune, in percent.
    #[id = "super_detune"]
    pub super_detune: FloatParam,
    /// Starting phase, in percent of a cycle.
    #[id = "phase"]
    pub phase: FloatParam,
    /// Phase randomisation, in percent of a cycle.
    #[id = "phase_rand"]
    pub phase_rand: FloatParam,
}

impl OscParams {
    pub fn new(default_amp: f32, default_wave: Wave) -> Self {
        Self {
            wave: EnumParam::new("Wave", default_wave),
            amp: FloatParam::new(
                "Amplitude",
                default_amp,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            semitone: FloatParam::new(
                "Detune",
                0.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 24.0,
                },
            )
            .with_unit(" st")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            octave: IntParam::new("Octave", 0, IntRange::Linear { min: -8, max: 8 }),
            multiplier: IntParam::new("Freq. Mult", 0, IntRange::Linear { min: -64, max: 64 })
                .with_value_to_string(v2s_multiplier())
                .with_string_to_value(s2v_multiplier()),
            voices: IntParam::new("Unison", 1, IntRange::Linear { min: 1, max: 32 }),
            super_detune: FloatParam::new(
                "Unison Detune",
                21.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            phase: FloatParam::new(
                "Phase",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            phase_rand: FloatParam::new(
                "Phase Rand.",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
        }
    }
}

/// How much an oscillator is modulated by the oscillator after it. Osc 3 has no modulator, so it
/// doesn't have these.
#[derive(Params)]
pub struct ModParams {
    #[id = "pm"]
    pub pm: FloatParam,
    #[id = "fm"]
    pub fm: FloatParam,
    #[id = "am"]
    pub am: FloatParam,
}

impl ModParams {
    pub fn new(modulator: &str) -> Self {
        Self {
            pm: amount_param(format!("<- {modulator} PM"), 0.0),
            fm: amount_param(format!("<- {modulator} FM"), 0.0),
            am: amount_param(format!("<- {modulator} AM"), 0.0),
        }
    }
}

/// Osc 3 swaps the modulation controls for pulse width modulation.
#[derive(Params)]
pub struct PulseParams {
    /// Pulse width, in percent of a cycle. 50 % is a square wave.
    #[id = "pwm"]
    pub pulse_width: FloatParam,
}

impl PulseParams {
    pub fn new() -> Self {
        Self {
            pulse_width: FloatParam::new(
                "Pulse Width",
                50.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 50.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
        }
    }
}

impl Default for PulseParams {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Params)]
pub struct FilterParams {
    #[id = "model"]
    pub model: EnumParam<FilterModelParam>,
    #[id = "type"]
    pub filter_type: EnumParam<FilterTypeParam>,
    /// Cutoff frequency in Hz.
    #[id = "cutoff"]
    pub cutoff: FloatParam,
    #[id = "resonance"]
    pub resonance: FloatParam,
    #[id = "drive"]
    pub drive: FloatParam,
    /// How much the cutoff follows the note pitch, 0 to 1.
    #[id = "keytrack"]
    pub keytrack: FloatParam,
}

impl FilterParams {
    pub fn new() -> Self {
        Self {
            model: EnumParam::new("Model", FilterModelParam::Digital),
            filter_type: EnumParam::new("Type", FilterTypeParam::Lowpass),
            cutoff: FloatParam::new(
                "Cutoff Freq.",
                22000.0,
                FloatRange::Skewed {
                    min: 10.0,
                    max: 22000.0,
                    factor: FloatRange::skew_factor(-2.5),
                },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            resonance: FloatParam::new(
                "Resonance",
                0.7,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            drive: FloatParam::new(
                "Drive",
                1.0,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            keytrack: amount_param("Keytrack", 0.0),
        }
    }
}

impl Default for FilterParams {
    fn default() -> Self {
        Self::new()
    }
}

/// The filter's cutoff envelope: an ADSR envelope plus the amount it affects the cutoff.
#[derive(Params)]
pub struct FilterEnvParams {
    #[id = "env_amount"]
    pub amount: FloatParam,
    #[nested]
    pub envelope: EnvelopeParams,
}

impl FilterEnvParams {
    pub fn new() -> Self {
        Self {
            amount: amount_param("Amount", 0.25),
            envelope: EnvelopeParams::new(0.0, 0.25, 0.0, 0.005),
        }
    }
}

impl Default for FilterEnvParams {
    fn default() -> Self {
        Self::new()
    }
}

/// An ADSR envelope with an adjustable slope.
#[derive(Params)]
pub struct EnvelopeParams {
    /// Attack time in seconds.
    #[id = "attack"]
    pub attack: FloatParam,
    /// Decay time in seconds.
    #[id = "decay"]
    pub decay: FloatParam,
    /// Sustain level, 0 to 1.
    #[id = "sustain"]
    pub sustain: FloatParam,
    /// Release time in seconds.
    #[id = "release"]
    pub release: FloatParam,
    /// Slope of the envelope segments: 0 is linear, positive is logarithmic (punchy), negative is
    /// exponential.
    #[id = "slope"]
    pub slope: FloatParam,
}

impl EnvelopeParams {
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Self {
            // The LV2 version used a minimum attack and decay time of 1 ms that was mapped to 0
            // internally, as a workaround for how hosts display logarithmic ranges. That's no
            // longer needed, so 0 is simply 0 here.
            attack: time_param("Attack", attack, 0.0),
            decay: time_param("Decay", decay, 0.0),
            sustain: amount_param("Sustain", sustain),
            // A zero length release would divide by zero in the envelope.
            release: time_param("Release", release, 0.001),
            slope: FloatParam::new(
                "Slope",
                1.0,
                FloatRange::Linear {
                    min: -8.0,
                    max: 8.0,
                },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
        }
    }
}

#[derive(Params)]
pub struct LfoParams {
    #[id = "target"]
    pub target: EnumParam<LfoTarget>,
    #[id = "wave"]
    pub wave: EnumParam<Wave>,
    /// LFO frequency in Hz.
    #[id = "freq"]
    pub freq: FloatParam,
    /// Pitch modulation amount (vibrato).
    #[id = "freq_mod"]
    pub freq_mod: FloatParam,
    /// Amplitude modulation amount (tremolo).
    #[id = "amp_mod"]
    pub amp_mod: FloatParam,
    /// Modulation of the PM/FM/AM amounts the target receives.
    #[id = "mod_mod"]
    pub mod_mod: FloatParam,
    /// Filter cutoff modulation amount.
    #[id = "filter_mod"]
    pub filter_mod: FloatParam,
}

impl LfoParams {
    pub fn new() -> Self {
        Self {
            target: EnumParam::new("Target Osc.", LfoTarget::All),
            wave: EnumParam::new("Wave", Wave::Sine),
            freq: FloatParam::new(
                "Freq.",
                5.0,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 22000.0,
                    factor: FloatRange::skew_factor(-3.0),
                },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(2))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            freq_mod: skewed_amount_param("-> Vibrato", 0.0),
            amp_mod: skewed_amount_param("-> Tremolo", 0.0),
            mod_mod: skewed_amount_param("-> Modulation", 0.0),
            filter_mod: skewed_amount_param("-> Fil. Cutoff", 0.0),
        }
    }
}

impl Default for LfoParams {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Params)]
pub struct GlobalParams {
    #[id = "polyphony"]
    pub polyphony: EnumParam<PolyphonyParam>,
    /// How slowly the pitch glides to a new note, 0 to 0.999.
    #[id = "portamento_rate"]
    pub portamento_rate: FloatParam,
    /// Initial pitch offset at the start of a note, in semitones.
    #[id = "pitch_offset"]
    pub pitch_offset: FloatParam,
    /// Octave stretch.
    #[id = "octave_detune"]
    pub octave_detune: FloatParam,
    /// Master output gain in dB.
    #[id = "output_gain"]
    pub output_gain: FloatParam,
    /// How far unison voices are spread across the stereo field, 0 (mono) to 1.
    #[id = "stereo_width"]
    pub stereo_width: FloatParam,
    /// Global pitch offset in semitones.
    #[id = "global_pitch"]
    pub global_pitch: IntParam,
    /// Pitch wheel range in semitones.
    #[id = "bend_range"]
    pub bend_range: IntParam,
}

impl GlobalParams {
    pub fn new() -> Self {
        Self {
            polyphony: EnumParam::new("Polyphony", PolyphonyParam::Polyphonic),
            portamento_rate: FloatParam::new(
                "Portamento",
                0.25,
                FloatRange::Linear {
                    min: 0.0,
                    max: 0.999,
                },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(3)),
            pitch_offset: FloatParam::new(
                "Pitch Slide",
                0.0,
                FloatRange::Linear {
                    min: -96.0,
                    max: 96.0,
                },
            )
            .with_unit(" st")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            octave_detune: FloatParam::new(
                "Octave Drift",
                0.0,
                FloatRange::Linear {
                    min: -0.04,
                    max: 0.04,
                },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(4)),
            output_gain: FloatParam::new(
                "Output Gain",
                -18.0,
                FloatRange::Linear {
                    min: -90.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            stereo_width: amount_param("Stereo Width", 0.0),
            global_pitch: IntParam::new("Output Pitch", 0, IntRange::Linear { min: -24, max: 24 })
                .with_unit(" st"),
            bend_range: IntParam::new("Bend Range", 2, IntRange::Linear { min: -24, max: 24 })
                .with_unit(" st"),
        }
    }
}

impl Default for GlobalParams {
    fn default() -> Self {
        Self::new()
    }
}

/// A linear `0.0..=1.0` parameter displayed as a percentage.
fn amount_param(name: impl Into<String>, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_percentage(1))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

/// A `0.0..=1.0` parameter displayed as a percentage, with more resolution for small values.
fn skewed_amount_param(name: impl Into<String>, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min: 0.0,
            max: 1.0,
            factor: FloatRange::skew_factor(-1.5),
        },
    )
    .with_unit(" %")
    .with_value_to_string(formatters::v2s_f32_percentage(1))
    .with_string_to_value(formatters::s2v_f32_percentage())
}

/// An envelope time parameter in seconds, up to 15 seconds, with more resolution for short times.
fn time_param(name: impl Into<String>, default: f32, min: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max: 15.0,
            factor: FloatRange::skew_factor(-2.5),
        },
    )
    .with_value_to_string(v2s_seconds())
    .with_string_to_value(s2v_seconds())
}

/// Formats a time in seconds as milliseconds when it is shorter than one second.
fn v2s_seconds() -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    Arc::new(|seconds| {
        if seconds < 1.0 {
            format!("{:.1} ms", seconds * 1000.0)
        } else {
            format!("{seconds:.2} s")
        }
    })
}

/// Parses a time, with an optional `ms` or `s` suffix, into seconds.
#[allow(clippy::type_complexity)]
fn s2v_seconds() -> Arc<dyn Fn(&str) -> Option<f32> + Send + Sync> {
    Arc::new(|string| {
        let string = string.trim();
        if let Some(number) = string.strip_suffix("ms") {
            number.trim().parse::<f32>().ok().map(|ms| ms / 1000.0)
        } else if let Some(number) = string.strip_suffix('s') {
            number.trim().parse().ok()
        } else {
            string.parse().ok()
        }
    })
}

/// Displays the frequency multiplier parameter as a multiplication (`x3`) or division (`/3`).
///
/// Positive values multiply the pitch, negative values divide it. Negative multiplication would
/// only reverse the wave, which isn't very useful.
fn v2s_multiplier() -> Arc<dyn Fn(i32) -> String + Send + Sync> {
    Arc::new(|value| {
        if value > 0 {
            format!("x{}", value + 1)
        } else if value < 0 {
            format!("/{}", 1 - value)
        } else {
            String::from("Off")
        }
    })
}

/// Parses the `x3` / `/3` / `Off` strings produced by [`v2s_multiplier()`], as well as plain
/// integers.
#[allow(clippy::type_complexity)]
fn s2v_multiplier() -> Arc<dyn Fn(&str) -> Option<i32> + Send + Sync> {
    Arc::new(|string| {
        let string = string.trim();
        if string.eq_ignore_ascii_case("off") {
            Some(0)
        } else if let Some(number) = string.strip_prefix(|c: char| matches!(c, 'x' | 'X' | '*')) {
            number.trim().parse::<i32>().ok().map(|n| n - 1)
        } else if let Some(number) = string.strip_prefix(|c: char| matches!(c, '/' | '÷')) {
            number.trim().parse::<i32>().ok().map(|n| 1 - n)
        } else {
            string.parse().ok()
        }
    })
}

/// Converts a multiplier parameter value to the pitch multiplier used by the synth.
pub fn multiplier_to_pitch_multiplier(multiplier: i32) -> f32 {
    if multiplier >= 0 {
        1.0 + multiplier as f32
    } else {
        1.0 / (1.0 - multiplier as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplier_formatting_round_trips() {
        let to_string = v2s_multiplier();
        let from_string = s2v_multiplier();
        for value in -64..=64 {
            assert_eq!(from_string(&to_string(value)), Some(value));
        }
        assert_eq!(from_string("3"), Some(3));
        assert_eq!(from_string("x1"), Some(0));
        assert_eq!(from_string("/1"), Some(0));
    }

    #[test]
    fn multiplier_matches_lv2_behaviour() {
        assert_eq!(multiplier_to_pitch_multiplier(0), 1.0);
        assert_eq!(multiplier_to_pitch_multiplier(1), 2.0);
        assert_eq!(multiplier_to_pitch_multiplier(-1), 0.5);
        assert_eq!(multiplier_to_pitch_multiplier(-3), 0.25);
    }

    #[test]
    fn seconds_formatting_round_trips() {
        let to_string = v2s_seconds();
        let from_string = s2v_seconds();
        assert_eq!(to_string(0.0025), "2.5 ms");
        assert_eq!(to_string(1.5), "1.50 s");
        assert!((from_string("2.5 ms").unwrap() - 0.0025).abs() < 1e-6);
        assert_eq!(from_string("1.50 s"), Some(1.5));
        assert_eq!(from_string("0.25"), Some(0.25));
        assert_eq!(from_string("nonsense"), None);
    }
}
