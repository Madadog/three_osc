//! Three Osc: a polyphonic subtractive synthesizer with three oscillators that can modulate each
//! other in various ways.
//!
//! This crate is the [nih-plug](https://github.com/robbert-vdh/nih-plug) wrapper around the synth
//! engine in the [`synth`] module. It exports the synth as a CLAP and a VST3 plugin (and as a
//! standalone application when the `standalone` feature is enabled), with an egui GUI.

use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;

#[cfg(test)]
mod benchmark;
mod editor;
mod folders;
mod params;
mod presets;
mod synth;

use params::{
    multiplier_to_pitch_multiplier, CurrentValues, EnvelopeParams, ModParams, OscParams,
    OscillatorWave, ParamValues, ThreeOscParams,
};
use synth::envelopes::AdsrEnvelope;
use synth::modulation::{ModSource, ModTarget, Route};
use synth::monitor::Monitor;
use synth::oscillator::{OscillatorParams, WavetableFrames};
use synth::wavetable_file::{self, LoadedSpectra};
use synth::ThreeOsc;

/// The sample rate the synth is created with before the host tells us the real one in
/// [`Plugin::initialize()`].
const DEFAULT_SAMPLE_RATE: f64 = 44100.0;

pub struct ThreeOscPlugin {
    params: Arc<ThreeOscParams>,
    synth: ThreeOsc,
    loader: Loader,
    /// What the GUI shows of the notes that are playing.
    monitor: Arc<Monitor>,
    /// The generation of the tuning the synth plays (see [`params::TuningParam`]).
    tuning_generation: u32,
}

/// Work the plugin does off the audio thread, see [`Plugin::task_executor()`].
pub enum Task {
    /// Loads the wavetable file an oscillator is set to (the path at the given generation of
    /// its [`params::WavetableFile`]), generates its tables and hands them to the audio thread.
    LoadWavetable { osc: usize, generation: u32 },
    /// Frees something the audio thread is done with, so it isn't deallocated there.
    Dispose(Arc<dyn Any + Send + Sync>),
}

/// A loaded wavetable file on its way to the audio thread.
struct LoadedWavetable {
    osc: usize,
    generation: u32,
    /// The sample rate the tables were generated for.
    sample_rate: f32,
    result: LoadResult,
}

enum LoadResult {
    /// The file's spectra and the tables generated from them.
    Loaded(Arc<LoadedSpectra>, Arc<WavetableFrames>),
    /// The path was cleared.
    Cleared,
    /// The file couldn't be loaded; the GUI shows why.
    Failed,
}

/// One finished load per oscillator, waiting for the audio thread. A newer load replaces an
/// older one that wasn't picked up yet, which then gets freed on the background thread.
type LoadedSlots = [Mutex<Option<LoadedWavetable>>; 3];

/// The plugin's side of wavetable file loading: loaded files come back from the background
/// thread through [`LoadedSlots`], which the audio thread only ever `try_lock`s, and the
/// spectra of each loaded file are kept so its tables can be regenerated when the sample rate
/// changes. Nothing is allocated or freed on the audio thread: replaced tables and spectra go
/// back to the background thread to be dropped.
struct Loader {
    slots: Arc<LoadedSlots>,
    /// The current sample rate (as `f32` bits), for the background thread.
    sample_rate: Arc<AtomicU32>,
    spectra: [Option<Arc<LoadedSpectra>>; 3],
}

impl Default for ThreeOscPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(ThreeOscParams::default()),
            synth: ThreeOsc::new(DEFAULT_SAMPLE_RATE),
            loader: Loader {
                slots: Arc::new(Default::default()),
                sample_rate: Arc::new(AtomicU32::new((DEFAULT_SAMPLE_RATE as f32).to_bits())),
                spectra: [None, None, None],
            },
            monitor: Arc::new(Monitor::default()),
            tuning_generation: 0,
        }
    }
}

impl ThreeOscPlugin {
    /// Picks up the wavetable files the background thread has loaded and installs them in the
    /// synth, and starts loads for paths that changed (from the GUI, or a loaded preset).
    /// `execute` runs a task on the background thread.
    fn update_wavetable_files(&mut self, mut execute: impl FnMut(Task)) {
        for osc in 0..3 {
            // Never waits: if the background thread is storing a result right now, it is
            // picked up next time.
            let loaded = match self.loader.slots[osc].try_lock() {
                Ok(mut slot) => slot.take(),
                Err(_) => None,
            };
            if let Some(loaded) = loaded {
                self.install_wavetable_file(loaded, &mut execute);
            }
            if let Some(generation) = self.params.wavetable_file(osc).claim_load() {
                execute(Task::LoadWavetable { osc, generation });
            }
        }
    }

    /// Installs a finished load, if it is still wanted.
    fn install_wavetable_file(&mut self, loaded: LoadedWavetable, execute: &mut impl FnMut(Task)) {
        let LoadedWavetable {
            osc,
            generation,
            sample_rate,
            result,
        } = loaded;
        let current = generation == self.params.wavetable_file(osc).generation();
        let right_rate = sample_rate == self.synth.sample_rate as f32;
        match result {
            LoadResult::Loaded(spectra, frames) if current && right_rate => {
                if let Some(old) = self.loader.spectra[osc].replace(spectra) {
                    execute(Task::Dispose(old));
                }
                if let Some(old) = self.synth.user_waves[osc].replace(frames) {
                    execute(Task::Dispose(old));
                }
            }
            LoadResult::Cleared if current => {
                if let Some(old) = self.loader.spectra[osc].take() {
                    execute(Task::Dispose(old));
                }
                if let Some(old) = self.synth.user_waves[osc].take() {
                    execute(Task::Dispose(old));
                }
            }
            LoadResult::Loaded(spectra, frames) => {
                execute(Task::Dispose(spectra));
                execute(Task::Dispose(frames));
                if current {
                    // Generated for a sample rate that changed in the meantime: again
                    execute(Task::LoadWavetable { osc, generation });
                }
                // Otherwise the path changed again since, and a newer load is on its way
            }
            // A cleared path that changed again, or a failed load (the synth keeps what it
            // had, and the GUI shows the error)
            LoadResult::Cleared | LoadResult::Failed => {}
        }
    }

    /// Reads the current parameter values and applies them to the synth engine.
    ///
    /// This mirrors what the LV2 version did with its control ports at the start of every `run()`
    /// call, including the control scaling (percentages, squared modulation amounts, and so on).
    fn apply_params(&mut self) {
        self.apply_param_values(&CurrentValues);
    }

    /// Applies the parameters' values as `values` has them, see [`apply_params()`].
    fn apply_param_values(&mut self, values: &impl ParamValues) {
        let params = &self.params;
        let synth = &mut self.synth;

        // Global
        let global = &params.global;
        let output_gain = values.get(&global.output_gain);
        synth.output_volume = if output_gain > -90.0 {
            10.0_f32.powf(output_gain * 0.05)
        } else {
            0.0
        };
        synth.bend_range = values.get(&global.bend_range) as f32;
        synth.polyphony = values.get(&global.polyphony).to_polyphony();
        synth.oversampling = values.get(&global.oversampling).to_oversampling();
        // Scaling: This is a lerp per sample at the voices' rate, and must be proportional to it
        // ... the '0.002' is just user-friendly control scaling.
        let voice_rate = synth.sample_rate as f32 * synth.oversampling.factor() as f32;
        synth.portamento_rate = 1.0 - values.get(&global.portamento_rate).powf(0.002 * voice_rate / 44100.0);
        synth.portamento_offset = values.get(&global.pitch_offset);
        // multiplies delta: smaller = higher pitch
        synth.octave_detune = 1.0 - values.get(&global.octave_detune);
        synth.stereo_width = values.get(&global.stereo_width);
        synth.effect_settings = params.fx.settings_from(values);
        synth.interpolation = values.get(&global.interpolation).to_interpolation();
        synth.modulation_rate = values.get(&global.modulation_rate).to_modulation_rate();
        let tuning = params.tuning.generation();
        if tuning != self.tuning_generation {
            self.tuning_generation = tuning;
            synth.key_pitches = params.tuning.pitches();
        }

        // Master gain envelope
        apply_envelope_values(&mut synth.gain_envelope, &params.vol_env, values);

        // Filter
        let filter = &params.filter;
        let filter_env = &params.filter_env;
        let controller = &mut synth.filter_controller;
        controller.envelope_amount = values.get(&filter_env.amount).powi(2);
        controller.keytrack = values.get(&filter.keytrack);
        apply_envelope_values(&mut controller.cutoff_envelope, &filter_env.envelope, values);
        controller.target_cutoff = values.get(&filter.cutoff);
        controller.resonance = values.get(&filter.resonance);
        controller.drive = values.get(&filter.drive);
        controller.filter_type = values.get(&filter.filter_type).to_filter_type();
        controller.filter_model = values.get(&filter.model).to_filter_model();

        // Modulation: LFOs, envelopes and the matrix. The routes vector has room for every
        // slot, so this doesn't allocate.
        for (lfo, lfo_params) in synth.lfos.iter_mut().zip(params.lfos.iter()) {
            lfo.wave = values.get(&lfo_params.wave).to_osc_wave();
            lfo.freq = values.get(&lfo_params.rate);
        }
        for (envelope, env_params) in synth.mod_envelopes.iter_mut().zip(params.mod_envs.iter()) {
            apply_envelope_values(envelope, &env_params.envelope, values);
        }
        synth.routes.clear();
        for slot in params.mod_slots.iter() {
            let route = Route {
                source: values.get(&slot.source),
                target: values.get(&slot.target),
                amount: values.get(&slot.amount),
            };
            if route.source != ModSource::None && route.target != ModTarget::None && route.amount != 0.0 {
                synth.routes.push(route);
            }
        }

        // Oscillators
        let global_pitch = values.get(&global.global_pitch) as f32;
        apply_osc_params(&mut synth.oscillators[0], &params.osc1, global_pitch, values);
        apply_mod_params(&mut synth.oscillators[0], &params.osc1_mod, values);

        apply_osc_params(&mut synth.oscillators[1], &params.osc2, global_pitch, values);
        apply_mod_params(&mut synth.oscillators[1], &params.osc2_mod, values);

        // The third oscillator has nothing after it to modulate it.
        apply_osc_params(&mut synth.oscillators[2], &params.osc3, global_pitch, values);
    }

    fn handle_event(&mut self, event: NoteEvent<()>) {
        match event {
            NoteEvent::NoteOn { note, velocity, .. } => {
                self.synth.note_on(note, midi_velocity(velocity));
            }
            NoteEvent::NoteOff { note, velocity, .. } => {
                self.synth.note_off(note, midi_velocity(velocity));
            }
            NoteEvent::Choke { note, .. } => {
                self.synth.note_off(note, 0);
            }
            NoteEvent::MidiPitchBend { value, .. } => {
                // The synth expects the raw 14-bit MIDI pitch bend value, with 8192 as the centre.
                self.synth
                    .pitch_bend((value * 16383.0).round().clamp(0.0, 16383.0) as u16);
            }
            // Modulation wheel
            NoteEvent::MidiCC { cc: 1, value, .. } => self.synth.mod_wheel = value.clamp(0.0, 1.0),
            NoteEvent::MidiChannelPressure { pressure, .. } => {
                self.synth.aftertouch = pressure.clamp(0.0, 1.0)
            }
            NoteEvent::PolyPressure { note, pressure, .. } => {
                self.synth.poly_pressure(note, pressure)
            }
            _ => (),
        }
    }

    /// Renders a buffer of audio into the left and right output channels, applying `events`
    /// (sorted by their timing) sample accurately: the buffer is rendered in chunks, with the
    /// events that fall on a chunk's first sample applied before it is rendered.
    fn render(
        &mut self,
        mut events: impl Iterator<Item = NoteEvent<()>>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let num_samples = out_l.len().min(out_r.len());
        // The synth adds its voices onto whatever is in the output buffers.
        out_l.fill(0.0);
        out_r.fill(0.0);

        let mut next_event = events.next();
        let mut block_start = 0;
        while block_start < num_samples {
            while let Some(event) = next_event {
                if event.timing() as usize > block_start {
                    break;
                }
                self.handle_event(event);
                next_event = events.next();
            }

            let block_end = next_event
                .map(|event| event.timing() as usize)
                .unwrap_or(num_samples)
                .clamp(block_start + 1, num_samples);

            self.synth.run(
                &mut out_l[block_start..block_end],
                &mut out_r[block_start..block_end],
            );
            block_start = block_end;
        }

        // Events at or past the end of the buffer are still applied, so the next buffer starts
        // with the right notes playing.
        while let Some(event) = next_event {
            self.handle_event(event);
            next_event = events.next();
        }
    }
}

/// Applies the controls shared by all oscillators.
fn apply_osc_params(osc: &mut OscillatorParams, params: &OscParams, global_pitch: f32, values: &impl ParamValues) {
    let wave = values.get(&params.wave);
    osc.use_file = wave == OscillatorWave::File;
    osc.wavetable = wave.wavetable_index().unwrap_or(0);
    osc.position = values.get(&params.position);
    osc.amp = values.get(&params.amp) / 100.0;
    osc.pan = values.get(&params.pan);
    osc.semitone = values.get(&params.semitone) + global_pitch;
    osc.octave = values.get(&params.octave);
    osc.pitch_multiplier = multiplier_to_pitch_multiplier(values.get(&params.multiplier));
    osc.voice_count = values.get(&params.voices) as u8;
    // The synth cubes the detune amount, after any modulation
    osc.voices_detune = values.get(&params.super_detune) / 100.0;
    osc.phase = values.get(&params.phase) / 100.0;
    osc.phase_rand = values.get(&params.phase_rand) / 100.0;
}

/// Applies the PM/FM/AM amounts for an oscillator that has a modulator after it. The synth
/// squares them, after any modulation.
fn apply_mod_params(osc: &mut OscillatorParams, params: &ModParams, values: &impl ParamValues) {
    osc.pm = values.get(&params.pm);
    osc.fm = values.get(&params.fm);
    osc.am = values.get(&params.am);
}

fn apply_envelope_params(envelope: &mut AdsrEnvelope, params: &EnvelopeParams) {
    apply_envelope_values(envelope, params, &CurrentValues);
}

fn apply_envelope_values(envelope: &mut AdsrEnvelope, params: &EnvelopeParams, values: &impl ParamValues) {
    envelope.attack_time = values.get(&params.attack);
    envelope.decay_time = values.get(&params.decay);
    envelope.sustain_level = values.get(&params.sustain);
    envelope.release_time = values.get(&params.release);
    envelope.set_slope(values.get(&params.slope));
}

/// Converts nih-plug's `[0, 1]` velocity to the 7-bit MIDI velocity the synth engine uses.
fn midi_velocity(velocity: f32) -> u8 {
    (velocity * 127.0).round().clamp(0.0, 127.0) as u8
}

impl Plugin for ThreeOscPlugin {
    const NAME: &'static str = "Three Osc";
    const VENDOR: &'static str = "Adam Godwin";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "evilspamalt@gmail.com";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    // `MidiCCs` rather than `Basic` so that pitch bend events are delivered.
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = Task;

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let params = self.params.clone();
        let slots = self.loader.slots.clone();
        let sample_rate = self.loader.sample_rate.clone();
        Box::new(move |task| match task {
            Task::LoadWavetable { osc, generation } => {
                let file = params.wavetable_file(osc);
                let path = file.path();
                let sample_rate = f32::from_bits(sample_rate.load(Ordering::Relaxed));
                let (result, status) = if path.is_empty() {
                    file.set_preview(None);
                    (LoadResult::Cleared, String::new())
                } else {
                    match LoadedSpectra::load(&wavetable_file::resolve(&path)) {
                        Ok(spectra) => {
                            let frames = Arc::new(spectra.generate(sample_rate));
                            let status = spectra.describe(&frames);
                            file.set_preview(Some(Arc::new(spectra.preview())));
                            (LoadResult::Loaded(Arc::new(spectra), frames), status)
                        }
                        // The synth keeps playing what it had, and so does the preview
                        Err(error) => (LoadResult::Failed, format!("Error: {error}")),
                    }
                };
                file.set_status(status);
                let loaded = LoadedWavetable {
                    osc,
                    generation,
                    sample_rate,
                    result,
                };
                // A result the audio thread hasn't picked up yet is replaced, and dropped here
                // once the lock is released
                let replaced = slots[osc]
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .replace(loaded);
                drop(replaced);
            }
            Task::Dispose(_) => {}
        })
    }

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.loader.sample_rate.clone(),
            self.monitor.clone(),
            async_executor,
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        // The synth's wavetables are generated for a specific sample rate, so the synth needs to
        // be rebuilt when the sample rate changes. This may be called several times in a row, so
        // avoid the (fairly expensive) rebuild when nothing changed.
        let sample_rate = buffer_config.sample_rate as f64;
        self.loader
            .sample_rate
            .store((sample_rate as f32).to_bits(), Ordering::Relaxed);
        if self.synth.sample_rate != sample_rate {
            self.synth = ThreeOsc::new(sample_rate);
            // Loaded wavetable files are regenerated for the new rate right here, since
            // initialising may block.
            for (osc, spectra) in self.loader.spectra.iter().enumerate() {
                if let Some(spectra) = spectra {
                    self.synth.user_waves[osc] = Some(Arc::new(spectra.generate(sample_rate as f32)));
                }
            }
        }
        // Wavetable files restored with the plugin's state are loaded before processing starts,
        // so offline renders have them from the first sample: the first pass runs the loads on
        // this thread, the second installs them.
        self.update_wavetable_files(|task| context.execute(task));
        self.update_wavetable_files(|task| context.execute(task));

        true
    }

    fn reset(&mut self) {
        self.synth.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        self.apply_params();
        self.update_wavetable_files(|task| context.execute_background(task));

        let (out_l, out_r) = match buffer.as_slice() {
            [out_l, out_r, ..] => (out_l, out_r),
            // Only a stereo layout is offered, so this shouldn't happen.
            _ => return ProcessStatus::Error("expected a stereo output"),
        };
        self.render(std::iter::from_fn(|| context.next_event()), out_l, out_r);
        if self.params.editor_state.is_open() {
            self.synth.publish(&self.monitor);
            self.monitor.output.write(out_l, out_r);
        }

        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for ThreeOscPlugin {
    const CLAP_ID: &'static str = "com.github.madadog.three_osc";
    const CLAP_DESCRIPTION: Option<&'static str> = Some(env!("CARGO_PKG_DESCRIPTION"));
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::Instrument,
        ClapFeature::Synthesizer,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for ThreeOscPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"ThreeOscMadadog!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

nih_export_clap!(ThreeOscPlugin);
nih_export_vst3!(ThreeOscPlugin);

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a note with the default patch and returns the first non-finite sample, if any.
    fn first_non_finite_sample(
        plugin: &mut ThreeOscPlugin,
        note: u8,
        samples: usize,
    ) -> Option<(usize, f32)> {
        plugin.synth.reset();
        plugin.apply_params();
        plugin.synth.note_on(note, 100);

        let mut left = vec![0.0; samples];
        let mut right = vec![0.0; samples];
        plugin.synth.run(&mut left, &mut right);

        left.iter()
            .chain(right.iter())
            .enumerate()
            .find(|(_, sample)| !sample.is_finite())
            .map(|(index, sample)| (index % samples, *sample))
    }

    #[test]
    fn default_patch_renders_finite_audio() {
        let mut failures = Vec::new();
        for sample_rate in [8000.0, 22050.0, 44100.0, 48000.0, 96000.0, 192000.0] {
            // Generating the wavetables is the expensive part, so one synth is shared per rate.
            let mut plugin = ThreeOscPlugin {
                synth: ThreeOsc::new(sample_rate),
                ..Default::default()
            };
            for note in [0, 36, 60, 96, 127] {
                if let Some((index, sample)) = first_non_finite_sample(&mut plugin, note, 4096) {
                    failures.push(format!(
                        "{sample_rate} Hz, note {note}: sample {index} is {sample}"
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "non-finite output:\n{}", failures.join("\n"));
    }

    #[test]
    fn wavetable_order_matches_the_wave_parameter() {
        let set = synth::oscillator::WavetableSet::new(44100.0, 64);
        let names: Vec<&str> = set.wavetables.iter().map(|wavetable| wavetable.name.as_str()).collect();
        // The last wave is the loaded file
        let variants = params::OscillatorWave::variants();
        assert_eq!(names, &variants[..variants.len() - 1]);
        assert_eq!(variants.last(), Some(&"File"));
        assert_eq!(OscillatorWave::File.wavetable_index(), None);
        assert_eq!(OscillatorWave::Saw.wavetable_index(), Some(2));
        assert_eq!(
            OscillatorWave::BasicShapes.wavetable_index(),
            Some(synth::oscillator::BASIC_SHAPES_WAVETABLE)
        );
    }

    /// Silent oscillators are skipped, unless they modulate an audible oscillator.
    #[test]
    fn silent_oscillators_only_matter_when_they_modulate() {
        let mut plugin = ThreeOscPlugin::default();
        let render = |plugin: &mut ThreeOscPlugin, osc2_amp: f32, osc1_fm: f32| {
            plugin.synth.reset();
            plugin.apply_params();
            for osc in plugin.synth.oscillators.iter_mut() {
                osc.phase_rand = 0.0;
            }
            plugin.synth.oscillators[1].amp = osc2_amp;
            plugin.synth.oscillators[1].semitone = 7.0;
            plugin.synth.oscillators[0].fm = osc1_fm;
            plugin.synth.note_on(60, 100);
            let mut left = vec![0.0; 1024];
            let mut right = vec![0.0; 1024];
            plugin.synth.run(&mut left, &mut right);
            left
        };

        let reference = render(&mut plugin, 0.0, 0.0);
        // Making osc 2 audible changes the output
        assert!(max_abs_diff(&render(&mut plugin, 1.0, 0.0), &reference) > 1e-3);
        // A silent osc 2 that frequency modulates osc 1 changes the output too
        assert!(max_abs_diff(&render(&mut plugin, 0.0, 0.1), &reference) > 1e-3);
        // Without modulation the silent oscillator doesn't change anything
        assert_eq!(max_abs_diff(&render(&mut plugin, 0.0, 0.0), &reference), 0.0);
    }

    /// A note that is started and choked in the same instant samples the envelopes at index 0.
    /// With a zero length attack this used to divide zero by zero and poison the filter with NaN.
    #[test]
    fn zero_length_note_renders_finite_audio() {
        let mut plugin = ThreeOscPlugin::default();
        plugin.apply_params();
        plugin.synth.gain_envelope.attack_time = 0.0;
        plugin.synth.gain_envelope.decay_time = 0.0;
        plugin.synth.filter_controller.cutoff_envelope.attack_time = 0.0;
        plugin.synth.filter_controller.cutoff_envelope.decay_time = 0.0;

        let events = [
            NoteEvent::NoteOn {
                timing: 10,
                voice_id: None,
                channel: 0,
                note: 60,
                velocity: 0.8,
            },
            NoteEvent::Choke {
                timing: 10,
                voice_id: None,
                channel: 0,
                note: 60,
            },
            NoteEvent::NoteOn {
                timing: 20,
                voice_id: None,
                channel: 0,
                note: 64,
                velocity: 0.8,
            },
        ];
        let mut left = vec![0.0; 512];
        let mut right = vec![0.0; 512];
        plugin.render(events.into_iter(), &mut left, &mut right);

        assert!(left.iter().chain(right.iter()).all(|sample| sample.is_finite()));
        assert!(left.iter().any(|sample| *sample != 0.0), "the second note should be audible");
    }

    /// Every interpolation mode renders finite audio, through both the scalar (1 voice) and the
    /// SIMD (8 voices) lookups, and the modes differ from each other.
    #[test]
    fn interpolation_modes_render_finite_and_distinct_audio() {
        use crate::synth::oscillator::Interpolation;

        let mut plugin = ThreeOscPlugin::default();
        assert_eq!(
            plugin.params.global.interpolation.value().to_interpolation(),
            Interpolation::Cubic
        );
        let mut render = |interpolation: Interpolation, voices: u8| {
            plugin.synth.reset();
            plugin.apply_params();
            plugin.synth.interpolation = interpolation;
            for osc in plugin.synth.oscillators.iter_mut() {
                osc.voice_count = voices;
                osc.phase_rand = 0.0;
            }
            plugin.synth.note_on(84, 100);
            let mut left = vec![0.0; 2048];
            let mut right = vec![0.0; 2048];
            plugin.synth.run(&mut left, &mut right);
            assert!(left.iter().all(|sample| sample.is_finite()), "{interpolation:?}");
            assert!(left.iter().any(|sample| sample.abs() > 0.01), "{interpolation:?}");
            left
        };
        for voices in [1, 8] {
            let none = render(Interpolation::None, voices);
            let linear = render(Interpolation::Linear, voices);
            let cubic = render(Interpolation::Cubic, voices);
            assert!(max_abs_diff(&none, &linear) > 1e-4, "{voices} voices");
            assert!(max_abs_diff(&linear, &cubic) > 1e-6, "{voices} voices");
            // The modes only differ in between the table samples, so they stay close
            assert!(max_abs_diff(&none, &cubic) < 0.1, "{voices} voices");
        }
    }

    /// Renders a note with the default patch plus the given modulation routes, with
    /// deterministic phases and random source.
    fn render_with_routes(plugin: &mut ThreeOscPlugin, routes: &[Route], note: u8) -> Vec<f32> {
        plugin.synth.reset();
        plugin.apply_params();
        plugin.synth.routes.clear();
        plugin.synth.routes.extend_from_slice(routes);
        for osc in plugin.synth.oscillators.iter_mut() {
            osc.phase_rand = 0.0;
        }
        plugin.synth.note_on(note, 100);
        let mut left = vec![0.0; 4096];
        let mut right = vec![0.0; 4096];
        plugin.synth.run(&mut left, &mut right);
        assert!(left.iter().all(|sample| sample.is_finite()));
        left
    }

    /// Modulation routes change the sound, and routes with no effect don't.
    #[test]
    fn modulation_routes_change_the_sound() {
        let mut plugin = ThreeOscPlugin::default();
        let route = |source, target, amount| Route {
            source,
            target,
            amount,
        };
        let reference = render_with_routes(&mut plugin, &[], 60);

        // A route to nothing, from nothing, or to a silent oscillator's position leaves the
        // sound alone
        for routes in [
            vec![route(ModSource::Lfo1, ModTarget::None, 1.0)],
            vec![route(ModSource::None, ModTarget::Cutoff, 1.0)],
            vec![route(ModSource::Lfo1, ModTarget::Osc3Position, 1.0)],
        ] {
            assert_eq!(max_abs_diff(&render_with_routes(&mut plugin, &routes, 60), &reference), 0.0);
        }
        // Every kind of target on the audible oscillator changes the sound (with a negative
        // amount, since the LFO starts with its positive half and the amplitude is already at
        // its maximum)
        for target in [
            ModTarget::Pitch,
            ModTarget::FinePitch,
            ModTarget::Volume,
            ModTarget::Osc1Pitch,
            ModTarget::Osc1Amp,
            ModTarget::Cutoff,
        ] {
            let modulated =
                render_with_routes(&mut plugin, &[route(ModSource::Lfo1, target, -0.5)], 60);
            assert!(max_abs_diff(&modulated, &reference) > 1e-3, "{target:?}");
        }
        // Keytracking is zero at middle C, so it changes nothing there and something elsewhere
        let key = [route(ModSource::Key, ModTarget::Pitch, 0.5)];
        assert_eq!(max_abs_diff(&render_with_routes(&mut plugin, &key, 60), &reference), 0.0);
        let reference_72 = render_with_routes(&mut plugin, &[], 72);
        assert!(max_abs_diff(&render_with_routes(&mut plugin, &key, 72), &reference_72) > 1e-3);
        // The mod wheel and aftertouch only matter once they're moved
        for source in [ModSource::ModWheel, ModSource::Aftertouch] {
            let routes = [route(source, ModTarget::Cutoff, -0.8)];
            plugin.synth.mod_wheel = 0.0;
            plugin.synth.aftertouch = 0.0;
            assert_eq!(max_abs_diff(&render_with_routes(&mut plugin, &routes, 60), &reference), 0.0);
            plugin.synth.mod_wheel = 1.0;
            plugin.synth.aftertouch = 1.0;
            assert!(max_abs_diff(&render_with_routes(&mut plugin, &routes, 60), &reference) > 1e-3);
        }
        plugin.synth.mod_wheel = 0.0;
        plugin.synth.aftertouch = 0.0;
    }

    /// A modulated amplitude makes a silent oscillator audible, and modulated PM makes a silent
    /// modulator matter.
    #[test]
    fn modulation_can_switch_oscillators_on() {
        let mut plugin = ThreeOscPlugin::default();
        let reference = render_with_routes(&mut plugin, &[], 60);
        let amp = Route {
            source: ModSource::Velocity,
            target: ModTarget::Osc2Amp,
            amount: 1.0,
        };
        assert!(max_abs_diff(&render_with_routes(&mut plugin, &[amp], 60), &reference) > 1e-3);
        let pm = Route {
            source: ModSource::Env1,
            target: ModTarget::Osc1Pm,
            amount: 0.3,
        };
        assert!(max_abs_diff(&render_with_routes(&mut plugin, &[pm], 60), &reference) > 1e-3);
    }

    /// MIDI control events reach the modulation sources.
    #[test]
    fn midi_control_events_reach_the_sources() {
        let mut plugin = ThreeOscPlugin::default();
        plugin.apply_params();
        let events = [
            NoteEvent::NoteOn {
                timing: 0,
                voice_id: None,
                channel: 0,
                note: 60,
                velocity: 0.8,
            },
            NoteEvent::MidiCC {
                timing: 1,
                channel: 0,
                cc: 1,
                value: 0.75,
            },
            NoteEvent::MidiChannelPressure {
                timing: 2,
                channel: 0,
                pressure: 0.5,
            },
            NoteEvent::PolyPressure {
                timing: 3,
                voice_id: None,
                channel: 0,
                note: 60,
                pressure: 0.25,
            },
            // Other controllers are ignored
            NoteEvent::MidiCC {
                timing: 4,
                channel: 0,
                cc: 7,
                value: 0.1,
            },
        ];
        let mut left = vec![0.0; 64];
        let mut right = vec![0.0; 64];
        plugin.render(events.into_iter(), &mut left, &mut right);
        assert_eq!(plugin.synth.mod_wheel, 0.75);
        assert_eq!(plugin.synth.aftertouch, 0.5);
        assert_eq!(plugin.synth.voices.len(), 1);
    }

    /// A wavetable file set for an oscillator is loaded on the background thread, installed on
    /// the audio thread, and played when the oscillator's wave is set to File.
    #[test]
    fn wavetable_files_are_loaded_and_played() {
        use std::f32::consts::PI;

        // A file with one cycle of a 5th harmonic, so it's nothing like the built in waves
        let dir = std::env::temp_dir().join(format!("three_osc_plugin_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cycle: Vec<f32> = (0..1024).map(|i| (2.0 * PI * 5.0 * i as f32 / 1024.0).sin()).collect();
        let path = dir.join("fifth.wav");
        std::fs::write(&path, synth::wav::write_float(44100, 1, &cycle, Some(1024))).unwrap();

        let mut plugin = ThreeOscPlugin::default();
        let executor = plugin.task_executor();
        let render = |plugin: &mut ThreeOscPlugin| {
            plugin.synth.reset();
            plugin.apply_params();
            plugin.synth.oscillators[0].use_file = true;
            plugin.synth.oscillators[0].phase_rand = 0.0;
            plugin.synth.filter_controller.filter_model = synth::filter::FilterModel::None;
            plugin.synth.note_on(60, 100);
            let mut left = vec![0.0; 2048];
            let mut right = vec![0.0; 2048];
            plugin.synth.run(&mut left, &mut right);
            left
        };
        // Nothing loaded yet: the file wave falls back to the first built in wave
        let fallback = render(&mut plugin);
        assert_eq!(plugin.synth.oscillator_frames(0).name, "Sine");

        // Set the path, as the GUI or a preset would, and run the load the way the host does
        let params = plugin.params.clone();
        let file = params.wavetable_file(0);
        file.set_path(path.to_string_lossy().into_owned());
        let mut tasks = Vec::new();
        plugin.update_wavetable_files(|task| tasks.push(task));
        assert!(matches!(tasks.as_slice(), [Task::LoadWavetable { osc: 0, generation: 1 }]));
        // A load is only started once per path
        let mut more = Vec::new();
        plugin.update_wavetable_files(|task| more.push(task));
        assert!(more.is_empty());
        for task in tasks.drain(..) {
            executor(task);
        }
        assert!(file.status().starts_with("1 frame of 1024 samples"), "{}", file.status());

        // The next process call installs it, with nothing to dispose of yet
        plugin.update_wavetable_files(|task| tasks.push(task));
        assert!(tasks.is_empty());
        assert_eq!(plugin.synth.oscillator_frames(0).name, "fifth.wav");
        let loaded = render(&mut plugin);
        assert!(max_abs_diff(&loaded, &fallback) > 0.05);
        // It plays at the 5th harmonic of middle C
        let cycle_samples = 44100.0 / (440.0 * 2.0_f32.powf(-9.0 / 12.0)) / 5.0;
        let zero_crossings = loaded.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        let expected = 2.0 * 2048.0 / cycle_samples;
        assert!((zero_crossings as f32 - expected).abs() < 4.0, "{zero_crossings} vs {expected}");

        // A missing file reports an error and keeps the loaded wavetable
        file.set_path(String::from("missing.wav"));
        plugin.update_wavetable_files(|task| tasks.push(task));
        for task in tasks.drain(..) {
            executor(task);
        }
        assert!(file.status().starts_with("Error"), "{}", file.status());
        plugin.update_wavetable_files(|task| tasks.push(task));
        assert!(tasks.is_empty());
        assert_eq!(plugin.synth.oscillator_frames(0).name, "fifth.wav");

        // Clearing the path unloads it, and the old tables are disposed of in the background
        file.set_path(String::new());
        plugin.update_wavetable_files(|task| tasks.push(task));
        for task in tasks.drain(..) {
            executor(task);
        }
        plugin.update_wavetable_files(|task| tasks.push(task));
        // Both the spectra and the tables
        assert!(matches!(tasks.as_slice(), [Task::Dispose(_), Task::Dispose(_)]));
        assert_eq!(plugin.synth.oscillator_frames(0).name, "Sine");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Parameter IDs must be unique for the host to tell the parameters apart.
    #[test]
    fn parameter_ids_are_unique() {
        let params = ThreeOscParams::default();
        let mut ids: Vec<String> = params.param_map().into_iter().map(|(id, _, _)| id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
        assert!(ids.contains(&String::from("mod_source_16")));
        assert!(ids.contains(&String::from("lfo_rate_3")));
        assert!(ids.contains(&String::from("env_attack_2")));
    }

    /// Every factory preset plays: a chord held for three seconds and let go, rendered through
    /// the synth as the preset sets it up. Prints each one's levels (with `--nocapture`), to
    /// keep them about as loud as each other.
    #[test]
    fn factory_presets_play_a_chord() {
        let mut plugin = ThreeOscPlugin::default();
        let sample_rate = plugin.synth.sample_rate as usize;
        let chord = [48, 55, 60, 64];
        let mut problems = Vec::new();
        for preset in presets::factory() {
            plugin.synth.reset();
            plugin.apply_param_values(&presets::PresetValues::new(&preset, &plugin.params));
            fastrand::seed(1);
            let held = sample_rate * 3;
            let mut left = vec![0.0; held + sample_rate * 2];
            let mut right = left.clone();
            for note in chord {
                plugin.synth.note_on(note, 100);
            }
            plugin.synth.run(&mut left[..held], &mut right[..held]);
            for note in chord {
                plugin.synth.note_off(note, 0);
            }
            plugin.synth.run(&mut left[held..], &mut right[held..]);

            let peak = left.iter().chain(&right).fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            // The loudest 300 ms
            let window = sample_rate * 3 / 10;
            let loudest = (0..left.len() - window)
                .step_by(window / 3)
                .map(|start| {
                    let power: f32 = (start..start + window).map(|i| left[i] * left[i] + right[i] * right[i]).sum();
                    (power / (2 * window) as f32).sqrt()
                })
                .fold(0.0, f32::max);
            let db = |level: f32| 20.0 * level.max(1e-9).log10();
            println!("{:<16} peak {:6.1} dB, loudest {:6.1} dB RMS", preset.name, db(peak), db(loudest));
            if !left.iter().chain(&right).all(|sample| sample.is_finite()) {
                problems.push(format!("{} isn't finite", preset.name));
            } else if peak > 0.9 {
                problems.push(format!("{} peaks at {peak}", preset.name));
            } else if db(loudest) < -30.0 {
                problems.push(format!("{} is too quiet ({} dB)", preset.name, db(loudest)));
            }
        }
        assert!(problems.is_empty(), "{problems:?}");
    }

    /// Sets the plugin up with the parameters a preset of `params` gives, at an oversampling.
    fn set_up(plugin: &mut ThreeOscPlugin, params: &[(&str, serde_json::Value)], oversampling: &str) {
        let mut preset = presets::Preset::default();
        for (id, value) in params {
            preset.params.insert(id.to_string(), value.clone());
        }
        // Presets leave the oversampling alone, so it's set apart from them
        let ParamPtr::EnumParam(inner) = plugin.params.global.oversampling.as_ptr() else { unreachable!() };
        assert!(unsafe { (*inner).set_from_id(oversampling) }, "{oversampling}");
        plugin.synth.reset();
        plugin.apply_param_values(&presets::PresetValues::new(&preset, &plugin.params));
        fastrand::seed(3);
    }

    /// Renders a note for `seconds`, letting go of it `held` seconds in.
    fn render_note(plugin: &mut ThreeOscPlugin, note: u8, held: f32, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let rate = plugin.synth.sample_rate as f32;
        let mut left = vec![0.0; (seconds * rate) as usize];
        let mut right = left.clone();
        let release = ((held * rate) as usize).min(left.len());
        plugin.synth.note_on(note, 100);
        plugin.synth.run(&mut left[..release], &mut right[..release]);
        plugin.synth.note_off(note, 0);
        plugin.synth.run(&mut left[release..], &mut right[release..]);
        (left, right)
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// A plain note sounds the same at every oversampling: its level, its pitch (counted in
    /// zero crossings) and its envelope's timing.
    #[test]
    fn oversampling_leaves_plain_notes_alone() {
        let mut plugin = ThreeOscPlugin::default();
        let patch = [
            ("osc1_wave", serde_json::Value::from("triangle")),
            ("osc1_phase_rand", serde_json::Value::from(0.0)),
            ("vol_decay", serde_json::Value::from(0.3)),
            ("vol_sustain", serde_json::Value::from(0.5)),
            ("vol_release", serde_json::Value::from(0.1)),
        ];
        let rate = plugin.synth.sample_rate as usize;
        let measure = |plugin: &mut ThreeOscPlugin, oversampling: &str| {
            set_up(plugin, &patch, oversampling);
            let (left, right) = render_note(plugin, 69, 0.5, 0.7);
            assert_eq!(left, right);
            let window = |from: f32| rms(&left[(from * rate as f32) as usize..][..rate / 20]);
            let crossings = left[rate / 10..rate / 2].windows(2).filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0).count();
            // Decaying, sustaining, released, and gone
            ([window(0.05), window(0.25), window(0.45), window(0.6)], crossings, plugin.synth.voices.len())
        };
        let (levels, crossings, voices) = measure(&mut plugin, "off");
        assert_eq!(crossings, 176);
        // (Released, it goes at the start of the next buffer)
        assert_eq!(voices, 1);
        assert!(levels[3] < 1e-6);
        for oversampling in ["2x", "4x", "8x"] {
            let (oversampled, oversampled_crossings, oversampled_voices) = measure(&mut plugin, oversampling);
            assert_eq!((oversampled_crossings, oversampled_voices), (crossings, voices), "{oversampling}");
            for (level, oversampled) in levels[..3].iter().zip(oversampled) {
                let db = 20.0 * (oversampled / level).log10();
                assert!(db.abs() < 0.05, "{oversampling}: {levels:?} vs {oversampled:?}");
            }
            assert!(oversampled[3] < 1e-6);
        }
    }

    /// The share of a note's output that isn't at a harmonic of the note (or at 0 Hz, where
    /// FM's and AM's sidebands can land too), in dB: what aliasing folds back in between them.
    fn inharmonic_db(samples: &[f32], frequency: f32, sample_rate: f32) -> f32 {
        let len = samples.len().next_power_of_two() / 2;
        let samples = &samples[samples.len() - len..];
        let fft = rustfft::FftPlanner::new().plan_fft_forward(len);
        // A Blackman-Harris window, whose leakage is 92 dB down
        let mut bins: Vec<rustfft::num_complex::Complex32> = samples
            .iter()
            .enumerate()
            .map(|(n, sample)| {
                let x = std::f32::consts::TAU * n as f32 / len as f32;
                let window = 0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos();
                rustfft::num_complex::Complex32::new(sample * window, 0.0)
            })
            .collect();
        fft.process(&mut bins);
        let bin_hz = sample_rate / len as f32;
        let (mut harmonic, mut other) = (0.0, 0.0);
        for (bin, value) in bins[1..len / 2].iter().enumerate().map(|(i, value)| (i + 1, value)) {
            let hz = bin as f32 * bin_hz;
            let nearest = (hz / frequency).round() * frequency;
            if (hz - nearest).abs() <= 6.0 * bin_hz {
                harmonic += value.norm_sqr();
            } else {
                other += value.norm_sqr();
            }
        }
        10.0 * (other / (harmonic + other)).log10()
    }

    /// Strong phase modulation of a high note has sidebands far above the Nyquist frequency,
    /// which fold back as aliasing at the sample rate and much less oversampled.
    #[test]
    fn oversampling_takes_the_aliasing_out_of_phase_modulation() {
        let mut plugin = ThreeOscPlugin::default();
        let patch = [
            ("osc1_wave", serde_json::Value::from("sine")),
            ("osc1_phase_rand", serde_json::Value::from(0.0)),
            ("osc1_pm", serde_json::Value::from(0.5)),
            ("osc2_phase_rand", serde_json::Value::from(0.0)),
            ("fil1_model", serde_json::Value::from("none")),
        ];
        let rate = plugin.synth.sample_rate as f32;
        let aliasing = |plugin: &mut ThreeOscPlugin, note: u8| -> Vec<f32> {
            ["off", "2x", "4x", "8x"]
                .into_iter()
                .map(|oversampling| {
                    set_up(plugin, &patch, oversampling);
                    let (left, _) = render_note(plugin, note, 1.0, 1.0);
                    inharmonic_db(&left, 440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0), rate)
                })
                .collect()
        };
        // C6, with sidebands up to about 40 kHz: twice the sample rate is plenty
        let c6 = aliasing(&mut plugin, 84);
        println!("Phase modulation of C6, aliasing (off, 2x, 4x, 8x): {c6:.1?} dB");
        assert!(c6[0] > -40.0, "{c6:?}");
        assert!(c6[1] < -80.0 && c6[2] < -80.0 && c6[3] < -80.0, "{c6:?}");
        // C7, with sidebands up to about 80 kHz, needs four times
        let c7 = aliasing(&mut plugin, 96);
        println!("Phase modulation of C7, aliasing (off, 2x, 4x, 8x): {c7:.1?} dB");
        assert!(c7[0] > -20.0 && c7[1] > -20.0, "{c7:?}");
        assert!(c7[2] < -80.0 && c7[3] < -80.0, "{c7:?}");
    }

    /// How much aliasing a few kinds of patch have at each oversampling: the share of their
    /// output that isn't at a harmonic of the note.
    #[test]
    #[ignore]
    fn benchmark_aliasing() {
        use serde_json::Value;
        let sine = |extra: &[(&'static str, Value)]| {
            let mut patch = vec![
                ("osc1_wave", Value::from("sine")),
                ("osc1_phase_rand", Value::from(0.0)),
                ("osc2_phase_rand", Value::from(0.0)),
                ("fil1_model", Value::from("none")),
            ];
            patch.extend_from_slice(extra);
            patch
        };
        let cases: Vec<(&str, u8, Vec<(&str, Value)>)> = vec![
            ("saw, no filter", 96, vec![("osc1_phase_rand", Value::from(0.0)), ("fil1_model", Value::from("none"))]),
            ("phase modulation, C6", 84, sine(&[("osc1_pm", Value::from(0.5))])),
            ("phase modulation, C7", 96, sine(&[("osc1_pm", Value::from(0.5))])),
            ("frequency modulation, C7", 96, sine(&[("osc1_fm", Value::from(0.3))])),
            ("ring modulation by a saw, C6", 84, sine(&[("osc1_am", Value::from(1.0)), ("osc2_wave", Value::from("saw"))])),
            (
                "saw through the ladder, driven, C6",
                84,
                vec![
                    ("osc1_phase_rand", Value::from(0.0)),
                    ("fil1_model", Value::from("ladder")),
                    ("fil1_cutoff", Value::from(5000.0)),
                    ("fil1_drive", Value::from(10.0)),
                    ("fil1_env_amount", Value::from(0.0)),
                ],
            ),
        ];
        let mut plugin = ThreeOscPlugin::default();
        let rate = plugin.synth.sample_rate as f32;
        println!("\n{:<36} {:>8} {:>8} {:>8} {:>8}", "aliasing (dB)", "off", "2x", "4x", "8x");
        for (name, note, patch) in cases {
            print!("{name:<36}");
            for oversampling in ["off", "2x", "4x", "8x"] {
                set_up(&mut plugin, &patch, oversampling);
                let (left, _) = render_note(&mut plugin, note, 1.0, 1.0);
                let db = inharmonic_db(&left, 440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0), rate);
                print!(" {db:>8.1}");
            }
            println!();
        }
    }

    /// Changing the oversampling while a note plays carries it on where it was.
    #[test]
    fn oversampling_can_change_while_notes_play() {
        let mut plugin = ThreeOscPlugin::default();
        set_up(&mut plugin, &[("vol_decay", serde_json::Value::from(1.0)), ("vol_sustain", serde_json::Value::from(0.2))], "off");
        let mut left = vec![0.0; 4410];
        let mut right = left.clone();
        plugin.synth.note_on(60, 100);
        plugin.synth.run(&mut left, &mut right);
        let mut before = rms(&left[4410 - 441..]);
        for oversampling in [synth::oversampling::Oversampling::Four, synth::oversampling::Oversampling::Two, synth::oversampling::Oversampling::Off] {
            plugin.synth.oversampling = oversampling;
            left.fill(0.0);
            right.fill(0.0);
            plugin.synth.run(&mut left, &mut right);
            assert!(left.iter().all(|sample| sample.is_finite()));
            // Still decaying from where it was, not started again
            let after = rms(&left[..441]);
            assert!((20.0 * (after / before).log10()).abs() < 1.0, "{oversampling:?}: {before} then {after}");
            assert_eq!(plugin.synth.voices.len(), 1);
            before = rms(&left[4410 - 441..]);
            assert!(before < after);
        }
    }

    /// ...and smoothly: across each change, a sine through each filter carries on in phase
    /// (give or take the few samples the decimators and the Ladder's resampling filters delay
    /// it by) and without a click. (Left to themselves, the decimators would start from
    /// silence, taking it down to 0 and back, the oscillators would ramp from their pitch in
    /// the old rate's samples, putting it out of phase, and the Ladder's resampling filters
    /// would start afresh.)
    #[test]
    fn oversampling_changes_without_a_click() {
        use synth::oversampling::Oversampling;
        let rate = 44100.0;
        let frequency = 440.0 * 2.0_f64.powf((60.0 - 69.0) / 12.0);
        // The sine's phase, in samples, over the end of a buffer that starts at sample `start`
        let period = rate / frequency;
        let phase = |samples: &[f32], start: usize| {
            let (mut sin, mut cos) = (0.0, 0.0);
            for (n, sample) in samples.iter().enumerate().skip(samples.len() - 2048) {
                let t = std::f64::consts::TAU * frequency * (start + n) as f64 / rate;
                sin += *sample as f64 * t.sin();
                cos += *sample as f64 * t.cos();
            }
            cos.atan2(sin) / std::f64::consts::TAU * period
        };
        let largest_step = |samples: &[f32]| samples.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max);
        for filter in ["none", "digital", "ladder"] {
            let mut plugin = ThreeOscPlugin::default();
            let patch = [
                ("osc1_wave", serde_json::Value::from("sine")),
                ("fil1_model", serde_json::Value::from(filter)),
                ("fil1_cutoff", serde_json::Value::from(2000.0)),
                ("fil1_env_amount", serde_json::Value::from(0.0)),
                ("vol_sustain", serde_json::Value::from(1.0)),
            ];
            set_up(&mut plugin, &patch, "off");
            let mut left = vec![0.0; 4410];
            let mut right = left.clone();
            plugin.synth.note_on(60, 100);
            plugin.synth.run(&mut left, &mut right);
            let usual = largest_step(&left[2205..]);
            let mut before = phase(&left, 0);
            // How far behind it is of where it would have been without the changes
            let mut delay = 0.0;
            for (buffer, oversampling) in [Oversampling::Two, Oversampling::Eight, Oversampling::Four, Oversampling::Off, Oversampling::Four]
                .into_iter()
                .enumerate()
            {
                let last = left[left.len() - 1];
                plugin.synth.oversampling = oversampling;
                left.fill(0.0);
                right.fill(0.0);
                plugin.synth.run(&mut left, &mut right);
                let across = largest_step(&[&[last], &left[..256]].concat());
                assert!(across < 3.5 * usual, "{filter}, {oversampling:?}: a step of {across} (usually {usual})");
                let after = phase(&left, (buffer + 1) * left.len());
                delay += (before - after + period / 2.0).rem_euclid(period) - period / 2.0;
                assert!((-2.5..5.0).contains(&delay), "{filter}, {oversampling:?}: {delay} samples behind");
                before = after;
            }
        }
    }

    /// Quarter tones: twice as many keys to the octave.
    fn quarter_tones() -> params::TuningFiles {
        let steps: String = (1..=24).map(|step| format!("{}.0\n", step * 50)).collect();
        params::TuningFiles {
            scale: Some(params::TextFile {
                name: String::from("24-EDO.scl"),
                text: format!("Quarter tones\n24\n{steps}"),
            }),
            mapping: None,
        }
    }

    #[test]
    fn keys_play_their_tuned_pitches() {
        let mut plugin = ThreeOscPlugin::default();
        plugin.params.tuning.try_set(quarter_tones()).unwrap();
        plugin.apply_params();
        let sample_rate = plugin.synth.sample_rate as f32;
        let delta = |pitch: f32| 440.0 * ((pitch - 69.0) / 12.0).exp2() / sample_rate;
        let close = |a: f32, b: f32| (a / b - 1.0).abs() < 1e-4;

        plugin.synth.note_on(61, 100);
        plugin.synth.note_on(84, 100);
        assert!(close(plugin.synth.voices[0].delta(sample_rate), delta(60.5)));
        assert!(close(plugin.synth.voices[1].delta(sample_rate), delta(72.0)));

        // Legato glides from the pitch it was at to the new key's
        plugin.synth.reset();
        plugin.synth.polyphony = synth::Polyphony::Legato;
        plugin.synth.note_on(60, 100);
        plugin.synth.note_on(64, 100);
        assert_eq!(plugin.synth.voices.len(), 1);
        assert!(close(plugin.synth.voices[0].delta(sample_rate), delta(60.0)));
        let mut left = vec![0.0; 44100];
        let mut right = vec![0.0; 44100];
        plugin.synth.run(&mut left, &mut right);
        assert!(close(plugin.synth.voices[0].delta(sample_rate), delta(62.0)));
        // ...and back to the key still held when the new one is let go
        plugin.synth.note_off(64, 0);
        plugin.synth.run(&mut left, &mut right);
        assert!(close(plugin.synth.voices[0].delta(sample_rate), delta(60.0)));

        // Keys the mapping leaves out don't play
        plugin.synth.reset();
        plugin.synth.polyphony = synth::Polyphony::Polyphonic;
        let white_keys = "12\n0\n127\n60\n69\n440.0\n12\n0\nx\n2\nx\n4\n5\nx\n7\nx\n9\nx\n11\n";
        let files = params::TuningFiles {
            scale: None,
            mapping: Some(params::TextFile { name: String::from("white.kbm"), text: String::from(white_keys) }),
        };
        plugin.params.tuning.try_set(files).unwrap();
        plugin.apply_params();
        plugin.synth.note_on(61, 100);
        assert!(plugin.synth.voices.is_empty());
        plugin.synth.note_on(62, 100);
        assert!(close(plugin.synth.voices[0].delta(sample_rate), delta(62.0)));

        // A held key that the tuning silences isn't glided back to: the note ends instead
        plugin.synth.reset();
        plugin.params.tuning.try_set(params::TuningFiles::default()).unwrap();
        plugin.apply_params();
        plugin.synth.polyphony = synth::Polyphony::Legato;
        plugin.synth.note_on(61, 100);
        plugin.synth.note_on(62, 100);
        plugin.params.tuning.try_set(quarter_tones()).unwrap();
        let mut silent = quarter_tones().tuning().unwrap();
        silent.keys[61] = None;
        plugin.synth.key_pitches = silent.keys.map(|pitch| pitch.unwrap_or(f32::NAN));
        plugin.synth.note_off(62, 0);
        for _ in 0..2 {
            plugin.synth.run(&mut left, &mut right);
            assert!(left.iter().all(|sample| sample.is_finite()));
        }
        assert!(plugin.synth.voices.is_empty());
    }

    /// Fast modulation of a sine note, whose bandlimited tables are all the same whatever the
    /// pitch: pitch and volume LFOs faster than the block rate can follow, an envelope on the
    /// amplitude, a slower LFO on the cutoff, a short attack and a pitch slide, and optionally
    /// an LFO modulating another's rate.
    fn fast_modulation(plugin: &mut ThreeOscPlugin, model: synth::filter::FilterModel, lfo_rate: bool) {
        let route = |source, target, amount| Route {
            source,
            target,
            amount,
        };
        let synth = &mut plugin.synth;
        synth.routes.extend_from_slice(&[
            route(ModSource::Lfo1, ModTarget::Pitch, 0.1),
            route(ModSource::Lfo2, ModTarget::Volume, -0.5),
            route(ModSource::Env1, ModTarget::Osc1Amp, -0.4),
            route(ModSource::Lfo3, ModTarget::Cutoff, -0.2),
        ]);
        if lfo_rate {
            synth.routes.push(route(ModSource::Lfo3, ModTarget::Lfo1Rate, 0.3));
        }
        synth.lfos[0].freq = 900.0;
        synth.lfos[1].freq = 700.0;
        synth.lfos[2].freq = 40.0;
        synth.mod_envelopes[0].decay_time = 0.01;
        synth.gain_envelope.attack_time = 0.002;
        synth.portamento_offset = 12.0;
        synth.filter_controller.filter_model = model;
        synth.filter_controller.target_cutoff = 3000.0;
        synth.oscillators[0].wavetable = 0;
    }

    /// Renders a note with the given patch changes, at the given modulation rate and in
    /// buffers of `buffer_len` samples.
    fn render_buffers(
        plugin: &mut ThreeOscPlugin,
        rate: synth::ModulationRate,
        buffer_len: usize,
        setup: impl Fn(&mut ThreeOscPlugin),
    ) -> Vec<f32> {
        plugin.synth.reset();
        plugin.apply_params();
        plugin.synth.modulation_rate = rate;
        plugin.synth.routes.clear();
        for osc in plugin.synth.oscillators.iter_mut() {
            osc.phase_rand = 0.0;
        }
        setup(plugin);
        plugin.synth.note_on(60, 100);

        let mut left = vec![0.0; 4096];
        let mut right = vec![0.0; 4096];
        for (left, right) in left.chunks_mut(buffer_len).zip(right.chunks_mut(buffer_len)) {
            plugin.synth.run(left, right);
        }
        assert!(left.iter().all(|sample| sample.is_finite()), "{rate:?}");
        left
    }

    /// At the sample rate, every sample gets the control values for its own time. That is
    /// what the block rate gives with buffers a sample long (the per sample path works a
    /// whole block's values out at once, and its filters take each sample's coefficients for
    /// that sample, where the block rate's take the next sample's), and far from what it gives
    /// for fast modulation otherwise.
    #[test]
    fn sample_rate_modulation_follows_every_sample() {
        use synth::filter::FilterModel;
        use synth::ModulationRate;

        let mut plugin = ThreeOscPlugin::default();
        assert_eq!(
            plugin.params.global.modulation_rate.value().to_modulation_rate(),
            ModulationRate::Block
        );
        for (model, lfo_rate) in [
            (FilterModel::None, false),
            (FilterModel::None, true),
            (FilterModel::SvfSimperFilter, false),
            (FilterModel::LadderFilter, false),
            (FilterModel::RcFilter, false),
        ] {
            let case = format!("{model:?}, LFO rate modulated: {lfo_rate}");
            let setup = |plugin: &mut ThreeOscPlugin| fast_modulation(plugin, model, lfo_rate);
            let sample_rate = render_buffers(&mut plugin, ModulationRate::Sample, 4096, setup);
            let one_at_a_time = render_buffers(&mut plugin, ModulationRate::Block, 1, setup);
            let block_rate = render_buffers(&mut plugin, ModulationRate::Block, 4096, setup);
            let peak = one_at_a_time.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.05, "{case}: {peak}");
            assert!(max_abs_diff(&sample_rate, &one_at_a_time) < 5e-3 * peak, "{case}");
            assert!(max_abs_diff(&sample_rate, &block_rate) > 0.1 * peak, "{case}");
            // The buffer sizes only change the rounding
            let odd_buffers = render_buffers(&mut plugin, ModulationRate::Sample, 37, setup);
            assert!(max_abs_diff(&sample_rate, &odd_buffers) < 1e-5, "{case}");
        }
    }

    /// At the sample rate, the controls that stay the same throughout a block are only worked
    /// out once, and the filters then run on one set of coefficients. That renders exactly
    /// what working out every sample's gives: modulation by a source that never changes (the
    /// mod wheel) sounds the same as by one that could, but doesn't (a square LFO slow enough
    /// to stay high throughout).
    #[test]
    fn unchanging_modulation_is_worked_out_once() {
        use synth::filter::FilterModel;
        use synth::ModulationRate;

        let mut plugin = ThreeOscPlugin::default();
        let targets = [
            (ModTarget::Cutoff, -0.3),
            (ModTarget::Resonance, 0.2),
            (ModTarget::Drive, 0.4),
            (ModTarget::Pitch, 0.05),
            (ModTarget::Osc1Amp, -0.2),
            (ModTarget::Osc1Position, 0.6),
            (ModTarget::Osc1Fm, 0.3),
            (ModTarget::Osc1Am, 0.5),
            (ModTarget::Volume, -0.25),
        ];
        for model in [
            FilterModel::SvfSimperFilter,
            FilterModel::LadderFilter,
            FilterModel::RcFilter,
            FilterModel::None,
        ] {
            let render = |plugin: &mut ThreeOscPlugin, source: ModSource| {
                // The same start in the ladder's noise for both
                fastrand::seed(7);
                render_buffers(plugin, ModulationRate::Sample, 4096, |plugin| {
                    let synth = &mut plugin.synth;
                    synth.filter_controller.filter_model = model;
                    synth.mod_wheel = 1.0;
                    synth.lfos[0].wave = synth::oscillator::OscWave::Square;
                    synth.lfos[0].freq = 0.01;
                    // The second oscillator, for the FM and AM
                    synth.oscillators[1].amp = 0.5;
                    synth.oscillators[0].wavetable = OscillatorWave::PwSweep.wavetable_index().unwrap();
                    for (target, amount) in targets {
                        synth.routes.push(Route {
                            source,
                            target,
                            amount,
                        });
                    }
                })
            };
            let constant = render(&mut plugin, ModSource::ModWheel);
            let changing = render(&mut plugin, ModSource::Lfo1);
            assert_eq!(constant, changing, "{model:?}");
            // (and the modulation does something)
            let unmodulated = render_buffers(&mut plugin, ModulationRate::Sample, 4096, |plugin| {
                plugin.synth.filter_controller.filter_model = model;
                plugin.synth.oscillators[1].amp = 0.5;
                plugin.synth.oscillators[0].wavetable = OscillatorWave::PwSweep.wavetable_index().unwrap();
            });
            assert!(max_abs_diff(&constant, &unmodulated) > 1e-3, "{model:?}");
        }
    }

    /// Renders one note with the default patch, the given number of unison voices on every
    /// oscillator, and the given stereo width.
    fn render_stereo(plugin: &mut ThreeOscPlugin, voices: u8, width: f32) -> (Vec<f32>, Vec<f32>) {
        plugin.synth.reset();
        plugin.apply_params();
        plugin.synth.stereo_width = width;
        for osc in plugin.synth.oscillators.iter_mut() {
            osc.voice_count = voices;
            osc.voices_detune = 0.37;
            // Deterministic phases, so that different renders can be compared.
            osc.phase_rand = 0.0;
        }
        plugin.synth.note_on(60, 100);

        let mut left = vec![0.0; 2048];
        let mut right = vec![0.0; 2048];
        plugin.synth.run(&mut left, &mut right);
        (left, right)
    }

    fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn stereo_width_spreads_unison_voices() {
        let mut plugin = ThreeOscPlugin::default();

        // A single voice stays in the centre whatever the width.
        let (left, right) = render_stereo(&mut plugin, 1, 1.0);
        assert!(left.iter().any(|sample| sample.abs() > 0.01));
        assert!(max_abs_diff(&left, &right) < 1e-4);

        // Unison voices are spread out...
        let (left, right) = render_stereo(&mut plugin, 8, 1.0);
        assert!(left.iter().chain(right.iter()).all(|sample| sample.is_finite()));
        assert!(max_abs_diff(&left, &right) > 1e-2);

        // ...and a width of (almost) zero matches the mono path.
        let (mono_left, mono_right) = render_stereo(&mut plugin, 8, 0.0);
        assert_eq!(max_abs_diff(&mono_left, &mono_right), 0.0);
        let (left, right) = render_stereo(&mut plugin, 8, 1e-6);
        assert!(max_abs_diff(&left, &mono_left) < 1e-3);
        assert!(max_abs_diff(&right, &mono_right) < 1e-3);
    }

    /// The stereo width defaults to the widest, but the default patch (one voice per
    /// oscillator, nothing panned) stays in the centre on the cheaper mono path: both channels
    /// are the same to the bit.
    #[test]
    fn default_patch_stays_centred() {
        let mut plugin = ThreeOscPlugin::default();
        plugin.apply_params();
        assert_eq!(plugin.synth.stereo_width, 1.0);
        plugin.synth.note_on(60, 100);
        let mut left = vec![0.0; 2048];
        let mut right = vec![0.0; 2048];
        plugin.synth.run(&mut left, &mut right);
        assert!(left.iter().any(|sample| sample.abs() > 0.01));
        assert_eq!(left, right);
    }

    /// Renders a note with the first oscillator panned to `pan`, and its pan modulated by
    /// `pan_mod` through a square LFO slow enough to stay high throughout.
    fn render_panned(plugin: &mut ThreeOscPlugin, rate: synth::ModulationRate, pan: f32, pan_mod: Option<f32>) -> (Vec<f32>, Vec<f32>) {
        plugin.synth.reset();
        plugin.apply_params();
        plugin.synth.modulation_rate = rate;
        plugin.synth.routes.clear();
        plugin.synth.oscillators[0].pan = pan;
        plugin.synth.oscillators[0].phase_rand = 0.0;
        if let Some(amount) = pan_mod {
            plugin.synth.lfos[0].wave = synth::oscillator::OscWave::Square;
            plugin.synth.lfos[0].freq = 0.01;
            plugin.synth.routes.push(Route {
                source: ModSource::Lfo1,
                target: ModTarget::Osc1Pan,
                amount,
            });
        }
        plugin.synth.note_on(60, 100);
        let mut left = vec![0.0; 2048];
        let mut right = vec![0.0; 2048];
        plugin.synth.run(&mut left, &mut right);
        (left, right)
    }

    /// An oscillator's pan moves it between the channels, and so does modulating the pan, at
    /// both modulation rates.
    #[test]
    fn pan_moves_oscillators() {
        let mut plugin = ThreeOscPlugin::default();
        let level = |samples: &[f32]| samples.iter().map(|sample| sample * sample).sum::<f32>().sqrt();
        for rate in [synth::ModulationRate::Block, synth::ModulationRate::Sample] {
            let (left, right) = render_panned(&mut plugin, rate, -1.0, None);
            assert!(level(&left) > 1.0 && level(&right) < 1e-3 * level(&left), "{rate:?}");
            // Halfway right: louder on the right, and still on the left
            let (left, right) = render_panned(&mut plugin, rate, 0.5, None);
            assert!(level(&right) > 1.5 * level(&left) && level(&left) > 0.1 * level(&right), "{rate:?}");
            // Modulated from the centre all the way right
            let (left, right) = render_panned(&mut plugin, rate, 0.0, Some(0.5));
            assert!(level(&right) > 1.0 && level(&left) < 1e-3 * level(&right), "{rate:?}");
        }
    }

    /// The effects process the synth's output when they are switched on: a reverb rings on
    /// after the note has stopped. A route from a source that holds still (the mod wheel) that
    /// turns its mix down to nothing gives exactly the dry output.
    #[test]
    fn effects_process_the_output() {
        let mut plugin = ThreeOscPlugin::default();
        let render = |plugin: &mut ThreeOscPlugin, rate: synth::ModulationRate, reverb: bool, routes: &[Route]| {
            plugin.synth.reset();
            plugin.apply_params();
            plugin.synth.modulation_rate = rate;
            plugin.synth.routes.clear();
            plugin.synth.routes.extend_from_slice(routes);
            plugin.synth.mod_wheel = 1.0;
            plugin.synth.oscillators[0].phase_rand = 0.0;
            plugin.synth.effect_settings.reverb.enabled = reverb;
            plugin.synth.effect_settings.reverb.mix = 0.5;
            plugin.synth.note_on(60, 100);
            let mut left = vec![0.0; 8192];
            let mut right = vec![0.0; 8192];
            plugin.synth.run(&mut left[..2048], &mut right[..2048]);
            plugin.synth.note_off(60, 0);
            plugin.synth.run(&mut left[2048..], &mut right[2048..]);
            (left, right)
        };
        let level = |samples: &[f32]| samples.iter().map(|sample| sample * sample).sum::<f32>().sqrt();
        for rate in [synth::ModulationRate::Block, synth::ModulationRate::Sample] {
            let (dry, _) = render(&mut plugin, rate, false, &[]);
            let (wet, wet_right) = render(&mut plugin, rate, true, &[]);
            // The dry note has finished well before the end; the reverb hasn't
            assert!(level(&dry[6000..]) < 1e-6, "{rate:?}");
            assert!(level(&wet[6000..]) > 1e-3, "{rate:?}");
            assert!(max_abs_diff(&wet, &wet_right) > 1e-4, "{rate:?}");
            let silenced = [Route {
                source: ModSource::ModWheel,
                target: ModTarget::ReverbMix,
                amount: -1.0,
            }];
            let (modulated, _) = render(&mut plugin, rate, true, &silenced);
            assert_eq!(modulated, dry, "{rate:?}");
        }
    }

    /// Feeds a deterministic stream of random notes, note offs, chokes and pitch bends through
    /// the sample accurate event handling, in randomly sized buffers, and checks that the output
    /// stays finite.
    #[test]
    fn random_note_streams_render_finite_audio() {
        let rng = fastrand::Rng::with_seed(0x7_05C);
        let mut plugin = ThreeOscPlugin::default();
        plugin.apply_params();

        let mut left = vec![0.0; 2048];
        let mut right = vec![0.0; 2048];
        for block in 0..300 {
            let len = rng.usize(1..=2048);
            // Switch between the mono and stereo paths, and vary the width, between blocks.
            plugin.synth.stereo_width = if rng.bool() { rng.f32() } else { 0.0 };
            // Every filter model, including the ladder's lockstep processing of partial groups
            plugin.synth.filter_controller.filter_model = match rng.usize(0..4) {
                0 => synth::filter::FilterModel::None,
                1 => synth::filter::FilterModel::RcFilter,
                2 => synth::filter::FilterModel::LadderFilter,
                _ => synth::filter::FilterModel::SvfSimperFilter,
            };
            // Now and then a change of tuning, which can leave held keys silent
            if rng.usize(0..32) == 0 {
                let tuning = match rng.usize(0..3) {
                    0 => synth::tuning::Tuning::standard(),
                    1 => quarter_tones().tuning().unwrap(),
                    _ => synth::tuning::Tuning::from_files(None, Some("12\n0\n127\n60\n60\n261.6\n12\n0\nx\n2\n")).unwrap(),
                };
                plugin.synth.key_pitches = tuning.keys.map(|pitch| pitch.unwrap_or(f32::NAN));
                if rng.bool() {
                    plugin.synth.polyphony = synth::Polyphony::Legato;
                } else {
                    plugin.synth.polyphony = synth::Polyphony::Polyphonic;
                }
            }
            // Now and then a change of oversampling
            if rng.usize(0..16) == 0 {
                use synth::oversampling::Oversampling;
                plugin.synth.oversampling =
                    [Oversampling::Off, Oversampling::Two, Oversampling::Four, Oversampling::Eight][rng.usize(0..4)];
            }
            // Now and then a block at the sample rate, which is much slower
            plugin.synth.modulation_rate = if rng.usize(0..8) == 0 {
                synth::ModulationRate::Sample
            } else {
                synth::ModulationRate::Block
            };
            let mut events = Vec::new();
            for _ in 0..rng.usize(0..8) {
                // Timings at the end of the buffer are allowed, they are applied afterwards.
                let timing = rng.u32(0..=len as u32);
                let channel = rng.u8(0..16);
                let note = rng.u8(0..128);
                let voice_id = None;
                events.push(match rng.usize(0..10) {
                    0..=3 => NoteEvent::NoteOn {
                        timing,
                        voice_id,
                        channel,
                        note,
                        velocity: rng.f32(),
                    },
                    4..=6 => NoteEvent::NoteOff {
                        timing,
                        voice_id,
                        channel,
                        note,
                        velocity: rng.f32(),
                    },
                    7 => NoteEvent::Choke {
                        timing,
                        voice_id,
                        channel,
                        note,
                    },
                    _ => NoteEvent::MidiPitchBend {
                        timing,
                        channel,
                        value: rng.f32(),
                    },
                });
            }
            events.sort_by_key(|event| event.timing());

            plugin.render(events.into_iter(), &mut left[..len], &mut right[..len]);

            let bad_sample = left[..len]
                .iter()
                .chain(right[..len].iter())
                .enumerate()
                .find(|(_, sample)| !sample.is_finite());
            if let Some((index, sample)) = bad_sample {
                panic!(
                    "block {block} ({len} samples, {} voices): sample {} is {sample}",
                    plugin.synth.voices.len(),
                    index % len
                );
            }
        }
    }
}
