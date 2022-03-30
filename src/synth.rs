use std::collections::hash_map::DefaultHasher;
use std::convert::TryFrom;
use std::convert::TryInto;
use std::f32::consts::PI;
use std::hash::Hash;
use std::hash::Hasher;

use self::envelopes::AdsrEnvelope;
use self::oscillator::BasicOscillator;
use self::oscillator::OscVoice;
use self::oscillator::naive_saw;

const DEFAULT_SRATE: f32 = 44100.0;

pub struct ThreeOsc {
    pub voices: Vec<Voice>,
    pub gain_envelope: AdsrEnvelope,
    pub sample_rate: f64,
    pub output_volume: f32,
    pub oscillators: [BasicOscillator; 1],
    pub filter: TestFilter,
}

impl ThreeOsc {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            voices: Vec::with_capacity(32),
            gain_envelope: AdsrEnvelope::new(0.0, 0.5, 0.05, 1.0, 1.0),
            sample_rate,
            output_volume: 0.3,
            oscillators: [BasicOscillator::default()],
            filter: TestFilter::default(),
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
                voice.advance();
                let envelope_index = voice.runtime as f32 / self.sample_rate as f32;
                let mut out = 0.0;
                for oscillator in self.oscillators.iter() {
                    out += oscillator.unison(&mut voice.osc_voice, |x| oscillator.wave.generate(x));
                }
                out = if let Some(release_time) = voice.release_time {
                    let release_index = release_time as f32 / self.sample_rate as f32;
                    out * self.gain_envelope.sample_released(release_index, envelope_index)
                } else {
                    out * self.gain_envelope.sample_held(envelope_index)
                };
                *out_l += out;
                *out_r += out;
            }
            *out_l = self.filter.process(*out_l) * self.output_volume;
            *out_r = *out_l;
        })
    }
}

/// An individual note press. 
pub struct Voice {
    id: u32,
    runtime: u32,
    release_time: Option<u32>,
    osc_voice: [OscVoice; 128],
}
impl Voice {
    pub fn from_midi_note(index: u8, sample_rate: f32) -> Self {
        let mut osc_voice = [OscVoice::new(0.0, (2.0 * PI * 440.0 * 2.0_f32.powf((index as i16 - 69) as f32 / 12.0)) / sample_rate); 128];
        let rng = fastrand::Rng::new();
        
        for voice in osc_voice.iter_mut() {
            voice.phase += rng.f32() * 2.0 * PI;
        }
        Self {
            id: index.into(),
            runtime: 0,
            release_time: None,
            osc_voice,
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

pub mod oscillator {
    use std::f32::consts::{PI, FRAC_1_SQRT_2};

    /// A single instance of a playing oscillator. Maintains phase for fm / pm stuff 
    #[derive(Debug, Clone, Copy)]
    pub struct OscVoice {
        pub phase: f32,
        pub delta: f32,
    }
    impl OscVoice {
        pub fn new(phase: f32, delta: f32) -> Self {
            Self { phase, delta }
        }
        pub fn add_phase(&mut self, delta: f32) -> f32 {
            self.phase = (self.phase + delta) % (2.0 * PI);
            self.phase
        }
    }
    pub struct BasicOscillator {
        pub amp: f32,
        pub semitone: f32,
        pub exponent: i32,
        pub voice_count: u8,
        pub voices_detune: f32,
        pub wave: OscWave,
    }
    impl BasicOscillator {
        fn mult_delta(&self, delta: f32) -> f32 {
            delta * 2.0_f32.powi(self.exponent) * 2.0_f32.powf(self.semitone / 12.0)
        }
        pub fn unison<T: Fn(f32) -> f32>(&self, voices: &mut [OscVoice], wave: T) -> f32 {
            let main_delta = self.mult_delta(voices[0].delta);
            voices.iter_mut().take(self.voice_count.into()).enumerate().map(|(i, voice)| {
                let i = i as f32 - (i % 2) as f32 * 2.0;
                let delta = main_delta + main_delta * i as f32 * self.voices_detune / (self.voice_count as f32);
                voice.phase = voice.add_phase(delta);
                wave(voice.phase)
            }).sum::<f32>() * self.amp / self.voice_count as f32
        }
    }
    impl Default for BasicOscillator {
        fn default() -> Self {
            Self { amp: 1.0, semitone: Default::default(), exponent: Default::default(), voice_count: 1, voices_detune: 0.1, wave: OscWave::Sine}
        }
    }
    pub enum OscWave {
        Sine,
        Tri,
        Saw,
        Exp,
        Square,
        PulseQuarter,
        PulseEighth,
    }
    impl OscWave {
        pub fn generate(&self, phase: f32) -> f32 {
            use OscWave::*;
            match self {
                Sine => phase.sin(),
                Tri => if phase <= PI {(phase / PI) * 2.0 - 1.0} else {((-phase + 2.0 * PI) / PI) * 2.0 - 1.0}
                Saw => (phase - PI) / (2.0 * PI),
                Exp => phase.sin().abs() - 0.55,
                Square => if phase <= PI { FRAC_1_SQRT_2 } else { -FRAC_1_SQRT_2 },
                PulseQuarter => if phase <= std::f32::consts::FRAC_PI_2 { FRAC_1_SQRT_2 } else { -FRAC_1_SQRT_2 },
                PulseEighth => if phase <= std::f32::consts::FRAC_PI_4 { FRAC_1_SQRT_2 } else { -FRAC_1_SQRT_2 },
            }
        }
    }
    pub fn naive_saw(phase: f32) -> f32 {
        (phase - PI) / (2.0 * PI)
    }
}

mod envelopes {
    use lyon_geom::{CubicBezierSegment, Monotonic};

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
        /// Returns the envelope CV (between 0.0 and 1.0) associated with the given index
        pub fn sample_held(&self, index: f32) -> f32 {
            if index <= self.attack_time {
                (index / self.attack_time).powf(self.slope)
            } else if index - self.attack_time <= self.decay_time {
                (1.0 - (index - self.attack_time) / self.decay_time).powf(self.slope) * (1.0 - self.sustain_level) + self.sustain_level
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
    impl BezierEnvelope {

    }
}

#[derive(Debug, Default)]
pub struct TestFilter {
    stage0: f32,
    stage1: f32,
    pub input0: f32,
    pub input1: f32,
    pub feedback0: f32,
    pub feedback1: f32,
    pub feedback0_1: f32,
    pub feedback1_0: f32,
}
impl TestFilter {
    fn process(&mut self, input: f32) -> f32 {
        if self.stage0.is_finite() && self.stage1.is_finite() {
            self.stage0 = input * self.input0 + self.stage0 * -self.feedback0 + self.stage0 * -self.feedback1_0; 
            self.stage1 = self.stage1 * -self.feedback1 + self.stage0 * -self.feedback0_1;
        } else {
            println!("Warning: filters were {} and {}", self.stage0, self.stage1);
            self.stage0 = 0.0; 
            self.stage1 = 0.0; 
        }
        self.stage1 + input
    }
}