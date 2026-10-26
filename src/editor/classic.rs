//! The Classic theme: the original "everything on one page" layout. The three oscillators, the
//! LFOs and the output's spectrogram and meters (see [`super::analyser`]) are across the top,
//! and the filter, envelopes, global controls and the modulation matrix underneath. The effects
//! and the advanced settings are on tabs that take the oscillators' place, so that the
//! modulation sources stay in view. Every control is a [`ParamSlider`] (or a combo box for the
//! enumerated parameters), and hovering a control's label shows what it does.
//!
//! Every modulation source has a handle that can be dragged onto any slider that can be
//! modulated. The matrix lists every route, with its amount, and modulated sliders are marked.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, RichText, Stroke, StrokeKind, Ui};
use nih_plug_egui::widgets::ParamSlider;

use super::{
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, lfo_display, set_param, title_bar,
    wavetable_file_combo, wavetable_file_status, wavetable_folder_rows, Gui, Live, Matrix, NumericParam, WaveDisplay,
    MOD_COLOR,
};
use crate::params::{
    ChorusParams, DelayParams, DistortionParams, EnvelopeParams, FilterEnvParams, FilterParams, FxParams,
    GlobalParams, LfoParams, ModParams, ModSlotParams, OscParams, OscillatorWave, ReverbParams, ThreeOscParams,
    WavetableFile,
};
use crate::synth::effects::{ChorusSettings, DelaySettings, DistortionSettings, ReverbSettings};
use crate::synth::modulation::{ModSource, ModTarget, Scaling};
use crate::synth::oversampling::Oversampling;

const SLIDER_WIDTH: f32 = 110.0;
const COMBO_WIDTH: f32 = 150.0;
/// Width of the combo boxes in the modulation matrix, which has two per row.
const MATRIX_COMBO_WIDTH: f32 = 100.0;
/// Width of the amount sliders in the modulation matrix.
const MATRIX_SLIDER_WIDTH: f32 = 90.0;
/// The width of a section of controls. Sections are laid out side by side, so they can't be
/// left to take all the width they are offered (their separators would).
const SECTION_WIDTH: f32 = 290.0;
/// The space a framed group takes besides its contents, on each side.
const FRAME_MARGIN: f32 = 7.0;
/// The height of a section's title and the separator under it.
const SECTION_HEADER: f32 = 30.0;
const MATRIX_WIDTH: f32 = 450.0;

/// What the Classic layout shows.
pub struct ClassicState {
    /// Whether the oscillators, the effects or the advanced settings are shown.
    tab: Tab,
    /// The size the oscillators took when they were last shown, which the effects take too, so
    /// that nothing else moves when the tabs are switched.
    oscillators_size: egui::Vec2,
}

impl ClassicState {
    pub fn new() -> Self {
        Self {
            tab: Tab::Oscillators,
            oscillators_size: egui::Vec2::ZERO,
        }
    }
}

/// What the top left of the window shows: the oscillators, or the effects or the advanced
/// settings in their place. Either way the modulation sources stay in view, to be dragged onto
/// the controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Oscillators,
    Effects,
    Advanced,
}

pub fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let matrix = Matrix {
        slots: &params.mod_slots,
        monitor: gui.live.monitor,
    };
    let live = gui.live;

    title_bar::title_bar(ui, params, setter, gui);
    ui.separator();
    tabs(ui, &params.fx, gui);

    ui.horizontal_top(|ui| {
        match gui.state.classic.tab {
            Tab::Oscillators => {
                let size = ui
                    .horizontal_top(|ui| {
                        for (index, title) in ["Osc 1", "Osc 2", "Osc 3"].into_iter().enumerate() {
                            let modulation = [Some(&params.osc1_mod), Some(&params.osc2_mod), None][index];
                            oscillator(
                                ui,
                                title,
                                index,
                                params.oscillator(index),
                                modulation,
                                params.wavetable_file(index),
                                &matrix,
                                setter,
                                gui,
                            );
                        }
                    })
                    .response
                    .rect
                    .size();
                gui.state.classic.oscillators_size = size;
            }
            Tab::Effects => {
                let size = gui.state.classic.oscillators_size;
                ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.set_min_size(size);
                    effects(ui, size, &params.fx, &matrix, setter, live);
                });
            }
            Tab::Advanced => {
                let size = gui.state.classic.oscillators_size;
                ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.set_min_size(size);
                    advanced(ui, &params.global, setter);
                    wavetable_folder(ui, params, setter, gui);
                });
            }
        }
        ui.vertical(|ui| {
            ui.horizontal_top(|ui| {
                let lfos = ui
                    .vertical(|ui| {
                        for (i, lfo_params) in params.lfos.iter().enumerate() {
                            lfo(ui, i, lfo_params, &matrix, setter, live);
                        }
                    })
                    .response
                    .rect;
                output_section(ui, lfos.height(), gui);
            });
            midi_sources(ui);
        });
    });

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            let oversampling = params.global.oversampling.value().to_oversampling();
            filter(ui, "Filter 1", &params.filter, &params.filter_env, oversampling, &matrix, setter, gui);
            filter_envelope(ui, "Filter 1 Envelope", &params.filter_env, setter, live);
        });
        ui.vertical(|ui| {
            envelope(ui, "Volume Envelope", ModSource::VolumeEnv, &params.vol_env, setter, live);
            global(ui, "Global", &params.global, &matrix, setter);
        });
        ui.vertical(|ui| {
            for (i, env) in params.mod_envs.iter().enumerate() {
                envelope(
                    ui,
                    &format!("Envelope {}", i + 1),
                    ModSource::envelope(i),
                    &env.envelope,
                    setter,
                    live,
                );
            }
        });
        modulation_matrix(ui, &matrix, setter);
    });
}

/// The tabs that switch between the oscillators and the effects.
fn tabs(ui: &mut Ui, fx: &FxParams, gui: &mut Gui) {
    ui.horizontal(|ui| {
        let tab = &mut gui.state.classic.tab;
        ui.selectable_value(tab, Tab::Oscillators, RichText::new("Oscillators").strong())
            .on_hover_text("The three oscillators.");
        let on = fx.enabled_count();
        let label = if on > 0 { format!("FX ({on} on)") } else { String::from("FX") };
        ui.selectable_value(tab, Tab::Effects, RichText::new(label).strong()).on_hover_text(
            "The effects after the voices: distortion, chorus, delay and reverb, in that order. \
             They take the oscillators' place, so the modulation sources stay in view to be \
             dragged onto them.",
        );
        ui.selectable_value(tab, Tab::Advanced, RichText::new("Advanced").strong())
            .on_hover_text(help::ADVANCED);
    });
}

/// The settings that trade quality against CPU.
fn advanced(ui: &mut Ui, global: &GlobalParams, setter: &ParamSetter) {
    section(ui, "Quality", None, Display::None, |ui| {
        combo(ui, &global.oversampling, help::OVERSAMPLING, setter);
        combo(ui, &global.interpolation, help::INTERPOLATION, setter);
        combo(ui, &global.modulation_rate, help::MODULATION_RATE, setter);
    });
}

/// The size of an effect's section and its display, for four of them in two rows of two in an
/// area of the given size.
#[derive(Clone, Copy)]
struct EffectLayout {
    width: f32,
    display_height: f32,
}

/// The effects in the order they process the sound, two by two, filling the area the
/// oscillators take.
fn effects(ui: &mut Ui, size: egui::Vec2, fx: &FxParams, matrix: &Matrix, setter: &ParamSetter, live: Live) {
    // The displays show the settings as the newest note's modulation moves them
    let settings = match live.modulation() {
        Some(modulation) => fx.settings().modulated(&modulation),
        None => fx.settings(),
    };
    let spacing = ui.spacing().item_spacing;
    let section_height = (size.y - spacing.y) / 2.0;
    let layout = EffectLayout {
        width: ((size.x - spacing.x) / 2.0 - 2.0 * FRAME_MARGIN).max(SECTION_WIDTH),
        display_height: (section_height - SECTION_HEADER - 2.0 * FRAME_MARGIN).clamp(60.0, 200.0),
    };
    ui.vertical(|ui| {
        ui.horizontal_top(|ui| {
            distortion(ui, layout, &fx.distortion, &settings.distortion, matrix, setter);
            chorus(ui, layout, &fx.chorus, &settings.chorus, matrix, setter);
        });
        ui.horizontal_top(|ui| {
            delay(ui, layout, &fx.delay, &settings.delay, matrix, setter);
            reverb(ui, layout, &fx.reverb, &settings.reverb, matrix, setter);
        });
    });
}

fn distortion(
    ui: &mut Ui,
    layout: EffectLayout,
    params: &DistortionParams,
    settings: &DistortionSettings,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    let mut display = |ui: &mut Ui| {
        displays::distortion(ui, layout.display_height, settings.drive_db, settings.mix, settings.enabled)
    };
    section_sized(ui, "Distortion", layout.width, None, Display::Beside(&mut display), |ui| {
        toggle(ui, "On", &params.enabled, help::DISTORTION, setter);
        mod_slider(ui, &params.drive, help::DISTORTION_DRIVE, ModTarget::DistortionDrive, matrix, setter);
        mod_slider(ui, &params.mix, help::DISTORTION_MIX, ModTarget::DistortionMix, matrix, setter);
    });
}

fn chorus(
    ui: &mut Ui,
    layout: EffectLayout,
    params: &ChorusParams,
    settings: &ChorusSettings,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    let mut display = |ui: &mut Ui| displays::chorus(ui, layout.display_height, settings.depth, settings.enabled);
    section_sized(ui, "Chorus", layout.width, None, Display::Beside(&mut display), |ui| {
        toggle(ui, "On", &params.enabled, help::CHORUS, setter);
        mod_slider(ui, &params.rate, help::CHORUS_RATE, ModTarget::ChorusRate, matrix, setter);
        mod_slider(ui, &params.depth, help::CHORUS_DEPTH, ModTarget::ChorusDepth, matrix, setter);
        mod_slider(ui, &params.mix, help::CHORUS_MIX, ModTarget::ChorusMix, matrix, setter);
    });
}

fn delay(
    ui: &mut Ui,
    layout: EffectLayout,
    params: &DelayParams,
    settings: &DelaySettings,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    let mut display = |ui: &mut Ui| {
        let DelaySettings { enabled, time, feedback, mix, ping_pong } = *settings;
        displays::delay(ui, layout.display_height, time, feedback, mix, ping_pong, enabled)
    };
    section_sized(ui, "Delay", layout.width, None, Display::Beside(&mut display), |ui| {
        toggle(ui, "On", &params.enabled, help::DELAY, setter);
        mod_slider(ui, &params.time, help::DELAY_TIME, ModTarget::DelayTime, matrix, setter);
        mod_slider(ui, &params.feedback, help::DELAY_FEEDBACK, ModTarget::DelayFeedback, matrix, setter);
        mod_slider(ui, &params.mix, help::DELAY_MIX, ModTarget::DelayMix, matrix, setter);
        toggle(ui, "Ping-Pong", &params.ping_pong, help::DELAY_PING_PONG, setter);
    });
}

fn reverb(
    ui: &mut Ui,
    layout: EffectLayout,
    params: &ReverbParams,
    settings: &ReverbSettings,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    let mut display = |ui: &mut Ui| {
        displays::reverb(ui, layout.display_height, settings.size, settings.damping, settings.enabled)
    };
    section_sized(ui, "Reverb", layout.width, None, Display::Beside(&mut display), |ui| {
        toggle(ui, "On", &params.enabled, help::REVERB, setter);
        mod_slider(ui, &params.size, help::REVERB_SIZE, ModTarget::ReverbSize, matrix, setter);
        mod_slider(ui, &params.damping, help::REVERB_DAMPING, ModTarget::ReverbDamping, matrix, setter);
        mod_slider(ui, &params.mix, help::REVERB_MIX, ModTarget::ReverbMix, matrix, setter);
    });
}

#[allow(clippy::too_many_arguments)]
fn oscillator(
    ui: &mut Ui,
    title: &str,
    index: usize,
    osc: &OscParams,
    modulation: Option<&ModParams>,
    file: &WavetableFile,
    matrix: &Matrix,
    setter: &ParamSetter,
    gui: &mut Gui,
) {
    let wave = WaveDisplay::new(index, osc, file, gui);
    let mut display = |ui: &mut Ui| wave.show(ui, displays::WAVE_HEIGHT);
    section(ui, title, Some(ModSource::osc(index)), Display::Above(&mut display), |ui| {
        combo(ui, &osc.wave, help::OSC_WAVE, setter);
        if osc.wave.value() == OscillatorWave::File {
            wavetable_file_rows(ui, index, file, gui);
        }
        mod_slider(ui, &osc.position, help::OSC_POSITION, ModTarget::osc_position(index), matrix, setter);
        mod_slider(ui, &osc.amp, help::OSC_AMP, ModTarget::osc_amp(index), matrix, setter);
        mod_slider(ui, &osc.pan, help::OSC_PAN, ModTarget::osc_pan(index), matrix, setter);
        mod_slider(ui, &osc.semitone, help::OSC_SEMITONE, ModTarget::osc_pitch(index), matrix, setter);
        mod_slider(ui, &osc.octave, help::OSC_OCTAVE, ModTarget::osc_octave(index), matrix, setter);
        mod_slider(ui, &osc.multiplier, help::OSC_MULTIPLIER, ModTarget::osc_multiplier(index), matrix, setter);
        if let Some(modulation) = modulation {
            let target = |target: Option<ModTarget>| target.unwrap_or(ModTarget::None);
            mod_slider(ui, &modulation.pm, help::OSC_PM, target(ModTarget::osc_pm(index)), matrix, setter);
            mod_slider(ui, &modulation.fm, help::OSC_FM, target(ModTarget::osc_fm(index)), matrix, setter);
            mod_slider(ui, &modulation.am, help::OSC_AM, target(ModTarget::osc_am(index)), matrix, setter);
        }
        slider(ui, &osc.voices, help::OSC_VOICES, setter);
        mod_slider(ui, &osc.super_detune, help::OSC_DETUNE, ModTarget::osc_detune(index), matrix, setter);
        slider(ui, &osc.phase, help::OSC_PHASE, setter);
        slider(ui, &osc.phase_rand, help::OSC_PHASE_RAND, setter);
    });
}

fn lfo(ui: &mut Ui, index: usize, lfo: &LfoParams, matrix: &Matrix, setter: &ParamSetter, live: Live) {
    section(
        ui,
        &format!("LFO {}", index + 1),
        Some(ModSource::lfo(index)),
        Display::Beside(&mut |ui| lfo_display(ui, lfo, index, live, displays::LFO_HEIGHT)),
        |ui| {
            combo(ui, &lfo.wave, help::LFO_WAVE, setter);
            toggle(ui, "Sync", &lfo.sync, help::LFO_SYNC, setter);
            let target = ModTarget::lfo_rate(index);
            if lfo.sync.value() {
                // (Its rate's modulation moves through about three note lengths an octave)
                let scaling = Scaling::Linear(3.0 * ModTarget::LFO_RATE_OCTAVES);
                mod_slider_scaled(ui, &lfo.division, help::LFO_DIVISION, target, scaling, matrix, setter);
            } else {
                mod_slider(ui, &lfo.rate, help::LFO_RATE, target, matrix, setter);
            }
            slider(ui, &lfo.phase, help::LFO_PHASE, setter);
            combo(ui, &lfo.trigger, help::LFO_TRIGGER, setter);
            slider(ui, &lfo.fade, help::LFO_FADE, setter);
        },
    );
}

/// The rows picking an oscillator's wavetable file and showing how its load went.
fn wavetable_file_rows(ui: &mut Ui, index: usize, file: &WavetableFile, gui: &mut Gui) {
    ui.label("File").on_hover_text(help::WAVETABLE_FILE);
    wavetable_file_combo(ui, index, file, gui, COMBO_WIDTH);
    ui.end_row();
    if let Some(status) = wavetable_file_status(file) {
        ui.label("");
        ui.add(egui::Label::new(RichText::new(status).small().weak()).wrap());
        ui.end_row();
    }
}

/// The wavetable folder: where the oscillators' File combo boxes look, with a way to load a
/// file from anywhere else.
fn wavetable_folder(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    ui.push_id("wavetable folder", |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(SECTION_WIDTH);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Wavetable Files").strong()).on_hover_text(help::WAVETABLE_FOLDER);
                    if ui.small_button("Rescan").clicked() {
                        gui.state.rescan();
                    }
                });
                ui.separator();
                wavetable_folder_rows(ui, params, setter, gui, SLIDER_WIDTH + 40.0);
            });
        });
    });
}

/// The narrowest the output section gets, when the window is too narrow for the layout.
const OUTPUT_MIN_WIDTH: f32 = 130.0;

/// What the synth plays, after the effects: a spectrogram and the level of each channel. It
/// fills the rest of the width beside the LFOs, `height` tall.
fn output_section(ui: &mut Ui, height: f32, gui: &mut Gui) {
    let width = (ui.available_width() - 2.0 * FRAME_MARGIN).max(OUTPUT_MIN_WIDTH);
    ui.push_id("output", |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.label(RichText::new("Output").strong()).on_hover_text(analyser::HELP);
                ui.separator();
                let display_height = height - SECTION_HEADER - 2.0 * FRAME_MARGIN;
                let live = gui.live;
                analyser::output(
                    ui,
                    &mut gui.state.analyser,
                    &live.monitor.output,
                    live.sample_rate,
                    width,
                    display_height,
                );
            });
        });
    });
}

/// The modulation sources that come from MIDI (and the random one), as a row of drag handles.
fn midi_sources(ui: &mut Ui) {
    ui.push_id("midi sources", |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(SECTION_WIDTH);
                ui.label(RichText::new("More Sources").strong())
                    .on_hover_text("Drag a source onto a slider to modulate it.");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    for source in ModSource::MIDI {
                        drag_handle(ui, source);
                    }
                });
            });
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn filter(
    ui: &mut Ui,
    title: &str,
    filter: &FilterParams,
    filter_env: &FilterEnvParams,
    oversampling: Oversampling,
    matrix: &Matrix,
    setter: &ParamSetter,
    gui: &mut Gui,
) {
    let (settings, live) = filter_display_inputs(filter, filter_env, oversampling, gui.live);
    let response = &mut gui.state.filter_response;
    let mut display = |ui: &mut Ui| {
        displays::filter(ui, settings, response, live, displays::FILTER_HEIGHT, None);
    };
    section(ui, title, None, Display::Above(&mut display), |ui| {
        combo(ui, &filter.model, help::FILTER_MODEL, setter);
        combo(ui, &filter.filter_type, help::FILTER_TYPE, setter);
        mod_slider(ui, &filter.cutoff, help::FILTER_CUTOFF, ModTarget::Cutoff, matrix, setter);
        mod_slider(ui, &filter.resonance, help::FILTER_RESONANCE, ModTarget::Resonance, matrix, setter);
        mod_slider(ui, &filter.drive, help::FILTER_DRIVE, ModTarget::Drive, matrix, setter);
        slider(ui, &filter.keytrack, help::FILTER_KEYTRACK, setter);
    });
}

fn filter_envelope(ui: &mut Ui, title: &str, env: &FilterEnvParams, setter: &ParamSetter, live: Live) {
    let mut display = |ui: &mut Ui| envelope_display(ui, &env.envelope, setter, live, displays::ENVELOPE_HEIGHT);
    section(ui, title, Some(ModSource::FilterEnv), Display::Above(&mut display), |ui| {
        slider(ui, &env.amount, help::FILTER_ENV_AMOUNT, setter);
        envelope_rows(ui, &env.envelope, setter);
    });
}

fn envelope(
    ui: &mut Ui,
    title: &str,
    source: ModSource,
    env: &EnvelopeParams,
    setter: &ParamSetter,
    live: Live,
) {
    let mut display = |ui: &mut Ui| envelope_display(ui, env, setter, live, displays::ENVELOPE_HEIGHT);
    section(ui, title, Some(source), Display::Above(&mut display), |ui| {
        envelope_rows(ui, env, setter)
    });
}

fn envelope_rows(ui: &mut Ui, env: &EnvelopeParams, setter: &ParamSetter) {
    slider(ui, &env.attack, help::ENV_ATTACK, setter);
    slider(ui, &env.decay, help::ENV_DECAY, setter);
    slider(ui, &env.sustain, help::ENV_SUSTAIN, setter);
    slider(ui, &env.release, help::ENV_RELEASE, setter);
}

fn global(ui: &mut Ui, title: &str, global: &GlobalParams, matrix: &Matrix, setter: &ParamSetter) {
    section(ui, title, None, Display::None, |ui| {
        combo(ui, &global.polyphony, help::POLYPHONY, setter);
        slider(ui, &global.portamento_rate, help::PORTAMENTO, setter);
        slider(ui, &global.pitch_offset, help::PITCH_SLIDE, setter);
        slider(ui, &global.octave_detune, help::OCTAVE_DRIFT, setter);
        mod_slider(ui, &global.output_gain, help::OUTPUT_GAIN, ModTarget::Volume, matrix, setter);
        slider(ui, &global.stereo_width, help::STEREO_WIDTH, setter);
        mod_slider(ui, &global.global_pitch, help::GLOBAL_PITCH, ModTarget::Pitch, matrix, setter);
        slider(ui, &global.bend_range, help::BEND_RANGE, setter);
    });
}

/// The modulation matrix: every slot as a row of source, target and amount.
fn modulation_matrix(ui: &mut Ui, matrix: &Matrix, setter: &ParamSetter) {
    ui.push_id("modulation matrix", |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(MATRIX_WIDTH);
                ui.label(RichText::new("Modulation Matrix").strong()).on_hover_text(
                    "Each row routes a source to a target. At 100% a source's full swing moves \
                     a slider across its whole length, pitch by an octave, and the filter \
                     cutoff by 10 octaves. Drag a source's handle (the LFOs', the envelopes', \
                     the oscillators' and those in More Sources) onto a slider to fill in a \
                     row.",
                );
                ui.separator();
                egui::Grid::new("routes")
                    .num_columns(6)
                    .min_col_width(0.0)
                    .spacing([6.0, 4.0])
                    .show(ui, |ui| {
                        for (index, slot) in matrix.slots.iter().enumerate() {
                            matrix_row(ui, index, slot, setter);
                        }
                    });
            });
        });
    });
}

fn matrix_row(ui: &mut Ui, index: usize, slot: &ModSlotParams, setter: &ParamSetter) {
    let source_help = slot.source.value().help();
    combo_cell(ui, &slot.source, index, MATRIX_COMBO_WIDTH, source_help, setter);
    let target_help = slot.target.value().help();
    combo_cell(ui, &slot.target, index, MATRIX_COMBO_WIDTH, target_help, setter);
    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(MATRIX_SLIDER_WIDTH))
        .on_hover_text(help::MOD_AMOUNT);
    super::widgets::polarity_button(ui, slot, setter);
    super::widgets::remap_button(ui, index, slot);
    let in_use = slot.source.value() != ModSource::None || slot.target.value() != ModTarget::None;
    if ui
        .add_enabled(in_use, egui::Button::new(RichText::new("×").strong()).small())
        .on_hover_text(help::CLEAR_ROUTE)
        .clicked()
    {
        Matrix::clear(setter, slot);
    }
    ui.end_row();
}

/// A handle that can be dragged onto a modulatable slider to route `source` to it.
fn drag_handle(ui: &mut Ui, source: ModSource) {
    let id = ui.id().with(("mod source", source.to_index()));
    let name = ModSource::variants()[source.to_index()];
    let response = ui
        .dnd_drag_source(id, source, |ui| {
            egui::Frame::new()
                .fill(MOD_COLOR.gamma_multiply(0.25))
                .stroke(Stroke::new(1.0_f32, MOD_COLOR))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(format!("☰ {name}")).small().strong())
                            .wrap_mode(egui::TextWrapMode::Extend)
                            .selectable(false),
                    );
                });
        })
        .response;
    response.on_hover_text(format!(
        "{}\nDrag this onto a slider to modulate it.",
        source.help()
    ));
}

/// Where a section shows the display of what its controls do.
enum Display<'a> {
    None,
    /// Between the title and the controls.
    Above(&'a mut dyn FnMut(&mut Ui)),
    /// To the right of the controls, for sections with only a few.
    Beside(&'a mut dyn FnMut(&mut Ui)),
}

/// A titled, framed group of controls laid out as a two column (label, control) grid, with a
/// display of what they do. Sections that are modulation sources get a drag handle next to
/// their title.
fn section(
    ui: &mut Ui,
    title: &str,
    source: Option<ModSource>,
    display: Display,
    add_rows: impl FnOnce(&mut Ui),
) {
    section_sized(ui, title, SECTION_WIDTH, source, display, add_rows)
}

/// A [`section()`] of the given width.
fn section_sized(
    ui: &mut Ui,
    title: &str,
    width: f32,
    source: Option<ModSource>,
    display: Display,
    add_rows: impl FnOnce(&mut Ui),
) {
    // Widget IDs inside the section are scoped by its title, so the same parameter name (e.g.
    // "Wave") can appear in several sections.
    ui.push_id(title, |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).strong());
                    if let Some(source) = source {
                        drag_handle(ui, source);
                    }
                });
                ui.separator();
                let grid = |ui: &mut Ui| {
                    egui::Grid::new("controls")
                        .num_columns(2)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| add_rows(ui));
                };
                match display {
                    Display::None => grid(ui),
                    Display::Above(display) => {
                        display(ui);
                        grid(ui);
                    }
                    Display::Beside(display) => {
                        ui.horizontal_top(|ui| {
                            grid(ui);
                            display(ui);
                        });
                    }
                }
            });
        });
    });
}

/// A grid row with a label and a check box for a switch.
fn toggle(ui: &mut Ui, label: &str, param: &BoolParam, help: &str, setter: &ParamSetter) {
    ui.label(label).on_hover_text(help);
    let mut on = param.value();
    if ui.checkbox(&mut on, "").on_hover_text(help).changed() {
        set_param(setter, param, on);
    }
    ui.end_row();
}

/// A grid row with the parameter's name and a slider for it.
fn slider<P: Param>(ui: &mut Ui, param: &P, help: &str, setter: &ParamSetter) {
    ui.label(param.name()).on_hover_text(help);
    ui.add(ParamSlider::for_param(param, setter).with_width(SLIDER_WIDTH));
    ui.end_row();
}

/// A grid row with a slider for a parameter that modulation sources can be dropped onto, with
/// a marker listing the routes that modulate it. While a note plays, a modulated slider shows
/// where the newest note's modulation takes it, from the slider's value to a mark.
fn mod_slider<P: NumericParam>(
    ui: &mut Ui,
    param: &P,
    help: &str,
    target: ModTarget,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    mod_slider_scaled(ui, param, help, target, target.scaling(), matrix, setter);
}

/// A [`mod_slider()`] whose target's modulation moves it as `scaling` says, rather than as it
/// moves the target's own control.
fn mod_slider_scaled<P: NumericParam>(
    ui: &mut Ui,
    param: &P,
    help: &str,
    target: ModTarget,
    scaling: Scaling,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    ui.label(param.name()).on_hover_text(help);
    ui.horizontal(|ui| {
        let response = ui.add(ParamSlider::for_param(param, setter).with_width(SLIDER_WIDTH));

        // Highlight the slider while a source is being dragged, and route it on a drop
        if egui::DragAndDrop::has_payload_of_type::<ModSource>(ui.ctx()) {
            let hovered = response.dnd_hover_payload::<ModSource>().is_some();
            let color = if hovered {
                MOD_COLOR
            } else {
                MOD_COLOR.gamma_multiply(0.4)
            };
            ui.painter().rect_stroke(
                response.rect.expand(2.0),
                3.0,
                Stroke::new(1.5_f32, color),
                StrokeKind::Outside,
            );
        }
        if let Some(source) = response.dnd_release_payload::<ModSource>() {
            matrix.add_route(setter, *source, target);
        }

        // A marker if routes modulate this control
        if matrix.routes_for(target).next().is_some() {
            // Painted rather than a glyph, which the default proportional fonts may lack
            let (rect, marker) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.5, MOD_COLOR);
            // The routes over the slider while it (or the marker) is hovered
            let hovered = response.hovered() || marker.hovered();
            let id = response.id.with("routes");
            super::widgets::routes_overlay(ui, id, response.rect, hovered, target, matrix, setter);

            if let Some(modulation) = matrix.monitor.modulation(target) {
                let value = param.value_f32();
                let from = param.normalized_at(value);
                let to = param.normalized_at(scaling.apply(value, modulation));
                let bar = response.rect;
                let x = |normalized: f32| bar.left() + bar.width() * normalized.clamp(0.0, 1.0);
                let painter = ui.painter();
                let y = bar.bottom() - 1.5;
                painter.line_segment([egui::pos2(x(from), y), egui::pos2(x(to), y)], Stroke::new(3.0_f32, MOD_COLOR));
                painter.line_segment(
                    [egui::pos2(x(to), bar.top()), egui::pos2(x(to), bar.bottom())],
                    Stroke::new(2.0_f32, MOD_COLOR),
                );
            }
        }
    });
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
    combo_cell(ui, param, 0, COMBO_WIDTH, help, setter);
    ui.end_row();
}
