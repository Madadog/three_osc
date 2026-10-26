//! The knobs each part of the synth has, for the layouts to arrange as they like: each one's
//! parameter, the short label it goes by, what it does, what it is a modulation target for, and
//! which kind of control it is, which the layouts colour it by.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{Color32, Response, Ui};

use super::help;
use super::knob::{Controls, Knob};
use crate::params::{
    ChorusParams, DelayParams, DistortionParams, EnvelopeParams, FilterEnvParams, FilterParams, GlobalParams,
    LfoParams, OscParams, ReverbParams, ThreeOscParams,
};
use crate::synth::modulation::{ModTarget, Scaling};

/// What a control is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// The sound's path: the oscillators, the filter and the effects.
    Sound,
    /// The modulation sources: the envelopes and the LFOs.
    Modulation,
    /// The settings for the whole synth.
    Global,
}

/// The colour a layout gives each group's knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub sound: Color32,
    pub modulation: Color32,
    pub global: Color32,
}

impl Colors {
    pub fn of(&self, group: Group) -> Color32 {
        match group {
            Group::Sound => self.sound,
            Group::Modulation => self.modulation,
            Group::Global => self.global,
        }
    }
}

/// A parameter a knob can turn.
#[derive(Clone, Copy)]
pub enum Numeric<'a> {
    Float(&'a FloatParam),
    Int(&'a IntParam),
}

/// A knob, see the module's documentation.
#[derive(Clone, Copy)]
pub struct Control<'a> {
    pub param: Numeric<'a>,
    pub label: &'static str,
    pub help: &'static str,
    pub target: ModTarget,
    /// How the modulation of `target` moves this control, when that isn't how it moves the
    /// target's own control (see [`ModTarget::scaling()`]).
    pub scaling: Option<Scaling>,
    pub group: Group,
}

impl<'a> Control<'a> {
    fn float(param: &'a FloatParam, label: &'static str, help: &'static str, group: Group) -> Self {
        Self {
            param: Numeric::Float(param),
            label,
            help,
            target: ModTarget::None,
            scaling: None,
            group,
        }
    }

    fn int(param: &'a IntParam, label: &'static str, help: &'static str, group: Group) -> Self {
        Self {
            param: Numeric::Int(param),
            label,
            help,
            target: ModTarget::None,
            scaling: None,
            group,
        }
    }

    fn modulated(self, target: ModTarget) -> Self {
        Self { target, ..self }
    }

    /// The same control, whose target's modulation moves it as `scaling` says.
    fn scaled(self, scaling: Scaling) -> Self {
        Self {
            scaling: Some(scaling),
            ..self
        }
    }

    /// The same control under another label.
    pub fn labelled(self, label: &'static str) -> Self {
        Self { label, ..self }
    }

    /// The same control under a short name in capitals (see [`short_name()`]), for a narrow
    /// knob.
    pub fn shortened(self) -> Self {
        self.labelled(short_name(self.label))
    }

    /// The parameter's full name, as the host shows it.
    pub fn name(&self) -> &str {
        match self.param {
            Numeric::Float(param) => param.name(),
            Numeric::Int(param) => param.name(),
        }
    }

    /// Adds the control's knob (see [`Knob`]), `width` wide, in its group's colour.
    pub fn show(&self, ui: &mut Ui, width: f32, controls: &Controls) -> Response {
        let color = controls.colors.of(self.group);
        match self.param {
            Numeric::Float(param) => Knob::new(param, self.label, self.help, color)
                .modulated(self.target, self.scaling)
                .show(ui, width, controls),
            Numeric::Int(param) => Knob::new(param, self.label, self.help, color)
                .modulated(self.target, self.scaling)
                .show(ui, width, controls),
        }
    }
}

/// A knob's name in five letters at most, in capitals, as the layouts with narrow knobs show it.
pub fn short_name(label: &'static str) -> &'static str {
    match label {
        "Level" => "LEVEL",
        "Pan" => "PAN",
        "Octave" => "OCT",
        "Semi" => "SEMI",
        "Mult" => "MULT",
        "Position" => "POS",
        "Phase" => "PHASE",
        "Fade" => "FADE",
        "Rand" => "RAND",
        "Unison" => "UNI",
        "Detune" => "DET",
        "Cutoff" => "CUT",
        "Resonance" => "RES",
        "Drive" => "DRIVE",
        "Keytrack" => "KEY",
        "Env Amt" => "ENV",
        "Attack" => "ATK",
        "Decay" => "DEC",
        "Sustain" => "SUS",
        "Release" => "REL",
        "Rate" => "RATE",
        "Glide" => "GLIDE",
        "Slide" => "SLIDE",
        "Bend" => "BEND",
        "Drift" => "DRIFT",
        "Pitch" => "PITCH",
        "Gain" => "GAIN",
        "Width" => "WIDTH",
        "Mix" => "MIX",
        "Depth" => "DEPTH",
        "Time" => "TIME",
        "Feedback" => "FDBK",
        "Size" => "SIZE",
        "Damping" => "DAMP",
        other => other,
    }
}

/// Adds `knobs` one after the other, each `width` wide.
pub fn show_all(ui: &mut Ui, knobs: &[Control], width: f32, controls: &Controls) {
    for knob in knobs {
        knob.show(ui, width, controls);
    }
}

/// An oscillator's knobs: its level and pan, its pitch (octave, semitones and multiplier), its
/// shape (wavetable position, phase and its randomness) and its unison.
pub fn oscillator(osc: &OscParams, index: usize) -> [Control<'_>; 10] {
    use Group::Sound;
    [
        Control::float(&osc.amp, "Level", help::OSC_AMP, Sound).modulated(ModTarget::osc_amp(index)),
        Control::float(&osc.pan, "Pan", help::OSC_PAN, Sound).modulated(ModTarget::osc_pan(index)),
        Control::int(&osc.octave, "Octave", help::OSC_OCTAVE, Sound).modulated(ModTarget::osc_octave(index)),
        Control::float(&osc.semitone, "Semi", help::OSC_SEMITONE, Sound).modulated(ModTarget::osc_pitch(index)),
        Control::int(&osc.multiplier, "Mult", help::OSC_MULTIPLIER, Sound).modulated(ModTarget::osc_multiplier(index)),
        Control::float(&osc.position, "Position", help::OSC_POSITION, Sound).modulated(ModTarget::osc_position(index)),
        Control::float(&osc.phase, "Phase", help::OSC_PHASE, Sound),
        Control::float(&osc.phase_rand, "Rand", help::OSC_PHASE_RAND, Sound),
        Control::int(&osc.voices, "Unison", help::OSC_VOICES, Sound),
        Control::float(&osc.super_detune, "Detune", help::OSC_DETUNE, Sound).modulated(ModTarget::osc_detune(index)),
    ]
}

/// How much the oscillator after an oscillator modulates it: phase, frequency and amplitude
/// modulation. Osc 3 has nothing after it.
pub fn oscillator_modulation(params: &ThreeOscParams, index: usize) -> Option<[Control<'_>; 3]> {
    use Group::Sound;
    let modulation = [Some(&params.osc1_mod), Some(&params.osc2_mod), None][index]?;
    let target = |target: Option<ModTarget>| target.unwrap_or(ModTarget::None);
    Some([
        Control::float(&modulation.pm, "PM", help::OSC_PM, Sound).modulated(target(ModTarget::osc_pm(index))),
        Control::float(&modulation.fm, "FM", help::OSC_FM, Sound).modulated(target(ModTarget::osc_fm(index))),
        Control::float(&modulation.am, "AM", help::OSC_AM, Sound).modulated(target(ModTarget::osc_am(index))),
    ])
}

/// The filter's knobs, besides how much its envelope moves it.
pub fn filter(filter: &FilterParams) -> [Control<'_>; 4] {
    use Group::Sound;
    [
        Control::float(&filter.cutoff, "Cutoff", help::FILTER_CUTOFF, Sound).modulated(ModTarget::Cutoff),
        Control::float(&filter.resonance, "Resonance", help::FILTER_RESONANCE, Sound).modulated(ModTarget::Resonance),
        Control::float(&filter.drive, "Drive", help::FILTER_DRIVE, Sound).modulated(ModTarget::Drive),
        Control::float(&filter.keytrack, "Keytrack", help::FILTER_KEYTRACK, Sound),
    ]
}

/// How much the filter envelope moves the cutoff.
pub fn filter_envelope_amount(filter_env: &FilterEnvParams) -> Control<'_> {
    Control::float(&filter_env.amount, "Env Amt", help::FILTER_ENV_AMOUNT, Group::Modulation)
}

/// An envelope's times and sustain level. (Its slopes are set on its display.)
pub fn envelope(envelope: &EnvelopeParams) -> [Control<'_>; 4] {
    use Group::Modulation;
    [
        Control::float(&envelope.attack, "Attack", help::ENV_ATTACK, Modulation),
        Control::float(&envelope.decay, "Decay", help::ENV_DECAY, Modulation),
        Control::float(&envelope.sustain, "Sustain", help::ENV_SUSTAIN, Modulation),
        Control::float(&envelope.release, "Release", help::ENV_RELEASE, Modulation),
    ]
}

/// An LFO's rate: in Hz, or synced, the note length a cycle takes.
pub fn lfo_rate(lfo: &LfoParams, index: usize) -> Control<'_> {
    let target = ModTarget::lfo_rate(index);
    if lfo.sync.value() {
        // (Its rate's modulation moves through about three note lengths an octave)
        let steps = 3.0 * ModTarget::LFO_RATE_OCTAVES;
        Control::int(&lfo.division, "Rate", help::LFO_DIVISION, Group::Modulation)
            .modulated(target)
            .scaled(Scaling::Linear(steps))
    } else {
        Control::float(&lfo.rate, "Rate", help::LFO_RATE, Group::Modulation).modulated(target)
    }
}

/// An LFO's knobs: its rate (see [`lfo_rate()`]), where in its cycle each note starts it, and
/// how long it takes to fade in.
pub fn lfo(lfo: &LfoParams, index: usize) -> [Control<'_>; 3] {
    use Group::Modulation;
    [
        lfo_rate(lfo, index),
        Control::float(&lfo.phase, "Phase", help::LFO_PHASE, Modulation),
        Control::float(&lfo.fade, "Fade", help::LFO_FADE, Modulation),
    ]
}

/// How notes are played: gliding, sliding into them, the pitch wheel's range, the octaves'
/// stretch and the pitch of the whole synth.
pub fn voice(global: &GlobalParams) -> [Control<'_>; 5] {
    use Group::Global;
    [
        Control::float(&global.portamento_rate, "Glide", help::PORTAMENTO, Global),
        Control::float(&global.pitch_offset, "Slide", help::PITCH_SLIDE, Global),
        Control::int(&global.bend_range, "Bend", help::BEND_RANGE, Global),
        Control::float(&global.octave_detune, "Drift", help::OCTAVE_DRIFT, Global),
        Control::int(&global.global_pitch, "Pitch", help::GLOBAL_PITCH, Global).modulated(ModTarget::Pitch),
    ]
}

/// The output's level and stereo width.
pub fn output(global: &GlobalParams) -> [Control<'_>; 2] {
    use Group::Global;
    [
        Control::float(&global.output_gain, "Gain", help::OUTPUT_GAIN, Global).modulated(ModTarget::Volume),
        Control::float(&global.stereo_width, "Width", help::STEREO_WIDTH, Global),
    ]
}

pub fn distortion(distortion: &DistortionParams) -> [Control<'_>; 2] {
    use Group::Sound;
    [
        Control::float(&distortion.drive, "Drive", help::DISTORTION_DRIVE, Sound).modulated(ModTarget::DistortionDrive),
        Control::float(&distortion.mix, "Mix", help::DISTORTION_MIX, Sound).modulated(ModTarget::DistortionMix),
    ]
}

pub fn chorus(chorus: &ChorusParams) -> [Control<'_>; 3] {
    use Group::Sound;
    [
        Control::float(&chorus.rate, "Rate", help::CHORUS_RATE, Sound).modulated(ModTarget::ChorusRate),
        Control::float(&chorus.depth, "Depth", help::CHORUS_DEPTH, Sound).modulated(ModTarget::ChorusDepth),
        Control::float(&chorus.mix, "Mix", help::CHORUS_MIX, Sound).modulated(ModTarget::ChorusMix),
    ]
}

pub fn delay(delay: &DelayParams) -> [Control<'_>; 3] {
    use Group::Sound;
    [
        Control::float(&delay.time, "Time", help::DELAY_TIME, Sound).modulated(ModTarget::DelayTime),
        Control::float(&delay.feedback, "Feedback", help::DELAY_FEEDBACK, Sound).modulated(ModTarget::DelayFeedback),
        Control::float(&delay.mix, "Mix", help::DELAY_MIX, Sound).modulated(ModTarget::DelayMix),
    ]
}

pub fn reverb(reverb: &ReverbParams) -> [Control<'_>; 3] {
    use Group::Sound;
    [
        Control::float(&reverb.size, "Size", help::REVERB_SIZE, Sound).modulated(ModTarget::ReverbSize),
        Control::float(&reverb.damping, "Damping", help::REVERB_DAMPING, Sound).modulated(ModTarget::ReverbDamping),
        Control::float(&reverb.mix, "Mix", help::REVERB_MIX, Sound).modulated(ModTarget::ReverbMix),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::modulation::N_TARGETS;

    /// Every knob of the synth.
    fn all(params: &ThreeOscParams) -> Vec<Control<'_>> {
        let mut controls = Vec::new();
        for index in 0..3 {
            controls.extend(oscillator(params.oscillator(index), index));
            controls.extend(oscillator_modulation(params, index).into_iter().flatten());
        }
        controls.extend(filter(&params.filter));
        controls.push(filter_envelope_amount(&params.filter_env));
        for envelope_params in [&params.vol_env, &params.filter_env.envelope, &params.mod_envs[0].envelope, &params.mod_envs[1].envelope] {
            controls.extend(envelope(envelope_params));
        }
        for (index, lfo) in params.lfos.iter().enumerate() {
            controls.push(lfo_rate(lfo, index));
        }
        controls.extend(voice(&params.global));
        controls.extend(output(&params.global));
        let fx = &params.fx;
        controls.extend(distortion(&fx.distortion));
        controls.extend(chorus(&fx.chorus));
        controls.extend(delay(&fx.delay));
        controls.extend(reverb(&fx.reverb));
        controls
    }

    #[test]
    fn every_modulation_target_has_one_knob() {
        let params = ThreeOscParams::default();
        let controls = all(&params);
        // (Fine Pitch is the only target without a knob of its own)
        for index in 1..N_TARGETS {
            let target = ModTarget::from_index(index);
            let knobs = controls.iter().filter(|control| control.target == target).count();
            let expected = usize::from(target != ModTarget::FinePitch);
            assert_eq!(knobs, expected, "{}", ModTarget::variants()[index]);
        }
        // And no parameter has two knobs
        let mut params: Vec<*const ()> = controls
            .iter()
            .map(|control| match control.param {
                Numeric::Float(param) => param as *const FloatParam as *const (),
                Numeric::Int(param) => param as *const IntParam as *const (),
            })
            .collect();
        let count = params.len();
        params.sort();
        params.dedup();
        assert_eq!(params.len(), count);
    }
}
