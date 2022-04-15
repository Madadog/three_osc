use std::{
    convert::TryInto,
    f32::consts::{FRAC_1_SQRT_2, FRAC_2_PI, PI},
    ops::Add,
};

use super::lerp;

/// A single instance of a playing oscillator. Maintains phase for fm / pm stuff
///
/// TODO: Use this somewhere or clean it up.
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

#[derive(Debug, Clone)]
/// Brute force approach to oscillator super (playing multiple detuned copies
/// of a wave simultaneously) which tracks the phase of every wave copy.
///
/// Implemented as a buffer of independent phases with a common base frequency
/// (delta) which differs for each phase according to the `detune` parameter in
/// `add_phase()`.
///
/// TODO: Remove delta. `SuperVoice` should only track phase.
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
        Self {
            voice_phases,
            delta,
        }
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
                let delta = delta + delta * i as f32 * detune / (voice_count as f32);
                *phase = (*phase + delta) % (2.0 * PI);
            });
    }
}

#[derive(Debug, Clone)]
/// Each voice generating a wave reads BasicOscillator once per sample,
/// and applies its current parameters to the generated wave.
pub struct BasicOscillator {
    pub amp: f32,
    // TODO: Condense frequency manipulation parameters. Can probably just be two parameters
    // (Freq. Multiplier and Pitch Bend) which expose multiple controls in the gui.
    pub semitone: f32,
    pub octave: i32,
    pub multiplier: f32,

    pub voice_count: u8,
    pub voices_detune: f32,
    pub wave: OscWave,
    pub phase: f32,
    pub phase_rand: f32,
    pub pitch_bend: f32,
}
impl BasicOscillator {
    fn pitch_mult_delta(&self, delta: f32) -> f32 {
        delta
            * 2.0_f32.powf((self.semitone + self.pitch_bend) / 12.0 + self.octave as f32)
            * self.multiplier
    }
    pub fn unison<T: Fn(f32) -> f32>(
        &self,
        voices: &mut SuperVoice,
        wave: T,
        pm: f32,
        fm: f32,
    ) -> f32 {
        let constant = 7018.73299; // sample_rate / 2pi
        let delta = ((voices.delta) * (1.0 + pm) * constant + fm * 100.0) / constant;
        voices.add_phase(
            self.pitch_mult_delta(delta),
            self.voice_count.into(),
            self.voices_detune,
        );
        voices
            .voice_phases
            .iter_mut()
            .take(self.voice_count.into())
            .enumerate()
            .map(|(i, phase)| wave(*phase))
            .sum::<f32>()
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
            pitch_bend: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
/// Naive wave generators to be replaced later on
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
            Exp => phase.sin().abs() - (0.5 + 0.5 / PI),
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

/// Efficient sine approximation for constant frequency.
///
/// From https://www.iquilezles.org/www/articles/sincos/sincos.htm
///
/// SimpleSin decays and approaches zero faster with higher delta,
/// due to floating point precision issues. I briefly tried f64 but
/// didn't notice much of an improvement.
pub struct SimpleSin {
    sin_dt: f32,
    cos_dt: f32,
    sin: f32,
    cos: f32,
}
impl SimpleSin {
    pub fn new(phase: f32, delta: f32) -> Self {
        let (sin_dt, cos_dt) = delta.sin_cos();
        let (sin, cos) = phase.sin_cos();
        Self {
            sin_dt,
            cos_dt,
            sin,
            cos,
        }
    }
    #[inline]
    pub fn next(&mut self) -> (f32, f32) {
        // sin(t+dt) = sin(t)cos(dt) + sin(dt)cos(t)
        // cos(t+dt) = cos(t)cos(dt) - sin(t)sin(dt)
        self.sin = self.sin * self.cos_dt + self.cos * self.sin_dt;
        self.cos = self.cos * self.cos_dt - self.sin * self.sin_dt;
        (self.sin, self.cos)
    }
    pub fn set_delta(&mut self, delta: f32) {
        let (sin_dt, cos_dt) = delta.sin_cos();
        *self = Self {
            sin_dt,
            cos_dt,
            ..*self
        }
    }
    pub fn set_phase(&mut self, phase: f32) {
        let (sin, cos) = phase.sin_cos();
        *self = Self { sin, cos, ..*self }
    }
    pub fn sin(&self) -> f32 {
        self.sin
    }
    pub fn cos(&self) -> f32 {
        self.cos
    }
}

#[derive(Debug, Clone)]
pub struct Wavetable {
    pub table: Vec<f32>,
}
impl Wavetable {
    pub fn new(table: Vec<f32>) -> Self {
        Self { table }
    }
    #[inline]
    pub fn index(&self, index: usize) -> f32 {
        self.table[index as usize]
    }
    #[inline]
    // TODO: This is very slow, compared to the index above
    pub fn index_lerp(&self, index: f32) -> f32 {
        let index_1 = index as usize % self.table.len();
        let index_2 = (index_1 + 1) % self.table.len();
        let from = self.table[index_1];
        let to = self.table[index_2];
        lerp(from, to, index.fract())
    }
    #[inline]
    pub fn phase_to_index(&self, phase: f32) -> f32 {
        if phase >= 2.0 * PI { panic!("Phase was greater than 2.0 * PI") };
        (phase / (2.0 * PI)) * self.table.len() as f32
    }
    #[inline]
    pub fn generate(&self, phase: f32) -> f32 {
        // self.index(self.phase_to_index(phase) as usize)
        self.index_lerp(self.phase_to_index(phase))
    }
    // Harmonics should be less than or equal to len
    pub fn from_additive_osc(osc: &AdditiveOsc, len: usize, harmonics: usize) -> Self {
        let table: Vec<f32> = (0..len)
            .into_iter()
            .map(|x| osc.generate(2.0 * PI * (x as f32 / len as f32), harmonics))
            .collect();
        Self { table }
    }
}

/// Wave generator with a unique wavetable for each midi note.
pub struct WavetableNotes {
    pub tables: [Wavetable; 128],
}
impl WavetableNotes {
    // 440.0 * 2.0_f32.powf(index / 12.0)
    pub fn frequency_to_note(frequency: f32) -> usize {
        (((frequency / 440.0).log2() * 12.0 + 69.0).round() as usize).clamp(0, 127)
    }
    pub fn from_additive_osc(osc: &AdditiveOsc, sample_rate: f32) -> Self {
        let oversampling_factor = 8.0; // Don't skip samples in lerp
        let tables: Vec<Wavetable> = (0..128)
            .into_iter()
            .map(|x| (oversampling_factor * sample_rate / (440.0 * 2.0_f32.powf((x - 69) as f32 / 12.0))).ceil() as usize)
            .map(|len| Wavetable::from_additive_osc(osc, len, len / (2.1 * oversampling_factor) as usize))
            .collect();
        Self {
            tables: tables.try_into().unwrap(),
        }
    }
}

/// Oscillator which generates waves by summing sines
pub struct AdditiveOsc {
    amplitudes: [f32; 2560],
    phases: [f32; 2560],
}
impl AdditiveOsc {
    #[inline]
    pub fn generate(&self, phase: f32, harmonics: usize) -> f32 {
        self.amplitudes
            .iter()
            .take(harmonics)
            .zip(self.phases.iter())
            .enumerate()
            .map(|(i, (amp, part_phase))| (part_phase + phase * (i + 1) as f32).sin() * amp)
            .sum()
    }
    pub fn generate_segment(&self, output: &mut [f32], delta: f32, harmonics: usize) {
        for (i, sample) in output.iter_mut().enumerate() {
            *sample = self.generate(delta * i as f32, harmonics);
        }
    }
    pub fn saw() -> Self {
        let mut amplitudes = [1.0; 2560];
        amplitudes
            .iter_mut()
            .enumerate()
            .for_each(|(i, x)| *x /= (i + 1) as f32);
        let phases = [0.0; 2560];
        Self { amplitudes, phases }
    }
}

mod tests {
    use super::{SimpleSin, Wavetable, WavetableNotes};

    #[test]
    fn test_simple_sin() {
        let mut simple_sin = SimpleSin::new(0.0, 1.0 / 100_000.0);
        for _ in 0..100_000 {
            simple_sin.next();
        }
        println!("{}, {}", simple_sin.sin(), 1.0_f32.sin());
        println!("{}, {}", simple_sin.cos(), 1.0_f32.cos());
        assert!((simple_sin.sin() - 1.0_f32.sin()).abs() <= 0.001);
        assert!((simple_sin.cos() - 1.0_f32.cos()).abs() <= 0.001);
    }

    #[test]
    fn test_frequency_to_note() {
        assert!(WavetableNotes::frequency_to_note(440.0) == 69);
        assert!(WavetableNotes::frequency_to_note(415.0) == 68);
    }
}
