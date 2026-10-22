//! The plugin's GUI, built with egui through `nih_plug_egui`.
//!
//! It's a deliberately simple "everything on one page" layout: the three oscillators across the
//! top, and the filter, envelopes, LFO and global controls underneath. Every control is a
//! [`ParamSlider`] (or a combo box for the enumerated parameters) bound directly to a parameter, so
//! host automation and the GUI stay in sync. Hovering a control's label shows what it does.

use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, RichText, Ui};
use nih_plug_egui::resizable_window::ResizableWindow;
use nih_plug_egui::widgets::ParamSlider;
use nih_plug_egui::{create_egui_editor, EguiState};

use crate::params::{
    EnvelopeParams, FilterEnvParams, FilterParams, GlobalParams, LfoParams, ModParams, OscParams,
    PulseParams, ThreeOscParams,
};
use crate::ThreeOscPlugin;

/// The window's default size, in logical pixels.
pub const DEFAULT_WIDTH: u32 = 1000;
pub const DEFAULT_HEIGHT: u32 = 720;

const SLIDER_WIDTH: f32 = 110.0;
const COMBO_WIDTH: f32 = 150.0;

/// The window size used when the plugin is first loaded. The size is stored with the plugin's
/// state, so resizing the window is remembered.
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(DEFAULT_WIDTH, DEFAULT_HEIGHT)
}

pub fn create(params: Arc<ThreeOscParams>) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();
    create_egui_editor(
        egui_state.clone(),
        (),
        |_, _| {},
        move |egui_ctx, setter, _state| {
            ResizableWindow::new("three-osc-window")
                .min_size(egui::vec2(320.0, 240.0))
                .show(egui_ctx, &egui_state, |ui| {
                    // The layout has a fixed natural size; if the window is made smaller than
                    // that the contents scroll instead of being cut off.
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| draw(ui, &params, setter));
                });
        },
    )
}

fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        ui.heading("Three Osc");
        ui.label(RichText::new(format!("v{}", ThreeOscPlugin::VERSION)).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(
                    "Double-click a slider to reset it, Shift+drag for fine control, \
                     click a value to type it in. Hover a label for help.",
                )
                .weak(),
            );
        });
    });
    ui.separator();

    ui.horizontal_top(|ui| {
        oscillator(ui, "Osc 1", &params.osc1, Some(&params.osc1_mod), None, setter);
        oscillator(ui, "Osc 2", &params.osc2, Some(&params.osc2_mod), None, setter);
        oscillator(ui, "Osc 3", &params.osc3, None, Some(&params.osc3_pulse), setter);
    });

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            filter(ui, "Filter 1", &params.filter, setter);
            filter_envelope(ui, "Filter 1 Envelope", &params.filter_env, setter);
        });
        ui.vertical(|ui| {
            envelope(ui, "Volume Envelope", &params.vol_env, setter);
            lfo(ui, "LFO 1", &params.lfo, setter);
        });
        global(ui, "Global", &params.global, setter);
    });
}

fn oscillator(
    ui: &mut Ui,
    title: &str,
    osc: &OscParams,
    modulation: Option<&ModParams>,
    pulse: Option<&PulseParams>,
    setter: &ParamSetter,
) {
    section(ui, title, |ui| {
        combo(
            ui,
            &osc.wave,
            "The oscillator's waveform. All waves are bandlimited (alias-free).",
            setter,
        );
        slider(
            ui,
            &osc.amp,
            "Oscillator output volume. Doesn't affect PM, FM or AM.",
            setter,
        );
        slider(ui, &osc.semitone, "Oscillator detune in semitones.", setter);
        slider(ui, &osc.octave, "Oscillator detune in octaves.", setter);
        slider(
            ui,
            &osc.multiplier,
            "Oscillator pitch multiplier / divider. Positive values multiply pitch, while \
             negative values divide. A value of 0 means this control is bypassed.",
            setter,
        );
        if let Some(modulation) = modulation {
            slider(
                ui,
                &modulation.pm,
                "Phase modulation of this oscillator by the oscillator after it.",
                setter,
            );
            slider(
                ui,
                &modulation.fm,
                "Frequency modulation of this oscillator by the oscillator after it. Can be used \
                 for vibrato effects when the modulator is at a low pitch.",
                setter,
            );
            slider(
                ui,
                &modulation.am,
                "Amplitude modulation of this oscillator by the oscillator after it. Also known as \
                 'Ring Modulation'. Can be used for telephone sounds and tremolo.",
                setter,
            );
        }
        if let Some(pulse) = pulse {
            slider(
                ui,
                &pulse.pulse_width,
                "Pulse width modulation. Adjusts the width of the square waveform.",
                setter,
            );
        }
        slider(
            ui,
            &osc.voices,
            "Copies of this oscillator which are overlaid at different pitches. Useful for the \
             supersaw sound.",
            setter,
        );
        slider(
            ui,
            &osc.super_detune,
            "Unison voice detune. Does nothing if unison = 1.",
            setter,
        );
        slider(
            ui,
            &osc.phase,
            "Point at which the oscillator wave starts. Does nothing when Phase Rand = 100.",
            setter,
        );
        slider(
            ui,
            &osc.phase_rand,
            "Partially or fully randomises the point where the oscillator wave starts. Keep this \
             fairly high when using Unison.",
            setter,
        );
    });
}

fn filter(ui: &mut Ui, title: &str, filter: &FilterParams, setter: &ParamSetter) {
    section(ui, title, |ui| {
        combo(
            ui,
            &filter.model,
            "There are 3 filter models: Digital is an SVF identical to an IIR biquad filter \
             (except stable over large cutoff changes), RC is a darker filter capable of \
             aggressive self-resonance, and Ladder is based on a famous analog filter and sounds \
             the best, with its code coming from janne808's Kocmoc Rack Modules project.",
            setter,
        );
        combo(
            ui,
            &filter.filter_type,
            "Lowpass cuts out high frequencies, Highpass cuts out low frequencies, and Bandpass \
             allows a small band of frequencies at the cutoff point.",
            setter,
        );
        slider(
            ui,
            &filter.cutoff,
            "Changes the frequency at which the filter starts taking effect. Try it out.",
            setter,
        );
        slider(
            ui,
            &filter.resonance,
            "Adds feedback to the filter loop, creating a volume spike at the filter's cutoff \
             frequency. On the RC and Ladder filters, setting this high enough creates a \
             self-sustaining sine wave.",
            setter,
        );
        slider(
            ui,
            &filter.drive,
            "Multiplies the amplitude of the filter input, creating distortion inside the RC and \
             Ladder filters. Does not amplify when Model = None or Digital, to keep volume equal \
             between filters.",
            setter,
        );
        slider(
            ui,
            &filter.keytrack,
            "Amount the filter cutoff is affected by note frequency; Keytrack of 100% means the \
             filter cutoff will follow the note frequency exactly, making higher notes brighter \
             and lower notes darker.",
            setter,
        );
    });
}

fn filter_envelope(ui: &mut Ui, title: &str, env: &FilterEnvParams, setter: &ParamSetter) {
    section(ui, title, |ui| {
        slider(
            ui,
            &env.amount,
            "Amount the envelope affects the filter cutoff.",
            setter,
        );
        envelope_rows(ui, &env.envelope, setter);
    });
}

fn envelope(ui: &mut Ui, title: &str, env: &EnvelopeParams, setter: &ParamSetter) {
    section(ui, title, |ui| envelope_rows(ui, env, setter));
}

fn envelope_rows(ui: &mut Ui, env: &EnvelopeParams, setter: &ParamSetter) {
    slider(
        ui,
        &env.attack,
        "Envelope start time. This gives a \"fade in\" effect when controlling volume.",
        setter,
    );
    slider(
        ui,
        &env.decay,
        "Time for envelope to reach sustain level. This gives a \"pluck\" effect when controlling \
         volume. Does nothing when sustain = 100%.",
        setter,
    );
    slider(
        ui,
        &env.sustain,
        "The level the envelope will remain at while the note is held. Used for sustained sounds, \
         like flutes or strings. When set to 0 the note ends when decay finishes.",
        setter,
    );
    slider(
        ui,
        &env.release,
        "Time for the envelope to finish after the note is released. Useful for sounds which \
         persist a while after they're played, like bells or chimes. Has mostly no effect when \
         sustain = 0.",
        setter,
    );
    slider(
        ui,
        &env.slope,
        "Controls the steepness of the attack, decay and release slopes either exponentially or \
         logarithmically. Positive slope means attack and decay will change logarithmically, \
         resulting in punchier sounds, while negative slope will do the opposite. A slope of 0 \
         results in exactly linear slopes.",
        setter,
    );
}

fn lfo(ui: &mut Ui, title: &str, lfo: &LfoParams, setter: &ParamSetter) {
    section(ui, title, |ui| {
        combo(
            ui,
            &lfo.target,
            "Which oscillator(s) the LFO's vibrato, tremolo and modulation outputs affect.",
            setter,
        );
        combo(ui, &lfo.wave, "The LFO's waveform.", setter);
        slider(
            ui,
            &lfo.freq,
            "The frequency the LFO oscillates at when active.",
            setter,
        );
        slider(
            ui,
            &lfo.freq_mod,
            "LFO modulation of the target's pitch.",
            setter,
        );
        slider(
            ui,
            &lfo.amp_mod,
            "LFO modulation of the target's amplitude.",
            setter,
        );
        slider(
            ui,
            &lfo.mod_mod,
            "LFO modulation of PM, AM and FM modulation the target is receiving.",
            setter,
        );
        slider(
            ui,
            &lfo.filter_mod,
            "LFO modulation of the filter's cutoff.",
            setter,
        );
    });
}

fn global(ui: &mut Ui, title: &str, global: &GlobalParams, setter: &ParamSetter) {
    section(ui, title, |ui| {
        combo(
            ui,
            &global.polyphony,
            "Polyphonic means an unlimited number of notes can be played simultaneously. \
             Monophonic means only one note can be played at a time. Legato is the same as \
             monophonic, except notes are connected; envelopes / oscillator phases won't reset \
             when gliding between notes.",
            setter,
        );
        slider(
            ui,
            &global.portamento_rate,
            "How slowly note pitch will glide to a new note. In Polyphonic mode, this only \
             affects Pitch Slide.",
            setter,
        );
        slider(
            ui,
            &global.pitch_offset,
            "Initial pitch shift at the start of a note.",
            setter,
        );
        slider(
            ui,
            &global.octave_detune,
            "Stretches the octave width so that consecutive octaves are further or closer \
             together. In 12TET (i.e. the chromatic scale), octaves are the only perfect \
             interval, which means playing them with harsh waves (saw, square, etc) will result \
             in a louder and harsher sound than playing any other two notes. This control is \
             meant to fix that.",
            setter,
        );
        slider(
            ui,
            &global.output_gain,
            "Master output volume. Adjust if the synth is too loud / too quiet.",
            setter,
        );
        slider(
            ui,
            &global.stereo_width,
            "Spreads each oscillator's unison voices across the stereo field, with the lowest \
             pitched voice furthest left and the highest furthest right. At 0 the output is mono: \
             both channels are identical. An oscillator with Unison = 1 always stays in the \
             centre.",
            setter,
        );
        slider(
            ui,
            &global.global_pitch,
            "Pitch detune for the whole synth, in semitones.",
            setter,
        );
        slider(
            ui,
            &global.bend_range,
            "Controls the range of the MIDI pitch wheel in semitones. Useful if you have a MIDI \
             keyboard.",
            setter,
        );
    });
}

/// A titled, framed group of controls laid out as a two column (label, control) grid.
fn section(ui: &mut Ui, title: &str, add_rows: impl FnOnce(&mut Ui)) {
    // Widget IDs inside the section are scoped by its title, so the same parameter name (e.g.
    // "Wave") can appear in several sections.
    ui.push_id(title, |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong());
                ui.separator();
                egui::Grid::new("controls")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| add_rows(ui));
            });
        });
    });
}

/// A grid row with the parameter's name and a slider for it.
fn slider<P: Param>(ui: &mut Ui, param: &P, help: &str, setter: &ParamSetter) {
    ui.label(param.name()).on_hover_text(help);
    ui.add(ParamSlider::for_param(param, setter).with_width(SLIDER_WIDTH));
    ui.end_row();
}

/// A grid row with the parameter's name and a combo box listing the enum's variants.
fn combo<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    param: &EnumParam<T>,
    help: &str,
    setter: &ParamSetter,
) {
    ui.label(param.name()).on_hover_text(help);

    let variants = T::variants();
    let current = param.value().to_index();
    let mut selected = current;
    egui::ComboBox::from_id_salt(param.name())
        .selected_text(variants[current])
        .width(COMBO_WIDTH)
        .show_ui(ui, |ui| {
            for (index, name) in variants.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *name);
            }
        });
    if selected != current {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, T::from_index(selected));
        setter.end_set_parameter(param);
    }

    ui.end_row();
}
