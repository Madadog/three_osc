//! The plugin's GUI, built with egui through `nih_plug_egui`.
//!
//! It's a deliberately simple "everything on one page" layout: the three oscillators, the
//! LFOs and the output's spectrogram and meters (see [`analyser`]) across the top, and the
//! filter, envelopes, global controls and the modulation matrix underneath. The effects are on
//! a tab that takes the oscillators' place, so that the modulation sources stay in view, and the
//! presets and the tuning are in the title bar (see [`title_bar`] and [`tuning_window`]). Every control is a
//! [`ParamSlider`] (or a combo box for the enumerated parameters) bound directly to a
//! parameter, so host automation and the GUI stay in sync. Hovering a control's label shows
//! what it does.
//!
//! Modulation is set up by dragging: every modulation source (the LFOs, the envelopes, and the
//! MIDI sources) has a handle that can be dragged onto any slider that can be modulated, which
//! fills in a free slot of the modulation matrix. The matrix itself lists every route, with its
//! amount, and modulated sliders are marked.
//!
//! The oscillators, LFOs, envelopes, the filter and the effects each have a display of what they
//! do, see [`displays`]. The displays follow the notes that are playing (see [`Monitor`]): the
//! envelopes and LFOs show where each note is in them, the waves and the filter move with the
//! modulation, and a modulated slider shows where the newest note's modulation takes it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, RichText, Stroke, StrokeKind, Ui};
use nih_plug_egui::resizable_window::ResizableWindow;
use nih_plug_egui::widgets::ParamSlider;
use nih_plug_egui::{create_egui_editor, EguiState};

use crate::params::{
    ChorusParams, DelayParams, DistortionParams, EnvelopeParams, FilterEnvParams, FilterParams,
    FxParams, GlobalParams, LfoParams, ModParams, ModSlotParams, OscParams, OscillatorWave,
    ReverbParams, ThreeOscParams, WavetableFile,
};
use crate::synth::effects::{ChorusSettings, DelaySettings, DistortionSettings, ReverbSettings};
use crate::synth::envelopes::AdsrEnvelope;
use crate::synth::modulation::{ModSource, ModTarget, Targets, N_SLOTS, N_TARGETS};
use crate::synth::monitor::Monitor;
use crate::synth::oscillator::{builtin_spectra, CycleRenderer, WavePreview};
use crate::synth::wavetable_file;
use crate::{Task, ThreeOscPlugin};

mod analyser;
mod displays;
mod title_bar;
mod tuning_window;

/// The window's default size, in logical pixels.
pub const DEFAULT_WIDTH: u32 = 1400;
pub const DEFAULT_HEIGHT: u32 = 1000;

const SLIDER_WIDTH: f32 = 110.0;
const COMBO_WIDTH: f32 = 150.0;
/// Width of the combo boxes in the modulation matrix, which has two per row.
const MATRIX_COMBO_WIDTH: f32 = 115.0;
/// The width of a section of controls. Sections are laid out side by side, so they can't be
/// left to take all the width they are offered (their separators would).
const SECTION_WIDTH: f32 = 290.0;
/// The space a framed group takes besides its contents, on each side.
const FRAME_MARGIN: f32 = 7.0;
/// The height of a section's title and the separator under it.
const SECTION_HEADER: f32 = 30.0;
const MATRIX_WIDTH: f32 = 450.0;

/// The colour of everything to do with modulation: source handles, route markers and the drop
/// highlights.
const MOD_COLOR: Color32 = Color32::from_rgb(255, 170, 60);

/// The window size used when the plugin is first loaded. The size is stored with the plugin's
/// state, so resizing the window is remembered.
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(DEFAULT_WIDTH, DEFAULT_HEIGHT)
}

/// State that only the GUI needs: the listing of the wavetable folder, the preset and tuning
/// browsers, and what the displays draw.
pub struct EditorState {
    folder: PathBuf,
    /// The `.wav` files in the folder.
    files: Vec<String>,
    scanned: bool,
    /// A path typed in to load a file from anywhere else.
    custom_path: String,
    /// The frames of the built in wavetables, in the order of the wave parameter, worked out
    /// when the GUI first opens.
    builtin_waves: Vec<Arc<WavePreview>>,
    filter_response: displays::FilterResponse,
    /// Whether the oscillators or the effects are shown.
    tab: Tab,
    /// The size the oscillators took when they were last shown, which the effects take too, so
    /// that nothing else moves when the tabs are switched.
    oscillators_size: egui::Vec2,
    presets: title_bar::PresetBrowser,
    tuning: tuning_window::TuningWindow,
    /// The output section's spectrogram and meters.
    analyser: analyser::Analyser,
}

/// What the top left of the window shows: the oscillators, or the effects in their place.
/// Either way the modulation sources stay in view, to be dragged onto the controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Oscillators,
    Effects,
}

impl EditorState {
    fn new() -> Self {
        Self {
            folder: wavetable_file::default_folder(),
            files: Vec::new(),
            scanned: false,
            custom_path: String::new(),
            builtin_waves: Vec::new(),
            filter_response: displays::FilterResponse::new(),
            tab: Tab::Oscillators,
            oscillators_size: egui::Vec2::ZERO,
            presets: title_bar::PresetBrowser::new(),
            tuning: tuning_window::TuningWindow::new(),
            analyser: analyser::Analyser::new(),
        }
    }
    fn rescan(&mut self) {
        self.files = wavetable_file::list_folder(&self.folder);
        self.scanned = true;
    }
}

/// What the GUI needs besides the parameters: its own state, a way to run wavetable loads in
/// the background, and what the synth is playing.
struct Gui<'a> {
    state: &'a mut EditorState,
    executor: &'a AsyncExecutor<ThreeOscPlugin>,
    live: Live<'a>,
}

/// What the displays need to follow the notes that are playing.
#[derive(Clone, Copy)]
struct Live<'a> {
    monitor: &'a Monitor,
    sample_rate: f32,
}

impl Live<'_> {
    /// Each shown voice's time into its note and the time it was released at, if it has
    /// been, in seconds, the newest last.
    fn voice_times(&self) -> Vec<(f32, Option<f32>)> {
        let seconds = |samples: u32| samples as f32 / self.sample_rate;
        self.monitor
            .voices()
            .map(|voice| (seconds(voice.runtime), voice.released.map(seconds)))
            .collect()
    }
    /// The newest voice's modulation of every target, if a voice is playing.
    fn modulation(&self) -> Option<Targets> {
        let mut targets = [0.0; N_TARGETS];
        for (index, modulation) in targets.iter_mut().enumerate() {
            *modulation = self.monitor.modulation(ModTarget::from_index(index))?;
        }
        Some(targets)
    }
}

/// Creates the editor. `sample_rate` is the synth's sample rate as `f32` bits, and `monitor`
/// what the synth shows of the notes it plays.
pub fn create(
    params: Arc<ThreeOscParams>,
    sample_rate: Arc<AtomicU32>,
    monitor: Arc<Monitor>,
    async_executor: AsyncExecutor<ThreeOscPlugin>,
) -> Option<Box<dyn Editor>> {
    let egui_state = params.editor_state.clone();
    create_egui_editor(
        egui_state.clone(),
        EditorState::new(),
        |_, _| {},
        move |egui_ctx, setter, state| {
            ResizableWindow::new("three-osc-window")
                .min_size(egui::vec2(320.0, 240.0))
                .show(egui_ctx, &egui_state, |ui| {
                    // The layout has a fixed natural size; if the window is made smaller than
                    // that the contents scroll instead of being cut off.
                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut gui = Gui {
                                state,
                                executor: &async_executor,
                                live: Live {
                                    monitor: &monitor,
                                    sample_rate: f32::from_bits(sample_rate.load(Ordering::Relaxed)),
                                },
                            };
                            draw(ui, &params, setter, &mut gui)
                        });
                });
        },
    )
}

fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    if !gui.state.scanned {
        gui.state.rescan();
    }
    if gui.state.builtin_waves.is_empty() {
        let mut renderer = CycleRenderer::new();
        gui.state.builtin_waves = builtin_spectra()
            .iter()
            .map(|(_, spectra)| Arc::new(WavePreview::from_spectra(spectra, &mut renderer)))
            .collect();
    }
    let matrix = Matrix {
        slots: &params.mod_slots,
        monitor: gui.live.monitor,
    };
    let live = gui.live;

    title_bar::title_bar(ui, params, setter, gui);
    tuning_window::show(ui.ctx(), params, &mut gui.state.tuning);
    ui.separator();
    tabs(ui, &params.fx, gui);

    ui.horizontal_top(|ui| {
        match gui.state.tab {
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
                gui.state.oscillators_size = size;
            }
            Tab::Effects => {
                let size = gui.state.oscillators_size;
                ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.set_min_size(size);
                    effects(ui, size, &params.fx, &matrix, setter, live);
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
            if gui.state.tab == Tab::Oscillators {
                wavetable_folder(ui, params, setter, gui);
            }
        });
    });

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            filter(ui, "Filter 1", &params.filter, &params.filter_env, &matrix, setter, gui);
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
        let tab = &mut gui.state.tab;
        ui.selectable_value(tab, Tab::Oscillators, RichText::new("Oscillators").strong())
            .on_hover_text("The three oscillators.");
        let on = [
            fx.distortion.enabled.value(),
            fx.chorus.enabled.value(),
            fx.delay.enabled.value(),
            fx.reverb.enabled.value(),
        ]
        .into_iter()
        .filter(|on| *on)
        .count();
        let label = if on > 0 { format!("FX ({on} on)") } else { String::from("FX") };
        ui.selectable_value(tab, Tab::Effects, RichText::new(label).strong()).on_hover_text(
            "The effects after the voices: distortion, chorus, delay and reverb, in that order. \
             They take the oscillators' place, so the modulation sources stay in view to be \
             dragged onto them.",
        );
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

/// The help shared by the effects' modulation targets.
const EFFECT_MODULATION_HELP: &str = "Modulation dropped here follows the newest note's sources, \
     since the effects process all the notes together.";

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
        toggle(
            ui,
            "On",
            &params.enabled,
            "Soft clipping (a tanh curve) of the synth's output, anti-aliased so that high \
             notes stay clean.",
            setter,
        );
        mod_slider(
            ui,
            &params.drive,
            &format!("How hard the sound is pushed into the clipping, in decibels. {EFFECT_MODULATION_HELP}"),
            ModTarget::DistortionDrive,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.mix,
            &format!("From the clean sound at 0% to only the distorted one at 100%. {EFFECT_MODULATION_HELP}"),
            ModTarget::DistortionMix,
            matrix,
            setter,
        );
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
        toggle(
            ui,
            "On",
            &params.enabled,
            "Mixes in a copy of the sound whose delay sweeps back and forth, a quarter of a cycle \
             apart in the two channels, for a wider, shimmering sound.",
            setter,
        );
        mod_slider(
            ui,
            &params.rate,
            &format!("How fast the delay sweeps. {EFFECT_MODULATION_HELP}"),
            ModTarget::ChorusRate,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.depth,
            &format!("How far the delay sweeps: up to 6 ms either way of 12 ms. {EFFECT_MODULATION_HELP}"),
            ModTarget::ChorusDepth,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.mix,
            &format!("From the dry sound at 0% to only the delayed copy at 100%; 50% is the classic \
                      chorus. {EFFECT_MODULATION_HELP}"),
            ModTarget::ChorusMix,
            matrix,
            setter,
        );
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
        toggle(ui, "On", &params.enabled, "Echoes of the sound.", setter);
        mod_slider(
            ui,
            &params.time,
            &format!("The time between echoes. Changing it glides the echoes' pitch, like tape. \
                      {EFFECT_MODULATION_HELP}"),
            ModTarget::DelayTime,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.feedback,
            &format!("How much of each echo comes back as the next one. {EFFECT_MODULATION_HELP}"),
            ModTarget::DelayFeedback,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.mix,
            &format!("From the dry sound at 0% to only the echoes at 100%. {EFFECT_MODULATION_HELP}"),
            ModTarget::DelayMix,
            matrix,
            setter,
        );
        toggle(
            ui,
            "Ping-Pong",
            &params.ping_pong,
            "The echoes bounce from one channel to the other, starting on the left.",
            setter,
        );
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
        toggle(
            ui,
            "On",
            &params.enabled,
            "A room around the sound (Freeverb: eight combs into four allpasses per channel).",
            setter,
        );
        mod_slider(
            ui,
            &params.size,
            &format!("How long the reverb rings. {EFFECT_MODULATION_HELP}"),
            ModTarget::ReverbSize,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.damping,
            &format!("How much faster the highs die away than the lows. {EFFECT_MODULATION_HELP}"),
            ModTarget::ReverbDamping,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &params.mix,
            &format!("From the dry sound at 0% to only the reverb at 100%. {EFFECT_MODULATION_HELP}"),
            ModTarget::ReverbMix,
            matrix,
            setter,
        );
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
    // The file's frames once it has loaded; until then the oscillator plays a sine
    let preview = match osc.wave.value().wavetable_index() {
        Some(wavetable) => gui.state.builtin_waves[wavetable].clone(),
        None => file.preview().unwrap_or_else(|| gui.state.builtin_waves[0].clone()),
    };
    let start = (osc.phase_rand.value() < 100.0).then(|| osc.phase.value() / 100.0);
    let silent = osc.amp.value() == 0.0;
    // Where the newest note's modulation has the wavetable position
    let target = ModTarget::osc_position(index);
    let position = match gui.live.monitor.modulation(target) {
        Some(modulation) => target.scaling().apply(osc.position.value(), modulation).clamp(0.0, 1.0),
        None => osc.position.value(),
    };
    let mut display = |ui: &mut Ui| {
        displays::wave(ui, &preview, position, start, silent);
    };
    section(ui, title, None, Display::Above(&mut display), |ui| {
        combo(
            ui,
            &osc.wave,
            "The wavetable the oscillator plays. The basic shapes and fixed pulse widths are \
             single frames; PW Sweep and Basic Shapes have several frames that Position morphs \
             between. File plays a wavetable loaded from a .wav file (see the Wavetable Files \
             box). All waves are bandlimited (alias-free).",
            setter,
        );
        if osc.wave.value() == OscillatorWave::File {
            wavetable_file_rows(ui, index, file, gui);
        }
        mod_slider(
            ui,
            &osc.position,
            "Position within the wavetable. Only has an effect on wavetables with more than one \
             frame: for PW Sweep it sweeps the pulse width from 50% (a square wave) down to a \
             very narrow pulse, and Basic Shapes morphs from a sine through the triangle, saw, \
             exponential and square to ever narrower pulses.",
            ModTarget::osc_position(index),
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &osc.amp,
            "Oscillator output volume. Doesn't affect PM, FM or AM.",
            ModTarget::osc_amp(index),
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &osc.pan,
            "Where the oscillator sits in the stereo field, from fully left to fully right. Its \
             unison voices spread out around it by the Stereo Width, less so the further it is \
             panned, so that they stay within the field. Modulation moves it once per control \
             rate block.",
            ModTarget::osc_pan(index),
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &osc.semitone,
            "Oscillator detune in semitones.",
            ModTarget::osc_pitch(index),
            matrix,
            setter,
        );
        slider(ui, &osc.octave, "Oscillator detune in octaves.", setter);
        slider(
            ui,
            &osc.multiplier,
            "Oscillator pitch multiplier / divider. Positive values multiply pitch, while \
             negative values divide. A value of 0 means this control is bypassed.",
            setter,
        );
        if let Some(modulation) = modulation {
            mod_slider(
                ui,
                &modulation.pm,
                "Phase modulation of this oscillator by the oscillator after it.",
                ModTarget::osc_pm(index).unwrap_or(ModTarget::None),
                matrix,
                setter,
            );
            mod_slider(
                ui,
                &modulation.fm,
                "Frequency modulation of this oscillator by the oscillator after it. Can be used \
                 for vibrato effects when the modulator is at a low pitch.",
                ModTarget::osc_fm(index).unwrap_or(ModTarget::None),
                matrix,
                setter,
            );
            mod_slider(
                ui,
                &modulation.am,
                "Amplitude modulation of this oscillator by the oscillator after it. Also known as \
                 'Ring Modulation'. Can be used for telephone sounds and tremolo.",
                ModTarget::osc_am(index).unwrap_or(ModTarget::None),
                matrix,
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
        mod_slider(
            ui,
            &osc.super_detune,
            "Unison voice detune. Does nothing if unison = 1.",
            ModTarget::osc_detune(index),
            matrix,
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

fn lfo(ui: &mut Ui, index: usize, lfo: &LfoParams, matrix: &Matrix, setter: &ParamSetter, live: Live) {
    let wave = lfo.wave.value().to_osc_wave();
    let phase = live.monitor.lfo_phase(index);
    section(
        ui,
        &format!("LFO {}", index + 1),
        Some(ModSource::lfo(index)),
        Display::Beside(&mut |ui| displays::lfo(ui, &wave, MOD_COLOR, phase)),
        |ui| {
            combo(ui, &lfo.wave, "The LFO's waveform.", setter);
            mod_slider(
                ui,
                &lfo.rate,
                "How fast the LFO oscillates. Each note has its own copy of the LFO, started \
                 when the note starts.",
                ModTarget::lfo_rate(index),
                matrix,
                setter,
            );
        },
    );
}

/// The rows picking an oscillator's wavetable file and showing how its load went.
fn wavetable_file_rows(ui: &mut Ui, index: usize, file: &WavetableFile, gui: &mut Gui) {
    ui.label("File").on_hover_text(
        "The .wav file this oscillator plays as a wavetable: one or more single cycles one \
         after the other, 2048 samples each unless the file says otherwise. Pick one from the \
         wavetable folder here, or type a path into the Wavetable Files box.",
    );
    let current = file.path();
    let current_name = Path::new(&current)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut picked = None;
    egui::ComboBox::from_id_salt(("wavetable file", index))
        .selected_text(if current.is_empty() {
            String::from("(none)")
        } else {
            current_name
        })
        .width(COMBO_WIDTH)
        .show_ui(ui, |ui| {
            if gui.state.files.is_empty() {
                ui.label(RichText::new("No .wav files in the wavetable folder").weak());
            }
            for name in &gui.state.files {
                if ui.selectable_label(current == *name, name).clicked() {
                    picked = Some(name.clone());
                }
            }
        });
    ui.end_row();
    if let Some(name) = picked {
        load_wavetable_file(file, index, name, gui.executor);
    }
    let mut status = file.status();
    if file.path().is_empty() {
        status = String::from("No file loaded: plays a sine until one is.");
    }
    if !status.is_empty() {
        ui.label("");
        ui.add(egui::Label::new(RichText::new(status).small().weak()).wrap());
        ui.end_row();
    }
}

/// Sets an oscillator's wavetable file and starts loading it in the background.
fn load_wavetable_file(
    file: &WavetableFile,
    osc: usize,
    path: String,
    executor: &AsyncExecutor<ThreeOscPlugin>,
) {
    file.set_path(path);
    if let Some(generation) = file.claim_load() {
        file.set_status(String::from("Loading..."));
        executor.execute_background(Task::LoadWavetable { osc, generation });
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
                    ui.label(RichText::new("Wavetable Files").strong()).on_hover_text(
                        "Put .wav wavetables in this folder and set an oscillator's Wave to \
                         File to play them. The folder is Documents/Three Osc/Wavetables in your \
                         home directory, or wherever the THREE_OSC_WAVETABLES environment \
                         variable points.",
                    );
                    if ui.small_button("Rescan").clicked() {
                        gui.state.rescan();
                    }
                });
                ui.separator();
                ui.add(
                    egui::Label::new(
                        RichText::new(gui.state.folder.to_string_lossy()).small().weak(),
                    )
                    .wrap(),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut gui.state.custom_path)
                            .hint_text("Path to a .wav file")
                            .desired_width(SLIDER_WIDTH + 40.0),
                    )
                    .on_hover_text(
                        "A file outside the folder, loaded into an oscillator with the buttons.",
                    );
                    for osc in 0..3 {
                        if ui
                            .small_button(format!("Osc {}", osc + 1))
                            .on_hover_text(
                                "Loads the file into this oscillator, and sets its Wave to File.",
                            )
                            .clicked()
                            && !gui.state.custom_path.trim().is_empty()
                        {
                            let path = gui.state.custom_path.trim().to_string();
                            load_wavetable_file(params.wavetable_file(osc), osc, path, gui.executor);
                            set_param(setter, &params.oscillator(osc).wave, OscillatorWave::File);
                        }
                    }
                });
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
                    for source in [
                        ModSource::Velocity,
                        ModSource::Key,
                        ModSource::ModWheel,
                        ModSource::Aftertouch,
                        ModSource::Random,
                    ] {
                        drag_handle(ui, source);
                    }
                });
            });
        });
    });
}

fn filter(
    ui: &mut Ui,
    title: &str,
    filter: &FilterParams,
    filter_env: &FilterEnvParams,
    matrix: &Matrix,
    setter: &ParamSetter,
    gui: &mut Gui,
) {
    let settings = displays::FilterSettings::new(
        filter.model.value().to_filter_model(),
        filter.filter_type.value().to_filter_type(),
        filter.cutoff.value(),
        filter.resonance.value(),
        // As the synth applies it
        filter_env.amount.value().powi(2),
        gui.live.sample_rate,
    );
    let response = &mut gui.state.filter_response;
    let live_cutoff = gui.live.monitor.cutoff();
    let mut display = |ui: &mut Ui| displays::filter(ui, settings, response, live_cutoff);
    section(ui, title, None, Display::Above(&mut display), |ui| {
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
        mod_slider(
            ui,
            &filter.cutoff,
            "Changes the frequency at which the filter starts taking effect. Try it out.",
            ModTarget::Cutoff,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &filter.resonance,
            "Adds feedback to the filter loop, creating a volume spike at the filter's cutoff \
             frequency. On the RC and Ladder filters, setting this high enough creates a \
             self-sustaining sine wave.",
            ModTarget::Resonance,
            matrix,
            setter,
        );
        mod_slider(
            ui,
            &filter.drive,
            "Multiplies the amplitude of the filter input, creating distortion inside the RC and \
             Ladder filters. Does not amplify when Model = None or Digital, to keep volume equal \
             between filters.",
            ModTarget::Drive,
            matrix,
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

fn filter_envelope(ui: &mut Ui, title: &str, env: &FilterEnvParams, setter: &ParamSetter, live: Live) {
    let mut display = |ui: &mut Ui| envelope_display(ui, &env.envelope, live);
    section(ui, title, Some(ModSource::FilterEnv), Display::Above(&mut display), |ui| {
        slider(
            ui,
            &env.amount,
            "Amount the envelope affects the filter cutoff.",
            setter,
        );
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
    let mut display = |ui: &mut Ui| envelope_display(ui, env, live);
    section(ui, title, Some(source), Display::Above(&mut display), |ui| {
        envelope_rows(ui, env, setter)
    });
}

/// An envelope's shape, worked out the way the synth does it, with where each note playing is
/// in it.
fn envelope_display(ui: &mut Ui, params: &EnvelopeParams, live: Live) {
    let mut envelope = AdsrEnvelope::new(0.0, 0.0, 0.0, 0.0, 0.0);
    crate::apply_envelope_params(&mut envelope, params);
    displays::envelope(ui, &envelope, MOD_COLOR, &live.voice_times());
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

fn global(ui: &mut Ui, title: &str, global: &GlobalParams, matrix: &Matrix, setter: &ParamSetter) {
    section(ui, title, None, Display::None, |ui| {
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
        mod_slider(
            ui,
            &global.output_gain,
            "Master output volume. Adjust if the synth is too loud / too quiet. Modulation \
             dropped here controls the volume of each note (Volume (all) in the matrix).",
            ModTarget::Volume,
            matrix,
            setter,
        );
        slider(
            ui,
            &global.stereo_width,
            "Spreads each oscillator's unison voices across the stereo field around the \
             oscillator's Pan, with the lowest pitched voice furthest left and the highest \
             furthest right. At 0 all of an oscillator's voices are where its Pan puts it (in \
             the centre, unless panned). An oscillator with Unison = 1 is always just where its \
             Pan puts it.",
            setter,
        );
        mod_slider(
            ui,
            &global.global_pitch,
            "Pitch detune for the whole synth, in semitones. Modulation dropped here controls \
             the pitch of every oscillator (Pitch (all) in the matrix).",
            ModTarget::Pitch,
            matrix,
            setter,
        );
        slider(
            ui,
            &global.bend_range,
            "Controls the range of the MIDI pitch wheel in semitones. Useful if you have a MIDI \
             keyboard.",
            setter,
        );
        combo(
            ui,
            &global.interpolation,
            "How the oscillators read their wavetables in between samples. Cubic is the \
             cleanest. Linear is a little cheaper and adds some aliasing noise on high notes. \
             None (no interpolation) is the cheapest and the grittiest: it steps between the \
             samples, which is clearly audible on all but the lowest notes.",
            setter,
        );
        combo(
            ui,
            &global.modulation_rate,
            "How often the envelopes, LFOs, modulation routes, pitch and filter settings are \
             worked out. Block works them out every 32 samples and glides gains, pitch and the \
             Digital filter in between, which is plenty for most sounds. Sample works them out at \
             every sample, for the snappiest envelopes and LFOs fast enough to be heard as \
             tones. It costs little extra while nothing moves, and up to two or three times the \
             CPU while modulation does.",
            setter,
        );
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
                     cutoff by 10 octaves. Drag a source's handle onto a slider to fill in a \
                     row.",
                );
                ui.separator();
                egui::Grid::new("routes")
                    .num_columns(4)
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
    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(SLIDER_WIDTH))
        .on_hover_text("How much of the source is applied to the target, from -100% to 100%.");
    let in_use = slot.source.value() != ModSource::None || slot.target.value() != ModTarget::None;
    if ui
        .add_enabled(in_use, egui::Button::new(RichText::new("×").strong()).small())
        .on_hover_text("Clears this row.")
        .clicked()
    {
        set_param(setter, &slot.source, ModSource::None);
        set_param(setter, &slot.target, ModTarget::None);
        set_param(setter, &slot.amount, 0.0);
    }
    ui.end_row();
}

/// A read-only view of the modulation matrix, plus the editing it needs, and what the synth
/// plays (for the modulated sliders to show where the newest note's modulation takes them).
struct Matrix<'a> {
    slots: &'a [ModSlotParams; N_SLOTS],
    monitor: &'a Monitor,
}

impl Matrix<'_> {
    /// The routes that modulate `target`, as `(source, amount)`.
    fn routes_for(&self, target: ModTarget) -> impl Iterator<Item = (ModSource, f32)> + '_ {
        self.slots.iter().filter_map(move |slot| {
            let source = slot.source.value();
            (source != ModSource::None && slot.target.value() == target)
                .then(|| (source, slot.amount.value()))
        })
    }
    /// Routes `source` to `target` in the first free slot (unless such a route already exists),
    /// with a starting amount of 50%. Returns whether a slot was found.
    fn add_route(&self, setter: &ParamSetter, source: ModSource, target: ModTarget) -> bool {
        if source == ModSource::None || target == ModTarget::None {
            return false;
        }
        let existing = self
            .slots
            .iter()
            .any(|slot| slot.source.value() == source && slot.target.value() == target);
        if existing {
            return true;
        }
        let free = self.slots.iter().find(|slot| {
            slot.source.value() == ModSource::None || slot.target.value() == ModTarget::None
        });
        match free {
            Some(slot) => {
                set_param(setter, &slot.source, source);
                set_param(setter, &slot.target, target);
                if slot.amount.value() == 0.0 {
                    set_param(setter, &slot.amount, 0.5);
                }
                true
            }
            None => false,
        }
    }
}

/// Sets a parameter from the GUI, with the begin/end gestures the host expects.
fn set_param<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
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

/// A parameter with numeric values, for showing where modulation takes its slider.
trait NumericParam: Param {
    /// The value the slider shows.
    fn value_f32(&self) -> f32;
    /// Where a value would put the slider, from 0 to 1.
    fn normalized_at(&self, value: f32) -> f32;
}

impl NumericParam for FloatParam {
    fn value_f32(&self) -> f32 {
        self.modulated_plain_value()
    }
    fn normalized_at(&self, value: f32) -> f32 {
        self.preview_normalized(value)
    }
}

impl NumericParam for IntParam {
    fn value_f32(&self) -> f32 {
        self.modulated_plain_value() as f32
    }
    fn normalized_at(&self, value: f32) -> f32 {
        self.preview_normalized(value.round() as i32)
    }
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

        // The routes that modulate this control
        let routes: Vec<String> = matrix
            .routes_for(target)
            .map(|(source, amount)| {
                format!(
                    "{} {:+.0}%",
                    ModSource::variants()[source.to_index()],
                    amount * 100.0
                )
            })
            .collect();
        if !routes.is_empty() {
            // Painted rather than a glyph, which the default proportional fonts may lack
            let (rect, marker) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.5, MOD_COLOR);
            marker.on_hover_text(format!("Modulated by:\n{}", routes.join("\n")));

            if let Some(modulation) = matrix.monitor.modulation(target) {
                let value = param.value_f32();
                let from = param.normalized_at(value);
                let to = param.normalized_at(target.scaling().apply(value, modulation));
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

/// A combo box listing the enum's variants, without a label.
fn combo_cell<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    param: &EnumParam<T>,
    id_salt: usize,
    width: f32,
    help: &str,
    setter: &ParamSetter,
) {
    let variants = T::variants();
    let current = param.value().to_index();
    let mut selected = current;
    egui::ComboBox::from_id_salt((param.name(), id_salt))
        .selected_text(variants[current])
        .width(width)
        .show_ui(ui, |ui| {
            for (index, name) in variants.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *name);
            }
        })
        .response
        .on_hover_text(help);
    if selected != current {
        set_param(setter, param, T::from_index(selected));
    }
}
