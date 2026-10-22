use std::f32::consts::PI;
use std::sync::Arc;

use itertools::izip;
use nih_plug::prelude::Enum;
use wide::f32x8;

use self::envelopes::AdsrEnvelope;
use self::filter::Filter;
use self::filter::FilterContainer;
use self::filter::FilterController;
#[cfg(target_arch = "x86_64")]
use self::filter::{LadderFilter, LADDER_LANES};
use self::filter::{SvfSimper, SVF_LANES};
use self::modulation::{
    apply_routes, ModSource, ModTarget, Route, Sources, Targets, N_LFOS, N_MOD_ENVELOPES,
    N_SOURCES,
};
use self::notes::Notes;
use self::oscillator::BlockOutput;
use self::oscillator::BlockParams;
use self::oscillator::Interpolation;
use self::oscillator::Modulation;
use self::oscillator::OscVoice;
use self::oscillator::OscWave;
use self::oscillator::OscillatorParams;
use self::oscillator::SimdLevel;
use self::oscillator::SuperVoice;
use self::oscillator::WavetableFrames;
use self::oscillator::WavetableNotes;
use self::oscillator::WavetableSet;
use self::oscillator::UNISON_VECTORS;

/// Samples per control rate block. Envelopes, pitch, the LFOs, the modulation matrix,
/// keytracking and the filter coefficients are only recalculated this often; gains and pitch are
/// ramped linearly in between (and so are the SVF's coefficients). Smaller is more accurate,
/// larger is cheaper. The trade-off mostly shows in fast envelopes, and in the LFOs, which can't
/// go faster than half the control rate (a few hundred Hz).
pub const CONTROL_BLOCK_SAMPLES: usize = 32;

/// Phase modulation depth, in cycles per unit of modulator output (150 radians).
const PM_DEPTH_CYCLES: f32 = 150.0 / (2.0 * PI);
/// Frequency modulation depth, as a multiple of the modulated oscillator's frequency per unit of
/// modulator output.
const FM_DEPTH: f32 = 125.0;

pub struct ThreeOsc {
    pub voices: Vec<Voice>,
    pub notes: Notes,
    pub gain_envelope: AdsrEnvelope,
    pub(crate) filter_controller: filter::FilterController,
    pub sample_rate: f64,
    pub output_volume: f32,
    pub oscillators: [OscillatorParams; 3],
    pub waves: WavetableSet,
    /// Wavetables loaded from files, one slot per oscillator, played when the oscillator's
    /// `use_file` is set.
    pub user_waves: [Option<Arc<WavetableFrames>>; 3],
    pub bend_range: f32,
    pub polyphony: Polyphony,
    pub octave_detune: f32,
    pub portamento_rate: f32,
    pub portamento_offset: f32,
    pub lfos: [LfoParams; N_LFOS],
    /// The envelopes that are only there to be modulation sources.
    pub mod_envelopes: [AdsrEnvelope; N_MOD_ENVELOPES],
    /// The modulation matrix's active routes (see [`modulation`]).
    pub routes: Vec<Route>,
    /// The MIDI modulation wheel, from 0 to 1.
    pub mod_wheel: f32,
    /// MIDI channel pressure, from 0 to 1.
    pub aftertouch: f32,
    /// How far unison voices are spread across the stereo field, from 0 (mono: both outputs are
    /// identical) to 1 (the lowest voice fully left and the highest fully right).
    pub stereo_width: f32,
    /// The SIMD instructions the wavetable lookups use.
    pub simd: SimdLevel,
    /// How the wavetables are read in between their samples.
    pub interpolation: Interpolation,
    /// Draws each note's random modulation source.
    rng: fastrand::Rng,
}

impl ThreeOsc {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            voices: Vec::with_capacity(64),
            notes: Notes::new(),
            gain_envelope: AdsrEnvelope::new(0.0, 0.5, 0.05, 1.0, 1.0),
            filter_controller: filter::FilterController::new(),
            sample_rate,
            output_volume: 0.3,
            oscillators: [
                OscillatorParams::default(),
                OscillatorParams::default(),
                OscillatorParams::default(),
            ],
            waves: WavetableSet::new(sample_rate as f32, 2048),
            user_waves: [None, None, None],
            bend_range: 2.0,
            polyphony: Polyphony::Polyphonic,
            octave_detune: 1.0,
            portamento_rate: 0.1,
            portamento_offset: 0.0,
            lfos: Default::default(),
            mod_envelopes: std::array::from_fn(|_| AdsrEnvelope::new(0.0, 0.5, 0.05, 1.0, 1.0)),
            routes: Vec::with_capacity(modulation::N_SLOTS),
            mod_wheel: 0.0,
            aftertouch: 0.0,
            stereo_width: 0.0,
            simd: SimdLevel::detect(),
            interpolation: Interpolation::default(),
            rng: fastrand::Rng::new(),
        }
    }
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.notes.note_on(note, velocity);
        let random = self.rng.f32() * 2.0 - 1.0;

        match &self.polyphony {
            Polyphony::Polyphonic => {
                let mut new_voice = Voice::from_midi_note(note, velocity, &self.oscillators, random);
                new_voice.semitone_detune += self.portamento_offset;
                self.voices.push(new_voice)
            }
            Polyphony::Legato | Polyphony::Monophonic => {
                if let Some(voice) = self.voices.last_mut() {
                    voice.semitone_detune = voice.id as f32 - note as f32 + voice.semitone_detune;
                    voice.id = note as u32;
                    voice.velocity = velocity;

                    // If the note is released, or if we are in monophonic mode, retrigger it.
                    if matches!(self.polyphony, Polyphony::Monophonic)
                        || voice.release_time.is_some()
                    {
                        voice.semitone_detune += self.portamento_offset;
                        voice.release_time = None;
                        voice.runtime = 0;
                        voice.random = random;
                    }
                } else {
                    let mut new_voice =
                        Voice::from_midi_note(note, velocity, &self.oscillators, random);
                    new_voice.semitone_detune += self.portamento_offset;
                    self.voices.push(new_voice)
                }
            }
        }
    }
    pub fn note_off(&mut self, note: u8, _velocity: u8) {
        self.notes.note_off(note);

        match self.polyphony {
            Polyphony::Polyphonic => self
                .voices
                .iter_mut()
                .filter(|voice| voice.id == note as u32)
                .for_each(|voice| voice.release()),
            // TODO: If there are two notes each with their own voice playing, when one is released
            // it will snap to the other note, resulting in two voices playing the same note at the
            // same time, which sounds bad. Stop this behaviour by filtering out notes with voices
            // currently playing.
            Polyphony::Legato | Polyphony::Monophonic => self
                .voices
                .iter_mut()
                .filter(|voice| voice.id == note as u32)
                .for_each(|voice| {
                    let latest_note = self.notes.notes.iter().reduce(|accum, note| {
                        if note.age() < accum.age() {
                            note
                        } else {
                            accum
                        }
                    });

                    if let Some(note) = latest_note {
                        voice.semitone_detune =
                            voice.id as f32 + voice.semitone_detune - note.id as f32;
                        voice.id = note.id as u32;
                        // Retrigger notes when releasing keys in Monophonic mode
                        if matches!(self.polyphony, Polyphony::Monophonic) {
                            voice.semitone_detune += self.portamento_offset;
                            voice.release_time = None;
                            voice.runtime = 0;
                        }
                    } else {
                        voice.release();
                    }
                }),
        }
    }
    /// Polyphonic aftertouch for the voices playing `note`, from 0 to 1.
    pub fn poly_pressure(&mut self, note: u8, pressure: f32) {
        for voice in self.voices.iter_mut().filter(|voice| voice.id == note as u32) {
            voice.pressure = pressure.clamp(0.0, 1.0);
        }
    }
    /// Stops all sound immediately by removing every playing voice and held note. Doesn't
    /// allocate, so this is safe to call from the audio thread.
    pub fn reset(&mut self) {
        self.voices.clear();
        self.notes.notes.clear();
    }
    pub fn release_voices(&mut self) {
        let duration = (self.gain_envelope.release_time * self.sample_rate as f32) as u32;

        self.voices.retain(|voice| {
            if let Some(release_time) = voice.release_time {
                voice.runtime - release_time < duration
            } else {
                true
            }
        })
    }
    /// The wavetable an oscillator plays: its loaded file if it is set to one (or the first
    /// built in wavetable while none is loaded), otherwise its pick from the built in set.
    pub fn oscillator_frames(&self, osc: usize) -> &WavetableFrames {
        let params = &self.oscillators[osc];
        if params.use_file {
            self.user_waves[osc]
                .as_deref()
                .unwrap_or(&self.waves.wavetables[0])
        } else {
            &self.waves.wavetables[params.wavetable.min(self.waves.wavetables.len() - 1)]
        }
    }
    /// Everything the voices need from the synth to render themselves, for the current settings.
    fn voice_context(&self, stereo: bool) -> VoiceContext<'_> {
        // An oscillator is only needed if it can be audible, or if it modulates an oscillator
        // that is needed (each oscillator modulates the one before it). Modulation routes can
        // turn up amounts that are set to zero, so a targeted amount counts as nonzero.
        let targeted = |target: Option<ModTarget>| {
            target.is_some_and(|target| self.routes.iter().any(|route| route.target == target))
        };
        let mut active = [false; 3];
        for i in 0..3 {
            let osc = &self.oscillators[i];
            let audible = osc.amp > 0.0 || targeted(Some(ModTarget::osc_amp(i)));
            let modulates_needed_osc = i > 0 && active[i - 1] && {
                let modulated = &self.oscillators[i - 1];
                modulated.pm > 0.0
                    || modulated.fm > 0.0
                    || modulated.am > 0.0
                    || targeted(ModTarget::osc_pm(i - 1))
                    || targeted(ModTarget::osc_fm(i - 1))
                    || targeted(ModTarget::osc_am(i - 1))
            };
            active[i] = audible || modulates_needed_osc;
        }
        let sources_used = std::array::from_fn(|source| {
            self.routes
                .iter()
                .any(|route| route.source.to_index() == source)
        });

        VoiceContext {
            oscillators: &self.oscillators,
            frames: std::array::from_fn(|i| self.oscillator_frames(i)),
            filter_controller: &self.filter_controller,
            gain_envelope: &self.gain_envelope,
            lfos: &self.lfos,
            mod_envelopes: &self.mod_envelopes,
            routes: &self.routes,
            sources_used,
            mod_wheel: self.mod_wheel,
            aftertouch: self.aftertouch,
            sample_rate: self.sample_rate as f32,
            stereo,
            octave_detune: self.octave_detune,
            portamento_rate: self.portamento_rate,
            portamento_block: (1.0 - self.portamento_rate).powi(CONTROL_BLOCK_SAMPLES as i32),
            active,
            simd: self.simd,
            interpolation: self.interpolation,
        }
    }
    pub fn run(&mut self, output_left: &mut [f32], output_right: &mut [f32]) {
        self.release_voices();
        self.filter_controller.cutoff = self.filter_controller.target_cutoff;

        // Unison voices are only spread across the stereo field when the width is above zero.
        // Otherwise the cheaper mono path is used, and both outputs are identical.
        let stereo = self.stereo_width > 0.0;

        // Minor optimisation: Semitone / octave / pitch multiplier offset is calculated and cached
        // once per run, not once per sample
        for osc in self.oscillators.iter_mut() {
            osc.update_total_pitch();
            osc.update_unison_amp();
            osc.update_detune_factors();
            osc.clamp_wavetable(&self.waves);
            if stereo {
                osc.update_pan_gains(self.stereo_width);
            }
        }

        // The voices are taken out of the synth while the context borrows the rest of it. This
        // doesn't allocate: the vector keeps its capacity and goes back afterwards.
        let mut voices = std::mem::take(&mut self.voices);
        let context = self.voice_context(stereo);

        // Write samples from all voices, one control rate block at a time. Each block goes
        // through three stages: the oscillators of every voice, then the filters (which can run
        // several voices in lockstep), then the amplitude envelopes mixing into the output.
        let len = output_left.len().min(output_right.len());
        let mut start = 0;
        while start < len {
            let end = (start + CONTROL_BLOCK_SAMPLES).min(len);
            let block_len = end - start;
            for voice in voices.iter_mut() {
                voice.update_controls(&context, block_len);
                voice.render_oscillators(&context, block_len);
            }
            filter_voices(&mut voices, block_len, stereo, self.simd);
            for voice in voices.iter_mut() {
                voice.mix_output(
                    block_len,
                    stereo,
                    &mut output_left[start..end],
                    &mut output_right[start..end],
                );
            }
            start = end;
        }
        self.voices = voices;

        // Apply output volume
        for (out_l, out_r) in izip!(output_left, output_right) {
            *out_l *= self.output_volume;
            *out_r *= self.output_volume;
        }
    }
    /// Runs only the control rate update of every voice for `blocks` blocks, for the benchmarks.
    #[cfg(test)]
    pub(crate) fn run_control_updates_only(&mut self, blocks: usize) {
        self.release_voices();
        let stereo = self.stereo_width > 0.0;
        for osc in self.oscillators.iter_mut() {
            osc.update_total_pitch();
            osc.update_unison_amp();
            osc.update_detune_factors();
            osc.clamp_wavetable(&self.waves);
        }
        let mut voices = std::mem::take(&mut self.voices);
        let context = self.voice_context(stereo);
        for _ in 0..blocks {
            for voice in voices.iter_mut() {
                voice.update_controls(&context, CONTROL_BLOCK_SAMPLES);
            }
        }
        self.voices = voices;
    }
    pub fn pitch_bend(&mut self, bend: u16) {
        let bend = (bend as i32 - 8192) as f32 / 8192.0 * self.bend_range;
        for osc in self.oscillators.iter_mut() {
            osc.pitch_bend = bend;
        }
    }
}

mod notes;

pub enum Polyphony {
    Polyphonic,
    Monophonic,
    Legato,
}

/// Everything a voice needs from the synth to render itself.
struct VoiceContext<'a> {
    oscillators: &'a [OscillatorParams; 3],
    /// The wavetable each oscillator plays.
    frames: [&'a WavetableFrames; 3],
    filter_controller: &'a FilterController,
    gain_envelope: &'a AdsrEnvelope,
    lfos: &'a [LfoParams; N_LFOS],
    mod_envelopes: &'a [AdsrEnvelope; N_MOD_ENVELOPES],
    routes: &'a [Route],
    /// Which modulation sources any route uses, so the unused ones aren't computed.
    sources_used: [bool; N_SOURCES],
    mod_wheel: f32,
    aftertouch: f32,
    sample_rate: f32,
    stereo: bool,
    octave_detune: f32,
    portamento_rate: f32,
    /// The portamento rate applied over a whole control rate block.
    portamento_block: f32,
    /// Whether each oscillator needs to be rendered at all: silent oscillators that don't
    /// modulate anything are skipped.
    active: [bool; 3],
    simd: SimdLevel,
    interpolation: Interpolation,
}

/// An individual note press.
// TODO: Separate phase, oscillator, and filter from note data.
pub struct Voice {
    id: u32,
    /// Samples rendered so far.
    runtime: u32,
    release_time: Option<u32>,
    osc_voice: [SuperVoice; 3],
    lfos: [OscVoice; N_LFOS],
    filter: filter::FilterContainer,
    /// Filter for the right channel, only used when the stereo width is above zero.
    filter_right: filter::FilterContainer,
    velocity: u8,
    /// Polyphonic aftertouch, from 0 to 1.
    pressure: f32,
    /// The random modulation source, from -1 to 1.
    random: f32,
    pitch_multiply: f32,
    semitone_detune: f32,

    // Control rate state, see `update_controls()`. The deltas and gains are ramped linearly
    // from one block to the next.
    /// Each oscillator's phase delta in cycles per sample, before frequency modulation.
    osc_delta: [f32; 3],
    osc_delta_step: [f32; 3],
    /// The MIDI note whose bandlimited wavetable each oscillator reads.
    osc_note: [usize; 3],
    /// The wavetable frames each oscillator crossfades between, for its (modulated) position.
    osc_frames: [(usize, usize, f32); 3],
    /// Each oscillator's output gain, before amplitude modulation.
    osc_gain: [f32; 3],
    osc_gain_step: [f32; 3],
    /// Each oscillator's PM, FM and AM amounts after modulation, with the control scaling applied.
    osc_pm: [f32; 3],
    osc_fm: [f32; 3],
    osc_am: [f32; 3],
    /// Each oscillator's unison detune factors when its unison detune is modulated, otherwise
    /// the oscillator's own are used.
    detune_factors: [[f32x8; UNISON_VECTORS]; 3],
    detune_modulated: [bool; 3],
    /// Whether each oscillator's ramps were set up in the previous block.
    osc_ready: [bool; 3],
    /// The modulation of every target in the previous block.
    mods: Targets,
    /// The voice's gain: amplitude envelope times velocity.
    gain: f32,
    gain_step: f32,
    /// Filter input gain.
    drive: f32,

    /// The current block's samples on their way from the oscillators through the filter to the
    /// output; `pre_filter_right` is only used in stereo.
    pre_filter: [f32; CONTROL_BLOCK_SAMPLES],
    pre_filter_right: [f32; CONTROL_BLOCK_SAMPLES],
}
impl Voice {
    pub fn from_midi_note(index: u8, velocity: u8, osc: &[OscillatorParams], random: f32) -> Self {
        let osc_voice = [
            SuperVoice::new(osc[0].phase, osc[0].phase_rand),
            SuperVoice::new(osc[1].phase, osc[1].phase_rand),
            SuperVoice::new(osc[2].phase, osc[2].phase_rand),
        ];

        Self {
            id: index.into(),
            runtime: 0,
            release_time: None,
            osc_voice,
            lfos: Default::default(),
            velocity,
            pressure: 0.0,
            random,
            filter: filter::FilterContainer::None,
            filter_right: filter::FilterContainer::None,
            pitch_multiply: 1.0,
            semitone_detune: 0.0,
            osc_delta: [0.0; 3],
            osc_delta_step: [0.0; 3],
            osc_note: [0; 3],
            osc_frames: [(0, 0, 0.0); 3],
            osc_gain: [0.0; 3],
            osc_gain_step: [0.0; 3],
            osc_pm: [0.0; 3],
            osc_fm: [0.0; 3],
            osc_am: [0.0; 3],
            detune_factors: [[f32x8::splat(1.0); UNISON_VECTORS]; 3],
            detune_modulated: [false; 3],
            osc_ready: [false; 3],
            mods: [0.0; modulation::N_TARGETS],
            gain: 0.0,
            gain_step: 0.0,
            drive: 1.0,
            pre_filter: [0.0; CONTROL_BLOCK_SAMPLES],
            pre_filter_right: [0.0; CONTROL_BLOCK_SAMPLES],
        }
    }
    pub fn release(&mut self) {
        if self.release_time.is_none() {
            self.release_time = Some(self.runtime)
        }
    }
    /// The voice's base phase delta, in cycles per sample.
    pub fn delta(&self, sample_rate: f32) -> f32 {
        440.0
            * fastmath::exp2(
                (((self.id as i16 - 69) as f32 + self.semitone_detune) * self.pitch_multiply)
                    / 12.0,
            )
            / sample_rate
    }
    /// An envelope's level at the given point in the voice's life, in samples.
    fn envelope(&self, envelope: &AdsrEnvelope, runtime: u32, sample_rate: f32) -> f32 {
        let index = runtime as f32 / sample_rate;
        match self.release_time {
            Some(release_time) => {
                envelope.sample_released(release_time as f32 / sample_rate, index)
            }
            None => envelope.sample_held(index),
        }
    }
    /// The value of every modulation source at the end of the current block. Sources no route
    /// uses are left at zero, apart from the two envelopes the voice needs anyway.
    fn modulation_sources(
        &mut self,
        context: &VoiceContext,
        block_len: usize,
        gain_level: f32,
        filter_level: f32,
    ) -> Sources {
        let sample_rate = context.sample_rate;
        let mut sources: Sources = [0.0; N_SOURCES];
        let used = |source: ModSource| context.sources_used[source.to_index()];

        // The LFOs keep running whether they are used or not, so they don't jump when routed.
        for (i, lfo) in context.lfos.iter().enumerate() {
            let rate_mod = self.mods[ModTarget::lfo_rate(i).to_index()];
            let mut freq = lfo.freq;
            if rate_mod != 0.0 {
                freq *= fastmath::exp2(rate_mod * ModTarget::LFO_RATE_OCTAVES);
            }
            let phase = self.lfos[i].add_phase(freq / sample_rate * block_len as f32);
            if used(ModSource::lfo(i)) {
                sources[ModSource::lfo(i).to_index()] = lfo.wave.generate(phase);
            }
        }
        for (i, envelope) in context.mod_envelopes.iter().enumerate() {
            if used(ModSource::envelope(i)) {
                sources[ModSource::envelope(i).to_index()] =
                    self.envelope(envelope, self.runtime, sample_rate);
            }
        }
        sources[ModSource::VolumeEnv.to_index()] = gain_level;
        sources[ModSource::FilterEnv.to_index()] = filter_level;
        sources[ModSource::Velocity.to_index()] = self.velocity as f32 / 127.0;
        sources[ModSource::Key.to_index()] = (self.id as f32 - 60.0) / 60.0;
        sources[ModSource::ModWheel.to_index()] = context.mod_wheel;
        sources[ModSource::Aftertouch.to_index()] = context.aftertouch.max(self.pressure);
        sources[ModSource::Random.to_index()] = self.random;
        sources
    }
    /// Recalculates the control rate state for the next `block_len` samples: the pitch, the
    /// LFOs, the envelopes, the modulation matrix, keytracking and the filter coefficients.
    /// Gains and pitch ramp from their current values to the values at the end of the block.
    fn update_controls(&mut self, context: &VoiceContext, block_len: usize) {
        let sample_rate = context.sample_rate;
        let block_start = self.runtime;
        self.runtime += block_len as u32;
        let step_scale = 1.0 / block_len as f32;
        // A new (or retriggered) voice starts from its initial values instead of ramping
        let first_block = block_start == 0;

        // Portamento, applied for the whole block at once
        self.semitone_detune *= if block_len == CONTROL_BLOCK_SAMPLES {
            context.portamento_block
        } else {
            (1.0 - context.portamento_rate).powi(block_len as i32)
        };
        self.pitch_multiply = context.octave_detune;
        let delta = self.delta(sample_rate);

        // The envelopes, sampled at the end of the block, and the modulation matrix
        let controller = context.filter_controller;
        let gain_level = self.envelope(context.gain_envelope, self.runtime, sample_rate);
        let filter_level = self.envelope(&controller.cutoff_envelope, self.runtime, sample_rate);
        let sources = self.modulation_sources(context, block_len, gain_level, filter_level);
        let mods = apply_routes(context.routes, &sources);
        self.mods = mods;
        let modulation = |target: ModTarget| mods[target.to_index()];

        let pitch_all = modulation(ModTarget::Pitch) * ModTarget::PITCH_SEMITONES
            + modulation(ModTarget::FinePitch) * ModTarget::FINE_PITCH_SEMITONES;

        let keytrack_freq = fastmath::exp2(
            (self.id as f32 - 69.0 + self.semitone_detune) / 12.0 * controller.keytrack,
        );

        for (i, osc) in context.oscillators.iter().enumerate() {
            if !context.active[i] {
                // Silent and not modulating anything: nothing of it is rendered this block, and
                // its ramps start afresh if it becomes active.
                self.osc_ready[i] = false;
                continue;
            }
            // A new voice, or an oscillator that was switched on, starts from its target values
            // instead of ramping towards them.
            let first_block = first_block || !self.osc_ready[i];
            self.osc_ready[i] = true;

            // Pitch
            let semitones = pitch_all
                + modulation(ModTarget::osc_pitch(i)) * ModTarget::PITCH_SEMITONES;
            let mut target_delta = osc.total_pitch_multiplier() * delta;
            if semitones != 0.0 {
                target_delta *= fastmath::exp2(semitones / 12.0);
            }
            self.osc_note[i] = WavetableNotes::cycles_to_note(target_delta, sample_rate);

            // Amplitude
            let amp = (osc.amp + modulation(ModTarget::osc_amp(i))).clamp(0.0, 1.0);
            let target_gain = osc.unison_amp * amp;
            if first_block {
                self.osc_delta[i] = target_delta;
                self.osc_gain[i] = target_gain;
            }
            self.osc_delta_step[i] = (target_delta - self.osc_delta[i]) * step_scale;
            self.osc_gain_step[i] = (target_gain - self.osc_gain[i]) * step_scale;

            // Wavetable position
            let position = osc.position + modulation(ModTarget::osc_position(i));
            self.osc_frames[i] = context.frames[i].frame_indices(position);

            // Unison detune
            let detune_mod = modulation(ModTarget::osc_detune(i));
            self.detune_modulated[i] = detune_mod != 0.0;
            if detune_mod != 0.0 {
                self.detune_factors[i] =
                    osc.detune_factors_for((osc.voices_detune + detune_mod).clamp(0.0, 1.0));
            }

            // Modulation by the next oscillator (the amounts are squared for finer control at
            // the low end)
            let scaled = |amount: f32, target: Option<ModTarget>| {
                (amount + target.map_or(0.0, modulation)).clamp(0.0, 1.0).powi(2)
            };
            self.osc_pm[i] = scaled(osc.pm, ModTarget::osc_pm(i));
            self.osc_fm[i] = scaled(osc.fm, ModTarget::osc_fm(i));
            self.osc_am[i] = scaled(osc.am, ModTarget::osc_am(i));
        }

        // Amplitude envelope, velocity and volume modulation
        let velocity = self.velocity as f32 / 128.0;
        let volume = (1.0 + modulation(ModTarget::Volume)).max(0.0) * velocity;
        if first_block {
            self.gain = self.envelope(context.gain_envelope, block_start, sample_rate) * volume;
        }
        let target_gain = gain_level * volume;
        self.gain_step = (target_gain - self.gain) * step_scale;

        // Filter
        let mut cutoff_mult = keytrack_freq;
        let cutoff_mod = modulation(ModTarget::Cutoff);
        if cutoff_mod != 0.0 {
            cutoff_mult *= fastmath::exp2(cutoff_mod * ModTarget::CUTOFF_OCTAVES);
        }
        let cutoff = controller.get_cutoff(cutoff_mult, filter_level, sample_rate);
        let mut resonance = controller.resonance;
        let resonance_mod = modulation(ModTarget::Resonance);
        if resonance_mod != 0.0 {
            resonance = (resonance * fastmath::exp2(resonance_mod * ModTarget::RESONANCE_OCTAVES))
                .clamp(0.1, 10.0);
        }
        let mut drive = controller.drive;
        let drive_mod = modulation(ModTarget::Drive);
        if drive_mod != 0.0 {
            drive = (drive * fastmath::exp2(drive_mod * ModTarget::DRIVE_OCTAVES)).clamp(0.01, 10.0);
        }
        let configure_filter = |filter: &mut FilterContainer| {
            filter.set(
                controller.filter_model,
                cutoff,
                resonance,
                sample_rate,
                controller.filter_type,
            );
            filter.set_filter_type(controller.filter_type);
            filter.set_params_ramped(sample_rate, cutoff, resonance, block_len);
        };
        configure_filter(&mut self.filter);
        if context.stereo {
            // Filters are stateful, so the right channel needs its own instance.
            configure_filter(&mut self.filter_right);
        }
        self.drive = match self.filter {
            // Biquad/SVF/none filters are unaffected by drive, so we clamp it between 0 and 1 to
            // keep the levels the same when switching filter.
            FilterContainer::BiquadFilter(_)
            | FilterContainer::SvfSimperFilter(_)
            | FilterContainer::None => drive.min(1.0),
            FilterContainer::LadderFilter(_) => drive / 2.0,
            _ => drive,
        };
    }
    /// Renders one control rate block of the oscillators into the voice's pre-filter buffers.
    ///
    /// Each oscillator renders the whole block in one kernel call (see
    /// [`FramePair::render_block()`][oscillator::FramePair::render_block()]). The oscillators
    /// go from the last to the first, since each one modulates the one before it: a modulated
    /// oscillator reads its modulator's block of output, sample for sample.
    fn render_oscillators(&mut self, context: &VoiceContext, block_len: usize) {
        let block_len = block_len.min(CONTROL_BLOCK_SAMPLES);
        let stereo = context.stereo;
        let oscillators = context.oscillators;
        self.pre_filter[..block_len].fill(0.0);
        if stereo {
            self.pre_filter_right[..block_len].fill(0.0);
        }

        // The block of output of the oscillator after the current one (its modulator), and
        // the current oscillator's output when the oscillator before it is modulated.
        let mut modulator = [0.0_f32; CONTROL_BLOCK_SAMPLES];
        let mut modulator_next = [0.0_f32; CONTROL_BLOCK_SAMPLES];
        let mut have_modulator = false;
        for i in (0..3).rev() {
            let modulates_next = i > 0
                && context.active[i - 1]
                && (self.osc_pm[i - 1] > 0.0 || self.osc_fm[i - 1] > 0.0 || self.osc_am[i - 1] > 0.0);
            let silent = self.osc_gain[i] == 0.0 && self.osc_gain_step[i] == 0.0;
            if !context.active[i] || (silent && !modulates_next) {
                // Its output would be silent, and it isn't modulating anything that matters
                have_modulator = false;
                continue;
            }
            let osc = &oscillators[i];
            let modulated = self.osc_pm[i] > 0.0 || self.osc_fm[i] > 0.0 || self.osc_am[i] > 0.0;
            let modulation = if modulated && have_modulator {
                Some(Modulation {
                    input: &modulator[..block_len],
                    fm: self.osc_fm[i] * FM_DEPTH,
                    pm: self.osc_pm[i] * PM_DEPTH_CYCLES,
                    am: self.osc_am[i],
                })
            } else {
                None
            };

            let params = BlockParams {
                delta: self.osc_delta[i],
                delta_step: self.osc_delta_step[i],
                gain: self.osc_gain[i] * self.drive,
                gain_step: self.osc_gain_step[i] * self.drive,
                modulation,
                voices: osc.unison_voices(),
                detune_factors: if self.detune_modulated[i] {
                    &self.detune_factors[i]
                } else {
                    &osc.detune_factors
                },
                mask: &osc.voice_mask,
                pan_left: &osc.pan_left,
                pan_right: &osc.pan_right,
            };
            let mut output = BlockOutput {
                left: &mut self.pre_filter[..block_len],
                right: if stereo {
                    Some(&mut self.pre_filter_right[..block_len])
                } else {
                    None
                },
                mono: if modulates_next {
                    Some((&mut modulator_next[..block_len], 1.0))
                } else {
                    None
                },
            };
            let frames = context.frames[i].frame_pair(self.osc_frames[i], self.osc_note[i]);
            frames.render_block(
                &mut self.osc_voice[i],
                &params,
                &mut output,
                context.simd,
                context.interpolation,
            );

            // The ramps continue from where the block left them
            self.osc_delta[i] += self.osc_delta_step[i] * block_len as f32;
            self.osc_gain[i] += self.osc_gain_step[i] * block_len as f32;
            have_modulator = modulates_next;
            std::mem::swap(&mut modulator, &mut modulator_next);
        }
    }
    /// Runs the voice's filter(s) over the current block, one sample at a time.
    fn filter_block(&mut self, block_len: usize, stereo: bool) {
        filter_samples(&mut self.filter, &mut self.pre_filter[..block_len]);
        if stereo {
            filter_samples(&mut self.filter_right, &mut self.pre_filter_right[..block_len]);
        }
    }
    /// Applies the amplitude envelope and velocity to the filtered block and adds it to the
    /// output buffers.
    fn mix_output(
        &mut self,
        block_len: usize,
        stereo: bool,
        out_left: &mut [f32],
        out_right: &mut [f32],
    ) {
        let block_len = block_len.min(out_left.len()).min(out_right.len());
        // The gain of each sample is computed from its index rather than accumulated, so the
        // samples don't depend on each other and the loop vectorises. The index goes through an
        // `i32`, which converts to a float in one instruction (a `usize` takes several).
        let (gain, step) = (self.gain, self.gain_step);
        let left = &self.pre_filter[..block_len];
        let out_left = &mut out_left[..block_len];
        let out_right = &mut out_right[..block_len];
        if stereo {
            let right = &self.pre_filter_right[..block_len];
            for (n, (((out_l, out_r), left), right)) in
                izip!(out_left, out_right).zip(left).zip(right).enumerate()
            {
                let gain = gain + step * n as i32 as f32;
                *out_l += left * gain;
                *out_r += right * gain;
            }
        } else {
            for (n, ((out_l, out_r), left)) in izip!(out_left, out_right).zip(left).enumerate() {
                let sample = left * (gain + step * n as i32 as f32);
                *out_l += sample;
                *out_r += sample;
            }
        }
        self.gain = gain + step * block_len as f32;
    }
}

/// Runs every voice's filter over the current block. The Digital (SVF) and Ladder filters are
/// processed several voices at a time, in lockstep.
fn filter_voices(voices: &mut [Voice], block_len: usize, stereo: bool, simd: SimdLevel) {
    // Every voice is configured from the same controller before being filtered, so they all
    // use the same filter model and the first voice's picks the lockstep kernel. Any voice that
    // doesn't match it is filtered one sample at a time.
    match voices.first().map(|voice| &voice.filter) {
        Some(FilterContainer::SvfSimperFilter(_)) => {
            for chunk in voices.chunks_mut(SVF_LANES) {
                filter_chunk_lockstep::<SvfSimper, SVF_LANES>(
                    chunk,
                    block_len,
                    stereo,
                    |filter| match filter {
                        FilterContainer::SvfSimperFilter(svf) => Ok(svf),
                        other => Err(other),
                    },
                    |lanes| {
                        #[cfg(target_arch = "x86_64")]
                        if simd.has_avx() {
                            // SAFETY: the CPU supports AVX.
                            return unsafe { SvfSimper::process_block_x8_avx(lanes) };
                        }
                        SvfSimper::process_block_x8(lanes)
                    },
                );
            }
        }
        #[cfg(target_arch = "x86_64")]
        Some(FilterContainer::LadderFilter(_)) if simd.has_avx() => {
            for chunk in voices.chunks_mut(LADDER_LANES) {
                filter_chunk_lockstep::<LadderFilter, LADDER_LANES>(
                    chunk,
                    block_len,
                    stereo,
                    |filter| {
                        let lockstep = matches!(
                            filter,
                            FilterContainer::LadderFilter(ladder) if ladder.supports_lockstep()
                        );
                        if !lockstep {
                            return Err(filter);
                        }
                        match filter {
                            FilterContainer::LadderFilter(ladder) => Ok(ladder),
                            other => Err(other),
                        }
                    },
                    // SAFETY: this path is only taken on the AVX2 levels, which are only
                    // picked when the CPU supports AVX2 and FMA.
                    |lanes| unsafe { LadderFilter::process_block_x8_fma(lanes) },
                );
            }
        }
        _ => {
            let _ = simd;
            for voice in voices {
                voice.filter_block(block_len, stereo);
            }
        }
    }
}

/// Runs a filter over a block of samples, in place.
fn filter_samples(filter: &mut FilterContainer, samples: &mut [f32]) {
    if matches!(filter, FilterContainer::None) {
        return;
    }
    for sample in samples {
        *sample = filter.process(*sample);
    }
}

/// Filters a chunk of up to `N` voices: the filters that `select` picks out run in lockstep
/// through `process`, the rest one sample at a time. In stereo the right channel's filters form
/// a second set of lanes.
fn filter_chunk_lockstep<'a, F: 'a, const N: usize>(
    chunk: &'a mut [Voice],
    block_len: usize,
    stereo: bool,
    select: impl Fn(&mut FilterContainer) -> Result<&mut F, &mut FilterContainer>,
    process: impl Fn([Option<(&mut F, &mut [f32])>; N]),
) {
    let mut left: [Option<(&'a mut F, &'a mut [f32])>; N] = std::array::from_fn(|_| None);
    let mut right: [Option<(&'a mut F, &'a mut [f32])>; N] = std::array::from_fn(|_| None);
    for (lane, voice) in chunk.iter_mut().enumerate() {
        let Voice {
            filter,
            filter_right,
            pre_filter,
            pre_filter_right,
            ..
        } = voice;
        let assign = |filter: &'a mut FilterContainer,
                      buffer: &'a mut [f32],
                      lanes: &mut [Option<(&'a mut F, &'a mut [f32])>; N]| {
            match select(filter) {
                Ok(filter) => lanes[lane] = Some((filter, buffer)),
                Err(filter) => filter_samples(filter, buffer),
            }
        };
        assign(filter, &mut pre_filter[..block_len], &mut left);
        if stereo {
            assign(filter_right, &mut pre_filter_right[..block_len], &mut right);
        }
    }
    process(left);
    if stereo {
        process(right);
    }
}

pub mod fastmath;
pub mod wav;
pub mod wavetable_file;
pub mod modulation;
pub mod oscillator;

pub mod envelopes {
    use super::fastmath;

    #[derive(Debug, Clone)]
    pub struct AdsrEnvelope {
        pub attack_time: f32,
        pub decay_time: f32,
        pub release_time: f32,
        pub sustain_level: f32,
        pub slope: f32,
        pub attack_slope: f32,
    }
    impl AdsrEnvelope {
        pub fn new(
            attack_time: f32,
            decay_time: f32,
            release_time: f32,
            sustain_level: f32,
            slope: f32,
        ) -> Self {
            Self {
                attack_time,
                decay_time,
                release_time,
                sustain_level,
                slope: Self::slope(slope),
                attack_slope: Self::slope(-slope),
            }
        }
        /// 0 is linear, positive is biased towards zero, negative is biased towards max.
        fn slope(slope: f32) -> f32 {
            2.0_f32.powf(slope)
        }
        pub fn set_slope(&mut self, slope: f32) {
            // The inverted / negative slope flares up too quickly compared to the
            // positive slope, so we divide the negative slope by an arbitrary number
            if slope.is_sign_positive() {
                self.slope = Self::slope(slope);
                self.attack_slope = Self::slope(-slope / 4.0);
            } else {
                self.slope = Self::slope(slope / 4.0);
                self.attack_slope = Self::slope(-slope);
            }
        }
        /// Returns the envelope CV (between 0.0 and 1.0) associated with the given index
        ///
        /// The comparisons are strict so that a zero length attack or decay stage is skipped
        /// instead of dividing zero by zero (e.g. a note that is released in the same instant
        /// it was started samples the envelope at index 0).
        pub fn sample_held(&self, index: f32) -> f32 {
            if index < self.attack_time {
                fastmath::pow(index / self.attack_time, self.attack_slope)
            } else if index - self.attack_time < self.decay_time {
                fastmath::pow(
                    1.0 - (index - self.attack_time) / self.decay_time,
                    self.slope,
                ) * (1.0 - self.sustain_level)
                    + self.sustain_level
            } else {
                self.sustain_level
            }
        }
        pub fn sample_released(&self, release_index: f32, index: f32) -> f32 {
            assert!(release_index <= index);
            if index - release_index >= self.release_time {
                0.0
            } else {
                let level = self.sample_held(release_index);
                fastmath::pow(1.0 - (index - release_index) / self.release_time, self.slope)
                    * level
            }
        }
    }
}

/// A low frequency oscillator's settings. Each voice runs its own copy of every LFO, started
/// when the note starts, and the LFOs' outputs are modulation sources (see [`modulation`]).
pub struct LfoParams {
    /// Rate in Hz.
    pub freq: f32,
    pub wave: OscWave,
}
impl Default for LfoParams {
    fn default() -> Self {
        Self {
            freq: 5.0,
            wave: OscWave::Sine,
        }
    }
}

pub(crate) mod filter;

#[inline]
fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    (to - from).mul_add(amount, from)
}
