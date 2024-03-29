use std::{
    convert::TryInto,
    f32::consts::{FRAC_1_SQRT_2, PI},
    iter,
};

use itertools::izip;
use rustfft::{
    num_complex::{Complex, Complex32},
    FftPlanner,
};

use super::lerp;

/// A single instance of a playing oscillator. Maintains phase in
/// internal state
#[derive(Debug, Clone, Copy, Default)]
pub struct OscVoice {
    pub phase: f32,
}
impl OscVoice {
    pub fn new(phase: f32) -> Self {
        Self { phase }
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
pub struct SuperVoice {
    pub voice_phases: [f32; 32],
}
impl SuperVoice {
    pub fn new(phase: f32, phase_random: f32) -> Self {
        let mut voice_phases = [phase; 32];
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
                let i = i as i32 - ((i % 2) * i * 2) as i32;
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
    ) -> [f32; 32] {
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
/// Each voice generating a wave reads OscillatorParams once per sample,
/// and applies its current parameters to the generated wave.
pub struct OscillatorParams {
    pub amp: f32,
    // TODO: Condense frequency manipulation parameters. Can probably just be two parameters
    // (Freq. Multiplier and Pitch Bend) which expose multiple controls in the gui.
    pub semitone: f32,
    pub octave: i32,
    pub pitch_multiplier: f32,

    total_multiplier: f32,

    pub voice_count: u8,
    pub voices_detune: f32,
    /// Amp to keep volume roughly equal across different voice counts
    pub unison_amp: f32,
    pub wave: OscWave,
    pub phase: f32,
    pub phase_rand: f32,
    pub pitch_bend: f32,

    pub pulse_width: f32,

    pub fm: f32,
    pub pm: f32,
    pub am: f32,
}
impl OscillatorParams {
    fn calc_pitch_mult(&self) -> f32 {
        2.0_f32.powf((self.semitone + self.pitch_bend) / 12.0 + self.octave as f32)
            * self.pitch_multiplier
    }
    pub fn semitone_detune(&self) -> f32 {
        self.semitone + self.pitch_bend + self.octave as f32 * 12.0
    }
    pub fn update_total_pitch(&mut self) {
        self.total_multiplier = self.calc_pitch_mult()
    }
    pub fn total_pitch_multiplier(&self) -> f32 {
        self.total_multiplier
    }
    pub fn calc_unison_amp(&self) -> f32 {
        1.0 / (self.voice_count as f32).sqrt()
    }
    pub fn update_unison_amp(&mut self) {
        self.unison_amp = self.calc_unison_amp()
    }
}
impl Default for OscillatorParams {
    fn default() -> Self {
        Self {
            amp: 1.0,
            semitone: Default::default(),
            octave: Default::default(),
            pitch_multiplier: 1.0,
            total_multiplier: 1.0,
            voice_count: 1,
            voices_detune: 0.1,
            unison_amp: 1.0,
            wave: OscWave::Sine,
            phase: 0.0,
            phase_rand: PI * 2.0,
            pitch_bend: 0.0,
            pulse_width: PI,
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
    Pulse { width: f32 },
}

impl OscWave {
    /// Generates the waveform at the specified phase, attempting to keep waveform volumes
    /// normalised.
    pub fn generate_normalised(&self, phase: f32) -> f32 {
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
            Pulse { width } => {
                if phase <= *width {
                    FRAC_1_SQRT_2
                } else {
                    -FRAC_1_SQRT_2
                }
            }
        }
    }
    /// Generates the waveform at the specified phase with all values
    /// ranging from -1 and +1 peak to peak.
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
            Exp => phase.sin().abs() * 2.0 - 1.0,
            Square => {
                if phase <= PI {
                    1.0
                } else {
                    -1.0
                }
            }
            Pulse { width } => {
                if phase <= *width {
                    1.0
                } else {
                    -1.0
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
            _ => OscWave::Sine,
        }
    }
    pub fn from_index_pulse(index: f32) -> Self {
        match index {
            x if x < 1.0 => OscWave::Sine,
            x if x < 2.0 => OscWave::Tri,
            x if x < 3.0 => OscWave::Saw,
            x if x < 4.0 => OscWave::Exp,
            x if x < 5.0 => OscWave::Pulse { width: PI },
            _ => OscWave::Sine,
        }
    }
}

pub fn modulate_delta(delta: f32, linear_fm: f32) -> f32 {
    // `linear_fm` is expected to be between -1.0 and 1.0,
    // must be stretched out.
    let linear_fm = linear_fm * 125.0;

    let delta = delta * (1.0 + linear_fm);

    // `rem_euclid()` prevents negative phases, unlike the default modulo
    delta.rem_euclid(2.0 * PI)
}

/// Efficient constant frequency sine approximation based on 2D rotation.
///
/// From https://www.iquilezles.org/www/articles/sincos/sincos.htm
///
/// Accurate at low frequencies, but grows or decays exponentially
/// depending on your processor due to floating point precision.
pub struct CoupledFormQuad {
    sin_dt: f32,
    cos_dt: f32,
    sin: f32,
    cos: f32,
}
impl CoupledFormQuad {
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

/// from https://ccrma.stanford.edu/~jos/pasp/Digital_Sinusoid_Generators.html
/// Supposedly more numerically stable, but cos is offset by half a sample.
pub struct MagicCircleQuad {
    parameter: f32,
    sin: f32,
    cos: f32,
}
impl MagicCircleQuad {
    pub fn new(delta: f32) -> Self {
        let (parameter, cos) = (delta / 2.0).sin_cos();
        let parameter = parameter * 2.0;
        Self {
            parameter,
            sin: 0.0,
            cos,
        }
    }
    #[inline]
    pub fn next(&mut self) -> (f32, f32) {
        self.cos = self.cos - self.sin * self.parameter;
        self.sin = self.sin + self.cos * self.parameter;
        (self.sin, self.cos)
    }
    pub fn set_delta(&mut self, delta: f32) {
        let parameter = (delta / 2.0).sin() * 2.0;
        *self = Self { parameter, ..*self }
    }
    pub fn sin(&self) -> f32 {
        self.sin
    }
    pub fn cos(&self) -> f32 {
        self.cos
    }
}

/// from https://vicanek.de/articles/QuadOsc.pdf
///
pub struct QuadOsc;

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
        let index_floor = index as usize;
        let index_next = (index_floor + 1) % self.table.len();
        let from = self.table[index_floor];
        let to = self.table[index_next];
        lerp(from, to, index - index_floor as f32)
    }
    #[inline]
    pub fn phase_to_index(&self, phase: f32) -> f32 {
        // Number is slightly greater than 2 to prevent OOB indexes
        // when phase/2PI == 1, at the expense of a small downwards
        // pitch detune.
        // TODO: Find out how many decimal points f32 can handle here, or rethink indexing so that it doesn't require this unnecessary division
        (phase / (2.000001 * PI)) * self.table.len() as f32
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
    #[inline]
    pub fn generate_multi_pm(&self, phases: &[f32], max: usize, phase_offset: f32) -> f32 {
        phases
            .iter()
            .take(max)
            .map(|phase| self.generate((*phase + phase_offset).rem_euclid(2.0 * PI)))
            .sum()
    }
    #[inline]
    pub fn generate_multi_stereo_pm(
        &self,
        phases: &[f32],
        max: usize,
        phase_offset: f32,
    ) -> (f32, f32) {
        phases
            .iter()
            .take(max)
            .map(|phase| self.generate((*phase + phase_offset).rem_euclid(2.0 * PI)))
            .enumerate()
            .fold((0.0, 0.0), |(l, r), (i, gen)| {
                if i % 2 == 0 {
                    (l + gen, r)
                } else {
                    (l, r + gen)
                }
            })
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
    pub fn from_additive_osc_ifft(osc: &AdditiveOsc, len: usize, harmonics: usize) -> Self {
        let mut table: Vec<f32> = vec![0.0; len];
        osc.generate_ifft(&mut table, harmonics);
        Self { table }
    }
}

/// Wave generator with a unique wavetable for each midi note index (extended to the 44.1 kHz
/// Nyquist frequency, so 138 notes).
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
    pub tables: [Wavetable; 138],
}

impl WavetableNotes {
    /// Returns `index` from the equation
    /// `440.0 * 2.0_f32.powf(index / 12.0) = frequency`
    pub fn frequency_to_note(frequency: f32) -> usize {
        (((frequency / 440.0).log2() * 12.0 + 69.0).round() as usize).clamp(0, 137)
    }
    /// Returns `index` from the equation
    /// `(2.0 * PI * 440.0 * 2.0_f32.powf(index / 12.0)) = delta * sample_rate`
    pub fn delta_to_note(delta: f32, sample_rate: f32) -> usize {
        (((sample_rate * delta / (440.0 * 2.0 * PI)).log2() * 12.0 + 69.0).ceil() as usize)
            .clamp(0, 137)
    }
    /// Returns the appropriate wavetable corresponding to `delta` at `sample_rate`
    pub fn delta_index(&self, delta: f32, sample_rate: f32) -> &Wavetable {
        &self.tables[WavetableNotes::delta_to_note(delta, sample_rate)]
    }
    /// Returns the appropriate wavetable corresponding to `frequency`
    pub fn frequency_index(&self, frequency: f32) -> &Wavetable {
        &self.tables[WavetableNotes::frequency_to_note(frequency)]
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
    /// Increasing `min_table_len` emulates increased oversampling for higher notes which, for reasons I now understand, require much more oversampling
    /// than the middle notes to achieve comparative noise reduction. E.g. a note at 2000 Hz theoretically requires a minimum of 22.05 samples, so a
    /// `min_table_len` of 2560 is equivalent to 116 times oversampling which is usually enough.
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
        let tables: Vec<Wavetable> = (0..=137)
            .into_iter()
            .map(|x| {
                if x == 137 {
                    // highest note has no harmonics, so notes above nyquist are silent
                    1
                } else {
                    // Sample length required for note
                    (max_oversample * sample_rate / (440.0 * 2.0_f32.powf((x - 69) as f32 / 12.0)))
                        .ceil() as usize
                }
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
    /// Generates a bandlimited wavetable for each midi note using fast fourier transforms, with constant wavetable length
    pub fn from_additive_osc_ifft(
        osc: &AdditiveOsc,
        sample_rate: f32,
        table_length: usize,
    ) -> Self {
        let tables: Vec<Wavetable> = (0..=137)
            .into_iter()
            .map(|x| {
                if x == 137 {
                    // highest note has no harmonics, so notes above nyquist are silent
                    0
                } else {
                    // Number of harmonics required for note
                    (sample_rate / (2.0 * 440.0 * 2.0_f32.powf((x - 69) as f32 / 12.0))).floor()
                        as usize
                }
            })
            .map(|harmonics| {
                // clamp to nyquist
                let harmonics = harmonics.min(table_length / 2);
                Wavetable::from_additive_osc_ifft(osc, table_length, harmonics)
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
    pub fn new(sample_rate: f32, table_len: usize) -> Self {
        Self {
            wavetables: vec![
                WavetableNotes::from_additive_osc_ifft(
                    &AdditiveOsc::sine(),
                    sample_rate,
                    table_len,
                ),
                WavetableNotes::from_additive_osc_ifft(
                    &AdditiveOsc::triangle(),
                    sample_rate,
                    table_len,
                ),
                WavetableNotes::from_additive_osc_ifft(&AdditiveOsc::saw(), sample_rate, table_len),
                WavetableNotes::from_additive_osc_ifft(
                    &AdditiveOsc::fake_exp(),
                    sample_rate,
                    table_len,
                ),
                WavetableNotes::from_additive_osc_ifft(
                    &AdditiveOsc::square(),
                    sample_rate,
                    table_len,
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
            OscWave::Pulse { width: _ } => 2,
            _ => 0,
        };
        &self.wavetables[wave_index]
    }
}

/// Oscillator which generates waves by summing sines.
///
/// The lowest-frequency signal that can be generated with all of its expected harmonics
/// is given by sample_rate / (2.0 * N). (i.e. with 2560 sinusoids: 44100/(2.0 * 2560) = 8.613 Hz)
pub struct AdditiveOsc<const N: usize = 2048> {
    amplitudes: [f32; N],
    phases: [f32; N],
}

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
    pub fn generate_ifft(&self, output: &mut [f32], harmonics: usize) {
        assert_eq!(output.len(), N);

        let dc_offset = iter::once((&0.0_f32, &0.0_f32));

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_inverse(N);
        let mut buffer: Vec<Complex32> = dc_offset
            .chain(
                self.amplitudes
                    .iter()
                    .take(harmonics)
                    .zip(self.phases.iter()),
            )
            // TODO: use phase
            .map(|(amplitude, _phase)| Complex {
                re: 0.0,
                im: *amplitude,
            })
            .collect();
        // FFTs must be constant length
        if buffer.len() < N {
            buffer.extend(vec![Complex32 { re: 0.0, im: 0.0 }; N - buffer.len()])
        }
        fft.process(&mut buffer);

        for (sample, output) in izip!(buffer.iter(), output.iter_mut()) {
            *output = sample.re;
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
            *x *= ((i + 1) % 2) as f32;
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
        let mut simple_sin = CoupledFormQuad::new(0.0, 1.0 / 100_000.0);
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
