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
mod params;
mod synth;

use params::{
    multiplier_to_pitch_multiplier, EnvelopeParams, ModParams, OscParams, OscillatorWave,
    ThreeOscParams,
};
use synth::envelopes::AdsrEnvelope;
use synth::modulation::{ModSource, ModTarget, Route};
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
        let params = &self.params;
        let synth = &mut self.synth;

        // Global
        let global = &params.global;
        let output_gain = global.output_gain.value();
        synth.output_volume = if output_gain > -90.0 {
            10.0_f32.powf(output_gain * 0.05)
        } else {
            0.0
        };
        synth.bend_range = global.bend_range.value() as f32;
        synth.polyphony = global.polyphony.value().to_polyphony();
        // Scaling: This is a lerp, and must be proportional to the sample rate
        // ... the '0.002' is just user-friendly control scaling.
        // TODO: test and make sure this actually keeps time constant across sample rates
        synth.portamento_rate = 1.0
            - global
                .portamento_rate
                .value()
                .powf(0.002 * synth.sample_rate as f32 / 44100.0);
        synth.portamento_offset = global.pitch_offset.value();
        // multiplies delta: smaller = higher pitch
        synth.octave_detune = 1.0 - global.octave_detune.value();
        synth.stereo_width = global.stereo_width.value();
        synth.interpolation = global.interpolation.value().to_interpolation();

        // Master gain envelope
        apply_envelope_params(&mut synth.gain_envelope, &params.vol_env);

        // Filter
        let filter = &params.filter;
        let filter_env = &params.filter_env;
        let controller = &mut synth.filter_controller;
        controller.envelope_amount = filter_env.amount.value().powi(2);
        controller.keytrack = filter.keytrack.value();
        apply_envelope_params(&mut controller.cutoff_envelope, &filter_env.envelope);
        controller.target_cutoff = filter.cutoff.value();
        controller.resonance = filter.resonance.value();
        controller.drive = filter.drive.value();
        controller.filter_type = filter.filter_type.value().to_filter_type();
        controller.filter_model = filter.model.value().to_filter_model();

        // Modulation: LFOs, envelopes and the matrix. The routes vector has room for every
        // slot, so this doesn't allocate.
        for (lfo, lfo_params) in synth.lfos.iter_mut().zip(params.lfos.iter()) {
            lfo.wave = lfo_params.wave.value().to_osc_wave();
            lfo.freq = lfo_params.rate.value();
        }
        for (envelope, env_params) in synth.mod_envelopes.iter_mut().zip(params.mod_envs.iter()) {
            apply_envelope_params(envelope, &env_params.envelope);
        }
        synth.routes.clear();
        for slot in params.mod_slots.iter() {
            let route = Route {
                source: slot.source.value(),
                target: slot.target.value(),
                amount: slot.amount.value(),
            };
            if route.source != ModSource::None && route.target != ModTarget::None && route.amount != 0.0 {
                synth.routes.push(route);
            }
        }

        // Oscillators
        let global_pitch = global.global_pitch.value() as f32;
        apply_osc_params(&mut synth.oscillators[0], &params.osc1, global_pitch);
        apply_mod_params(&mut synth.oscillators[0], &params.osc1_mod);

        apply_osc_params(&mut synth.oscillators[1], &params.osc2, global_pitch);
        apply_mod_params(&mut synth.oscillators[1], &params.osc2_mod);

        // The third oscillator has nothing after it to modulate it.
        apply_osc_params(&mut synth.oscillators[2], &params.osc3, global_pitch);
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
fn apply_osc_params(osc: &mut OscillatorParams, params: &OscParams, global_pitch: f32) {
    let wave = params.wave.value();
    osc.use_file = wave == OscillatorWave::File;
    osc.wavetable = wave.wavetable_index().unwrap_or(0);
    osc.position = params.position.value();
    osc.amp = params.amp.value() / 100.0;
    osc.semitone = params.semitone.value() + global_pitch;
    osc.octave = params.octave.value();
    osc.pitch_multiplier = multiplier_to_pitch_multiplier(params.multiplier.value());
    osc.voice_count = params.voices.value() as u8;
    // The synth cubes the detune amount, after any modulation
    osc.voices_detune = params.super_detune.value() / 100.0;
    osc.phase = params.phase.value() / 100.0;
    osc.phase_rand = params.phase_rand.value() / 100.0;
}

/// Applies the PM/FM/AM amounts for an oscillator that has a modulator after it. The synth
/// squares them, after any modulation.
fn apply_mod_params(osc: &mut OscillatorParams, params: &ModParams) {
    osc.pm = params.pm.value();
    osc.fm = params.fm.value();
    osc.am = params.am.value();
}

fn apply_envelope_params(envelope: &mut AdsrEnvelope, params: &EnvelopeParams) {
    envelope.attack_time = params.attack.value();
    envelope.decay_time = params.decay.value();
    envelope.sustain_level = params.sustain.value();
    envelope.release_time = params.release.value();
    envelope.set_slope(params.slope.value());
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
        editor::create(self.params.clone(), self.loader.sample_rate.clone(), async_executor)
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
