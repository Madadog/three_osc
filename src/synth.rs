use std::collections::hash_map::DefaultHasher;
use std::convert::TryFrom;
use std::convert::TryInto;
use std::f32::consts::PI;
use std::hash::Hash;
use std::hash::Hasher;

const DEFAULT_SRATE: f32 = 44100.0;

pub struct ThreeOsc {
    pub voices: Vec<Voice>,
    pub gain_envelope: AdsrEnvelope,
    pub sample_rate: f64,
    pub output_volume: f32,
}

impl ThreeOsc {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            voices: Vec::with_capacity(16),
            gain_envelope: AdsrEnvelope::new(0.0, 0.5, 0.05, 1.0, 1.0),
            sample_rate,
            output_volume: 0.3,
        }
    }
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.voices.push(Voice::from_midi_note(note, self.sample_rate as f32))
    }
    pub fn note_off(&mut self, note: u8, velocity: u8) {
        self.voices.iter_mut().filter(|voice| voice.id == note as u32).for_each(|voice| voice.release())
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
    pub fn run(&mut self, output: std::iter::Zip<std::slice::IterMut<f32>, std::slice::IterMut<f32>>) {
        output.for_each(|(out_l, out_r)| {
            self.release_voices();
            for voice in self.voices.iter_mut() {
                let phase = voice.advance();
                let envelope_index = voice.runtime as f32 / self.sample_rate as f32;
                let out = if let Some(release_time) = voice.release_time {
                    let release_index = release_time as f32 / self.sample_rate as f32;
                    phase.sin() * self.gain_envelope.sample_released(release_index, envelope_index)
                } else {
                    phase.sin() * self.gain_envelope.sample_held(envelope_index)
                } * self.output_volume;
                *out_l += out;
                *out_r += out;
            }
        })
    }
}

pub struct Voice {
    id: u32, // from hashing note index
    delta: f32,
    phase: f32,
    runtime: u32,
    release_time: Option<u32>,
}
impl Voice {
    pub fn from_midi_note(index: u8, sample_rate: f32) -> Self {
        //let mut hasher = DefaultHasher::default();
        Self {
            id: {
                // index.hash(&mut hasher);
                // hasher.finish() as u32

                index.into() // Don't bother with hash
            },
            delta: (2.0 * PI * 440.0 * 2.0_f32.powf((index as i16 - 69) as f32 / 12.0)) / sample_rate,
            phase: 0.0,
            runtime: 0,
            release_time: None,
        }
    }
    pub fn release(&mut self) {
        if let None = self.release_time {
            self.release_time = Some(self.runtime)
        }
    }
    pub fn advance(&mut self) -> f32 {
        self.phase = (self.phase + self.delta) % (2.0 * PI);
        self.runtime += 1;
        self.phase
    }
}

pub struct AdsrEnvelope {
    pub attack_time: f32,
    pub decay_time: f32,
    pub release_time: f32,
    pub sustain_level: f32,
    pub slope: f32,
}
impl AdsrEnvelope {
    pub fn new(attack_time: f32, decay_time: f32, release_time: f32, sustain_level: f32, slope: f32) -> Self {
        Self {
            attack_time,
            decay_time,
            release_time,
            sustain_level,
            slope,
        }
    }
    /// Returns a point between 0.0 and 1.0
    pub fn sample_held(&self, index: f32) -> f32 {
        if index <= self.attack_time {
            (index / self.attack_time).powf(self.slope)
        } else if index - self.attack_time <= self.decay_time {
            (1.0 + (self.sustain_level - 1.0) * ((index - self.attack_time) / self.decay_time).powf(self.slope))
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
            (level - level * ((index - release_index) / self.release_time).powf(self.slope))
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