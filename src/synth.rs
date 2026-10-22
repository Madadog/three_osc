use std::f32::consts::PI;

use itertools::izip;

use self::envelopes::AdsrEnvelope;
use self::filter::Filter;
use self::filter::FilterContainer;
use self::filter::FilterController;
#[cfg(target_arch = "x86_64")]
use self::filter::{LadderFilter, LADDER_LANES};
use self::notes::Notes;
use self::oscillator::wrap_cycles_signed;
use self::oscillator::FramePair;
use self::oscillator::OscVoice;
use self::oscillator::OscWave;
use self::oscillator::OscillatorParams;
use self::oscillator::SimdLevel;
use self::oscillator::SuperVoice;
use self::oscillator::WavetableNotes;
use self::oscillator::WavetableSet;

/// Samples per control rate block. Envelopes, pitch, the LFO, keytracking and the filter
/// coefficients are only recalculated this often; gains and pitch are ramped linearly in between
/// (and so are the SVF's coefficients). Smaller is more accurate, larger is cheaper. The
/// trade-off mostly shows in fast envelopes, and in the LFO, which can't go faster than half the
/// control rate (a few hundred Hz).
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
    pub bend_range: f32,
    pub polyphony: Polyphony,
    pub octave_detune: f32,
    pub portamento_rate: f32,
    pub portamento_offset: f32,
    pub lfo_params: LfoParams,
    /// How far unison voices are spread across the stereo field, from 0 (mono: both outputs are
    /// identical) to 1 (the lowest voice fully left and the highest fully right).
    pub stereo_width: f32,
    /// The SIMD instructions the wavetable lookups use.
    pub simd: SimdLevel,
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
            // wavetables: WavetableNotes::from_additive_osc_2(&AdditiveOsc::saw(), sample_rate as f32, 1.0, 2048),
            waves: WavetableSet::new(sample_rate as f32, 2048),
            bend_range: 2.0,
            polyphony: Polyphony::Polyphonic,
            octave_detune: 1.0,
            portamento_rate: 0.1,
            portamento_offset: 0.0,
            lfo_params: Default::default(),
            stereo_width: 0.0,
            simd: SimdLevel::detect(),
        }
    }
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.notes.note_on(note, velocity);

        match &self.polyphony {
            Polyphony::Polyphonic => {
                let mut new_voice = Voice::from_midi_note(note, velocity, &self.oscillators);
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
                    }
                } else {
                    let mut new_voice = Voice::from_midi_note(note, velocity, &self.oscillators);
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
            osc.update_frames(&self.waves);
            if stereo {
                osc.update_pan_gains(self.stereo_width);
            }
        }

        // An oscillator is only needed if it is audible, or if it modulates an oscillator that
        // is needed (each oscillator modulates the one before it).
        let mut active = [false; 3];
        for i in 0..3 {
            let osc = &self.oscillators[i];
            let modulates_needed_osc = i > 0 && active[i - 1] && {
                let modulated = &self.oscillators[i - 1];
                modulated.pm > 0.0 || modulated.fm > 0.0 || modulated.am > 0.0
            };
            active[i] = osc.amp > 0.0 || modulates_needed_osc;
        }

        let context = VoiceContext {
            oscillators: &self.oscillators,
            waves: &self.waves,
            filter_controller: &self.filter_controller,
            gain_envelope: &self.gain_envelope,
            lfo_params: &self.lfo_params,
            sample_rate: self.sample_rate as f32,
            stereo,
            octave_detune: self.octave_detune,
            portamento_rate: self.portamento_rate,
            active,
            simd: self.simd,
        };

        // Write samples from all voices, one control rate block at a time. Each block goes
        // through three stages: the oscillators of every voice, then the filters (which can run
        // several voices in lockstep), then the amplitude envelopes mixing into the output.
        let len = output_left.len().min(output_right.len());
        let mut start = 0;
        while start < len {
            let end = (start + CONTROL_BLOCK_SAMPLES).min(len);
            let block_len = end - start;
            for voice in self.voices.iter_mut() {
                voice.update_controls(&context, block_len);
                voice.render_oscillators(&context, block_len);
            }
            filter_voices(&mut self.voices, block_len, stereo, self.simd);
            for voice in self.voices.iter_mut() {
                voice.mix_output(
                    block_len,
                    stereo,
                    &mut output_left[start..end],
                    &mut output_right[start..end],
                );
            }
            start = end;
        }

        // Apply output volume
        for (out_l, out_r) in izip!(output_left, output_right) {
            *out_l *= self.output_volume;
            *out_r *= self.output_volume;
        }
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
    waves: &'a WavetableSet,
    filter_controller: &'a FilterController,
    gain_envelope: &'a AdsrEnvelope,
    lfo_params: &'a LfoParams,
    sample_rate: f32,
    stereo: bool,
    octave_detune: f32,
    portamento_rate: f32,
    /// Whether each oscillator needs to be rendered at all: silent oscillators that don't
    /// modulate anything are skipped.
    active: [bool; 3],
    simd: SimdLevel,
}

/// An individual note press.
// TODO: Separate phase, oscillator, and filter from note data.
pub struct Voice {
    id: u32,
    /// Samples rendered so far.
    runtime: u32,
    release_time: Option<u32>,
    osc_voice: [SuperVoice; 3],
    lfo: OscVoice,
    filter: filter::FilterContainer,
    /// Filter for the right channel, only used when the stereo width is above zero.
    filter_right: filter::FilterContainer,
    velocity: u8,
    pitch_multiply: f32,
    semitone_detune: f32,

    // Control rate state, see `update_controls()`. The deltas and gains are ramped linearly
    // from one block to the next.
    /// Each oscillator's phase delta in cycles per sample, before frequency modulation.
    osc_delta: [f32; 3],
    osc_delta_step: [f32; 3],
    /// The MIDI note whose bandlimited wavetable each oscillator reads.
    osc_note: [usize; 3],
    /// Each oscillator's output gain, before amplitude modulation.
    osc_gain: [f32; 3],
    osc_gain_step: [f32; 3],
    /// Scales each oscillator's output before it modulates the oscillator before it.
    osc_mod_scale: [f32; 3],
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
    pub fn from_midi_note(index: u8, velocity: u8, osc: &[OscillatorParams]) -> Self {
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
            lfo: Default::default(),
            velocity,
            filter: filter::FilterContainer::None,
            filter_right: filter::FilterContainer::None,
            pitch_multiply: 1.0,
            semitone_detune: 0.0,
            osc_delta: [0.0; 3],
            osc_delta_step: [0.0; 3],
            osc_note: [0; 3],
            osc_gain: [0.0; 3],
            osc_gain_step: [0.0; 3],
            osc_mod_scale: [1.0; 3],
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
            * 2.0_f32.powf(
                (((self.id as i16 - 69) as f32 + self.semitone_detune) * self.pitch_multiply)
                    / 12.0,
            )
            / sample_rate
    }
    /// The amplitude envelope's level at the given point in the voice's life, in samples.
    fn envelope(&self, envelope: &AdsrEnvelope, runtime: u32, sample_rate: f32) -> f32 {
        let index = runtime as f32 / sample_rate;
        match self.release_time {
            Some(release_time) => {
                envelope.sample_released(release_time as f32 / sample_rate, index)
            }
            None => envelope.sample_held(index),
        }
    }
    /// Recalculates the control rate state for the next `block_len` samples: the pitch, the
    /// LFO, the envelopes, keytracking and the filter coefficients. Gains and pitch ramp from
    /// their current values to the values at the end of the block.
    fn update_controls(&mut self, context: &VoiceContext, block_len: usize) {
        let sample_rate = context.sample_rate;
        let lfo_params = context.lfo_params;
        let block_start = self.runtime;
        self.runtime += block_len as u32;
        let step_scale = 1.0 / block_len as f32;
        // A new (or retriggered) voice starts from its initial values instead of ramping
        let first_block = block_start == 0;

        // Portamento, applied for the whole block at once
        self.semitone_detune *= (1.0 - context.portamento_rate).powi(block_len as i32);
        self.pitch_multiply = context.octave_detune;
        let delta = self.delta(sample_rate);

        // LFO, sampled at the end of the block
        let lfo_phase = self
            .lfo
            .add_phase(lfo_params.delta(sample_rate) * block_len as f32);
        let lfo = lfo_params.wave.generate(lfo_phase);
        let lfo_unipolar = (lfo + 1.0) / 2.0;

        let keytrack_freq = 2.0_f32.powf(
            (self.id as f32 - 69.0 + self.semitone_detune) / 12.0
                * context.filter_controller.keytrack,
        );

        for (i, osc) in context.oscillators.iter().enumerate() {
            // The LFO modulates either all oscillators or only the target. The modulation
            // amount is modulated on the target's modulator, which is the oscillator after it.
            let targeted = lfo_params.target_osc.is_none_or(|target| target == i);
            let modulator_targeted = lfo_params.target_osc.is_none_or(|target| target + 1 == i);
            let (lfo_amp, lfo_freq) = if targeted {
                (
                    lerp(1.0, lfo_unipolar, lfo_params.amp_mod),
                    lfo * lfo_params.freq_mod,
                )
            } else {
                (1.0, 0.0)
            };
            self.osc_mod_scale[i] = if modulator_targeted {
                lerp(1.0, lfo_unipolar, lfo_params.mod_mod)
            } else {
                1.0
            };

            let target_delta = osc.total_pitch_multiplier() * delta * (1.0 + lfo_freq);
            self.osc_note[i] = WavetableNotes::cycles_to_note(target_delta, sample_rate);
            let target_gain = osc.unison_amp * osc.amp * lfo_amp;
            if first_block {
                self.osc_delta[i] = target_delta;
                self.osc_gain[i] = target_gain;
            }
            self.osc_delta_step[i] = (target_delta - self.osc_delta[i]) * step_scale;
            self.osc_gain_step[i] = (target_gain - self.osc_gain[i]) * step_scale;
        }

        // Amplitude envelope and velocity
        let velocity = self.velocity as f32 / 128.0;
        if first_block {
            self.gain = self.envelope(context.gain_envelope, block_start, sample_rate) * velocity;
        }
        let target_gain = self.envelope(context.gain_envelope, self.runtime, sample_rate) * velocity;
        self.gain_step = (target_gain - self.gain) * step_scale;

        // Filter
        let controller = context.filter_controller;
        let cutoff = controller.get_cutoff(
            keytrack_freq + keytrack_freq * lfo * lfo_params.filter_mod,
            self.runtime as f32 / sample_rate,
            self.release_time,
            sample_rate,
        );
        let configure_filter = |filter: &mut FilterContainer| {
            filter.set(
                controller.filter_model,
                cutoff,
                controller.resonance,
                sample_rate,
                controller.filter_type,
            );
            filter.set_filter_type(controller.filter_type);
            filter.set_params_ramped(sample_rate, cutoff, controller.resonance, block_len);
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
            | FilterContainer::None => controller.drive.min(1.0),
            FilterContainer::LadderFilter(_) => controller.drive / 2.0,
            _ => controller.drive,
        };
    }
    /// Renders one control rate block of the oscillators into the voice's pre-filter buffers.
    fn render_oscillators(&mut self, context: &VoiceContext, block_len: usize) {
        let stereo = context.stereo;
        let oscillators = context.oscillators;
        let frames: [FramePair; 3] =
            std::array::from_fn(|i| oscillators[i].frame_pair(context.waves, self.osc_note[i]));
        let voice_counts: [usize; 3] = std::array::from_fn(|i| oscillators[i].unison_voices());

        for sample in 0..block_len.min(CONTROL_BLOCK_SAMPLES) {
            let mut out = 0.0;
            // Only used when `stereo` is set, `out` is then the left channel.
            let mut out_right = 0.0;

            // Generate the oscillators from the last to the first, since each oscillator
            // modulates the one before it. `mod_out` is the (mono) output of the oscillator
            // after the current one.
            let mut mod_out = 0.0;
            for i in (0..3).rev() {
                if !context.active[i] {
                    // Its output would be silent, and it isn't modulating anything that matters
                    mod_out = 0.0;
                    continue;
                }
                let osc = &oscillators[i];

                let mut delta = self.osc_delta[i];
                self.osc_delta[i] += self.osc_delta_step[i];
                if osc.fm > 0.0 {
                    delta = wrap_cycles_signed(delta * (1.0 + mod_out * osc.fm * FM_DEPTH));
                }

                let phase_offset = mod_out * osc.pm * PM_DEPTH_CYCLES;
                let gain = self.osc_gain[i] * lerp(1.0, (mod_out + 1.0) / 2.0, osc.am);
                self.osc_gain[i] += self.osc_gain_step[i];

                let mono = if stereo {
                    let (mono, left, right) = frames[i].advance_and_sum_panned(
                        &mut self.osc_voice[i],
                        delta,
                        &osc.detune_factors,
                        phase_offset,
                        voice_counts[i],
                        &osc.voice_mask,
                        &osc.pan_left,
                        &osc.pan_right,
                        context.simd,
                    );
                    out += left * gain;
                    out_right += right * gain;
                    mono
                } else {
                    let mono = frames[i].advance_and_sum(
                        &mut self.osc_voice[i],
                        delta,
                        &osc.detune_factors,
                        phase_offset,
                        voice_counts[i],
                        &osc.voice_mask,
                        context.simd,
                    );
                    out += mono * gain;
                    mono
                };

                mod_out = mono * self.osc_mod_scale[i];
            }

            self.pre_filter[sample] = out * self.drive;
            if stereo {
                self.pre_filter_right[sample] = out_right * self.drive;
            }
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
        for sample in 0..block_len {
            out_left[sample] += self.pre_filter[sample] * self.gain;
            out_right[sample] += if stereo {
                self.pre_filter_right[sample] * self.gain
            } else {
                self.pre_filter[sample] * self.gain
            };
            self.gain += self.gain_step;
        }
    }
}

/// Runs every voice's filter over the current block. Ladder filters are processed several
/// voices at a time when the CPU allows it.
fn filter_voices(voices: &mut [Voice], block_len: usize, stereo: bool, simd: SimdLevel) {
    #[cfg(target_arch = "x86_64")]
    if simd.has_avx() {
        for chunk in voices.chunks_mut(LADDER_LANES) {
            filter_chunk_lockstep(chunk, block_len, stereo);
        }
        return;
    }
    let _ = simd;
    for voice in voices {
        voice.filter_block(block_len, stereo);
    }
}

/// Runs a filter over a block of samples, in place.
fn filter_samples(filter: &mut FilterContainer, samples: &mut [f32]) {
    for sample in samples {
        *sample = filter.process(*sample);
    }
}

/// Filters up to [`LADDER_LANES`] voices, running their ladder filters in lockstep. Voices with
/// other filters are processed one at a time.
#[cfg(target_arch = "x86_64")]
fn filter_chunk_lockstep(chunk: &mut [Voice], block_len: usize, stereo: bool) {
    const NONE: Option<(&mut LadderFilter, &mut [f32])> = None;
    let mut left = [NONE; LADDER_LANES];
    let mut right = [NONE; LADDER_LANES];
    for (lane, voice) in chunk.iter_mut().enumerate() {
        let Voice {
            filter,
            filter_right,
            pre_filter,
            pre_filter_right,
            ..
        } = voice;
        let lockstep = |filter: &FilterContainer| {
            matches!(filter, FilterContainer::LadderFilter(ladder) if ladder.supports_lockstep())
        };
        if lockstep(filter) && (!stereo || lockstep(filter_right)) {
            if let FilterContainer::LadderFilter(ladder) = filter {
                left[lane] = Some((ladder, &mut pre_filter[..block_len]));
            }
            if stereo {
                if let FilterContainer::LadderFilter(ladder) = filter_right {
                    right[lane] = Some((ladder, &mut pre_filter_right[..block_len]));
                }
            }
        } else {
            filter_samples(filter, &mut pre_filter[..block_len]);
            if stereo {
                filter_samples(filter_right, &mut pre_filter_right[..block_len]);
            }
        }
    }
    // SAFETY: `filter_voices()` only takes this path when the CPU supports AVX.
    unsafe {
        LadderFilter::process_block_x4(left);
        if stereo {
            LadderFilter::process_block_x4(right);
        }
    }
}

pub mod oscillator;

mod envelopes {
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
                (index / self.attack_time).powf(self.attack_slope)
            } else if index - self.attack_time < self.decay_time {
                (1.0 - (index - self.attack_time) / self.decay_time).powf(self.slope)
                    * (1.0 - self.sustain_level)
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
                (1.0 - (index - release_index) / self.release_time).powf(self.slope) * level
            }
        }
    }
}

pub struct LfoParams {
    pub freq: f32,
    pub wave: OscWave,
    pub freq_mod: f32,
    pub amp_mod: f32,
    pub mod_mod: f32,
    pub filter_mod: f32,
    pub target_osc: Option<usize>,
}
impl LfoParams {
    fn delta(&self, sample_rate: f32) -> f32 {
        2.0 * PI * self.freq / sample_rate
    }
}
impl Default for LfoParams {
    fn default() -> Self {
        Self {
            freq: 5.0,
            wave: OscWave::Sine,
            freq_mod: 0.0,
            amp_mod: 0.0,
            mod_mod: 0.0,
            filter_mod: 0.0,
            target_osc: None,
        }
    }
}

pub(crate) mod filter;

#[inline]
fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    (to - from).mul_add(amount, from)
}
