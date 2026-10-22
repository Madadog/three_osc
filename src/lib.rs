//! Three Osc: a polyphonic subtractive synthesizer with three oscillators that can modulate each
//! other in various ways.
//!
//! This crate is the [nih-plug](https://github.com/robbert-vdh/nih-plug) wrapper around the synth
//! engine in the [`synth`] module. It exports the synth as a CLAP and a VST3 plugin (and as a
//! standalone application when the `standalone` feature is enabled), with an egui GUI.

use std::f32::consts::PI;
use std::sync::Arc;

use nih_plug::prelude::*;

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
        synth.oscillators[0].wave = params.osc1.wave.value().to_osc_wave();

        apply_osc_params(&mut synth.oscillators[1], &params.osc2, global_pitch);
        apply_mod_params(&mut synth.oscillators[1], &params.osc2_mod);
        synth.oscillators[1].wave = params.osc2.wave.value().to_osc_wave();

        // The third oscillator has nothing after it to modulate it, but its square wave has an
        // adjustable pulse width instead.
        apply_osc_params(&mut synth.oscillators[2], &params.osc3, global_pitch);
        synth.oscillators[2].wave = params.osc3.wave.value().to_osc_wave_pulse();
        synth.oscillators[2].pulse_width = params.osc3_pulse.pulse_width.value() * 2.0 * PI / 100.0;
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
}

/// Applies the controls shared by all oscillators.
fn apply_osc_params(osc: &mut OscillatorParams, params: &OscParams, global_pitch: f32) {
    osc.amp = params.amp.value() / 100.0;
    osc.semitone = params.semitone.value() + global_pitch;
    osc.octave = params.octave.value();
    osc.pitch_multiplier = multiplier_to_pitch_multiplier(params.multiplier.value());
    osc.voice_count = params.voices.value() as u8;
    osc.voices_detune = (params.super_detune.value() / 100.0).powi(3);
    osc.phase = params.phase.value() * 2.0 * PI / 100.0;
    osc.phase_rand = params.phase_rand.value() * 2.0 * PI / 100.0;
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

        let num_samples = buffer.samples();
        let output = buffer.as_slice();
        // The synth adds its voices onto whatever is in the output buffers.
        for channel in output.iter_mut() {
            channel.fill(0.0);
        }
        let (out_l, out_r) = match output {
            [out_l, out_r, ..] => (out_l, out_r),
            // Only a stereo layout is offered, so this shouldn't happen.
            _ => return ProcessStatus::Error("expected a stereo output"),
        };

        // Note events are sample accurate: the buffer is rendered in chunks, with the events
        // that fall on a chunk's first sample applied before it is rendered.
        let mut next_event = context.next_event();
        let mut block_start = 0;
        while block_start < num_samples {
            while let Some(event) = next_event {
                if event.timing() as usize > block_start {
                    break;
                }
                self.handle_event(event);
                next_event = context.next_event();
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
            next_event = context.next_event();
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
