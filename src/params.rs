//! Plugin parameters.
//!
//! These replace the LV2 control ports (and the `build.rs` script that used to generate the `.ttl`
//! metadata for them). Each parameter's name, range, default value and display formatting is
//! declared right here, and nih-plug exposes them to the host and to the GUI.
//!
//! The parameter IDs (e.g. `osc1_wave`, `fil1_cutoff`, `vol_attack`) intentionally match the old
//! LV2 port symbols.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use serde::{Deserialize, Serialize};

use crate::synth::effects::{self, EffectSettings};
use crate::synth::filter::{FilterModel, FilterType};
use crate::synth::modulation::{ModSource, ModTarget, N_LFOS, N_MOD_ENVELOPES, N_SLOTS};
use crate::synth::oscillator::{Interpolation, OscWave, WavePreview};
use crate::synth::oversampling::Oversampling;
use crate::synth::tuning::{Tuning, KEYS};
use crate::synth::{ModulationRate, Polyphony};

/// Waveforms available to the LFO.
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
}

/// Wavetables available to the oscillators, in the order they are stored in the synth's
/// `WavetableSet`.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscillatorWave {
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
    #[id = "pulse40"]
    #[name = "Pulse 40%"]
    Pulse40,
    #[id = "pulse30"]
    #[name = "Pulse 30%"]
    Pulse30,
    #[id = "pulse20"]
    #[name = "Pulse 20%"]
    Pulse20,
    #[id = "pulse10"]
    #[name = "Pulse 10%"]
    Pulse10,
    #[id = "pw_sweep"]
    #[name = "PW Sweep"]
    PwSweep,
    /// The nine shapes above (sine to the 10% pulse), one frame each, in that order.
    #[id = "basic_shapes"]
    #[name = "Basic Shapes"]
    BasicShapes,
    /// The wavetable loaded from the oscillator's file (see [`WavetableFile`]).
    #[id = "file"]
    #[name = "File"]
    File,
}

impl OscillatorWave {
    /// The index of this wavetable in the synth's `WavetableSet`, or `None` for the wavetable
    /// loaded from a file.
    pub fn wavetable_index(self) -> Option<usize> {
        (self != OscillatorWave::File).then(|| self.to_index())
    }
}

/// The path of the wavetable file an oscillator plays when its wave is set to File, saved with
/// the plugin's state. The file is read and its tables generated on a background thread (see
/// [`crate::Task`]); the audio thread only compares a generation counter to notice a new path,
/// and a load is claimed by whichever of the GUI and the audio thread sees the change first.
#[derive(Debug, Default)]
pub struct WavetableFile {
    /// Absolute, or a file name in the wavetable folder (see
    /// [`wavetable_file::resolve()`][crate::synth::wavetable_file::resolve()]). Empty for none.
    path: RwLock<String>,
    /// Bumped whenever the path changes.
    generation: AtomicU32,
    /// The generation a load was last started for.
    requested: AtomicU32,
    /// What became of the last load, for the GUI.
    status: Mutex<String>,
    /// The frames of the loaded file, for the GUI to draw.
    preview: Mutex<Option<Arc<WavePreview>>>,
}

impl WavetableFile {
    pub fn path(&self) -> String {
        self.path.read().unwrap().clone()
    }
    pub fn set_path(&self, path: String) {
        PersistentField::set(self, path);
    }
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::SeqCst)
    }
    /// Claims the load of the current path, if nobody has yet: returns the generation to load
    /// for, or `None` when it is already being (or has been) loaded.
    pub fn claim_load(&self) -> Option<u32> {
        let generation = self.generation();
        let previous = self.requested.load(Ordering::SeqCst);
        if previous == generation {
            return None;
        }
        self.requested
            .compare_exchange(previous, generation, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| generation)
    }
    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
    pub fn set_status(&self, status: String) {
        *self.status.lock().unwrap() = status;
    }
    pub fn preview(&self) -> Option<Arc<WavePreview>> {
        self.preview.lock().unwrap().clone()
    }
    pub fn set_preview(&self, preview: Option<Arc<WavePreview>>) {
        *self.preview.lock().unwrap() = preview;
    }
}

impl PersistentField<'_, String> for WavetableFile {
    fn set(&self, new_value: String) {
        *self.path.write().unwrap() = new_value;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&String) -> R,
    {
        f(&self.path.read().unwrap())
    }
}

/// The Scala files the synth is tuned with (see [`crate::synth::tuning`]), saved with the
/// plugin's state: their names and contents, so that a project sounds the same on a computer
/// without the files.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuningFiles {
    /// The scale, or `None` for twelve-tone equal temperament.
    pub scale: Option<TextFile>,
    /// The keyboard mapping, or `None` for a degree per key with the root on middle C.
    pub mapping: Option<TextFile>,
}

/// A small text file's name and contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextFile {
    pub name: String,
    pub text: String,
}

impl TextFile {
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("can't read the file: {error}"))?;
        Ok(Self {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            // Scala files are often in Latin-1, whose accents this replaces
            text: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}

impl TuningFiles {
    pub fn tuning(&self) -> Result<Tuning, String> {
        let text = |file: &Option<TextFile>| file.as_ref().map(|file| file.text.clone());
        Tuning::from_files(text(&self.scale).as_deref(), text(&self.mapping).as_deref())
    }
}

/// The synth's tuning, see [`TuningFiles`]. The pitch of each key is worked out whenever the
/// files change, for the audio thread to pick up without allocating.
pub struct TuningParam {
    files: RwLock<TuningFiles>,
    /// Each key's pitch as `f32` bits, NaN for keys that don't play.
    pitches: [AtomicU32; KEYS],
    /// Bumped whenever the pitches change.
    generation: AtomicU32,
    /// Why the files saved with the state couldn't be used, if they couldn't.
    error: Mutex<Option<String>>,
}

impl Default for TuningParam {
    fn default() -> Self {
        let tuning = Self {
            files: RwLock::default(),
            pitches: std::array::from_fn(|_| AtomicU32::new(0)),
            generation: AtomicU32::new(0),
            error: Mutex::default(),
        };
        tuning.store(&Tuning::standard());
        tuning
    }
}

impl TuningParam {
    pub fn files(&self) -> TuningFiles {
        self.files.read().unwrap().clone()
    }
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }
    /// Each key's pitch, NaN for keys that don't play.
    pub fn pitches(&self) -> [f32; KEYS] {
        std::array::from_fn(|key| f32::from_bits(self.pitches[key].load(Ordering::Relaxed)))
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    /// Tunes the synth with new files, if they make a tuning. Otherwise the tuning stays as it
    /// was, and the error says why.
    pub fn try_set(&self, files: TuningFiles) -> Result<(), String> {
        let tuning = files.tuning()?;
        *self.files.write().unwrap() = files;
        *self.error.lock().unwrap() = None;
        self.store(&tuning);
        Ok(())
    }
    fn store(&self, tuning: &Tuning) {
        for (slot, pitch) in self.pitches.iter().zip(tuning.keys) {
            slot.store(pitch.unwrap_or(f32::NAN).to_bits(), Ordering::Relaxed);
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl PersistentField<'_, TuningFiles> for TuningParam {
    fn set(&self, files: TuningFiles) {
        // Files restored with the state made a tuning when they were saved. If they somehow
        // don't any more, the keys are tuned as usual, and the GUI says why
        let (tuning, error) = match files.tuning() {
            Ok(tuning) => (tuning, None),
            Err(error) => (Tuning::standard(), Some(error)),
        };
        *self.files.write().unwrap() = files;
        *self.error.lock().unwrap() = error;
        self.store(&tuning);
    }
    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&TuningFiles) -> R,
    {
        f(&self.files.read().unwrap())
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

/// How the oscillators read their wavetables in between samples, see [`Interpolation`].
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpolationParam {
    #[id = "none"]
    None,
    #[id = "linear"]
    Linear,
    #[id = "cubic"]
    Cubic,
}

impl InterpolationParam {
    pub fn to_interpolation(self) -> Interpolation {
        match self {
            InterpolationParam::None => Interpolation::None,
            InterpolationParam::Linear => Interpolation::Linear,
            InterpolationParam::Cubic => Interpolation::Cubic,
        }
    }
}

/// How often the modulation is recalculated, see [`ModulationRate`].
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulationRateParam {
    #[id = "block"]
    #[name = "Block"]
    Block,
    #[id = "sample"]
    #[name = "Sample"]
    Sample,
}

impl ModulationRateParam {
    pub fn to_modulation_rate(self) -> ModulationRate {
        match self {
            ModulationRateParam::Block => ModulationRate::Block,
            ModulationRateParam::Sample => ModulationRate::Sample,
        }
    }
}

/// How many times the sample rate the voices run at, see [`Oversampling`].
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OversamplingParam {
    #[id = "off"]
    Off,
    #[id = "2x"]
    #[name = "2x"]
    Two,
    #[id = "4x"]
    #[name = "4x"]
    Four,
    #[id = "8x"]
    #[name = "8x"]
    Eight,
}

impl OversamplingParam {
    pub fn to_oversampling(self) -> Oversampling {
        match self {
            OversamplingParam::Off => Oversampling::Off,
            OversamplingParam::Two => Oversampling::Two,
            OversamplingParam::Four => Oversampling::Four,
            OversamplingParam::Eight => Oversampling::Eight,
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

/// Where the synth's settings are read from: the parameters' current values, or (to test
/// presets) the values a preset sets them to.
pub trait ParamValues {
    fn get<P: Param>(&self, param: &P) -> P::Plain;
}

/// The parameters' current values.
pub struct CurrentValues;

impl ParamValues for CurrentValues {
    fn get<P: Param>(&self, param: &P) -> P::Plain {
        param.modulated_plain_value()
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

    /// The wavetable files the oscillators play when their wave is set to File.
    #[persist = "osc1_file"]
    pub osc1_file: WavetableFile,
    #[persist = "osc2_file"]
    pub osc2_file: WavetableFile,
    #[persist = "osc3_file"]
    pub osc3_file: WavetableFile,

    /// The Scala files the keys are tuned with.
    #[persist = "tuning"]
    pub tuning: TuningParam,
    /// The name of the preset last loaded or saved, which the title bar shows.
    #[persist = "preset"]
    pub preset_name: RwLock<String>,

    #[nested(id_prefix = "fil1", group = "Filter 1")]
    pub filter: FilterParams,
    #[nested(id_prefix = "fil1", group = "Filter 1 Env.")]
    pub filter_env: FilterEnvParams,

    #[nested(id_prefix = "vol", group = "Volume Env.")]
    pub vol_env: EnvelopeParams,

    #[nested(array, group = "LFO")]
    pub lfos: [LfoParams; N_LFOS],

    /// The envelopes that are only there to be modulation sources.
    #[nested(array, group = "Env")]
    pub mod_envs: [ModEnvParams; N_MOD_ENVELOPES],

    /// The modulation matrix.
    #[nested(array, group = "Mod")]
    pub mod_slots: [ModSlotParams; N_SLOTS],

    #[nested(group = "Global")]
    pub global: GlobalParams,

    /// The effects after the voices.
    #[nested(group = "FX")]
    pub fx: FxParams,
}

impl Default for ThreeOscParams {
    fn default() -> Self {
        Self {
            editor_state: crate::editor::default_state(),

            // The first oscillator defaults to an audible saw wave, the others are silent.
            osc1: OscParams::new(100.0, OscillatorWave::Saw),
            osc1_mod: ModParams::new("Osc 2"),
            osc2: OscParams::new(0.0, OscillatorWave::Sine),
            osc2_mod: ModParams::new("Osc 3"),
            osc3: OscParams::new(0.0, OscillatorWave::Sine),
            osc1_file: WavetableFile::default(),
            osc2_file: WavetableFile::default(),
            osc3_file: WavetableFile::default(),
            tuning: TuningParam::default(),
            preset_name: RwLock::new(String::from("Init")),

            filter: FilterParams::new(),
            filter_env: FilterEnvParams::new(),

            vol_env: EnvelopeParams::new(0.002, 0.25, 1.0, 0.005),

            lfos: std::array::from_fn(|_| LfoParams::new()),
            mod_envs: std::array::from_fn(|_| ModEnvParams::new()),
            mod_slots: std::array::from_fn(|_| ModSlotParams::new()),

            global: GlobalParams::new(),
            fx: FxParams::new(),
        }
    }
}

impl ThreeOscParams {
    /// An oscillator's wavetable file, by oscillator index.
    pub fn wavetable_file(&self, osc: usize) -> &WavetableFile {
        [&self.osc1_file, &self.osc2_file, &self.osc3_file][osc]
    }
    /// An oscillator's controls, by oscillator index.
    pub fn oscillator(&self, osc: usize) -> &OscParams {
        [&self.osc1, &self.osc2, &self.osc3][osc]
    }
}

/// Controls shared by all three oscillators.
#[derive(Params)]
pub struct OscParams {
    #[id = "wave"]
    pub wave: EnumParam<OscillatorWave>,
    /// Position within the wavetable, 0 to 1. Morphs between frames for multi-frame wavetables.
    #[id = "position"]
    pub position: FloatParam,
    /// Oscillator output volume, in percent.
    #[id = "amp"]
    pub amp: FloatParam,
    /// Where the oscillator sits in the stereo field, from -1 (left) to 1 (right).
    #[id = "pan"]
    pub pan: FloatParam,
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
    pub fn new(default_amp: f32, default_wave: OscillatorWave) -> Self {
        Self {
            wave: EnumParam::new("Wave", default_wave),
            position: amount_param("Position", 0.0),
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
            pan: FloatParam::new("Pan", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 })
                .with_value_to_string(formatters::v2s_f32_panning())
                .with_string_to_value(formatters::s2v_f32_panning()),
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

/// A low frequency oscillator, which is a modulation source (see [`ModSlotParams`]).
#[derive(Params)]
pub struct LfoParams {
    #[id = "lfo_wave"]
    pub wave: EnumParam<Wave>,
    /// LFO frequency in Hz.
    #[id = "lfo_rate"]
    pub rate: FloatParam,
}

impl LfoParams {
    pub fn new() -> Self {
        Self {
            wave: EnumParam::new("Wave", Wave::Sine),
            rate: FloatParam::new(
                "Rate",
                5.0,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 100.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(2))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
        }
    }
}

impl Default for LfoParams {
    fn default() -> Self {
        Self::new()
    }
}

/// An envelope that is only a modulation source.
#[derive(Params)]
pub struct ModEnvParams {
    #[nested(id_prefix = "env")]
    pub envelope: EnvelopeParams,
}

impl ModEnvParams {
    pub fn new() -> Self {
        Self {
            envelope: EnvelopeParams::new(0.0, 0.5, 0.0, 0.1),
        }
    }
}

impl Default for ModEnvParams {
    fn default() -> Self {
        Self::new()
    }
}

/// One route of the modulation matrix: a source, a target, and how much of the source is
/// applied to the target (see [`crate::synth::modulation`]).
#[derive(Params)]
pub struct ModSlotParams {
    #[id = "mod_source"]
    pub source: EnumParam<ModSource>,
    #[id = "mod_target"]
    pub target: EnumParam<ModTarget>,
    /// From -100% to 100%, with finer control around zero.
    #[id = "mod_amount"]
    pub amount: FloatParam,
}

impl ModSlotParams {
    pub fn new() -> Self {
        Self {
            source: EnumParam::new("Source", ModSource::None),
            target: EnumParam::new("Target", ModTarget::None),
            amount: FloatParam::new(
                "Amount",
                0.0,
                FloatRange::SymmetricalSkewed {
                    min: -1.0,
                    max: 1.0,
                    factor: FloatRange::skew_factor(-1.0),
                    center: 0.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(v2s_signed_percentage())
            .with_string_to_value(formatters::s2v_f32_percentage()),
        }
    }
}

impl Default for ModSlotParams {
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
    /// How far unison voices are spread across the stereo field, 0 (all where their
    /// oscillator's pan puts them) to 1.
    #[id = "stereo_width"]
    pub stereo_width: FloatParam,
    /// Global pitch offset in semitones.
    #[id = "global_pitch"]
    pub global_pitch: IntParam,
    /// Pitch wheel range in semitones.
    #[id = "bend_range"]
    pub bend_range: IntParam,
    /// How the oscillators read their wavetables in between samples.
    #[id = "interpolation"]
    pub interpolation: EnumParam<InterpolationParam>,
    /// Whether the envelopes, LFOs and modulation are recalculated per control rate block or
    /// at every sample.
    #[id = "mod_rate"]
    pub modulation_rate: EnumParam<ModulationRateParam>,
    /// How many times the sample rate the voices run at.
    #[id = "oversampling"]
    pub oversampling: EnumParam<OversamplingParam>,
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
            stereo_width: amount_param("Stereo Width", 1.0),
            global_pitch: IntParam::new("Output Pitch", 0, IntRange::Linear { min: -24, max: 24 })
                .with_unit(" st"),
            bend_range: IntParam::new("Bend Range", 2, IntRange::Linear { min: -24, max: 24 })
                .with_unit(" st"),
            interpolation: EnumParam::new("Interpolation", InterpolationParam::Cubic),
            modulation_rate: EnumParam::new("Modulation Rate", ModulationRateParam::Block),
            oversampling: EnumParam::new("Oversampling", OversamplingParam::Off),
        }
    }
}

impl Default for GlobalParams {
    fn default() -> Self {
        Self::new()
    }
}

/// The effects after the voices, see [`crate::synth::effects`].
#[derive(Params)]
pub struct FxParams {
    #[nested(id_prefix = "dist", group = "Distortion")]
    pub distortion: DistortionParams,
    #[nested(id_prefix = "chorus", group = "Chorus")]
    pub chorus: ChorusParams,
    #[nested(id_prefix = "delay", group = "Delay")]
    pub delay: DelayParams,
    #[nested(id_prefix = "reverb", group = "Reverb")]
    pub reverb: ReverbParams,
}

impl FxParams {
    pub fn new() -> Self {
        let defaults = EffectSettings::default();
        Self {
            distortion: DistortionParams {
                enabled: BoolParam::new("Distortion", defaults.distortion.enabled),
                drive: FloatParam::new(
                    "Drive",
                    defaults.distortion.drive_db,
                    FloatRange::Linear {
                        min: 0.0,
                        max: effects::DRIVE_MAX_DB,
                    },
                )
                .with_unit(" dB")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
                mix: amount_param("Mix", defaults.distortion.mix),
            },
            chorus: ChorusParams {
                enabled: BoolParam::new("Chorus", defaults.chorus.enabled),
                rate: FloatParam::new(
                    "Rate",
                    defaults.chorus.rate,
                    FloatRange::Skewed {
                        min: effects::CHORUS_MIN_RATE,
                        max: effects::CHORUS_MAX_RATE,
                        factor: FloatRange::skew_factor(-1.5),
                    },
                )
                .with_value_to_string(formatters::v2s_f32_hz_then_khz(2))
                .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
                depth: amount_param("Depth", defaults.chorus.depth),
                mix: amount_param("Mix", defaults.chorus.mix),
            },
            delay: DelayParams {
                enabled: BoolParam::new("Delay", defaults.delay.enabled),
                time: FloatParam::new(
                    "Time",
                    defaults.delay.time,
                    FloatRange::Skewed {
                        min: effects::DELAY_MIN_SECONDS,
                        max: effects::DELAY_MAX_SECONDS,
                        factor: FloatRange::skew_factor(-1.5),
                    },
                )
                .with_value_to_string(v2s_seconds())
                .with_string_to_value(s2v_seconds()),
                feedback: FloatParam::new(
                    "Feedback",
                    defaults.delay.feedback,
                    FloatRange::Linear {
                        min: 0.0,
                        max: effects::FEEDBACK_MAX,
                    },
                )
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(1))
                .with_string_to_value(formatters::s2v_f32_percentage()),
                mix: amount_param("Mix", defaults.delay.mix),
                ping_pong: BoolParam::new("Ping-Pong", defaults.delay.ping_pong),
            },
            reverb: ReverbParams {
                enabled: BoolParam::new("Reverb", defaults.reverb.enabled),
                size: amount_param("Size", defaults.reverb.size),
                damping: amount_param("Damping", defaults.reverb.damping),
                mix: amount_param("Mix", defaults.reverb.mix),
            },
        }
    }

    /// The effects' settings, for the synth.
    pub fn settings(&self) -> EffectSettings {
        self.settings_from(&CurrentValues)
    }

    /// The effects' settings with the parameters' values as `values` has them.
    pub fn settings_from(&self, values: &impl ParamValues) -> EffectSettings {
        let (distortion, chorus, delay, reverb) = (&self.distortion, &self.chorus, &self.delay, &self.reverb);
        EffectSettings {
            distortion: effects::DistortionSettings {
                enabled: values.get(&distortion.enabled),
                drive_db: values.get(&distortion.drive),
                mix: values.get(&distortion.mix),
            },
            chorus: effects::ChorusSettings {
                enabled: values.get(&chorus.enabled),
                rate: values.get(&chorus.rate),
                depth: values.get(&chorus.depth),
                mix: values.get(&chorus.mix),
            },
            delay: effects::DelaySettings {
                enabled: values.get(&delay.enabled),
                time: values.get(&delay.time),
                feedback: values.get(&delay.feedback),
                mix: values.get(&delay.mix),
                ping_pong: values.get(&delay.ping_pong),
            },
            reverb: effects::ReverbSettings {
                enabled: values.get(&reverb.enabled),
                size: values.get(&reverb.size),
                damping: values.get(&reverb.damping),
                mix: values.get(&reverb.mix),
            },
        }
    }
}

impl Default for FxParams {
    fn default() -> Self {
        Self::new()
    }
}

/// Soft clipping, see [`effects::DistortionSettings`].
#[derive(Params)]
pub struct DistortionParams {
    #[id = "on"]
    pub enabled: BoolParam,
    /// In decibels.
    #[id = "drive"]
    pub drive: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
}

/// See [`effects::ChorusSettings`].
#[derive(Params)]
pub struct ChorusParams {
    #[id = "on"]
    pub enabled: BoolParam,
    /// In Hz.
    #[id = "rate"]
    pub rate: FloatParam,
    #[id = "depth"]
    pub depth: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
}

/// See [`effects::DelaySettings`].
#[derive(Params)]
pub struct DelayParams {
    #[id = "on"]
    pub enabled: BoolParam,
    /// In seconds.
    #[id = "time"]
    pub time: FloatParam,
    #[id = "feedback"]
    pub feedback: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "ping_pong"]
    pub ping_pong: BoolParam,
}

/// See [`effects::ReverbSettings`].
#[derive(Params)]
pub struct ReverbParams {
    #[id = "on"]
    pub enabled: BoolParam,
    #[id = "size"]
    pub size: FloatParam,
    #[id = "damping"]
    pub damping: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
}

/// A linear `0.0..=1.0` parameter displayed as a percentage.
fn amount_param(name: impl Into<String>, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_percentage(1))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

/// Formats a `-1.0..=1.0` value as a whole percentage, without a negative zero (which would
/// not round trip through the string conversions).
fn v2s_signed_percentage() -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    Arc::new(|value| format!("{:.0}", (value * 100.0).round() + 0.0))
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
    fn wavetable_file_loads_are_claimed_once_per_path() {
        let file = WavetableFile::default();
        assert_eq!(file.claim_load(), None);
        file.set_path(String::from("saw.wav"));
        assert_eq!(file.path(), "saw.wav");
        assert_eq!(file.claim_load(), Some(1));
        assert_eq!(file.claim_load(), None);
        PersistentField::set(&file, String::from("square.wav"));
        assert_eq!(PersistentField::map(&file, |path| path.clone()), "square.wav");
        assert_eq!(file.claim_load(), Some(2));
        file.set_status(String::from("ok"));
        assert_eq!(file.status(), "ok");
    }

    #[test]
    fn tunings_are_kept_with_the_files_that_make_them() {
        let tuning = TuningParam::default();
        let first = tuning.generation();
        assert_eq!(tuning.pitches()[69], 69.0);
        let scale = TextFile {
            name: String::from("quarter tones.scl"),
            text: String::from("Quarter tones\n24\n") + &(1..=24).map(|step| format!("{}.0\n", step * 50)).collect::<String>(),
        };
        let files = TuningFiles { scale: Some(scale), mapping: None };
        tuning.try_set(files.clone()).unwrap();
        assert_eq!(tuning.files(), files);
        assert_ne!(tuning.generation(), first);
        assert_eq!(tuning.pitches()[61], 60.5);

        // A broken file leaves the tuning as it was
        let broken = TuningFiles {
            mapping: Some(TextFile { name: String::from("x.kbm"), text: String::from("nonsense") }),
            ..files.clone()
        };
        assert!(tuning.try_set(broken.clone()).unwrap_err().starts_with("Mapping: "));
        assert_eq!(tuning.files(), files);
        assert_eq!(tuning.pitches()[61], 60.5);

        // ...but one restored with the state is kept, playing the usual tuning
        PersistentField::set(&tuning, broken.clone());
        assert_eq!(PersistentField::map(&tuning, |files| files.clone()), broken);
        assert_eq!(tuning.pitches()[61], 61.0);
        assert!(tuning.error().is_some());
        PersistentField::set(&tuning, TuningFiles::default());
        assert_eq!(tuning.error(), None);

        // Silent keys are NaN
        let white_keys = TextFile {
            name: String::from("white.kbm"),
            text: String::from("12\n0\n127\n60\n69\n440.0\n12\n0\nx\n2\nx\n4\n5\nx\n7\nx\n9\nx\n11\n"),
        };
        tuning.try_set(TuningFiles { scale: None, mapping: Some(white_keys) }).unwrap();
        assert!(tuning.pitches()[61].is_nan());
        assert_eq!(tuning.pitches()[69], 69.0);
    }

    #[test]
    fn signed_percentage_has_no_negative_zero() {
        let to_string = v2s_signed_percentage();
        assert_eq!(to_string(-0.004), "0");
        assert_eq!(to_string(-0.5), "-50");
        assert_eq!(to_string(0.996), "100");
        let from_string = formatters::s2v_f32_percentage();
        assert_eq!(from_string("-50 %"), Some(-0.5));
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
