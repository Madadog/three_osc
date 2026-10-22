//! Three Osc: a polyphonic subtractive synthesizer with three oscillators that can modulate each
//! other in various ways.
//!
//! This crate is the [nih-plug](https://github.com/robbert-vdh/nih-plug) wrapper around the synth
//! engine in the [`synth`] module. It exports the synth as a CLAP and a VST3 plugin (and as a
//! standalone application when the `standalone` feature is enabled), with an egui GUI.

use std::sync::Arc;

use nih_plug::prelude::*;

#[cfg(test)]
mod benchmark;
mod editor;
mod params;
mod synth;

use params::{multiplier_to_pitch_multiplier, ModParams, OscParams, ThreeOscParams};
use synth::oscillator::OscillatorParams;
use synth::ThreeOsc;

/// The sample rate the synth is created with before the host tells us the real one in
/// [`Plugin::initialize()`].
const DEFAULT_SAMPLE_RATE: f64 = 44100.0;

pub struct ThreeOscPlugin {
    params: Arc<ThreeOscParams>,
    synth: ThreeOsc,
}

impl Default for ThreeOscPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(ThreeOscParams::default()),
            synth: ThreeOsc::new(DEFAULT_SAMPLE_RATE),
        }
    }
}

impl ThreeOscPlugin {
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
        let vol_env = &params.vol_env;
        synth.gain_envelope.attack_time = vol_env.attack.value();
        synth.gain_envelope.decay_time = vol_env.decay.value();
        synth.gain_envelope.sustain_level = vol_env.sustain.value();
        synth.gain_envelope.release_time = vol_env.release.value();
        synth.gain_envelope.set_slope(vol_env.slope.value());

        // Filter
        let filter = &params.filter;
        let filter_env = &params.filter_env;
        let controller = &mut synth.filter_controller;
        controller.envelope_amount = filter_env.amount.value().powi(2);
        controller.keytrack = filter.keytrack.value();
        controller.cutoff_envelope.attack_time = filter_env.envelope.attack.value();
        controller.cutoff_envelope.decay_time = filter_env.envelope.decay.value();
        controller.cutoff_envelope.sustain_level = filter_env.envelope.sustain.value();
        controller.cutoff_envelope.release_time = filter_env.envelope.release.value();
        controller
            .cutoff_envelope
            .set_slope(filter_env.envelope.slope.value());
        controller.target_cutoff = filter.cutoff.value();
        controller.resonance = filter.resonance.value();
        controller.drive = filter.drive.value();
        controller.filter_type = filter.filter_type.value().to_filter_type();
        controller.filter_model = filter.model.value().to_filter_model();

        // LFO
        let lfo = &params.lfo;
        synth.lfo_params.target_osc = lfo.target.value().to_osc_index();
        synth.lfo_params.wave = lfo.wave.value().to_osc_wave();
        synth.lfo_params.freq = lfo.freq.value();
        synth.lfo_params.freq_mod = lfo.freq_mod.value().powi(2);
        synth.lfo_params.amp_mod = lfo.amp_mod.value();
        synth.lfo_params.mod_mod = lfo.mod_mod.value();
        synth.lfo_params.filter_mod = lfo.filter_mod.value();

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
    osc.wavetable = params.wave.value().wavetable_index();
    osc.position = params.position.value();
    osc.amp = params.amp.value() / 100.0;
    osc.semitone = params.semitone.value() + global_pitch;
    osc.octave = params.octave.value();
    osc.pitch_multiplier = multiplier_to_pitch_multiplier(params.multiplier.value());
    osc.voice_count = params.voices.value() as u8;
    osc.voices_detune = (params.super_detune.value() / 100.0).powi(3);
    osc.phase = params.phase.value() / 100.0;
    osc.phase_rand = params.phase_rand.value() / 100.0;
}

/// Applies the PM/FM/AM amounts for an oscillator that has a modulator after it.
fn apply_mod_params(osc: &mut OscillatorParams, params: &ModParams) {
    osc.pm = params.pm.value().powi(2);
    osc.fm = params.fm.value().powi(2);
    osc.am = params.am.value().powi(2);
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
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        // The synth's wavetables are generated for a specific sample rate, so the synth needs to
        // be rebuilt when the sample rate changes. This may be called several times in a row, so
        // avoid the (fairly expensive) rebuild when nothing changed.
        let sample_rate = buffer_config.sample_rate as f64;
        if self.synth.sample_rate != sample_rate {
            self.synth = ThreeOsc::new(sample_rate);
        }

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
        let names: Vec<&str> = set.wavetables.iter().map(|wavetable| wavetable.name).collect();
        assert_eq!(names, params::OscillatorWave::variants());
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

    /// Renders one note with the default patch, the given number of unison voices on every
    /// oscillator, and the given stereo width.
    fn render_stereo(plugin: &mut ThreeOscPlugin, voices: u8, width: f32) -> (Vec<f32>, Vec<f32>) {
        plugin.synth.reset();
        plugin.apply_params();
        plugin.synth.stereo_width = width;
        for osc in plugin.synth.oscillators.iter_mut() {
            osc.voice_count = voices;
            osc.voices_detune = 0.05;
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
