use std::collections::hash_map::DefaultHasher;
use std::convert::TryFrom;
use std::convert::TryInto;
use std::f32::consts::PI;
use std::hash::Hash;
use std::hash::Hasher;

use self::envelopes::AdsrEnvelope;
use self::oscillator::SuperVoice;
use self::oscillator::naive_saw;
use self::oscillator::BasicOscillator;
use self::oscillator::OscVoice;

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
        self.voices
            .push(Voice::from_midi_note(note, self.sample_rate as f32, &self.oscillators))
    }
    pub fn note_off(&mut self, note: u8, velocity: u8) {
        self.voices
            .iter_mut()
            .filter(|voice| voice.id == note as u32)
            .for_each(|voice| voice.release())
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
                    out * self
                        .gain_envelope
                        .sample_released(release_index, envelope_index)
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
    osc_voice: SuperVoice,
}
impl Voice {
    pub fn from_midi_note(index: u8, sample_rate: f32, osc: &[BasicOscillator]) -> Self {
        let mut osc_voice = SuperVoice::new((2.0 * PI * 440.0 * 2.0_f32.powf(((index as i16 - 69) as f32) / 12.0)) / sample_rate,
            osc[0].phase,
            osc[0].phase_rand,
        );
 
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
    use std::f32::consts::{FRAC_1_SQRT_2, PI};

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
    pub struct SuperVoice {
        pub voice_phases: [f32; 128],
        pub delta: f32,
    }
    impl SuperVoice {
        pub fn new(delta: f32, phase: f32, phase_random: f32) -> Self {
            let mut voice_phases = [phase; 128];
            let rng = fastrand::Rng::new();

            for phase in voice_phases.iter_mut() {
                *phase += rng.f32() * phase_random;
            }
            Self { voice_phases, delta }
        }
        pub fn add_phase(&mut self, delta: f32, voice_count: usize, detune: f32) {
            // self.phase = (self.phase + delta) % (2.0 * PI);
            // self.phase
            self.voice_phases
                .iter_mut()
                .take(voice_count)
                .enumerate()
                .for_each(|(i, phase): (usize, &mut f32)| {
                    let i = i as f32 - (i % 2) as f32 * 2.0;
                    let delta = delta
                        + delta * i as f32 * detune / (voice_count as f32);
                    *phase = (*phase + delta) % (2.0 * PI);
                });
        }
    }

    pub struct BasicOscillator {
        pub amp: f32,
        pub semitone: f32,
        pub octave: i32,
        pub multiplier: f32,
        pub voice_count: u8,
        pub voices_detune: f32,
        pub wave: OscWave,
        pub phase: f32,
        pub phase_rand: f32,
    }
    impl BasicOscillator {
        fn pitch_mult_delta(&self, delta: f32) -> f32 {
            // delta * 2.0_f32.powi(self.octave) * 2.0_f32.powf(self.semitone / 12.0)
            delta * 2.0_f32.powf(self.semitone / 12.0 + self.octave as f32) * self.multiplier
        }
        pub fn unison<T: Fn(f32) -> f32>(&self, voices: &mut SuperVoice, wave: T) -> f32 {
            voices.add_phase(self.pitch_mult_delta(voices.delta), self.voice_count.into(), self.voices_detune);
            voices
                .voice_phases
                .iter_mut()
                .take(self.voice_count.into())
                .enumerate()
                .map(|(i, phase)| {
                    wave(*phase)
                })
                .sum::<f32>()
                * self.amp
                / self.voice_count as f32
        }
    }
    impl Default for BasicOscillator {
        fn default() -> Self {
            Self {
                amp: 1.0,
                semitone: Default::default(),
                octave: Default::default(),
                multiplier: 1.0,
                voice_count: 1,
                voices_detune: 0.1,
                wave: OscWave::Sine,
                phase: 0.0,
                phase_rand: PI * 2.0,
            }
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
                Tri => {
                    if phase <= PI {
                        (phase / PI) * 2.0 - 1.0
                    } else {
                        ((-phase + 2.0 * PI) / PI) * 2.0 - 1.0
                    }
                }
                Saw => (phase - PI) / (2.0 * PI),
                Exp => phase.sin().abs() - 0.55,
                Square => {
                    if phase <= PI {
                        FRAC_1_SQRT_2
                    } else {
                        -FRAC_1_SQRT_2
                    }
                }
                PulseQuarter => {
                    if phase <= std::f32::consts::FRAC_PI_2 {
                        FRAC_1_SQRT_2
                    } else {
                        -FRAC_1_SQRT_2
                    }
                }
                PulseEighth => {
                    if phase <= std::f32::consts::FRAC_PI_4 {
                        FRAC_1_SQRT_2
                    } else {
                        -FRAC_1_SQRT_2
                    }
                }
            }
        }
    }
    pub fn naive_saw(phase: f32) -> f32 {
        (phase - PI) / (2.0 * PI)
    }
}

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

#[derive(Debug, Default, Clone)]
/// Reproduced from https://ccrma.stanford.edu/~jos/filters/Direct_Form_II.html
/// 
pub struct TestFilter {
    stage0: f32, // internal feedback storage
    stage1: f32,
    pub a0: f32, // gain compensation
    pub a1: f32, // [n-1] feedback
    pub a2: f32, // [n-2] feedback
    pub b0: f32, // [n] out
    pub b1: f32, // [n-1] out
    pub b2: f32, // [n-2] out
    target_a: (f32, f32, f32), // smoothing
    target_b: (f32, f32, f32),
}
impl Filter for TestFilter {
    fn process(&mut self, input: f32) -> f32 {
        if !(self.stage0.is_finite() && self.stage1.is_finite()) {
            println!(
                "Warning: filters were unstable, {} and {}",
                self.stage0, self.stage1
            );
            self.stage0 = 0.0;
            self.stage1 = 0.0;
        }

        let previous_previous_sample = self.stage1;
        let previous_sample = self.stage0;
        let current_sample = (input - self.a1 * self.stage0 - self.a2 * self.stage1) / self.a0;
        //let current_sample = -self.stage0.mul_add(self.a1,  -self.stage1.mul_add(self.a2, input));

        // Propogate
        self.stage0 = current_sample;
        self.stage1 = previous_sample;

        (self.b0 * current_sample + self.b1 * previous_sample + self.b2 * previous_previous_sample)
            / self.a0
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        // Coefficients and formulas from https://www.w3.org/TR/audio-eq-cookbook/

        // "This software or document includes material copied from or derived from Audio Eq Cookbook (https://www.w3.org/TR/audio-eq-cookbook/). Copyright © 2021 W3C® (MIT, ERCIM, Keio, Beihang)." 
        
        // [This notice should be placed within redistributed or derivative software code or text when appropriate. This particular formulation became active on May 13, 2015, and edited for clarity 7 April, 2021, superseding the 2002 version.]
        // Audio Eq Cookbook: https://www.w3.org/TR/audio-eq-cookbook/
        // Copyright © 2021 World Wide Web Consortium, (Massachusetts Institute of Technology, European Research Consortium for Informatics and Mathematics, Keio University, Beihang). All Rights Reserved. This work is distributed under the W3C® Software and Document License [1] in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
        // [1] http://www.w3.org/Consortium/Legal/copyright-software

        let phase_change = 2.0 * PI * cutoff / sample_rate;
        let (sin, cos) = phase_change.sin_cos();
        let a = sin / (2.0 * resonance);

        self.b0 = (1.0 - cos) / 2.0;
        self.b1 = 1.0 - cos;
        self.b2 = (1.0 - cos) / 2.0;

        self.a0 = 1.0 + a;
        self.a1 = -2.0 * cos;
        self.a2 = 1.0 - a;
    }
}

#[derive(Debug, Default)]
/// Filter in series
pub struct CascadeFilter {
    filter_1: TestFilter,
    filter_2: TestFilter,
}
impl Filter for CascadeFilter {
    fn process(&mut self, input: f32) -> f32 {
        self.filter_2.process(self.filter_1.process(input))
    }
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32) {
        self.filter_1.set_params(sample_rate, cutoff, resonance);
        self.filter_2.set_params(sample_rate, cutoff, resonance);
    }
}

pub trait Filter {
    fn process(&mut self, input: f32) -> f32;
    fn set_params(&mut self, sample_rate: f32, cutoff: f32, resonance: f32);
}

#[derive(Debug)]
/// Filter in series
pub struct FilterController {
    filter: TestFilter,
    cutoff_envelope: AdsrEnvelope,
    envelope_amount: f32,
    cutoff: f32,
    resonance: f32,
}
impl FilterController {
    pub fn process(&mut self, input: f32) -> f32 {
        self.filter.process(input)
    }
    pub fn process_with_envelope_held(&mut self, input: f32, envelope_index: f32, sample_rate: f32) -> f32 {
        self.filter.set_params(sample_rate,
            (self.cutoff + self.cutoff_envelope.sample_held(envelope_index) * self.envelope_amount).clamp(1.0, 22000.0),
            self.resonance
        );
        let out = self.filter.process(input);
        self.filter.set_params(sample_rate, self.cutoff, self.resonance);
        out
    }
    
    pub fn lerp_controls(&mut self, sample_rate: f32, target_cutoff: f32, target_resonance: f32) {
        self.cutoff = lerp(self.cutoff, target_cutoff, 500.0 / sample_rate);
        self.resonance = lerp(self.cutoff, target_resonance, 500.0 / sample_rate);
        self.filter.set_params(sample_rate, self.cutoff, self.resonance);
    }
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    (to - from).mul_add(amount, from)
}
