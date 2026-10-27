//! The plugin's GUI, built with egui through `nih_plug_egui`.
//!
//! It comes in seven themes (see [`theme`]), each a layout of the same controls with its own
//! look: Studio, knobs grouped in cards that stretch to fill the window (see [`studio`]);
//! Classic, the original layout of sliders all on one page (see [`classic`]); and Flow, Rack,
//! Focus, Compact and Vintage (see [`flow`], [`rack`], [`focus`], [`compact`] and [`vintage`]),
//! which share the knobs (see [`knob`]), what they show of each control (see [`catalog`]) and
//! other pieces (see [`widgets`]). Whichever it is, the presets and the tuning are in the title
//! bar (see [`title_bar`] and [`tuning_window`]), every control is bound directly to a
//! parameter, so host automation and the GUI stay in sync, and hovering a control shows what it
//! does (see [`help`]).
//!
//! Modulation is set up by dragging: every modulation source (the LFOs, the envelopes, the
//! oscillators and the MIDI sources) has a handle that can be dragged onto any control that
//! can be modulated, which fills in a free slot of the modulation matrix.
//!
//! The oscillators, LFOs, envelopes, the filter and the effects each have a display of what they
//! do, see [`displays`]. The displays follow the notes that are playing (see [`Monitor`]): the
//! envelopes and LFOs show where each note is in them, the waves and the filter move with the
//! modulation, and a modulated control shows where the newest note's modulation takes it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Instant;
use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, RichText, Ui};
use nih_plug_egui::resizable_window::ResizableWindow;
use nih_plug_egui::{create_egui_editor, EguiState};

use crate::params::{
    CurrentValues, EnvelopeParams, FilterEnvParams, FilterParams, LfoParams, ModSlotParams, OscParams, OscillatorWave,
    ThreeOscParams, WavetableFile,
};
use crate::synth::envelopes::AdsrEnvelope;
use crate::synth::modulation::{ModSource, ModTarget, Polarity, Targets, N_SLOTS, N_TARGETS};
use crate::synth::monitor::Monitor;
use crate::synth::oscillator::{builtin_spectra, CycleRenderer, WavePreview};
use crate::synth::oversampling::Oversampling;
use crate::synth::wavetable_file;
use crate::{Task, ThreeOscPlugin};
use theme::Theme;

mod analyser;
mod catalog;
mod classic;
mod compact;
mod displays;
mod flow;
mod focus;
mod help;
mod knob;
mod rack;
mod studio;
mod theme;
mod title_bar;
mod tuning_window;
mod vintage;
mod widgets;
mod window_thread;

/// The window's default size, in logical pixels.
pub const DEFAULT_WIDTH: u32 = 1400;
pub const DEFAULT_HEIGHT: u32 = 1000;

/// The colour of everything to do with modulation: source handles, route markers and the drop
/// highlights.
const MOD_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 170, 60);

/// The window size used when the plugin is first loaded. The size is stored with the plugin's
/// state, so resizing the window is remembered.
pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(DEFAULT_WIDTH, DEFAULT_HEIGHT)
}

/// State that only the GUI needs: the listing of the wavetable folder, the preset and tuning
/// browsers, what the displays draw, and what each layout shows.
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
    presets: title_bar::PresetBrowser,
    tuning: tuning_window::TuningWindow,
    /// The output section's spectrogram and meters.
    analyser: analyser::Analyser,
    /// Why the theme last picked couldn't be saved, if it couldn't.
    theme_problem: Option<String>,
    /// Whether the settings window is open (see [`widgets::settings_window()`]), and the routes
    /// window.
    settings_open: bool,
    routes_open: bool,
    /// The modulation matrix's slots the routes editor shows although they don't route
    /// anything (yet), because they were added there (see [`widgets::routes_list()`]).
    added_routes: [bool; N_SLOTS],
    classic: classic::ClassicState,
    studio: studio::StudioState,
    rack: rack::RackState,
    focus: focus::FocusState,
    compact: compact::CompactState,
    vintage: vintage::VintageState,
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
            presets: title_bar::PresetBrowser::new(),
            tuning: tuning_window::TuningWindow::new(),
            analyser: analyser::Analyser::new(),
            theme_problem: None,
            settings_open: false,
            routes_open: false,
            added_routes: [false; N_SLOTS],
            classic: classic::ClassicState::new(),
            studio: studio::StudioState::new(),
            rack: rack::RackState::new(),
            focus: focus::FocusState::new(),
            compact: compact::CompactState::new(),
            vintage: vintage::VintageState::new(),
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
    let threads = Arc::new(window_thread::WindowThreads::default());
    let opened = threads.clone();
    let editor = create_egui_editor(
        egui_state.clone(),
        EditorState::new(),
        // (On the window's thread, as each window opens)
        move |_, _| opened.opened(),
        move |egui_ctx, setter, state| {
            window_thread::end_if_orphaned();
            // Each window has an egui context of its own, which needs the theme's look set
            let theme = theme::current();
            let styled = egui::Id::new("styled theme");
            if egui_ctx.data(|data| data.get_temp::<Theme>(styled)) != Some(theme) {
                theme::apply(egui_ctx, theme);
                egui_ctx.data_mut(|data| data.insert_temp(styled, theme));
            }
            ResizableWindow::new("three-osc-window")
                .min_size(egui::vec2(320.0, 240.0))
                .show(egui_ctx, &egui_state, |ui| {
                    // The layout has a natural size; if the window is made smaller than that
                    // the contents scroll instead of being cut off.
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
                            draw(ui, &params, setter, &mut gui, theme)
                        });
                });
        },
    );
    editor.map(|editor| Box::new(window_thread::WindowedEditor::new(editor, threads)) as Box<dyn Editor>)
}

/// Seconds since the editor first opened, for what's timed across frames: unlike the clock of
/// a window's egui context, it doesn't start again each time the editor opens.
fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui, theme: Theme) {
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
    tuning_window::show(ui.ctx(), params, &mut gui.state.tuning);
    widgets::settings_window(ui.ctx(), params, setter, gui);
    widgets::remap_window(ui.ctx(), params, setter);
    match theme {
        Theme::Studio => studio::draw(ui, params, setter, gui),
        Theme::Classic => classic::draw(ui, params, setter, gui),
        Theme::Flow => flow::draw(ui, params, setter, gui),
        Theme::Rack => rack::draw(ui, params, setter, gui),
        Theme::Focus => focus::draw(ui, params, setter, gui),
        Theme::Compact => compact::draw(ui, params, setter, gui),
        Theme::Vintage => vintage::draw(ui, params, setter, gui),
    }
}

/// A combo box for switching the theme, with a warning when the choice couldn't be saved.
fn theme_picker(ui: &mut Ui, gui: &mut Gui) {
    let current = theme::current();
    let mut picked = current;
    egui::ComboBox::from_id_salt("theme")
        .selected_text(format!("Theme: {}", current.name()))
        .show_ui(ui, |ui| {
            for theme in Theme::ALL {
                ui.selectable_value(&mut picked, theme, theme.name())
                    .on_hover_text(theme.description());
            }
        })
        .response
        .on_hover_text(
            "Switches the editor's layout and look (hover a theme for what it's like). The \
             choice is kept for every project, in Settings.json in Documents/Three Osc.",
        );
    if picked != current {
        switch_theme(ui, gui, picked);
    }
    if let Some(problem) = &gui.state.theme_problem {
        let clear = ui
            .add(egui::Label::new(RichText::new("⚠").color(title_bar::ERROR_COLOR)).sense(egui::Sense::click()))
            .on_hover_text(format!("{problem}\nThe theme is used until the plugin is reloaded. (Click to dismiss.)"))
            .clicked();
        if clear {
            gui.state.theme_problem = None;
        }
    }
}

/// Switches every editor to `theme`, keeping what went wrong saving it, if anything, to show.
fn switch_theme(ui: &Ui, gui: &mut Gui, theme: Theme) {
    gui.state.theme_problem = theme::set(theme).err();
    ui.ctx().request_repaint();
}

/// A read-only view of the modulation matrix, plus the editing it needs, and what the synth
/// plays (for the modulated controls to show where the newest note's modulation takes them).
struct Matrix<'a> {
    slots: &'a [ModSlotParams; N_SLOTS],
    monitor: &'a Monitor,
}

impl Matrix<'_> {
    /// The routes that modulate `target`, as their source, whether they swing both ways (see
    /// [`Polarity`]), and their amount.
    fn routes_for(&self, target: ModTarget) -> impl Iterator<Item = (ModSource, bool, f32)> + '_ {
        self.slots_for(target).map(|slot| {
            let source = slot.source.value();
            (source, slot.polarity.value().is_bipolar(source), slot.amount.value())
        })
    }
    /// The slots that route a source to `target` (none, for no target).
    fn slots_for(&self, target: ModTarget) -> impl Iterator<Item = &ModSlotParams> + '_ {
        self.indexed_slots_for(target).map(|(_, slot)| slot)
    }
    /// [`slots_for()`][Self::slots_for()], with each slot's index in the matrix.
    fn indexed_slots_for(&self, target: ModTarget) -> impl Iterator<Item = (usize, &ModSlotParams)> + '_ {
        self.slots.iter().enumerate().filter(move |(_, slot)| {
            target != ModTarget::None && slot.source.value() != ModSource::None && slot.target.value() == target
        })
    }
    /// Whether a slot routes a source to a target.
    fn is_used(slot: &ModSlotParams) -> bool {
        slot.source.value() != ModSource::None && slot.target.value() != ModTarget::None
    }
    /// How many routes `source` drives.
    fn routes_from(&self, source: ModSource) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.source.value() == source && slot.target.value() != ModTarget::None)
            .count()
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
        match self.slots.iter().find(|slot| !Self::is_used(slot)) {
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
    /// Empties a slot, and puts its remap back as it was.
    fn clear(setter: &ParamSetter, slot: &ModSlotParams) {
        set_param(setter, &slot.source, ModSource::None);
        set_param(setter, &slot.target, ModTarget::None);
        set_param(setter, &slot.amount, 0.0);
        reset_remap(setter, slot);
    }
}

/// Puts a slot's remap back to leaving the source as it comes (see [`Remap`]).
fn reset_remap(setter: &ParamSetter, slot: &ModSlotParams) {
    if slot.polarity.value() != Polarity::Source {
        set_param(setter, &slot.polarity, Polarity::Source);
    }
    if slot.curve.value() != 0.0 {
        set_param(setter, &slot.curve, 0.0);
    }
    if slot.steps.value() != 1 {
        set_param(setter, &slot.steps, 1);
    }
    if slot.via.value() != ModSource::None {
        set_param(setter, &slot.via, ModSource::None);
    }
}

/// Sets a parameter from the GUI, with the begin/end gestures the host expects.
fn set_param<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// A parameter with numeric values, for showing where modulation takes its control.
trait NumericParam: Param {
    /// The value the control shows.
    fn value_f32(&self) -> f32;
    /// Where a value would put the control, from 0 to 1.
    fn normalized_at(&self, value: f32) -> f32;
    /// The value at a position of the control, from 0 to 1.
    fn plain_at(&self, normalized: f32) -> f32;
}

impl NumericParam for FloatParam {
    fn value_f32(&self) -> f32 {
        self.modulated_plain_value()
    }
    fn normalized_at(&self, value: f32) -> f32 {
        self.preview_normalized(value)
    }
    fn plain_at(&self, normalized: f32) -> f32 {
        self.preview_plain(normalized)
    }
}

impl NumericParam for IntParam {
    fn value_f32(&self) -> f32 {
        self.modulated_plain_value() as f32
    }
    fn normalized_at(&self, value: f32) -> f32 {
        self.preview_normalized(value.round() as i32)
    }
    fn plain_at(&self, normalized: f32) -> f32 {
        self.preview_plain(normalized) as f32
    }
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
    enum_combo(ui, egui::ComboBox::from_id_salt((param.name(), id_salt)).width(width), param, help, setter);
}

/// Like [`combo_cell()`], but cutting the choice's name short where it doesn't fit in the space
/// left in `ui`, rather than running past it.
fn short_combo_cell<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    param: &EnumParam<T>,
    id_salt: usize,
    width: f32,
    help: &str,
    setter: &ParamSetter,
) {
    let combo = egui::ComboBox::from_id_salt((param.name(), id_salt)).width(width).truncate();
    enum_combo(ui, combo, param, help, setter);
}

fn enum_combo<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    combo: egui::ComboBox,
    param: &EnumParam<T>,
    help: &str,
    setter: &ParamSetter,
) {
    let variants = T::variants();
    let current = param.value().to_index();
    let mut selected = current;
    combo
        .selected_text(variants[current])
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

/// What an oscillator's display shows, see [`displays::wave()`].
struct WaveDisplay {
    preview: Arc<WavePreview>,
    /// Where the newest note's modulation has the wavetable position.
    position: f32,
    phase: f32,
    phase_rand: f32,
    silent: bool,
}

impl WaveDisplay {
    fn new(index: usize, osc: &OscParams, file: &WavetableFile, gui: &Gui) -> Self {
        // The file's frames once it has loaded; until then the oscillator plays a sine
        let preview = match osc.wave.value().wavetable_index() {
            Some(wavetable) => gui.state.builtin_waves[wavetable].clone(),
            None => file.preview().unwrap_or_else(|| gui.state.builtin_waves[0].clone()),
        };
        let target = ModTarget::osc_position(index);
        let position = match gui.live.monitor.modulation(target) {
            Some(modulation) => target.scaling().apply(osc.position.value(), modulation).clamp(0.0, 1.0),
            None => osc.position.value(),
        };
        Self {
            preview,
            position,
            phase: osc.phase.value() / 100.0,
            phase_rand: osc.phase_rand.value() / 100.0,
            silent: osc.amp.value() == 0.0,
        }
    }

    fn show(&self, ui: &mut Ui, height: f32) {
        displays::wave(ui, &self.preview, self.position, self.phase, self.phase_rand, self.silent, height);
    }
}

/// The filter's settings for its display, and the newest note's cutoff and resonance (as the
/// synth modulates them) while a note plays, see [`displays::filter()`].
fn filter_display_inputs(
    filter: &FilterParams,
    filter_env: &FilterEnvParams,
    oversampling: Oversampling,
    live: Live,
) -> (displays::FilterSettings, Option<(f32, f32)>) {
    let settings = displays::FilterSettings::new(
        filter.model.value().to_filter_model(),
        filter.filter_type.value().to_filter_type(),
        filter.cutoff.value(),
        filter.resonance.value(),
        // As the synth applies it
        filter_env.amount.value().powi(2),
        live.sample_rate,
        oversampling.factor(),
    );
    let monitor = live.monitor;
    let playing = monitor.cutoff().map(|cutoff| {
        let resonance = match monitor.modulation(ModTarget::Resonance) {
            Some(modulation) if modulation != 0.0 => {
                ModTarget::Resonance.scaling().apply(filter.resonance.value(), modulation).clamp(0.1, 10.0)
            }
            _ => filter.resonance.value(),
        };
        (cutoff, resonance)
    });
    (settings, playing)
}

/// An envelope's shape, worked out the way the synth does it, with where each note playing is
/// in it, and its handles to drag, `height` tall.
fn envelope_display(ui: &mut Ui, params: &EnvelopeParams, setter: &ParamSetter, live: Live, height: f32) {
    let mut envelope = AdsrEnvelope::new(0.0, 0.0, 0.0, 0.0, 0.0);
    crate::apply_envelope_params(&mut envelope, params);
    displays::envelope(ui, &envelope, MOD_COLOR, &live.voice_times(), params, setter, height);
}

/// An LFO's display (see [`displays::lfo()`]), with where the newest note's copy of it is.
fn lfo_display(ui: &mut Ui, params: &LfoParams, index: usize, live: Live, height: f32) {
    let lfo = params.lfo(&CurrentValues);
    let newest = live.monitor.lfo(index).zip(live.monitor.lfo_value(index));
    displays::lfo(ui, &lfo, MOD_COLOR, newest, height);
}

/// A combo box picking an oscillator's wavetable file from the wavetable folder.
fn wavetable_file_combo(ui: &mut Ui, index: usize, file: &WavetableFile, gui: &mut Gui, width: f32) {
    let current = file.path();
    let current_name = Path::new(&current)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut picked = None;
    egui::ComboBox::from_id_salt(("wavetable file", index))
        // (A long name is cut short rather than running past the space there is)
        .truncate()
        .selected_text(if current.is_empty() {
            String::from("(none)")
        } else {
            current_name
        })
        .width(width)
        .show_ui(ui, |ui| {
            if gui.state.files.is_empty() {
                ui.label(RichText::new("No .wav files in the wavetable folder").weak());
            }
            for name in &gui.state.files {
                if ui.selectable_label(current == *name, name).clicked() {
                    picked = Some(name.clone());
                }
            }
        })
        .response
        .on_hover_text(help::WAVETABLE_FILE);
    if let Some(name) = picked {
        load_wavetable_file(file, index, name, gui.executor);
    }
}

/// How an oscillator's wavetable file's load went, if there's anything to say.
fn wavetable_file_status(file: &WavetableFile) -> Option<String> {
    let status = if file.path().is_empty() {
        String::from("No file loaded: plays a sine until one is.")
    } else {
        file.status()
    };
    (!status.is_empty()).then_some(status)
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

/// The wavetable folder, where the oscillators' File combo boxes look, with a button to look
/// again, and a way to load a file from anywhere else. `path_width` is the width of the path
/// to load from.
fn wavetable_folder_rows(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui, path_width: f32) {
    ui.add(egui::Label::new(RichText::new(gui.state.folder.to_string_lossy()).small().weak()).wrap());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut gui.state.custom_path)
                .hint_text("Path to a .wav file")
                .desired_width(path_width),
        )
        .on_hover_text(help::WAVETABLE_PATH);
        for osc in 0..3 {
            if ui
                .small_button(format!("Osc {}", osc + 1))
                .on_hover_text(help::WAVETABLE_LOAD)
                .clicked()
                && !gui.state.custom_path.trim().is_empty()
            {
                let path = gui.state.custom_path.trim().to_string();
                load_wavetable_file(params.wavetable_file(osc), osc, path, gui.executor);
                set_param(setter, &params.oscillator(osc).wave, OscillatorWave::File);
            }
        }
    });
}
