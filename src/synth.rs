use std::collections::hash_map::DefaultHasher;
use std::convert::TryFrom;
use std::convert::TryInto;
use std::f32::consts::PI;
use std::f64::consts::PI as PI64;
use std::hash::Hash;
use std::hash::Hasher;

use self::envelopes::AdsrEnvelope;
use self::filter::Filter;
use self::filter::FilterModel;
use self::notes::MidiNote;
use self::notes::Notes;
use self::oscillator::AdditiveOsc;
use self::oscillator::SimpleSin;
use self::oscillator::SuperVoice;
use self::oscillator::BasicOscillator;
use self::oscillator::OscVoice;
use self::oscillator::Wavetable;
use self::oscillator::WavetableNotes;
use self::oscillator::WavetableSet;
use self::oscillator::modulate_delta;

const DEFAULT_SRATE: f32 = 44100.0;

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
    additive: AdditiveOsc,
    pub bend_range: f32,
    pub polyphony: Polyphony,
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
            oscillators: [BasicOscillator::default(), BasicOscillator::default(), BasicOscillator::default()],
            // wavetables: WavetableNotes::from_additive_osc_2(&AdditiveOsc::saw(), sample_rate as f32, 1.0, 2048),
            wavetables: WavetableNotes::from_additive_osc(&AdditiveOsc::saw(), sample_rate as f32, 32.0, 2048, 256),
            waves: WavetableSet::new(sample_rate as f32, 32.0, 2048, 256),
            additive: AdditiveOsc::saw(),
            bend_range: 2.0,
            polyphony: Polyphony::Legato,
        }
    }
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.notes.note_on(note, velocity);
        
        match &self.polyphony {
            Polyphony::Polyphonic => {
                self.voices
                    .push(Voice::from_midi_note(note, velocity, self.sample_rate as f32, &self.oscillators))
                },
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
                        .push(Voice::from_midi_note(note, velocity, self.sample_rate as f32, &self.oscillators))
                }
            },
        }
    }
    pub fn note_off(&mut self, note: u8, velocity: u8) {
        self.notes.note_off(note);

        match self.polyphony {
            Polyphony::Polyphonic => {
                self.voices
                .iter_mut()
                .filter(|voice| voice.id == note as u32)
                .for_each(|voice| voice.release())
            },
            // TODO: If there are two notes each with their own voice playing, when one is released
            // it will snap to the other note, resulting in two voices playing the same note at the
            // same time, which sounds bad. Stop this behaviour by filtering out notes with voices
            // currently playing.
            Polyphony::Legato | Polyphony::Monophonic => {
                self.voices
                .iter_mut()
                .filter(|voice| voice.id == note as u32)
                .for_each(|voice| {
                    let nearest_note = self.notes.notes.iter()
                        .reduce(|accum, note| {
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
                })
            },
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
        self.filter_controller.interpolate_cutoff(0.15 * 44100.0 / self.sample_rate as f32);
        for voice in self.voices.iter_mut() {
            voice.advance();
            let envelope_index = voice.runtime as f32 / self.sample_rate as f32;
            let mut out = 0.0;
            let velocity = voice.velocity as f32 / 128.0;
            let index = voice.id as usize;
    
            let osc3_out = if let Some(osc) = self.oscillators.get(2) {
                let delta = osc.pitch_mult_delta(voice.voice_delta(self.sample_rate as f32));
                let phases = osc.unison_phases(&mut voice.osc_voice[2], delta);
                let osc_out = self.waves.select(&osc.wave).tables[index].generate_multi(phases, osc.voice_count.into()) / osc.voice_count as f32;
                out += osc_out * osc.amp * velocity;
                osc_out
            } else {
                unreachable!("Osc3 wasn't found...");
            };
            
            let osc2_out = if let Some(osc) = self.oscillators.get(1) {
                let delta = osc.pitch_mult_delta(voice.voice_delta(self.sample_rate as f32));
                let delta = modulate_delta(delta, osc3_out * osc.pm, osc3_out * osc.fm, self.sample_rate as f32);
                let phases = osc.unison_phases(&mut voice.osc_voice[1], delta);
                let osc_out = self.waves.select(&osc.wave).tables[index].generate_multi(phases, osc.voice_count.into()) / osc.voice_count as f32;
                out += osc_out * osc.amp * velocity;
                osc_out
            } else {
                unreachable!("Osc2 wasn't found...");
            };
            
            if let Some(osc) = self.oscillators.get(0) {
                let delta = osc.pitch_mult_delta(voice.voice_delta(self.sample_rate as f32));
                let delta = modulate_delta(delta, osc2_out * osc.pm, osc2_out * osc.fm, self.sample_rate as f32).abs();
                let phases = osc.unison_phases(&mut voice.osc_voice[0], delta);

                let osc_out = self.waves.select(&osc.wave).tables[index].generate_multi(phases, osc.voice_count.into()) / osc.voice_count as f32;
                // let osc_out = self.additive.generate(phases[0], (22050.0/(2.0*440.0 * 2.0_f32.powf((voice.id as f32 - 69.0) / 12.0))) as usize);
                out += osc_out * osc.amp * velocity;
            }
            let voice_freq = 2.0_f32.powf((voice.id as f32 - 69.0) / 12.0 * self.filter_controller.keytrack);
            
            voice.filter.set(self.filter_controller.filter_model);
            voice.filter.set_filter_type(self.filter_controller.filter_type);

            match voice.filter {
                // Hack to keep biquad filter / none at same level as others
                filter::FilterContainer::BiquadFilter(_)
                | filter::FilterContainer::None => self.filter_controller.drive = self.filter_controller.drive.min(1.0),
                _ => {}
            };
            
            // filter envelope
            out = if let Some(release_time) = voice.release_time {
                let release_index = release_time as f32 / self.sample_rate as f32;
                self.filter_controller.process_envelope_released(&mut voice.filter, voice_freq, out, envelope_index, release_index, self.sample_rate as f32)
            } else {
                self.filter_controller.process_envelope_held(&mut voice.filter, voice_freq, out, envelope_index, self.sample_rate as f32)
            };

            
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
}
impl Voice {
    pub fn from_midi_note(index: u8, velocity: u8, sample_rate: f32, osc: &[BasicOscillator]) -> Self {
        // w = (2pi*f) / sample_rate
        let mut osc_voice = [SuperVoice::new(
            // MidiNote::midi_to_delta(index, sample_rate),
            osc[0].phase,
            osc[0].phase_rand,
        ),
        SuperVoice::new(
            // MidiNote::midi_to_delta(index, sample_rate),
            osc[1].phase,
            osc[1].phase_rand,
        ),
        SuperVoice::new(
            // MidiNote::midi_to_delta(index, sample_rate),
            osc[2].phase,
            osc[2].phase_rand,
        ),
        ];
 
        Self {
            id: index.into(),
            runtime: 0,
            release_time: None,
            osc_voice,
            velocity,
            filter: filter::FilterContainer::None,
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
        2.0 * PI * 440.0 * 2.0_f32.powf(((self.id as i16 - 69) as f32) / 12.0) / sample_rate
    }
}



fn delta(sample_rate: f64) -> f64 {
    1.0 / sample_rate
}

/// converts a certain number of samples to an f32 time in seconds
fn samples_to_time(samples: u32, delta: f64) -> f64 {
    samples as f64 * delta
}
fn time_to_samples(time: f64, delta: f64) -> u32 {
    (time / delta) as u32
}

pub mod oscillator;

mod envelopes {
    use lyon_geom::{CubicBezierSegment, Monotonic};

    #[derive(Debug, Clone)]
    pub struct AdsrEnvelope {
        pub attack_time: f32,
        pub decay_time: f32,
        pub release_time: f32,
        pub sustain_level: f32,
        pub slope: f32,
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
                slope,
            }
        }
        /// Returns the envelope CV (between 0.0 and 1.0) associated with the given index
        pub fn sample_held(&self, index: f32) -> f32 {
            if index <= self.attack_time {
                (index / self.attack_time).powf(self.slope)
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
        /// Modifies the envelope to prevent negative times and sustain levels outside of 0 to 1
        pub fn limits(&mut self) {
            self.attack_time = self.attack_time.max(0.0);
            self.decay_time = self.decay_time.max(0.0);
            self.release_time = self.release_time.max(0.0);
            self.sustain_level = self.sustain_level.clamp(0.0, 1.0);
            self.slope = self.sustain_level.max(0.0001);
        }
    }

    /// An ADSR envelope with adjustable curves
    pub struct BezierEnvelope {
        attack: Monotonic<CubicBezierSegment<f32>>,
        decay: Monotonic<CubicBezierSegment<f32>>,
        release: Monotonic<CubicBezierSegment<f32>>,
        sustain_level: f32,
    }
    impl BezierEnvelope {}
}



pub(crate) mod filter;

#[inline]
fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    (to - from).mul_add(amount, from)
}
