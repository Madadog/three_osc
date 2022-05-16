use std::f32::consts::PI;

use itertools::izip;

use self::envelopes::AdsrEnvelope;
use self::filter::Filter;
use self::filter::FilterContainer;
use self::notes::Notes;
use self::oscillator::OscVoice;
use self::oscillator::OscWave;
use self::oscillator::modulate_delta;
use self::oscillator::AdditiveOsc;
use self::oscillator::OscillatorParams;
use self::oscillator::SuperVoice;
use self::oscillator::WavetableNotes;
use self::oscillator::WavetableSet;

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

        // Minor optimisation: Semitone / octave / pitch multiplier offset is calculated and cached
        // once per run, not once per sample
        self.oscillators[0].update_total_pitch();
        self.oscillators[1].update_total_pitch();
        self.oscillators[2].update_total_pitch();

        let lfo_delta = self.lfo_params.delta(self.sample_rate as f32);

        let osc1_unison_amp = 1.0 / (self.oscillators[0].voice_count as f32).sqrt();
        let osc2_unison_amp = 1.0 / (self.oscillators[1].voice_count as f32).sqrt();
        let osc3_unison_amp = 1.0 / (self.oscillators[2].voice_count as f32).sqrt();

        // Write samples from all voices
        for voice in self.voices.iter_mut() {
            let velocity = voice.velocity as f32 / 128.0;

            voice.pitch_multiply = self.octave_detune;

            match voice.filter {
                // Biquad filter / none are unaffected by drive, so we clamp it between 0 and 1 to
                // keep the levels the same when switching filter.
                FilterContainer::BiquadFilter(_) | FilterContainer::None => {
                    self.filter_controller.drive = self.filter_controller.drive.min(1.0)
                }
                _ => {}
            };

            for (out_l, out_r) in izip!(output_left.iter_mut(), output_right.iter_mut()) {
                voice.semitone_detune = lerp(voice.semitone_detune, 0.0, self.portamento_rate);

                let delta = voice.delta(self.sample_rate as f32);
                let osc3_delta = delta * self.oscillators[2].total_pitch_multiplier();
                let osc2_delta = delta * self.oscillators[1].total_pitch_multiplier();
                let osc1_delta = delta * self.oscillators[0].total_pitch_multiplier();

                for osc in self.oscillators.iter_mut() {
                    if let OscWave::Pulse { width } = &mut osc.wave {
                        *width = osc.pulse_width;
                    }
                }

                let lfo_phase = voice.lfo.add_phase(lfo_delta);
                let lfo = self.lfo_params.wave.generate(lfo_phase);
                
                // set / bypass modulation depending on LFO target 
                let (
                    (osc1_delta, osc2_delta, osc3_delta),
                    (osc1_lfo_amp, osc2_lfo_amp, osc3_lfo_amp),
                    (osc1_lfo_mod, osc2_lfo_mod)
                ) = match self.lfo_params.target_osc {
                    Some(0) => {((osc1_delta + osc1_delta * lfo * self.lfo_params.freq_mod, osc2_delta, osc3_delta),
                        (lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.amp_mod), 1.0, 1.0),
                        (lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.mod_mod), 1.0),
                    )},
                    Some(1) => {((osc1_delta, osc2_delta + osc2_delta * lfo * self.lfo_params.freq_mod, osc3_delta),
                        (1.0, lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.amp_mod), 1.0),
                        (1.0, lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.mod_mod)),
                    )},
                    Some(2) => {((osc1_delta, osc2_delta, osc3_delta + osc3_delta * lfo * self.lfo_params.freq_mod),
                        (1.0, lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.amp_mod), 1.0),
                        (1.0, 1.0),
                    )},
                    // no target / invalid target: default to modulating all
                    _ => {(
                        (osc1_delta + osc1_delta * lfo * self.lfo_params.freq_mod,
                            osc2_delta + osc2_delta * lfo * self.lfo_params.freq_mod,
                            osc3_delta + osc3_delta * lfo * self.lfo_params.freq_mod),
                        (lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.amp_mod),
                            lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.amp_mod),
                            lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.amp_mod)),
                        (lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.mod_mod),
                            lerp(1.0, (lfo + 1.) / 2.0, self.lfo_params.mod_mod),)
                    )}
                };

                let keytrack_freq = 2.0_f32.powf(
                    (voice.id as f32 - 69.0 + voice.semitone_detune) / 12.0
                        * self.filter_controller.keytrack,
                );

                let mut out = 0.0;

                voice.advance();
                let envelope_index = voice.runtime as f32 / self.sample_rate as f32;

                let osc3_out = if self.oscillators[2].amp > 0.0
                || self.oscillators[1].am > 0.0
                || self.oscillators[1].pm > 0.0
                || self.oscillators[1].fm > 0.0 {
                    let osc = &self.oscillators[2];
                    let phases = voice.osc_voice[2].unison_phases(
                        osc3_delta,
                        osc.voice_count.into(),
                        osc.voices_detune,
                    );
                    let mut osc_out = self.waves.select(&osc.wave).delta_index(osc3_delta, self.sample_rate as f32)
                        .generate_multi(phases, osc.voice_count.into())
                        * osc3_unison_amp;
                    // if pulse wave, subtract 2 saw waves
                    if let OscWave::Pulse { width } = osc.wave {
                        osc_out -= self.waves.select(&osc.wave).delta_index(osc3_delta, self.sample_rate as f32)
                        .generate_multi_pm(phases, osc.voice_count.into(), width)
                        * osc3_unison_amp;
                    }
                    out += osc_out * osc.amp * velocity * osc3_lfo_amp;
                    osc_out * osc2_lfo_mod
                } else { 0.0 };

                let osc2_out = if self.oscillators[1].amp > 0.0
                || self.oscillators[0].am > 0.0
                || self.oscillators[0].pm > 0.0
                || self.oscillators[0].fm > 0.0 {
                    let osc = &self.oscillators[1];
                    let delta = modulate_delta(osc2_delta, osc3_out * osc.fm);
                    let phases = voice.osc_voice[1].unison_phases(
                        delta,
                        osc.voice_count.into(),
                        osc.voices_detune,
                    );

                    let mut osc_out = self.waves.select(&osc.wave).delta_index(osc2_delta, self.sample_rate as f32)
                        .generate_multi_pm(&phases, osc.voice_count.into(), osc3_out * osc.pm * 150.0)
                        * osc2_unison_amp;

                    osc_out *= lerp(1.0, (osc3_out + 1.0) / 2.0, osc.am);
                    out += osc_out * osc.amp * velocity * osc2_lfo_amp;
                    osc_out * osc1_lfo_mod
                } else {0.0};

                if self.oscillators[0].amp > 0.0 {
                    let osc = &self.oscillators[0];
                    let delta = modulate_delta(osc1_delta, osc2_out * osc.fm).abs();
                    let phases = voice.osc_voice[0].unison_phases(
                        delta,
                        osc.voice_count.into(),
                        osc.voices_detune,
                    );

                    let mut osc_out = self.waves.select(&osc.wave).delta_index(osc1_delta, self.sample_rate as f32)
                        .generate_multi_pm(
                        &phases,
                        osc.voice_count.into(),
                        osc2_out * osc.pm * 150.0,
                    ) * osc1_unison_amp;

                    out += osc_out * osc.amp * velocity * lerp(1.0, (osc2_out + 1.0) / 2.0, osc.am) * osc1_lfo_amp;
                }

                // Update filter controls
                let cutoff = self.filter_controller.get_cutoff(
                    keytrack_freq + keytrack_freq * lfo * self.lfo_params.filter_mod,
                    envelope_index,
                    voice.release_time,
                    self.sample_rate as f32,
                );

                
                voice.filter.set(
                    self.filter_controller.filter_model,
                    cutoff,
                    self.filter_controller.resonance,
                    self.sample_rate as f32,
                    self.filter_controller.filter_type,
                );

                voice
                    .filter
                    .set_filter_type(self.filter_controller.filter_type);
                
                // Process filter
                voice.filter.set_params(
                    self.sample_rate as f32,
                    cutoff,
                    self.filter_controller.resonance,
                );

                out = voice.filter.process(out * self.filter_controller.drive);

                // amplitude envelope
                out = if let Some(release_time) = voice.release_time {
                    let release_index = release_time as f32 / self.sample_rate as f32;
                    out * self
                        .gain_envelope
                        .sample_released(release_index, envelope_index)
                } else {
                    out * self.gain_envelope.sample_held(envelope_index)
                };

                *out_l += out;
                *out_r += out;
            }
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

/// An individual note press.
// TODO: Separate phase, oscillator, and filter from note data.
pub struct Voice {
    id: u32,
    runtime: u32,
    release_time: Option<u32>,
    osc_voice: [SuperVoice; 3],
    lfo: OscVoice,
    filter: filter::FilterContainer,
    velocity: u8,
    pitch_multiply: f32,
    semitone_detune: f32,
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
            pitch_multiply: 1.0,
            semitone_detune: 0.0,
        }
    }
    pub fn release(&mut self) {
        if self.release_time.is_none() {
            self.release_time = Some(self.runtime)
        }
    }
    pub fn advance(&mut self) {
        self.runtime += 1;
    }
    pub fn delta(&self, sample_rate: f32) -> f32 {
        2.0 * PI
            * 440.0
            * 2.0_f32.powf(
                (((self.id as i16 - 69) as f32 + self.semitone_detune) * self.pitch_multiply)
                    / 12.0,
            )
            / sample_rate
    }
    pub fn delta_with_oscillator(&self, sample_rate: f32, oscillator: &OscillatorParams) -> f32 {
        2.0 * PI
            * 440.0
            * 2.0_f32.powf(
                (((self.id as i16 - 69) as f32
                    + self.semitone_detune
                    + oscillator.semitone_detune())
                    * self.pitch_multiply)
                    / 12.0,
            )
            / sample_rate
            * oscillator.pitch_multiplier
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
        pub fn sample_held(&self, index: f32) -> f32 {
            if index <= self.attack_time {
                (index / self.attack_time).powf(self.attack_slope)
            } else if index - self.attack_time <= self.decay_time {
                (1.0 - (index - self.attack_time) / self.decay_time).powf(self.slope)
                    * (1.0 - self.sustain_level)
                    + self.sustain_level
            } else {
                self.sustain_level
            }
        }
        pub fn sample_released(&self, release_index: f32, index: f32) -> f32 {
            assert!(release_index <= index);
            if index - release_index > self.release_time {
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
