use std::{
    convert::TryInto,
    f32::consts::{FRAC_1_SQRT_2, PI},
};

use super::lerp;

#[derive(Debug, Clone)]
/// Brute force approach to oscillator super (playing multiple detuned copies
/// of a wave simultaneously) which tracks the phase of every wave copy.
///
/// Implemented as a buffer of independent phases with a common base frequency
/// (delta) which differs for each phase according to the `detune` parameter in
/// `add_phase()`.
pub struct SuperVoice {
    pub voice_phases: [f32; 128],
}
impl SuperVoice {
    pub fn new(phase: f32, phase_random: f32) -> Self {
        let mut voice_phases = [phase; 128];
        let rng = fastrand::Rng::new();

        for phase in voice_phases.iter_mut() {
            *phase += rng.f32() * phase_random;
        }
        Self { voice_phases }
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
    pub fn unison_phases(&mut self, delta: f32, voice_count: usize, voices_detune: f32) -> &[f32] {
        self.add_phase(delta, voice_count, voices_detune);
        &self.voice_phases
    }
    /// TODO: this duplicates all 128 phases, every time. 
    pub fn unison_phases_pm(
        &mut self,
        delta: f32,
        voice_count: usize,
        voices_detune: f32,
        pm: f32,
    ) -> [f32; 128] {
        self.add_phase(delta, voice_count, voices_detune);
        let pm = pm * 150.0;
        let mut out = self.voice_phases;
        for phase in out.iter_mut().take(voice_count) {
            *phase = (*phase + pm).rem_euclid(2.0 * PI);
        }
        out
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

    // Modulation multipliers. Not actually used by `BasicOscillator` itself, but it's
    // closely associated and I have no idea where else to put it since there's no
    // "modulation matrix" yet.
    pub fm: f32,
    pub pm: f32,
    pub am: f32,
}
impl BasicOscillator {
    pub fn pitch_mult_delta(&self, delta: f32) -> f32 {
        delta
            * 2.0_f32.powf((self.semitone + self.pitch_bend) / 12.0 + self.octave as f32)
            * self.multiplier
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
            fm: 0.0,
            pm: 0.0,
            am: 0.0,
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
#[allow(dead_code)]
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
    pub fn from_index(index: f32) -> Self {
        match index {
            x if x < 1.0 => OscWave::Sine,
            x if x < 2.0 => OscWave::Tri,
            x if x < 3.0 => OscWave::Saw,
            x if x < 4.0 => OscWave::Exp,
            x if x < 5.0 => OscWave::Square,
            x if x < 6.0 => OscWave::PulseQuarter,
            x if x < 7.0 => OscWave::PulseEighth,
            _ => OscWave::Sine,
        }
    }
}

pub fn modulate_delta(delta: f32, linear_fm: f32, constant_fm: f32, sample_rate: f32) -> f32 {
    // `linear_fm` and `constant_fm` are expected to be between -1.0 and 1.0,
    // must be stretched out.
    let linear_fm = linear_fm * 125.0;
    let constant_fm = constant_fm * 10000.0 * 1.5;

    // `delta_to_freq` is required because delta varies with sample rate. Constant FM
    // requires this, but linear FM is multiplicative / relative, so it's unaffected.
    let delta_to_freq = sample_rate / (2.0 * PI);

    let delta = ((delta) * (1.0 + linear_fm) * delta_to_freq + constant_fm) / delta_to_freq;
    // `rem_euclid()` doesn't allow negatives, while regular modulo does
    delta.rem_euclid(2.0 * PI)
}

/// Efficient constant frequency sine approximation.
///
/// From https://www.iquilezles.org/www/articles/sincos/sincos.htm
///
/// SimpleSin decays and approaches zero faster with higher delta,
/// due to floating point precision issues. I briefly tried f64 but
/// didn't notice much of an improvement.
#[allow(dead_code)]
pub struct SinApprox {
    sin_dt: f32,
    cos_dt: f32,
    sin: f32,
    cos: f32,
}
#[allow(dead_code)]
impl SinApprox {
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
#[allow(dead_code)]
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
        (phase / (2.0 * PI)) * self.table.len() as f32
    }
    #[inline]
    pub fn generate(&self, phase: f32) -> f32 {
        self.index_lerp(self.phase_to_index(phase))
    }
    #[inline]
    pub fn generate_multi(&self, phases: &[f32], max: usize) -> f32 {
        phases
            .iter()
            .take(max)
            .map(|phase| self.generate(*phase))
            .sum()
    }
    /// `harmonics` should be less than or equal to half of `len` to prevent aliasing
    pub fn from_additive_osc(osc: &AdditiveOsc, len: usize, harmonics: usize) -> Self {
        assert!(
            harmonics <= (len / 2),
            "Generated {harmonics} harmonics with {len} len"
        );
        let table: Vec<f32> = (0..len)
            .into_iter()
            .map(|x| osc.generate(2.0 * PI * (x as f32 / len as f32), harmonics))
            .collect();
        Self { table }
    }
}

/// Wave generator with a unique wavetable for each midi note index.
///
/// Wavetables can be trivially bandlimited by generating each table with additive synthesis
/// with harmonics up to the Nyquist frequency. The table can then be sampled for cheap,
/// alias-free synthesis.
///
/// ... At least, in theory. If we had a perfect sampling function (which consists of summing
/// sines again), this would be the case but we are instead using linear interpolation, which creates
/// discontinuous straight lines which cause aliasing. Currently we suppress these artifacts by
/// oversampling the wavetables (initialising them several times larger than they need to be).
/// This reduces the amplitude of the artifacts and makes them more harmonically distant from
/// the source. 8 times oversampling seems to provide reasonable artifact suppression (-60 dB or
/// lower compared to the fundamental).
///
/// Of course, all this is made sort of irrelevant by the fact that you can just use additive
/// synthesis while rendering since you have unlimited time to generate the sound, thus providing
/// completely alias-free oscillators. But it was fun to implement, and it's fun to perform in
/// realtime because of how cheap it is to sample + lerp.
///
/// The artifacts could be reduced further by using better interpolation (with a derivative closer
/// to that of the original signal).
pub struct WavetableNotes {
    pub tables: [Wavetable; 128],
}
#[allow(dead_code)]
impl WavetableNotes {
    /// Returns `index` from the equation
    /// `440.0 * 2.0_f32.powf(index / 12.0) = frequency`
    pub fn frequency_to_note(frequency: f32) -> usize {
        (((frequency / 440.0).log2() * 12.0 + 69.0).round() as usize).clamp(0, 127)
    }
    /// Create a bandlimited wavetable for each midi note from a base additive function.
    ///
    /// Each note is given a wavetable with the exact number of samples required to reproduce the note and all its harmonics.
    /// The table size will be multiplied by `max_oversample` (to reduce aliasing from linear interpolation) but limited by
    /// min and max `table_len`.
    ///
    /// Limiting `max_table_len` mostly prevents low notes from growing too big (given you have `(sample_rate * max_oversample) / note_freq` samples per table)
    /// at the expense of sample quality.
    ///
    /// Increasing `min_table_len` emulates increased oversampling for higher notes which, for reasons I don't fully understand, require much more oversampling
    /// than the middle notes to achieve comparative noise reduction. E.g. a note at 2000 Hz requires 22.05 samples, so a `min_table_len` of 2560 is equivalent
    /// to 116 times oversampling which is usually enough.
    ///
    /// Note that all other wavetable synths just use wavetable lengths that are powers of two, because they use FFTs. Of course, a need for FFTs implies
    /// that you're reading user samples or doing spectral manipulation, both of which are beyond the scope of this synth. Inverse FFTs, on the other hand, will
    /// allow me to stop summing sine waves manually, speeding up table generation to realtime speeds...  
    pub fn from_additive_osc(
        osc: &AdditiveOsc,
        sample_rate: f32,
        max_oversample: f32,
        max_table_len: usize,
        min_table_len: usize,
    ) -> Self {
        debug_assert!(
            max_oversample >= 1.0,
            "Oversampling factor should be 1.0 or greater"
        );
        let tables: Vec<Wavetable> = (0..128)
            .into_iter()
            .map(|x| {
                // Sample length required for note
                (max_oversample * sample_rate / (440.0 * 2.0_f32.powf((x - 69) as f32 / 12.0)))
                    .ceil() as usize
            })
            .map(|len| {
                let clamped_len = len.clamp(min_table_len, max_table_len);
                // Ensure waveform is bandlimited
                let harmonics =
                    ((len as f32 / (2.0 * max_oversample)) as usize).min(clamped_len / 2);
                Wavetable::from_additive_osc(osc, clamped_len, harmonics)
            })
            .collect();
        Self {
            tables: tables.try_into().unwrap(),
        }
    }
    /// `WavetableNotes::from_additive_osc()` with constant table length
    ///
    /// I will migrate to this once I start using fast fourier transforms.
    pub fn from_additive_osc_2(
        osc: &AdditiveOsc,
        sample_rate: f32,
        oversampling_factor: f32,
        table_length: usize,
    ) -> Self {
        debug_assert!(
            oversampling_factor >= 1.0,
            "Oversampling factor should be 1.0 or greater"
        );
        let tables: Vec<Wavetable> = (0..128)
            .into_iter()
            .map(|x| {
                // Number of harmonics required for note
                (sample_rate / (2.0 * 440.0 * 2.0_f32.powf((x - 69) as f32 / 12.0))).floor()
                    as usize
            })
            .map(|harmonics| {
                // clamp to nyquist
                let harmonics = harmonics.min(table_length / 2);
                Wavetable::from_additive_osc(
                    osc,
                    (table_length as f32 * oversampling_factor) as usize,
                    harmonics,
                )
            })
            .collect();
        Self {
            tables: tables.try_into().unwrap(),
        }
    }
}

pub struct WavetableSet {
    pub wavetables: Vec<WavetableNotes>,
}
impl WavetableSet {
    pub fn new(
        sample_rate: f32,
        max_oversample: f32,
        max_table_len: usize,
        min_table_len: usize,
    ) -> Self {
        Self {
            wavetables: vec![
                WavetableNotes::from_additive_osc(
                    &AdditiveOsc::sine(),
                    sample_rate,
                    max_oversample,
                    max_table_len,
                    min_table_len,
                ),
                WavetableNotes::from_additive_osc(
                    &AdditiveOsc::triangle(),
                    sample_rate,
                    max_oversample,
                    max_table_len,
                    min_table_len,
                ),
                WavetableNotes::from_additive_osc(
                    &AdditiveOsc::saw(),
                    sample_rate,
                    max_oversample,
                    max_table_len,
                    min_table_len,
                ),
                WavetableNotes::from_additive_osc(
                    &AdditiveOsc::fake_exp(),
                    sample_rate,
                    max_oversample,
                    max_table_len,
                    min_table_len,
                ),
                WavetableNotes::from_additive_osc(
                    &AdditiveOsc::square(),
                    sample_rate,
                    max_oversample,
                    max_table_len,
                    min_table_len,
                ),
            ],
        }
    }
    pub fn select(&self, wave: &OscWave) -> &WavetableNotes {
        let wave_index = match wave {
            OscWave::Sine => 0,
            OscWave::Tri => 1,
            OscWave::Saw => 2,
            OscWave::Exp => 3,
            OscWave::Square => 4,
            _ => 0,
        };
        &self.wavetables[wave_index]
    }
}

/// Oscillator which generates waves by summing sines.
///
/// The lowest-frequency signal that can be generated with all of its expected harmonics
/// is given by sample_rate / (2.0 * N). (i.e. with 2560 sinusoids: 44100/(2.0 * 2560) = 8.613 Hz)
pub struct AdditiveOsc<const N: usize = 1280> {
    amplitudes: [f32; N],
    phases: [f32; N],
}
#[allow(dead_code)]
impl<const N: usize> AdditiveOsc<N> {
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
    pub fn sine() -> Self {
        let mut amplitudes = [0.0; N];
        amplitudes[0] = 1.0;
        let phases = [0.0; N];
        Self { amplitudes, phases }
    }
    pub fn triangle() -> Self {
        let mut amplitudes = [0.0; N];
        amplitudes
            .iter_mut()
            .enumerate()
            .step_by(2)
            .for_each(|(i, x)| {
                *x = if i % 4 == 0 { 1.0 } else { -1.0 };
                *x /= ((i + 1) as f32).powi(2);
            });
        let phases = [0.0; N];
        Self { amplitudes, phases }
    }
    pub fn saw() -> Self {
        let mut amplitudes = [1.0; N];
        amplitudes
            .iter_mut()
            .enumerate()
            .for_each(|(i, x)| *x /= (i + 1) as f32);
        let phases = [0.0; N];
        Self { amplitudes, phases }
    }
    pub fn fake_exp() -> Self {
        let mut amplitudes = [1.0; N];
        amplitudes
            .iter_mut()
            .enumerate()
            .for_each(|(i, x)| *x /= ((i + 1) as f32).powi(2));
        let phases = [0.0; N];
        Self { amplitudes, phases }
    }
    pub fn square() -> Self {
        let mut amplitudes = [1.0; N];
        amplitudes.iter_mut().enumerate().for_each(|(i, x)| {
            *x /= (i + 1) as f32;
            *x = *x * ((i + 1) % 2) as f32;
        });
        let phases = [0.0; N];
        Self { amplitudes, phases }
    }
}

mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_simple_sin() {
        let mut simple_sin = SinApprox::new(0.0, 1.0 / 100_000.0);
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
