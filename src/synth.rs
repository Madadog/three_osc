use std::f32::consts::PI;

use self::envelopes::AdsrEnvelope;
use self::filter::Filter;
use self::notes::Notes;
use self::oscillator::modulate_delta;
use self::oscillator::AdditiveOsc;
use self::oscillator::BasicOscillator;
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
    pub oscillators: [BasicOscillator; 3],
    pub wavetables: WavetableNotes,
    pub waves: WavetableSet,
    pub bend_range: f32,
    pub polyphony: Polyphony,
    pub octave_detune: f32,
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
                BasicOscillator::default(),
                BasicOscillator::default(),
                BasicOscillator::default(),
            ],
            // wavetables: WavetableNotes::from_additive_osc_2(&AdditiveOsc::saw(), sample_rate as f32, 1.0, 2048),
            wavetables: WavetableNotes::from_additive_osc(
                &AdditiveOsc::saw(),
                sample_rate as f32,
                32.0,
                2048,
                256,
            ),
            waves: WavetableSet::new(sample_rate as f32, 32.0, 2048, 256),
            bend_range: 2.0,
            polyphony: Polyphony::Legato,
            octave_detune: 1.0,
        }
    }
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.notes.note_on(note, velocity);

        match &self.polyphony {
            Polyphony::Polyphonic => {
                self.voices
                    .push(Voice::from_midi_note(note, velocity, &self.oscillators))
            }
            polyphony => {
                if let Some(voice) = self.voices.last_mut() {
                    voice.id = note as u32;
                    voice.velocity = velocity;

                    if matches!(polyphony, Polyphony::Monophonic) {
                        // Monophonic always re-presses notes.
                        voice.release_time = None;
                        voice.runtime = 0;
                    } else if let Some(_) = voice.release_time {
                        // If the note is released, re-press it.
                        voice.release_time = None;
                        voice.runtime = 0;
                    }
                } else {
                    self.voices
                        .push(Voice::from_midi_note(note, velocity, &self.oscillators))
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
                    let nearest_note = self.notes.notes.iter().reduce(|accum, note| {
                        if note.age() < accum.age() {
                            note
                        } else {
                            accum
                        }
                    });
                    if let Some(note) = nearest_note {
                        voice.id = note.id as u32;
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
    pub fn run(
        &mut self,
        output: std::iter::Zip<std::slice::IterMut<f32>, std::slice::IterMut<f32>>,
    ) {
        output.for_each(|(out_l, out_r)| {
            self.run_voices(out_l, out_r);
        })
    }
    pub fn run_voices(&mut self, out_l: &mut f32, out_r: &mut f32) {
        self.release_voices();
        self.filter_controller.cutoff = self.filter_controller.target_cutoff;
        for voice in self.voices.iter_mut() {
            voice.advance();
            let envelope_index = voice.runtime as f32 / self.sample_rate as f32;
            let mut out = 0.0;
            let velocity = voice.velocity as f32 / 128.0;
            let index = voice.id as usize;
            voice.detune = self.octave_detune;

            let osc3_out = if let Some(osc) = self.oscillators.get(2) {
                let delta = osc.pitch_mult_delta(voice.voice_delta(self.sample_rate as f32));
                let phases = voice.osc_voice[2].unison_phases(
                    delta,
                    osc.voice_count.into(),
                    osc.voices_detune,
                );
                let osc_out = self.waves.select(&osc.wave).tables[index]
                    .generate_multi(phases, osc.voice_count.into())
                    / osc.voice_count as f32;
                out += osc_out * osc.amp * velocity;
                osc_out
            } else {
                unreachable!("Osc3 wasn't found...");
            };

            let osc2_out = if let Some(osc) = self.oscillators.get(1) {
                let delta = osc.pitch_mult_delta(voice.voice_delta(self.sample_rate as f32));
                let delta = modulate_delta(delta, osc3_out * osc.fm, 0.0, self.sample_rate as f32);
                let phases = voice.osc_voice[1].unison_phases_pm(
                    delta,
                    osc.voice_count.into(),
                    osc.voices_detune,
                    osc3_out * osc.pm,
                );

                let mut osc_out = self.waves.select(&osc.wave).tables[index]
                    .generate_multi(&phases, osc.voice_count.into())
                    / osc.voice_count as f32;
                osc_out *= lerp(1.0, osc3_out, osc.am);
                out += osc_out * osc.amp * velocity;
                osc_out
            } else {
                unreachable!("Osc2 wasn't found...");
            };

            if let Some(osc) = self.oscillators.get(0) {
                let delta = osc.pitch_mult_delta(voice.voice_delta(self.sample_rate as f32));
                let delta =
                    modulate_delta(delta, osc2_out * osc.fm, 0.0, self.sample_rate as f32).abs();
                let phases = voice.osc_voice[0].unison_phases_pm(
                    delta,
                    osc.voice_count.into(),
                    osc.voices_detune,
                    osc2_out * osc.pm,
                );

                let osc_out = self.waves.select(&osc.wave).tables[index]
                    .generate_multi(&phases, osc.voice_count.into())
                    / osc.voice_count as f32;
                out += osc_out * osc.amp * velocity * lerp(1.0, osc2_out, osc.am);
            }

            // Update filter controls, calculate keytrack
            let voice_freq =
                2.0_f32.powf((voice.id as f32 - 69.0) / 12.0 * self.filter_controller.keytrack);

            let cutoff = self.filter_controller.get_cutoff(
                voice_freq,
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

            match voice.filter {
                // Biquad filter / none are unaffected by drive, so we clamp it between 0 and 1 to
                // keep the levels the same when switching filter.
                filter::FilterContainer::BiquadFilter(_) | filter::FilterContainer::None => {
                    self.filter_controller.drive = self.filter_controller.drive.min(1.0)
                }
                _ => {}
            };

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
        *out_l *= self.output_volume;
        *out_r *= self.output_volume;
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
    filter: filter::FilterContainer,
    velocity: u8,
    detune: f32,
}
impl Voice {
    pub fn from_midi_note(index: u8, velocity: u8, osc: &[BasicOscillator]) -> Self {
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
            velocity,
            filter: filter::FilterContainer::None,
            detune: 0.0,
        }
    }
    pub fn release(&mut self) {
        if let None = self.release_time {
            self.release_time = Some(self.runtime)
        }
    }
    pub fn advance(&mut self) {
        self.runtime += 1;
    }
    pub fn voice_delta(&self, sample_rate: f32) -> f32 {
        2.0 * PI * 440.0 * 2.0_f32.powf(((self.id as i16 - 69) as f32 * self.detune) / 12.0)
            / sample_rate //* (1.0 + self.detune)
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

pub(crate) mod filter;

#[inline]
fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    (to - from).mul_add(amount, from)
}
