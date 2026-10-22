use std::{
    convert::TryInto,
    f32::consts::{FRAC_PI_4, PI, SQRT_2},
};

use itertools::izip;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use wide::{bytemuck::cast, f32x8, i32x8, u32x8};

use super::fastmath;
use super::lerp;

/// Which SIMD instructions the wavetable lookups can use, detected at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdLevel {
    /// The portable path: `wide` vectors for the arithmetic (SSE2 on x86-64), scalar table reads.
    Portable,
    /// AVX2 and FMA: each voice's 4 taps are loaded with one 128-bit load, transposed into
    /// vectors, and interpolated 8 voices at a time.
    #[cfg(target_arch = "x86_64")]
    Avx2,
    /// Like `Avx2`, but the taps are read with gather instructions. Whether that is faster
    /// depends on the CPU: gathers are microcoded and slow on the Intel generations affected by
    /// the 2023 "Downfall" mitigation (Skylake to Tiger Lake), and a good deal faster than the
    /// loads and shuffles on newer ones, so `detect()` times both.
    #[cfg(target_arch = "x86_64")]
    Avx2Gather,
}

impl SimdLevel {
    /// Whether this level implies AVX support (used by the ladder filter).
    pub fn has_avx(self) -> bool {
        !matches!(self, SimdLevel::Portable)
    }
    /// The fastest level supported by this CPU. On AVX2 CPUs the two lookup variants are timed
    /// against each other, which takes well under a millisecond.
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return Self::fastest_avx2();
        }
        SimdLevel::Portable
    }
    /// Times the two AVX2 lookup variants on a small table and returns the faster one. The
    /// timing mimics the common case of one vector of 8 unison voices per block; the loads
    /// and shuffles variant is kept unless the gathers are clearly faster.
    #[cfg(target_arch = "x86_64")]
    fn fastest_avx2() -> Self {
        use std::time::{Duration, Instant};
        const BLOCKS: usize = 64;
        const ROUNDS: usize = 3;

        let len = 256;
        let table = Wavetable::new((0..len).map(|i| i as f32 / len as f32 * 2.0 - 1.0).collect());
        let frames = FramePair {
            a: &table,
            b: &table,
            mix: 0.0,
        };
        let factors: [f32x8; UNISON_VECTORS] = std::array::from_fn(|vector| {
            f32x8::new(std::array::from_fn(|lane| 1.0 + (vector * UNISON_LANES + lane) as f32 * 1e-3))
        });
        let mask = [f32x8::splat(1.0); UNISON_VECTORS];
        let params = BlockParams {
            delta: 0.0123,
            delta_step: 0.0,
            gain: 1.0,
            gain_step: 0.0,
            modulation: None,
            voices: UNISON_LANES,
            detune_factors: &factors,
            mask: &mask,
            pan_left: &mask,
            pan_right: &mask,
        };
        let time = |simd: SimdLevel| {
            let mut voice = SuperVoice::new(0.0, 1.0);
            let mut out = [0.0_f32; 32];
            let start = Instant::now();
            for _ in 0..BLOCKS {
                let mut output = BlockOutput {
                    left: &mut out,
                    right: None,
                    mono: None,
                };
                frames.render_block(&mut voice, &params, &mut output, simd, Interpolation::Cubic);
            }
            std::hint::black_box(out);
            start.elapsed()
        };
        // Alternate between the variants and keep each one's best time, to ride out
        // interruptions by other threads.
        let (mut transpose, mut gather) = (Duration::MAX, Duration::MAX);
        for _ in 0..ROUNDS {
            transpose = transpose.min(time(SimdLevel::Avx2));
            gather = gather.min(time(SimdLevel::Avx2Gather));
        }
        if gather.as_nanos() * 20 < transpose.as_nanos() * 19 {
            SimdLevel::Avx2Gather
        } else {
            SimdLevel::Avx2
        }
    }
}

/// How the wavetables are read in between their samples, from cheapest to cleanest.
///
/// The lookup kernels are specialised for each mode at compile time through a `const INTERP:
/// u8` parameter holding the mode's discriminant, so switching modes costs nothing per sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Interpolation {
    /// No interpolation: the sample below the read position is used as is. The cheapest, and
    /// the noisiest: the steps between samples alias audibly on all but the lowest notes.
    None = NONE,
    /// Linear interpolation between the two nearest samples.
    Linear = LINEAR,
    /// 4-point cubic Hermite (Catmull-Rom) interpolation. The cleanest, with the most reads and
    /// arithmetic per voice.
    #[default]
    Cubic = CUBIC,
}

/// Const generic tags for the interpolation modes, see [`Interpolation`].
const NONE: u8 = 0;
const LINEAR: u8 = 1;
const CUBIC: u8 = 2;

/// Up to this many unison voices are read one at a time instead of in SIMD vectors, since
/// filling a vector for one or two voices costs more than it saves.
const SCALAR_VOICE_LIMIT: usize = 3;

/// Maximum number of unison voices per oscillator.
pub const MAX_UNISON: usize = 32;
/// Unison voices are processed in SIMD vectors of this many lanes.
pub const UNISON_LANES: usize = 8;
/// Number of SIMD vectors needed for all unison voices.
pub const UNISON_VECTORS: usize = MAX_UNISON / UNISON_LANES;

/// Phases are kept in 32-bit fixed point: one cycle is `2^32`, so they wrap for free when they
/// overflow, and the pitch resolution is `2^-32` cycles per sample (10 nHz at 44.1 kHz).
const FIXED_CYCLE: f64 = 4_294_967_296.0;

/// Converts a phase or phase delta in cycles (of either sign and any size) to fixed point,
/// keeping only the fraction of a cycle.
#[inline]
pub fn cycles_to_fixed(cycles: f32) -> u32 {
    (cycles as f64 * FIXED_CYCLE).floor() as i64 as u32
}

/// Converts a fixed point phase back to cycles in `[0, 1)`.
#[inline]
#[allow(dead_code)] // only needed by the tests so far
pub fn fixed_to_cycles(phase: u32) -> f32 {
    (phase as f64 / FIXED_CYCLE) as f32
}

/// Wraps a phase (or phase delta) in cycles of either sign into `[0, 1)`.
#[inline]
pub fn wrap_cycles_signed(phase: f32) -> f32 {
    let wrapped = phase - (phase as i32) as f32;
    if wrapped < 0.0 {
        wrapped + 1.0
    } else {
        wrapped
    }
}

/// A single instance of a playing oscillator (the LFO), with its phase in cycles.
#[derive(Debug, Clone, Copy, Default)]
pub struct OscVoice {
    pub phase: f32,
}
impl OscVoice {
    /// Advances the phase by `delta` cycles and returns the new phase, wrapped to `[0, 1)`.
    pub fn add_phase(&mut self, delta: f32) -> f32 {
        self.phase = wrap_cycles_signed(self.phase + delta);
        self.phase
    }
}

#[derive(Debug, Clone)]
/// Brute force approach to oscillator super (playing multiple detuned copies
/// of a wave simultaneously) which tracks the phase of every wave copy.
///
/// Implemented as a buffer of independent phases in fixed point (see [`cycles_to_fixed()`])
/// with a common base frequency (delta) which differs for each phase according to the
/// oscillator's detune factors. The phases are stored as SIMD vectors so all voices advance at
/// once, see [`FramePair::render_block()`].
pub struct SuperVoice {
    pub voice_phases: [u32x8; UNISON_VECTORS],
}
impl SuperVoice {
    pub fn new(phase: f32, phase_random: f32) -> Self {
        let rng = fastrand::Rng::new();
        let mut phases = [0_u32; MAX_UNISON];
        for phase_out in phases.iter_mut() {
            *phase_out = cycles_to_fixed(phase + rng.f32() * phase_random);
        }
        Self {
            voice_phases: std::array::from_fn(|vector| {
                let lanes = &phases[vector * UNISON_LANES..(vector + 1) * UNISON_LANES];
                u32x8::new(lanes.try_into().unwrap())
            }),
        }
    }
    /// One voice's phase, in cycles.
    #[allow(dead_code)] // only needed by the tests so far
    pub fn phase_cycles(&self, voice: usize) -> f32 {
        fixed_to_cycles(lane_u32(&self.voice_phases, voice))
    }
}

/// The detune multiplier of unison voice `i`: 0 for the centre voice, then alternating
/// -1, +2, -3, +4, ... so that the voices spread out on both sides of the centre pitch.
#[inline]
pub fn unison_detune_index(i: usize) -> i32 {
    i as i32 - ((i % 2) * i * 2) as i32
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
    /// Unison detune amount from 0 to 1, see `update_detune_factors()`.
    pub voices_detune: f32,
    /// Amp to keep volume roughly equal across different voice counts
    pub unison_amp: f32,
    /// Left and right gain of each unison voice, zero for unused voices, see
    /// `update_pan_gains()`.
    pub pan_left: [f32x8; UNISON_VECTORS],
    pub pan_right: [f32x8; UNISON_VECTORS],
    /// 1 for each unison voice in use and 0 for the rest, see `update_detune_factors()`.
    pub voice_mask: [f32x8; UNISON_VECTORS],
    /// Each unison voice's phase delta relative to the base delta, see `update_detune_factors()`.
    pub detune_factors: [f32x8; UNISON_VECTORS],

    /// Index of the wavetable (in the synth's [`WavetableSet`]) this oscillator plays.
    pub wavetable: usize,
    /// Play the wavetable loaded from a file for this oscillator instead.
    pub use_file: bool,
    /// Position within the wavetable, from 0 to 1. Morphs between the frames of wavetables that
    /// have more than one.
    pub position: f32,

    /// Starting phase, in cycles.
    pub phase: f32,
    /// Starting phase randomisation, in cycles.
    pub phase_rand: f32,
    pub pitch_bend: f32,

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
    /// Number of unison voices in use.
    #[inline]
    pub fn unison_voices(&self) -> usize {
        (self.voice_count as usize).clamp(1, MAX_UNISON)
    }
    /// Number of SIMD vectors needed for the unison voices in use.
    #[inline]
    pub fn unison_vectors(&self) -> usize {
        self.unison_voices().div_ceil(UNISON_LANES)
    }
    /// Updates each unison voice's phase delta relative to the base delta (the centre voice is
    /// 1, the others spread out above and below it by the unison detune) and the voice mask.
    pub fn update_detune_factors(&mut self) {
        let voice_count = self.unison_voices();
        let mut mask = [0.0_f32; MAX_UNISON];
        for mask in mask.iter_mut().take(voice_count) {
            *mask = 1.0;
        }
        self.detune_factors = self.detune_factors_for(self.voices_detune);
        self.voice_mask = to_vectors(&mask);
    }
    /// The unison voices' phase delta factors for a unison detune amount from 0 to 1. The amount
    /// is cubed, for finer control over small detunes, and spread over the voices in use.
    pub fn detune_factors_for(&self, detune: f32) -> [f32x8; UNISON_VECTORS] {
        let voice_count = self.unison_voices();
        let spread = detune.powi(3) / voice_count as f32;
        let mut factors = [1.0_f32; MAX_UNISON];
        for (i, factor) in factors.iter_mut().enumerate().take(voice_count) {
            *factor = 1.0 + unison_detune_index(i) as f32 * spread;
        }
        to_vectors(&factors)
    }
    /// The `(left, right)` gain of a unison voice.
    pub fn pan_gain(&self, voice: usize) -> (f32, f32) {
        (
            self.pan_left[voice / UNISON_LANES].as_array()[voice % UNISON_LANES],
            self.pan_right[voice / UNISON_LANES].as_array()[voice % UNISON_LANES],
        )
    }
    /// Keeps the wavetable index within the set.
    pub fn clamp_wavetable(&mut self, set: &WavetableSet) {
        self.wavetable = self.wavetable.min(set.wavetables.len() - 1);
    }
    /// Updates the left and right gain of each unison voice for the given stereo width, where 0
    /// is mono and 1 spreads the voices across the whole stereo field.
    ///
    /// The voices are ordered by pitch and spread out from the centre, alternating sides as the
    /// pitch rises (the lowest voice is furthest left, the second lowest is on the right, ...),
    /// mirrored about the centre so that the highest voice is furthest right. Both sides thus get
    /// an even mix of low and high voices, which keeps the bass end balanced. Gains follow a
    /// constant power pan law scaled so that a centred voice has a gain of 1 on both channels.
    /// That way a width of 0 is identical to a mono mix, and an oscillator with a single voice
    /// always stays in the centre.
    pub fn update_pan_gains(&mut self, stereo_width: f32) {
        let voice_count = self.unison_voices();
        let mut left = [0.0_f32; MAX_UNISON];
        let mut right = [0.0_f32; MAX_UNISON];

        for (i, gains) in left.iter_mut().zip(right.iter_mut()).enumerate().take(voice_count) {
            // Pan position between -1 (left) and 1 (right)
            let pan = if voice_count == 1 {
                0.0
            } else {
                // The voice's rank when sorted by pitch (the detune indices are all distinct).
                let detune = unison_detune_index(i);
                let rank = (0..voice_count)
                    .filter(|&j| unison_detune_index(j) < detune)
                    .count();
                let magnitude = ((2 * rank) as f32 / (voice_count - 1) as f32 - 1.0).abs();

                let mirrored_rank = voice_count - 1 - rank;
                let lower_rank = rank.min(mirrored_rank);
                let mut side = if lower_rank % 2 == 0 { -1.0 } else { 1.0 };
                if rank > mirrored_rank {
                    side = -side;
                }

                side * magnitude
            };
            let angle = (pan * stereo_width + 1.0) * FRAC_PI_4;
            let (sin, cos) = angle.sin_cos();
            *gains.0 = cos * SQRT_2;
            *gains.1 = sin * SQRT_2;
        }

        self.pan_left = to_vectors(&left);
        self.pan_right = to_vectors(&right);
    }
}

/// Packs per voice values into SIMD vectors.
fn to_vectors(values: &[f32; MAX_UNISON]) -> [f32x8; UNISON_VECTORS] {
    std::array::from_fn(|vector| {
        let lanes = &values[vector * UNISON_LANES..(vector + 1) * UNISON_LANES];
        f32x8::new(lanes.try_into().unwrap())
    })
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
            voices_detune: 0.21,
            unison_amp: 1.0,
            pan_left: [f32x8::splat(1.0); UNISON_VECTORS],
            pan_right: [f32x8::splat(1.0); UNISON_VECTORS],
            voice_mask: [f32x8::splat(1.0); UNISON_VECTORS],
            detune_factors: [f32x8::splat(1.0); UNISON_VECTORS],
            wavetable: 0,
            use_file: false,
            position: 0.0,
            phase: 0.0,
            phase_rand: 1.0,
            pitch_bend: 0.0,
            fm: 0.0,
            pm: 0.0,
            am: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
/// Naive wave generators, used by the LFO.
pub enum OscWave {
    Sine,
    Tri,
    Saw,
    Exp,
    Square,
}

impl OscWave {
    /// Generates the waveform at the given phase in cycles (`[0, 1)`), with all values ranging
    /// from -1 to +1 peak to peak.
    pub fn generate(&self, phase: f32) -> f32 {
        use OscWave::*;

        match self {
            Sine => fastmath::sin_cycles(phase),
            Tri => {
                if phase <= 0.5 {
                    phase * 4.0 - 1.0
                } else {
                    3.0 - phase * 4.0
                }
            }
            Saw => phase * 2.0 - 1.0,
            Exp => fastmath::sin_cycles(phase).abs() * 2.0 - 1.0,
            Square => {
                if phase <= 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
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

/// Extra samples stored before and after each wavetable cycle, so that the cubic interpolation
/// can read the neighbouring samples across the ends of the cycle without any wrapping logic:
/// one sample before the cycle and two after it.
const TABLE_PADDING_BEFORE: usize = 1;
const TABLE_PADDING_AFTER: usize = 2;

#[derive(Debug, Clone)]
/// One cycle of a waveform, padded with copies of its neighbouring samples (see
/// [`TABLE_PADDING_BEFORE`]). The cycle is a power of two samples long, so that a fixed point
/// phase splits into a sample index and a fraction of a sample with a shift and a mask.
pub struct Wavetable {
    pub table: Vec<f32>,
    /// `32 - log2(cycle_len)`: shifting a fixed point phase right by this many bits gives the
    /// index of the sample below it.
    index_shift: u32,
    /// Scales the phase bits below the index to a fraction of a sample.
    frac_scale: f32,
}

impl Wavetable {
    pub fn new(cycle: Vec<f32>) -> Self {
        let len = cycle.len();
        assert!(
            len >= 2 && len.is_power_of_two(),
            "a wavetable cycle must be a power of two samples long, not {len}"
        );
        let mut table = Vec::with_capacity(len + TABLE_PADDING_BEFORE + TABLE_PADDING_AFTER);
        table.push(cycle[len - 1]);
        table.extend_from_slice(&cycle);
        table.push(cycle[0]);
        table.push(cycle[1]);
        let index_shift = 32 - len.trailing_zeros();
        Self {
            table,
            index_shift,
            frac_scale: 1.0 / (1_u64 << index_shift) as f32,
        }
    }
    /// Samples per cycle.
    #[inline]
    pub fn cycle_len(&self) -> usize {
        self.table.len() - TABLE_PADDING_BEFORE - TABLE_PADDING_AFTER
    }
    /// The cycle's samples, without the padding.
    pub fn cycle(&self) -> &[f32] {
        &self.table[TABLE_PADDING_BEFORE..TABLE_PADDING_BEFORE + self.cycle_len()]
    }
    /// Shifting a fixed point phase right by this many bits gives the index of the sample
    /// below it.
    #[inline]
    pub fn index_shift(&self) -> u32 {
        self.index_shift
    }
    /// The bits of a fixed point phase below the sample index.
    #[inline]
    pub fn frac_mask(&self) -> u32 {
        (1_u32 << self.index_shift) - 1
    }
    /// Scales the masked phase bits to a fraction of a sample.
    #[inline]
    pub fn frac_scale(&self) -> f32 {
        self.frac_scale
    }
    /// Splits a fixed point phase into the index of the sample below it and the fraction of a
    /// sample above that.
    #[inline]
    pub fn split_phase(&self, phase: u32) -> (usize, f32) {
        (
            (phase >> self.index_shift) as usize,
            (phase & self.frac_mask()) as f32 * self.frac_scale,
        )
    }
    /// Reads the table at a fixed point phase with the given interpolation mode (an
    /// [`Interpolation`] discriminant).
    #[inline]
    pub fn read_fixed<const INTERP: u8>(&self, phase: u32) -> f32 {
        let (floor, frac) = self.split_phase(phase);
        self.read_taps::<INTERP>(floor, frac)
    }
    /// Reads the table at a fractional sample index in `[0, cycle_len)` with cubic
    /// interpolation. Out of range indices are clamped.
    #[inline]
    pub fn read(&self, index: f32) -> f32 {
        self.read_with::<CUBIC>(index)
    }
    /// Reads the table at a fractional sample index in `[0, cycle_len)` with the given
    /// interpolation mode (an [`Interpolation`] discriminant). Out of range indices are clamped.
    #[inline]
    pub fn read_with<const INTERP: u8>(&self, index: f32) -> f32 {
        let floor = (index as usize).min(self.cycle_len() - 1);
        self.read_taps::<INTERP>(floor, index - floor as f32)
    }
    /// Interpolates `frac` samples past sample `floor` of the cycle. The table is padded, so
    /// the 4 taps around any sample of the cycle are inside it.
    #[inline]
    fn read_taps<const INTERP: u8>(&self, floor: usize, frac: f32) -> f32 {
        let taps = &self.table[floor..floor + 4];
        match INTERP {
            NONE => taps[1],
            LINEAR => taps[1] + (taps[2] - taps[1]) * frac,
            _ => hermite(taps[0], taps[1], taps[2], taps[3], frac),
        }
    }
    /// Generates a sample at the given phase in cycles, with cubic interpolation.
    #[inline]
    pub fn generate(&self, phase: f32) -> f32 {
        self.read_fixed::<CUBIC>(cycles_to_fixed(phase))
    }
    /// One cycle of the first `harmonics` partials of a spectrum, `len` samples long. `fft`
    /// must be an inverse FFT plan of length `len`.
    pub fn from_spectrum(
        spectrum: &Spectrum,
        len: usize,
        harmonics: usize,
        fft: &dyn Fft<f32>,
    ) -> Self {
        let mut table: Vec<f32> = vec![0.0; len];
        spectrum.render_ifft(&mut table, harmonics, fft);
        Self::new(table)
    }
}

/// 4-point, 3rd-order Hermite (Catmull-Rom) interpolation between `y1` and `y2`, `t` samples
/// past `y1`.
#[inline]
pub fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// A position within a multi-frame wavetable, resolved to the (bandlimited) tables of the two
/// frames to crossfade between. Single frame wavetables have `mix` set to 0, which skips the
/// second lookup. Both frames must have the same cycle length.
#[derive(Debug, Clone, Copy)]
pub struct FramePair<'a> {
    pub a: &'a Wavetable,
    pub b: &'a Wavetable,
    pub mix: f32,
}

/// The inputs to [`FramePair::render_block()`] that are shared by all of an oscillator's
/// unison voices over one block.
pub struct BlockParams<'a> {
    /// The base phase delta in cycles per sample at the first sample of the block, and how
    /// much it changes per sample.
    pub delta: f32,
    pub delta_step: f32,
    /// The output gain at the first sample of the block, and how much it changes per sample.
    pub gain: f32,
    pub gain_step: f32,
    /// Modulation by the output of another oscillator, if any.
    pub modulation: Option<Modulation<'a>>,
    /// Number of unison voices in use.
    pub voices: usize,
    /// Per voice multipliers of the phase delta, see
    /// [`OscillatorParams::update_detune_factors()`].
    pub detune_factors: &'a [f32x8; UNISON_VECTORS],
    /// 1 for the voices in use and 0 for the rest.
    pub mask: &'a [f32x8; UNISON_VECTORS],
    /// Per voice gains for the left and right outputs, zero for unused voices, see
    /// [`OscillatorParams::update_pan_gains()`]. Only used when the output is stereo.
    pub pan_left: &'a [f32x8; UNISON_VECTORS],
    pub pan_right: &'a [f32x8; UNISON_VECTORS],
}

/// One oscillator's output modulating another's over a block, see [`BlockParams`]. The depths
/// are in the units the kernels use; a depth of zero switches that modulation off.
pub struct Modulation<'a> {
    /// The modulator's output, one sample per sample of the block.
    pub input: &'a [f32],
    /// Frequency modulation: the phase delta is multiplied by `1 + input * fm`.
    pub fm: f32,
    /// Phase modulation: `input * pm` cycles are added to the phase that is read.
    pub pm: f32,
    /// Amplitude modulation: the gain is scaled by the unipolar input, `(input + 1) / 2`, fully
    /// at `am = 1` and not at all at `am = 0`.
    pub am: f32,
}

/// Where [`FramePair::render_block()`] puts its samples. The oscillator's output is added to
/// `left` (and `right`), so several oscillators can render into the same buffers.
pub struct BlockOutput<'a> {
    /// The left output or, without `right`, the mono output.
    pub left: &'a mut [f32],
    /// The right output. When given, the voices are panned with their pan gains instead of
    /// being summed with the mask.
    pub right: Option<&'a mut [f32]>,
    /// The plain (masked, gain-free) sum of the voices times the given scale, for modulating
    /// another oscillator.
    pub mono: Option<(&'a mut [f32], f32)>,
}

impl FramePair<'_> {
    /// Samples per cycle.
    #[inline]
    pub fn cycle_len(&self) -> usize {
        self.a.cycle_len()
    }
    /// Reads the frames at a fractional sample index in `[0, cycle_len)` with cubic
    /// interpolation.
    #[inline]
    pub fn read(&self, index: f32) -> f32 {
        self.read_with::<CUBIC>(index)
    }
    /// Reads the frames at a fractional sample index in `[0, cycle_len)` with the given
    /// interpolation mode (an [`Interpolation`] discriminant).
    #[inline]
    pub fn read_with<const INTERP: u8>(&self, index: f32) -> f32 {
        let a = self.a.read_with::<INTERP>(index);
        if self.mix > 0.0 {
            lerp(a, self.b.read_with::<INTERP>(index), self.mix)
        } else {
            a
        }
    }
    /// Reads the frames at a fixed point phase with the given interpolation mode.
    #[inline]
    pub fn read_fixed<const INTERP: u8>(&self, phase: u32) -> f32 {
        let a = self.a.read_fixed::<INTERP>(phase);
        if self.mix > 0.0 {
            lerp(a, self.b.read_fixed::<INTERP>(phase), self.mix)
        } else {
            a
        }
    }
    /// Renders one block of an oscillator's unison voices into the output: every sample
    /// advances the voices' phases by their (detuned, possibly modulated) deltas, reads and
    /// interpolates the frames at each voice, and sums the voices. The block is
    /// `output.left.len()` samples long; the other buffers must be at least that long.
    pub fn render_block(
        &self,
        voice: &mut SuperVoice,
        params: &BlockParams,
        output: &mut BlockOutput,
        simd: SimdLevel,
        interpolation: Interpolation,
    ) {
        let block_len = output.left.len();
        if let Some(right) = &output.right {
            assert!(right.len() >= block_len);
        }
        if let Some((mono, _)) = &output.mono {
            assert!(mono.len() >= block_len);
        }
        if let Some(modulation) = &params.modulation {
            assert!(modulation.input.len() >= block_len);
        }
        debug_assert_eq!(self.a.cycle_len(), self.b.cycle_len());
        match interpolation {
            Interpolation::None => self.render_block_with::<NONE>(voice, params, output, simd),
            Interpolation::Linear => self.render_block_with::<LINEAR>(voice, params, output, simd),
            Interpolation::Cubic => self.render_block_with::<CUBIC>(voice, params, output, simd),
        }
    }
    /// [`render_block()`][Self::render_block()] specialised for one interpolation mode.
    fn render_block_with<const INTERP: u8>(
        &self,
        voice: &mut SuperVoice,
        params: &BlockParams,
        output: &mut BlockOutput,
        simd: SimdLevel,
    ) {
        if params.voices <= SCALAR_VOICE_LIMIT {
            return lookup::scalar::render_block::<INTERP>(self, voice, params, output);
        }
        match simd {
            SimdLevel::Portable => {
                lookup::portable::render_block::<INTERP>(self, voice, params, output)
            }
            // SAFETY: the AVX2 levels are only produced by `SimdLevel::detect()` when the CPU
            // supports AVX2 and FMA.
            #[cfg(target_arch = "x86_64")]
            SimdLevel::Avx2 => unsafe {
                lookup::avx2::render_block::<false, INTERP>(self, voice, params, output)
            },
            #[cfg(target_arch = "x86_64")]
            SimdLevel::Avx2Gather => unsafe {
                lookup::avx2::render_block::<true, INTERP>(self, voice, params, output)
            },
        }
    }
}

/// One voice's value from the per voice vectors.
#[inline]
fn lane(vectors: &[f32x8; UNISON_VECTORS], voice: usize) -> f32 {
    vectors[voice / UNISON_LANES].as_array()[voice % UNISON_LANES]
}

/// One voice's phase from the per voice vectors.
#[inline]
fn lane_u32(vectors: &[u32x8; UNISON_VECTORS], voice: usize) -> u32 {
    vectors[voice / UNISON_LANES].as_array()[voice % UNISON_LANES]
}

/// One voice's phase in the per voice vectors.
#[inline]
fn lane_u32_mut(vectors: &mut [u32x8; UNISON_VECTORS], voice: usize) -> &mut u32 {
    &mut vectors[voice / UNISON_LANES].as_mut_array()[voice % UNISON_LANES]
}

/// The hot loop: rendering a block of all the unison voices from the wavetable frames. Each
/// kernel is specialised per interpolation mode through `INTERP`, an [`Interpolation`]
/// discriminant. The kernels agree with each other to the last bit of the phases: the fixed
/// point conversions are exact, so only the interpolation arithmetic differs (by rounding).
mod lookup {
    use super::*;

    /// The control values of a block that are shared by all the voices: the ramped gain and
    /// base delta, and the modulation of each sample.
    pub struct Controls<'a> {
        params: &'a BlockParams<'a>,
        delta: f32,
        gain: f32,
    }

    /// One sample's control values.
    pub struct SampleControls {
        /// The base phase delta in cycles when frequency modulation changed it this sample.
        /// Otherwise the per voice deltas just ramp.
        pub fm_delta: Option<f32>,
        /// Added to the phases that are read (phase modulation).
        pub phase_offset: u32,
        pub gain: f32,
    }

    impl<'a> Controls<'a> {
        pub fn new(params: &'a BlockParams<'a>) -> Self {
            Self {
                params,
                delta: params.delta,
                gain: params.gain,
            }
        }
        /// The control values of sample `n` of the block.
        #[inline]
        pub fn next(&mut self, n: usize) -> SampleControls {
            let mut controls = SampleControls {
                fm_delta: None,
                phase_offset: 0,
                gain: self.gain,
            };
            if let Some(modulation) = &self.params.modulation {
                let input = modulation.input[n];
                if modulation.fm > 0.0 {
                    controls.fm_delta =
                        Some(wrap_cycles_signed(self.delta * (1.0 + input * modulation.fm)));
                }
                if modulation.pm > 0.0 {
                    controls.phase_offset = cycles_to_fixed(input * modulation.pm);
                }
                if modulation.am > 0.0 {
                    controls.gain *= lerp(1.0, (input + 1.0) / 2.0, modulation.am);
                }
            }
            self.delta += self.params.delta_step;
            self.gain += self.params.gain_step;
            controls
        }
    }

    /// Adds one sample's voice sums to the outputs. The sums are closures so that only the
    /// ones needed are reduced.
    #[inline]
    fn write_sample(
        output: &mut BlockOutput,
        n: usize,
        gain: f32,
        mono: impl Fn() -> f32,
        left: impl FnOnce() -> f32,
        right: impl FnOnce() -> f32,
    ) {
        if let Some(right_out) = output.right.as_deref_mut() {
            output.left[n] += left() * gain;
            right_out[n] += right() * gain;
        } else {
            output.left[n] += mono() * gain;
        }
        if let Some((mono_out, scale)) = &mut output.mono {
            mono_out[n] = mono() * *scale;
        }
    }

    /// Up to [`SCALAR_VOICE_LIMIT`] voices, one at a time.
    pub mod scalar {
        use super::*;

        pub fn render_block<const INTERP: u8>(
            frames: &FramePair,
            voice: &mut SuperVoice,
            params: &BlockParams,
            output: &mut BlockOutput,
        ) {
            let voices = params.voices.min(SCALAR_VOICE_LIMIT);
            let mut phases = [0_u32; SCALAR_VOICE_LIMIT];
            let mut deltas = [0_u32; SCALAR_VOICE_LIMIT];
            let mut delta_steps = [0_u32; SCALAR_VOICE_LIMIT];
            for i in 0..voices {
                let factor = lane(params.detune_factors, i);
                phases[i] = lane_u32(&voice.voice_phases, i);
                deltas[i] = cycles_to_fixed(params.delta * factor);
                delta_steps[i] = cycles_to_fixed(params.delta_step * factor);
            }
            let stereo = output.right.is_some();
            let needs_mono = !stereo || output.mono.is_some();

            let mut controls = Controls::new(params);
            for n in 0..output.left.len() {
                let sample = controls.next(n);
                if let Some(delta) = sample.fm_delta {
                    for (i, voice_delta) in deltas.iter_mut().enumerate().take(voices) {
                        *voice_delta = cycles_to_fixed(delta * lane(params.detune_factors, i));
                    }
                }
                let (mut mono, mut left, mut right) = (0.0, 0.0, 0.0);
                for i in 0..voices {
                    phases[i] = phases[i].wrapping_add(deltas[i]);
                    if sample.fm_delta.is_none() {
                        deltas[i] = deltas[i].wrapping_add(delta_steps[i]);
                    }
                    let value =
                        frames.read_fixed::<INTERP>(phases[i].wrapping_add(sample.phase_offset));
                    if needs_mono {
                        mono += value * lane(params.mask, i);
                    }
                    if stereo {
                        left += value * lane(params.pan_left, i);
                        right += value * lane(params.pan_right, i);
                    }
                }
                write_sample(output, n, sample.gain, || mono, || left, || right);
            }
            for (i, phase) in phases.iter().enumerate().take(voices) {
                *lane_u32_mut(&mut voice.voice_phases, i) = *phase;
            }
        }
    }

    /// The `wide` path: vector arithmetic (SSE2 or NEON), scalar table reads.
    pub mod portable {
        use super::*;

        /// Horizontal sum of a vector's lanes, as a tree to keep the dependency chain short.
        #[inline]
        fn horizontal_sum(vector: f32x8) -> f32 {
            let l = vector.as_array();
            ((l[0] + l[1]) + (l[2] + l[3])) + ((l[4] + l[5]) + (l[6] + l[7]))
        }

        /// Cubic interpolation of 8 voices, see [`hermite()`][super::super::hermite()].
        #[inline]
        fn hermite8(y0: f32x8, y1: f32x8, y2: f32x8, y3: f32x8, t: f32x8) -> f32x8 {
            let half = f32x8::splat(0.5);
            let c1 = half * (y2 - y0);
            let c2 = y0 - f32x8::splat(2.5) * y1 + f32x8::splat(2.0) * y2 - half * y3;
            let c3 = half * (y3 - y0) + f32x8::splat(1.5) * (y1 - y2);
            ((c3 * t + c2) * t + c1) * t + y1
        }

        /// [`cycles_to_fixed()`] for 8 lanes. Exact: the fraction of a cycle is converted in
        /// two halves of 16 bits, each of which fits a float's mantissa.
        #[inline]
        pub fn cycles_to_fixed8(cycles: f32x8) -> u32x8 {
            let scale = f32x8::splat(65536.0);
            let scaled = (cycles - cycles.floor()) * scale;
            let high = scaled.trunc_int();
            let low = ((scaled - high.round_float()) * scale).trunc_int();
            (cast::<i32x8, u32x8>(high) << 16) | cast::<i32x8, u32x8>(low)
        }

        /// The fixed point phase deltas of one vector of voices: the base delta times their
        /// detune factors.
        #[inline]
        fn voice_deltas(delta: f32, factors: &f32x8) -> u32x8 {
            cycles_to_fixed8(f32x8::splat(delta) * *factors)
        }

        /// Reads 8 voices from a table at the given fixed point phases, with the interpolation
        /// mode given by `INTERP` (an [`Interpolation`] discriminant).
        #[inline]
        fn read8<const INTERP: u8>(table: &Wavetable, phases: u32x8) -> f32x8 {
            let floors = (phases >> table.index_shift()).to_array();
            let samples = &table.table;
            let mut taps = [[0.0_f32; UNISON_LANES]; 4];
            for (lane, &floor) in floors.iter().enumerate() {
                let floor = floor as usize;
                // The index is below the cycle length and the table is padded, so the 4 taps
                // are always inside it
                let row = &samples[floor..floor + 4];
                match INTERP {
                    NONE => taps[1][lane] = row[1],
                    LINEAR => {
                        taps[1][lane] = row[1];
                        taps[2][lane] = row[2];
                    }
                    _ => {
                        taps[0][lane] = row[0];
                        taps[1][lane] = row[1];
                        taps[2][lane] = row[2];
                        taps[3][lane] = row[3];
                    }
                }
            }
            let y1 = f32x8::new(taps[1]);
            if INTERP == NONE {
                return y1;
            }
            let frac = cast::<u32x8, i32x8>(phases & u32x8::splat(table.frac_mask())).round_float()
                * f32x8::splat(table.frac_scale());
            if INTERP == LINEAR {
                y1 + (f32x8::new(taps[2]) - y1) * frac
            } else {
                hermite8(
                    f32x8::new(taps[0]),
                    y1,
                    f32x8::new(taps[2]),
                    f32x8::new(taps[3]),
                    frac,
                )
            }
        }

        /// Reads 8 voices from the frame pair.
        #[inline]
        fn read_frames8<const INTERP: u8>(frames: &FramePair, phases: u32x8) -> f32x8 {
            let a = read8::<INTERP>(frames.a, phases);
            if frames.mix > 0.0 {
                let b = read8::<INTERP>(frames.b, phases);
                a + (b - a) * f32x8::splat(frames.mix)
            } else {
                a
            }
        }

        pub fn render_block<const INTERP: u8>(
            frames: &FramePair,
            voice: &mut SuperVoice,
            params: &BlockParams,
            output: &mut BlockOutput,
        ) {
            let vectors = params.voices.div_ceil(UNISON_LANES).min(UNISON_VECTORS);
            let mut phases = voice.voice_phases;
            // Unused voices get a delta of zero, so they stay where they are
            let factors: [f32x8; UNISON_VECTORS] =
                std::array::from_fn(|vector| params.detune_factors[vector] * params.mask[vector]);
            let mut deltas = [u32x8::ZERO; UNISON_VECTORS];
            let mut delta_steps = [u32x8::ZERO; UNISON_VECTORS];
            for vector in 0..vectors {
                deltas[vector] = voice_deltas(params.delta, &factors[vector]);
                delta_steps[vector] = voice_deltas(params.delta_step, &factors[vector]);
            }
            let stereo = output.right.is_some();
            let needs_mono = !stereo || output.mono.is_some();

            let mut controls = Controls::new(params);
            for n in 0..output.left.len() {
                let sample = controls.next(n);
                if let Some(delta) = sample.fm_delta {
                    for (vector_deltas, factors) in deltas.iter_mut().zip(&factors).take(vectors) {
                        *vector_deltas = voice_deltas(delta, factors);
                    }
                }
                let offset = u32x8::splat(sample.phase_offset);
                let (mut mono, mut left, mut right) = (f32x8::ZERO, f32x8::ZERO, f32x8::ZERO);
                for vector in 0..vectors {
                    phases[vector] += deltas[vector];
                    if sample.fm_delta.is_none() {
                        deltas[vector] += delta_steps[vector];
                    }
                    let samples = read_frames8::<INTERP>(frames, phases[vector] + offset);
                    if needs_mono {
                        mono += samples * params.mask[vector];
                    }
                    if stereo {
                        left += samples * params.pan_left[vector];
                        right += samples * params.pan_right[vector];
                    }
                }
                write_sample(
                    output,
                    n,
                    sample.gain,
                    || horizontal_sum(mono),
                    || horizontal_sum(left),
                    || horizontal_sum(right),
                );
            }
            voice.voice_phases = phases;
        }
    }

    /// The same with AVX2 and FMA. Every function here must only be called after checking
    /// that the CPU supports both.
    #[cfg(target_arch = "x86_64")]
    pub mod avx2 {
        use std::arch::x86_64::*;

        use super::*;

        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn horizontal_sum(vector: __m256) -> f32 {
            let halves = _mm_add_ps(
                _mm256_castps256_ps128(vector),
                _mm256_extractf128_ps::<1>(vector),
            );
            let pairs = _mm_add_ps(halves, _mm_movehl_ps(halves, halves));
            _mm_cvtss_f32(_mm_add_ss(pairs, _mm_shuffle_ps::<0x55>(pairs, pairs)))
        }

        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn load(vector: &f32x8) -> __m256 {
            _mm256_loadu_ps(vector.as_array().as_ptr())
        }

        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn load_u32(vector: &u32x8) -> __m256i {
            _mm256_loadu_si256(vector.as_array().as_ptr() as *const __m256i)
        }

        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn store_u32(vector: &mut u32x8, value: __m256i) {
            _mm256_storeu_si256(vector.as_mut_array().as_mut_ptr() as *mut __m256i, value)
        }

        /// [`cycles_to_fixed()`] for 8 lanes, see the portable version.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        pub unsafe fn cycles_to_fixed8(cycles: __m256) -> __m256i {
            let scale = _mm256_set1_ps(65536.0);
            let scaled = _mm256_mul_ps(_mm256_sub_ps(cycles, _mm256_floor_ps(cycles)), scale);
            let high = _mm256_cvttps_epi32(scaled);
            let low = _mm256_cvttps_epi32(_mm256_mul_ps(
                _mm256_sub_ps(scaled, _mm256_cvtepi32_ps(high)),
                scale,
            ));
            _mm256_or_si256(_mm256_slli_epi32::<16>(high), low)
        }

        /// The fixed point phase deltas of one vector of voices.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn voice_deltas(delta: f32, factors: &f32x8) -> __m256i {
            cycles_to_fixed8(_mm256_mul_ps(_mm256_set1_ps(delta), load(factors)))
        }

        /// Cubic interpolation of 8 voices, see [`hermite()`][super::super::hermite()].
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn hermite8(y0: __m256, y1: __m256, y2: __m256, y3: __m256, t: __m256) -> __m256 {
            let half = _mm256_set1_ps(0.5);
            let c1 = _mm256_mul_ps(half, _mm256_sub_ps(y2, y0));
            // y0 - 2.5 y1 + 2 y2 - 0.5 y3
            let c2 = _mm256_fmadd_ps(
                _mm256_set1_ps(-2.5),
                y1,
                _mm256_fmadd_ps(_mm256_set1_ps(2.0), y2, _mm256_fnmadd_ps(half, y3, y0)),
            );
            // 0.5 (y3 - y0) + 1.5 (y1 - y2)
            let c3 = _mm256_fmadd_ps(
                half,
                _mm256_sub_ps(y3, y0),
                _mm256_mul_ps(_mm256_set1_ps(1.5), _mm256_sub_ps(y1, y2)),
            );
            let poly = _mm256_fmadd_ps(c3, t, c2);
            let poly = _mm256_fmadd_ps(poly, t, c1);
            _mm256_fmadd_ps(poly, t, y1)
        }

        /// A wavetable's lookup constants in vector form.
        struct Table {
            base: *const f32,
            /// The index shift as a shift count.
            shift: __m128i,
            frac_mask: __m256i,
            frac_scale: __m256,
        }

        impl Table {
            #[inline]
            #[target_feature(enable = "avx2,fma")]
            unsafe fn new(table: &Wavetable) -> Self {
                Self {
                    base: table.table.as_ptr(),
                    shift: _mm_cvtsi32_si128(table.index_shift() as i32),
                    frac_mask: _mm256_set1_epi32(table.frac_mask() as i32),
                    frac_scale: _mm256_set1_ps(table.frac_scale()),
                }
            }
        }

        /// The 4 taps `(y0, y1, y2, y3)` of 8 voices, each as a vector with one lane per voice.
        /// `y1` is the sample below the read position. Modes that use fewer taps leave the
        /// others zero.
        type Taps = (__m256, __m256, __m256, __m256);

        /// Reads the taps of 8 voices with gather instructions.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn taps_gather<const INTERP: u8>(base: *const f32, floor: __m256i) -> Taps {
            let zero = _mm256_setzero_ps();
            let y1 = _mm256_i32gather_ps::<4>(base.add(1), floor);
            if INTERP == NONE {
                return (zero, y1, zero, zero);
            }
            let y2 = _mm256_i32gather_ps::<4>(base.add(2), floor);
            if INTERP == LINEAR {
                return (zero, y1, y2, zero);
            }
            (
                _mm256_i32gather_ps::<4>(base, floor),
                y1,
                y2,
                _mm256_i32gather_ps::<4>(base.add(3), floor),
            )
        }

        /// Reads the taps of 8 voices with one 128-bit load per voice (all 4 taps, which is
        /// no dearer than loading fewer), then transposes the 8 rows of 4 taps into one vector
        /// of 8 voices per tap the interpolation needs.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn taps_transpose<const INTERP: u8>(base: *const f32, floor: __m256i) -> Taps {
            let mut offsets = [0_i32; UNISON_LANES];
            _mm256_storeu_si256(offsets.as_mut_ptr() as *mut __m256i, floor);
            let row = |voice: usize| _mm_loadu_ps(base.add(offsets[voice] as usize));
            // Pair up rows so that each 128-bit half holds one row: (r0|r4), (r1|r5), ...
            let pair = |low: usize| _mm256_insertf128_ps::<1>(_mm256_castps128_ps256(row(low)), row(low + 4));
            let (a, b, c, d) = (pair(0), pair(1), pair(2), pair(3));
            let zero = _mm256_setzero_ps();
            // Interleave: t0 = r0[0] r1[0] r0[1] r1[1] | r4[0] r5[0] r4[1] r5[1], etc.
            let t0 = _mm256_unpacklo_ps(a, b);
            let t2 = _mm256_unpacklo_ps(c, d);
            let y1 = _mm256_shuffle_ps::<0xEE>(t0, t2);
            if INTERP == NONE {
                return (zero, y1, zero, zero);
            }
            let t1 = _mm256_unpackhi_ps(a, b);
            let t3 = _mm256_unpackhi_ps(c, d);
            let y2 = _mm256_shuffle_ps::<0x44>(t1, t3);
            if INTERP == LINEAR {
                return (zero, y1, y2, zero);
            }
            (
                _mm256_shuffle_ps::<0x44>(t0, t2),
                y1,
                y2,
                _mm256_shuffle_ps::<0xEE>(t1, t3),
            )
        }

        /// Reads 8 voices from a table at the given fixed point phases, with the interpolation
        /// mode given by `INTERP` (an [`Interpolation`] discriminant).
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn read8<const GATHER: bool, const INTERP: u8>(
            table: &Table,
            phases: __m256i,
        ) -> __m256 {
            let floor = _mm256_srl_epi32(phases, table.shift);
            // SAFETY: the index is below the cycle length, and the table is padded with the
            // neighbouring samples, so all 4 taps are inside it.
            let (y0, y1, y2, y3) = if GATHER {
                taps_gather::<INTERP>(table.base, floor)
            } else {
                taps_transpose::<INTERP>(table.base, floor)
            };
            if INTERP == NONE {
                return y1;
            }
            let frac = _mm256_mul_ps(
                _mm256_cvtepi32_ps(_mm256_and_si256(phases, table.frac_mask)),
                table.frac_scale,
            );
            if INTERP == LINEAR {
                _mm256_fmadd_ps(_mm256_sub_ps(y2, y1), frac, y1)
            } else {
                hermite8(y0, y1, y2, y3, frac)
            }
        }

        /// Reads 8 voices from the frame pair.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn read_frames8<const GATHER: bool, const INTERP: u8>(
            a: &Table,
            b: &Table,
            mix: f32,
            phases: __m256i,
        ) -> __m256 {
            let a = read8::<GATHER, INTERP>(a, phases);
            if mix > 0.0 {
                let b = read8::<GATHER, INTERP>(b, phases);
                _mm256_fmadd_ps(_mm256_sub_ps(b, a), _mm256_set1_ps(mix), a)
            } else {
                a
            }
        }

        #[target_feature(enable = "avx2,fma")]
        pub unsafe fn render_block<const GATHER: bool, const INTERP: u8>(
            frames: &FramePair,
            voice: &mut SuperVoice,
            params: &BlockParams,
            output: &mut BlockOutput,
        ) {
            let table_a = Table::new(frames.a);
            let table_b = Table::new(frames.b);
            let vectors = params.voices.div_ceil(UNISON_LANES).min(UNISON_VECTORS);
            let mut phases: [__m256i; UNISON_VECTORS] =
                std::array::from_fn(|vector| load_u32(&voice.voice_phases[vector]));
            // Unused voices get a delta of zero, so they stay where they are
            let factors: [f32x8; UNISON_VECTORS] =
                std::array::from_fn(|vector| params.detune_factors[vector] * params.mask[vector]);
            let mut deltas = [_mm256_setzero_si256(); UNISON_VECTORS];
            let mut delta_steps = [_mm256_setzero_si256(); UNISON_VECTORS];
            for vector in 0..vectors {
                deltas[vector] = voice_deltas(params.delta, &factors[vector]);
                delta_steps[vector] = voice_deltas(params.delta_step, &factors[vector]);
            }
            let stereo = output.right.is_some();
            let needs_mono = !stereo || output.mono.is_some();

            let mut controls = Controls::new(params);
            for n in 0..output.left.len() {
                let sample = controls.next(n);
                if let Some(delta) = sample.fm_delta {
                    for (vector_deltas, factors) in deltas.iter_mut().zip(&factors).take(vectors) {
                        *vector_deltas = voice_deltas(delta, factors);
                    }
                }
                let offset = _mm256_set1_epi32(sample.phase_offset as i32);
                let mut mono = _mm256_setzero_ps();
                let mut left = _mm256_setzero_ps();
                let mut right = _mm256_setzero_ps();
                for vector in 0..vectors {
                    phases[vector] = _mm256_add_epi32(phases[vector], deltas[vector]);
                    if sample.fm_delta.is_none() {
                        deltas[vector] = _mm256_add_epi32(deltas[vector], delta_steps[vector]);
                    }
                    let samples = read_frames8::<GATHER, INTERP>(
                        &table_a,
                        &table_b,
                        frames.mix,
                        _mm256_add_epi32(phases[vector], offset),
                    );
                    if needs_mono {
                        mono = _mm256_fmadd_ps(samples, load(&params.mask[vector]), mono);
                    }
                    if stereo {
                        left = _mm256_fmadd_ps(samples, load(&params.pan_left[vector]), left);
                        right = _mm256_fmadd_ps(samples, load(&params.pan_right[vector]), right);
                    }
                }
                write_sample(
                    output,
                    n,
                    sample.gain,
                    || horizontal_sum(mono),
                    || horizontal_sum(left),
                    || horizontal_sum(right),
                );
            }
            for (vector, phase) in phases.iter().enumerate() {
                store_u32(&mut voice.voice_phases[vector], *phase);
            }
        }
    }
}

/// Wave generator with a bandlimited wavetable for each midi note index (extended to the 44.1 kHz
/// Nyquist frequency, so 138 notes), or for each group of notes (see [`NoteLevels`]).
///
/// Wavetables can be trivially bandlimited by generating each table with additive synthesis
/// with harmonics up to the Nyquist frequency. The table can then be sampled for cheap,
/// alias-free synthesis.
///
/// ... At least, in theory. If we had a perfect sampling function (which consists of summing
/// sines again), this would be the case but we are instead using (at best) cubic interpolation,
/// which deviates from the bandlimited signal in between the samples and so aliases a little.
/// The artifacts are suppressed by oversampling the wavetables (making them several times
/// longer than the harmonics need), which reduces their amplitude and makes them more
/// harmonically distant from the source: each table is at least 8 times longer than twice its
/// highest harmonic (see [`table_len_for()`]), up to a maximum length that caps the lowest notes.
///
/// Of course, all this is made sort of irrelevant by the fact that you can just use additive
/// synthesis while rendering since you have unlimited time to generate the sound, thus providing
/// completely alias-free oscillators. But it was fun to implement, and it's fun to perform in
/// realtime because of how cheap it is to sample + interpolate.
pub struct WavetableNotes {
    /// One table per level of the frames' [`NoteLevels`].
    pub tables: Vec<Wavetable>,
}

impl WavetableNotes {
    /// Returns `index` from the equation
    /// `440.0 * 2.0_f32.powf(index / 12.0) = frequency`
    pub fn frequency_to_note(frequency: f32) -> usize {
        (((frequency / 440.0).log2() * 12.0 + 69.0).round() as usize).clamp(0, 137)
    }
    /// Returns the index of the lowest note whose table is bandlimited enough for a phase delta
    /// of `delta` cycles per sample, i.e. `index` from `440.0 * 2.0_f32.powf(index / 12.0) =
    /// delta * sample_rate`, rounded up.
    pub fn cycles_to_note(delta: f32, sample_rate: f32) -> usize {
        let note = fastmath::log2(sample_rate * delta / 440.0) * 12.0 + 69.0;
        // Rounded up, without the `ceil()` library call: `as` truncates, and saturates at 0.
        let truncated = note as usize;
        let note = if (truncated as f32) < note {
            truncated + 1
        } else {
            truncated
        };
        note.min(137)
    }
    /// The number of harmonics a note can have below the Nyquist frequency.
    pub fn harmonics_for_note(note: usize, sample_rate: f32) -> usize {
        if note >= NoteLevels::NOTES - 1 {
            // The highest note has no harmonics, so notes above the Nyquist frequency are silent
            0
        } else {
            (sample_rate / (2.0 * 440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0))).floor()
                as usize
        }
    }
    /// Generates a bandlimited wavetable for each level of `levels` from a spectrum, each at
    /// most `max_len` samples long (see [`table_len_for()`]).
    pub fn from_spectrum(
        spectrum: &Spectrum,
        sample_rate: f32,
        levels: &NoteLevels,
        max_len: usize,
        planner: &mut FftPlanner<f32>,
    ) -> Self {
        let tables = (0..levels.count())
            .map(|level| {
                let harmonics = WavetableNotes::harmonics_for_note(levels.top_note(level), sample_rate)
                    .min(spectrum.harmonics());
                let len = table_len_for(harmonics, max_len);
                let fft = planner.plan_fft_inverse(len);
                Wavetable::from_spectrum(spectrum, len, harmonics.min(len / 2), &*fft)
            })
            .collect();
        Self { tables }
    }
}

/// The smallest table a note gets, so that the top notes with only a few harmonics still have
/// something to interpolate.
pub const MIN_TABLE_LEN: usize = 64;

/// The length of a table that holds `harmonics` harmonics: a power of two at least 8 times the
/// 2 samples per cycle the highest harmonic strictly needs (so the interpolation has little to
/// do), between [`MIN_TABLE_LEN`] and `max_len`.
pub fn table_len_for(harmonics: usize, max_len: usize) -> usize {
    (harmonics * 16)
        .next_power_of_two()
        .clamp(MIN_TABLE_LEN, max_len.max(MIN_TABLE_LEN))
}

/// How the notes of a wavetable share bandlimited tables: notes are grouped into levels of
/// `step` semitones, and each level's table is bandlimited for the highest note of the group.
/// A step of 1 gives every note its own table, so the harmonics of every note extend to the
/// Nyquist frequency; larger steps trade the top of the spectrum of the lower notes of each group
/// (less than a step's worth of octave, so the highest harmonics) for memory, which matters for
/// wavetables with many frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteLevels {
    step: usize,
}

impl NoteLevels {
    /// The MIDI notes covered: 0 to 137, which reaches the 44.1 kHz Nyquist frequency.
    pub const NOTES: usize = 138;
    /// The steps to choose from when fitting a wavetable into a memory budget.
    pub const STEPS: [usize; 6] = [1, 2, 3, 4, 6, 12];

    pub fn new(step: usize) -> Self {
        Self {
            step: step.clamp(1, Self::NOTES),
        }
    }
    /// A level for every note.
    pub fn per_note() -> Self {
        Self::new(1)
    }
    pub fn step(&self) -> usize {
        self.step
    }
    /// The number of levels (tables).
    pub fn count(&self) -> usize {
        Self::NOTES.div_ceil(self.step)
    }
    /// The level a note reads.
    #[inline]
    pub fn level(&self, note: usize) -> usize {
        note.min(Self::NOTES - 1) / self.step
    }
    /// The highest note of a level, which its table is bandlimited for.
    pub fn top_note(&self, level: usize) -> usize {
        ((level + 1) * self.step - 1).min(Self::NOTES - 1)
    }
    /// The sample data of a wavetable with `frames` frames of a spectrum with `harmonics`
    /// harmonics at these levels, in bytes.
    pub fn memory_bytes(&self, frames: usize, harmonics: usize, sample_rate: f32, max_len: usize) -> usize {
        let samples: usize = (0..self.count())
            .map(|level| {
                let harmonics = WavetableNotes::harmonics_for_note(self.top_note(level), sample_rate)
                    .min(harmonics);
                table_len_for(harmonics, max_len) + TABLE_PADDING_BEFORE + TABLE_PADDING_AFTER
            })
            .sum();
        samples * frames * std::mem::size_of::<f32>()
    }
    /// The finest levels whose wavetable fits in `budget` bytes (or the coarsest, if none do).
    pub fn fitting(budget: usize, frames: usize, harmonics: usize, sample_rate: f32, max_len: usize) -> Self {
        Self::STEPS
            .iter()
            .map(|&step| Self::new(step))
            .find(|levels| levels.memory_bytes(frames, harmonics, sample_rate, max_len) <= budget)
            .unwrap_or(Self::new(*Self::STEPS.last().unwrap()))
    }
}

/// The memory a wavetable's tables may take, which decides its [`NoteLevels`].
pub const WAVETABLE_MEMORY_BUDGET: usize = 24 << 20;

/// Number of frames in the pulse width sweep wavetable.
pub const PW_SWEEP_FRAMES: usize = 32;

/// A wavetable in the wavetable synth sense: one or more frames (waveforms, each bandlimited per
/// note) that an oscillator can morph between with its position control.
pub struct WavetableFrames {
    /// Display name, which matches the oscillators' wave parameter for the built in ones.
    pub name: String,
    pub frames: Vec<WavetableNotes>,
    /// Which table of each frame a note reads.
    pub levels: NoteLevels,
}
impl WavetableFrames {
    /// Generates a wavetable from the spectra of its frames, for the given sample rate. The
    /// levels are picked to fit [`WAVETABLE_MEMORY_BUDGET`].
    pub fn from_spectra(
        name: impl Into<String>,
        spectra: &[Spectrum],
        sample_rate: f32,
        max_len: usize,
        planner: &mut FftPlanner<f32>,
    ) -> Self {
        let harmonics = spectra
            .iter()
            .map(Spectrum::harmonics)
            .max()
            .unwrap_or(0);
        let levels = NoteLevels::fitting(
            WAVETABLE_MEMORY_BUDGET,
            spectra.len(),
            harmonics,
            sample_rate,
            max_len,
        );
        Self {
            name: name.into(),
            frames: spectra
                .iter()
                .map(|spectrum| {
                    WavetableNotes::from_spectrum(spectrum, sample_rate, &levels, max_len, planner)
                })
                .collect(),
            levels,
        }
    }
    /// Resolves a position (0 to 1) to the two frames to crossfade between and the mix amount
    /// between them.
    pub fn frame_indices(&self, position: f32) -> (usize, usize, f32) {
        let last = self.frames.len().saturating_sub(1);
        let scaled = position.clamp(0.0, 1.0) * last as f32;
        let a = (scaled as usize).min(last);
        let b = (a + 1).min(last);
        (a, b, scaled - a as f32)
    }
    /// The table of a frame that is bandlimited for a MIDI note (see
    /// [`WavetableNotes::cycles_to_note()`]).
    #[inline]
    pub fn table(&self, frame: usize, note: usize) -> &Wavetable {
        &self.frames[frame].tables[self.levels.level(note)]
    }
    /// The wavetable frames to generate a voice from: the two frames to crossfade between and
    /// the mix between them (see [`frame_indices()`][Self::frame_indices()]), bandlimited for
    /// the given MIDI note.
    #[inline]
    pub fn frame_pair(&self, (a, b, mix): (usize, usize, f32), note: usize) -> FramePair<'_> {
        FramePair {
            a: self.table(a, note),
            b: self.table(b, note),
            mix,
        }
    }
    /// Size of the wavetable's sample data.
    pub fn memory_bytes(&self) -> usize {
        self.frames
            .iter()
            .flat_map(|notes| notes.tables.iter())
            .map(|table| table.table.len() * std::mem::size_of::<f32>())
            .sum()
    }
}

/// All the wavetables the oscillators can choose from.
pub struct WavetableSet {
    pub wavetables: Vec<WavetableFrames>,
}
impl WavetableSet {
    /// Generates the wavetables for the given sample rate, with tables of at most `max_len`
    /// samples. The order matches the oscillators' wave parameter.
    pub fn new(sample_rate: f32, max_len: usize) -> Self {
        // One planner is shared by all tables; it caches a plan per length.
        let mut planner = FftPlanner::new();
        let mut single = |name: &str, osc: AdditiveOsc| {
            WavetableFrames::from_spectra(name, &[osc.spectrum()], sample_rate, max_len, &mut planner)
        };
        let sine = single("Sine", AdditiveOsc::sine());
        let triangle = single("Triangle", AdditiveOsc::triangle());
        let saw = single("Saw", AdditiveOsc::saw());
        let exponential = single("Exponential", AdditiveOsc::fake_exp());
        let square = single("Square", AdditiveOsc::square());
        let pulse40 = single("Pulse 40%", AdditiveOsc::pulse(0.4));
        let pulse30 = single("Pulse 30%", AdditiveOsc::pulse(0.3));
        let pulse20 = single("Pulse 20%", AdditiveOsc::pulse(0.2));
        let pulse10 = single("Pulse 10%", AdditiveOsc::pulse(0.1));

        // From a square wave down to a very narrow pulse
        let sweep_spectra: Vec<Spectrum> = (0..PW_SWEEP_FRAMES)
            .map(|frame| {
                let duty = 0.5 * (1.0 - frame as f32 / PW_SWEEP_FRAMES as f32);
                AdditiveOsc::<2048>::pulse(duty).spectrum()
            })
            .collect();
        let sweep =
            WavetableFrames::from_spectra("PW Sweep", &sweep_spectra, sample_rate, max_len, &mut planner);

        Self {
            wavetables: vec![
                sine,
                triangle,
                saw,
                exponential,
                square,
                pulse40,
                pulse30,
                pulse20,
                pulse10,
                sweep,
            ],
        }
    }
    /// Total size of all wavetable sample data.
    #[allow(dead_code)] // used by the benchmarks
    pub fn memory_bytes(&self) -> usize {
        self.wavetables.iter().map(WavetableFrames::memory_bytes).sum()
    }
}

/// The harmonics of one cycle of a waveform: `bins[k]` is the complex amplitude of harmonic
/// `k` (`bins[0]`, the DC offset, is ignored), in the convention where the waveform is the real
/// part of `sum(bins[k] * e^(i * k * theta))`. A real cycle `x` of `N` samples with forward FFT
/// `X` has `bins[k] = 2 * X[k] / N`.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    pub bins: Vec<Complex32>,
}

impl Spectrum {
    /// The spectrum of one cycle of a waveform, with the DC offset dropped. `fft` must be a
    /// forward FFT plan of the cycle's length.
    pub fn from_cycle(cycle: &[f32], fft: &dyn Fft<f32>) -> Self {
        assert_eq!(cycle.len(), fft.len());
        let mut buffer: Vec<Complex32> = cycle.iter().map(|&x| Complex32::new(x, 0.0)).collect();
        fft.process(&mut buffer);
        let scale = 2.0 / cycle.len() as f32;
        // Harmonics up to, but not including, the Nyquist bin
        let mut bins: Vec<Complex32> = buffer
            .iter()
            .take(cycle.len().div_ceil(2))
            .map(|bin| bin * scale)
            .collect();
        if let Some(dc) = bins.first_mut() {
            *dc = Complex32::new(0.0, 0.0);
        }
        Self { bins }
    }
    /// The highest harmonic.
    pub fn harmonics(&self) -> usize {
        self.bins.len().saturating_sub(1)
    }
    /// Renders one cycle of the first `harmonics` harmonics into `output` using an inverse FFT.
    /// `fft` must be an inverse FFT plan with the same length as `output`, which must be at least
    /// twice `harmonics` to keep the result bandlimited.
    pub fn render_ifft(&self, output: &mut [f32], harmonics: usize, fft: &dyn Fft<f32>) {
        assert_eq!(output.len(), fft.len());
        assert!(harmonics <= output.len() / 2);

        // Bin 0 is the DC offset, which is always zero.
        let mut buffer = vec![Complex32::new(0.0, 0.0); output.len()];
        for (bin, value) in buffer.iter_mut().zip(self.bins.iter()).skip(1).take(harmonics) {
            *bin = *value;
        }
        fft.process(&mut buffer);

        for (sample, output) in izip!(buffer.iter(), output.iter_mut()) {
            *output = sample.re;
        }
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
    /// The partials as a [`Spectrum`], for rendering with an inverse FFT.
    pub fn spectrum(&self) -> Spectrum {
        let mut bins = Vec::with_capacity(N + 1);
        bins.push(Complex32::new(0.0, 0.0));
        for (amplitude, phase) in self.amplitudes.iter().zip(self.phases.iter()) {
            // `i * amplitude * e^(i * phase)`, which the inverse FFT turns into
            // `-amplitude * sin(n * theta + phase)`.
            let (sin, cos) = phase.sin_cos();
            bins.push(Complex32::new(-amplitude * sin, amplitude * cos));
        }
        Spectrum { bins }
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
    /// A pulse wave that is high for `duty` (between 0 and 1) of its cycle, scaled so that a duty
    /// of 0.5 is identical to [`square()`][Self::square()].
    ///
    /// Built as the difference of two saw waves offset by the duty, which gives the pulse no DC
    /// offset at any width.
    pub fn pulse(duty: f32) -> Self {
        let mut amplitudes = [0.0; N];
        let mut phases = [0.0; N];
        let offset = 2.0 * PI * duty;
        for (i, (amplitude, phase)) in amplitudes.iter_mut().zip(phases.iter_mut()).enumerate() {
            let n = (i + 1) as f32;
            // The saw's partials are `1 / n`, and delaying it by `offset` multiplies partial `n`
            // by `e^(i * n * offset)`.
            let coefficient =
                (Complex32::new(1.0, 0.0) - Complex32::from_polar(1.0, n * offset)) / (2.0 * n);
            *amplitude = coefficient.norm();
            *phase = coefficient.arg();
        }
        Self { amplitudes, phases }
    }
}

#[cfg(test)]
mod tests {
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

    /// Recovers the pan position (-1 to 1) of a voice from its gains at a stereo width of 1.
    fn pan_from_gains((left, right): (f32, f32)) -> f32 {
        right.atan2(left) / FRAC_PI_4 - 1.0
    }

    #[test]
    fn unison_pan_is_interleaved_and_symmetric() {
        // A single voice stays in the centre
        let mut osc = OscillatorParams {
            voice_count: 1,
            ..Default::default()
        };
        osc.update_pan_gains(1.0);
        assert!((osc.pan_gain(0).0 - 1.0).abs() < 1e-5);
        assert!((osc.pan_gain(0).1 - 1.0).abs() < 1e-5);
        // Unused voices have no gain
        assert_eq!(osc.pan_gain(1), (0.0, 0.0));

        // Detune indices per voice: 0, -1, 2, -3, 4, -5, 6, -7
        osc.voice_count = 8;
        osc.update_pan_gains(1.0);
        let pans: Vec<f32> = (0..8).map(|i| pan_from_gains(osc.pan_gain(i))).collect();
        // Lowest voice hard left, highest hard right
        assert!((pans[7] + 1.0).abs() < 1e-5, "{pans:?}");
        assert!((pans[6] - 1.0).abs() < 1e-5, "{pans:?}");
        // Second lowest on the right, second highest on the left
        assert!(pans[5] > 0.5, "{pans:?}");
        assert!(pans[4] < -0.5, "{pans:?}");
        // The field is symmetric
        let mut sorted = pans.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for i in 0..4 {
            assert!((sorted[i] + sorted[7 - i]).abs() < 1e-5, "{sorted:?}");
        }

        // With three voices the centre voice stays centred and the others go to opposite sides
        osc.voice_count = 3;
        osc.update_pan_gains(1.0);
        let pans: Vec<f32> = (0..3).map(|i| pan_from_gains(osc.pan_gain(i))).collect();
        assert!(pans[0].abs() < 1e-5, "{pans:?}");
        assert!((pans[1] + 1.0).abs() < 1e-5, "{pans:?}");
        assert!((pans[2] - 1.0).abs() < 1e-5, "{pans:?}");
    }

    #[test]
    fn half_duty_pulse_is_a_square_wave() {
        let square = AdditiveOsc::<64>::square();
        let pulse = AdditiveOsc::<64>::pulse(0.5);
        for (n, (square, pulse)) in square.amplitudes.iter().zip(pulse.amplitudes.iter()).enumerate() {
            assert!((square - pulse).abs() < 1e-5, "harmonic {}", n + 1);
        }
        for phase in pulse.phases.iter().zip(pulse.amplitudes.iter()).filter(|(_, a)| **a > 1e-5).map(|(p, _)| p) {
            assert!(phase.abs() < 1e-4, "{phase}");
        }
    }

    #[test]
    fn wavetable_reads_interpolate_across_the_wrap() {
        let len = 64;
        let cycle: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * i as f32 / len as f32).sin())
            .collect();
        let table = Wavetable::new(cycle.clone());
        assert_eq!(table.cycle_len(), len);
        assert_eq!(table.cycle(), &cycle[..]);
        // Samples are reproduced exactly, and in between them the cubic interpolation is close
        for (i, sample) in cycle.iter().enumerate() {
            assert_eq!(table.read(i as f32), *sample);
            let between = table.read(i as f32 + 0.5);
            let expected = (2.0 * PI * (i as f32 + 0.5) / len as f32).sin();
            assert!((between - expected).abs() < 1e-3, "{i}: {between} vs {expected}");
        }
        // Reads across the end of the cycle see the start of the next one
        assert!((table.read(len as f32 - 0.5) - (-2.0 * PI * 0.5 / len as f32).sin()).abs() < 1e-3);
        assert!((table.generate(0.25) - 1.0).abs() < 1e-3);
        assert!((table.generate(-0.25) + 1.0).abs() < 1e-3);
        // Out of range reads are clamped instead of reading out of bounds
        assert!(table.read(len as f32).is_finite());
        assert!(table.read(1e9).is_finite());
        assert!(table.read(-1.0).is_finite());
    }

    /// Reads the frames at a fixed point phase with a runtime selected interpolation mode.
    fn read_fixed_mode(frames: &FramePair, phase: u32, mode: Interpolation) -> f32 {
        match mode {
            Interpolation::None => frames.read_fixed::<NONE>(phase),
            Interpolation::Linear => frames.read_fixed::<LINEAR>(phase),
            Interpolation::Cubic => frames.read_fixed::<CUBIC>(phase),
        }
    }

    const MODES: [Interpolation; 3] = [Interpolation::None, Interpolation::Linear, Interpolation::Cubic];

    #[test]
    fn interpolation_modes_read_as_expected() {
        let len = 64;
        let cycle: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * i as f32 / len as f32).sin())
            .collect();
        let table = Wavetable::new(cycle.clone());
        assert_eq!(Interpolation::default(), Interpolation::Cubic);
        assert_eq!(Interpolation::Cubic as u8, CUBIC);
        assert_eq!(table.index_shift(), 26);
        assert_eq!(table.frac_mask(), (1 << 26) - 1);
        for (i, sample) in cycle.iter().enumerate() {
            let next = cycle[(i + 1) % len];
            let at = |offset: f32| cycles_to_fixed((i as f32 + offset) / len as f32);
            // Every mode reproduces the samples themselves, by index or by phase
            assert_eq!(table.read_with::<NONE>(i as f32), *sample);
            assert_eq!(table.read_with::<LINEAR>(i as f32), *sample);
            assert_eq!(table.read_with::<CUBIC>(i as f32), *sample);
            assert_eq!(table.read_fixed::<NONE>(at(0.0)), *sample);
            assert_eq!(table.read_fixed::<LINEAR>(at(0.0)), *sample);
            assert_eq!(table.read_fixed::<CUBIC>(at(0.0)), *sample);
            // No interpolation holds the sample below the read position
            assert_eq!(table.read_with::<NONE>(i as f32 + 0.75), *sample);
            assert_eq!(table.read_fixed::<NONE>(at(0.75)), *sample);
            // Linear interpolation is the weighted average of the two neighbours
            let want = 0.75 * sample + 0.25 * next;
            let linear = table.read_with::<LINEAR>(i as f32 + 0.25);
            assert!((linear - want).abs() < 1e-6, "{i}: {linear}");
            let linear = table.read_fixed::<LINEAR>(at(0.25));
            assert!((linear - want).abs() < 1e-6, "{i}: {linear}");
            // Cubic interpolation is closer to the true sine than linear
            let want = (2.0 * PI * (i as f32 + 0.5) / len as f32).sin();
            let cubic = table.read_with::<CUBIC>(i as f32 + 0.5);
            let linear = table.read_with::<LINEAR>(i as f32 + 0.5);
            assert!((cubic - want).abs() <= (linear - want).abs() + 1e-7, "{i}: {cubic} vs {linear}");
            assert_eq!(table.read_fixed::<CUBIC>(at(0.5)), cubic);
        }
        // Out of range indices are clamped in every mode
        for index in [len as f32, 1e9, -1.0] {
            assert!(table.read_with::<NONE>(index).is_finite());
            assert!(table.read_with::<LINEAR>(index).is_finite());
            assert!(table.read_with::<CUBIC>(index).is_finite());
        }
        // Phases always land inside the cycle, right up to the last fixed point step
        assert_eq!(table.split_phase(u32::MAX).0, len - 1);
        assert!(table.read_fixed::<CUBIC>(u32::MAX).is_finite());
    }

    #[test]
    fn fixed_point_phases_wrap_and_convert_exactly() {
        assert_eq!(cycles_to_fixed(0.0), 0);
        assert_eq!(cycles_to_fixed(0.5), 1 << 31);
        assert_eq!(cycles_to_fixed(0.25), 1 << 30);
        assert_eq!(cycles_to_fixed(1.25), 1 << 30);
        assert_eq!(cycles_to_fixed(-0.75), 1 << 30);
        assert_eq!(cycles_to_fixed(-3.75), 1 << 30);
        assert_eq!(cycles_to_fixed(1.0), 0);
        assert!((fixed_to_cycles(cycles_to_fixed(0.3)) - 0.3).abs() < 1e-7);
        assert!((wrap_cycles_signed(-0.3) - 0.7).abs() < 1e-6);
        assert!((wrap_cycles_signed(2.3) - 0.3).abs() < 1e-6);
        // An upwards delta wraps the phase around, a downwards one wraps it back
        let phase = cycles_to_fixed(0.9).wrapping_add(cycles_to_fixed(0.2));
        assert!((fixed_to_cycles(phase) - 0.1).abs() < 1e-7);
        let phase = cycles_to_fixed(0.1).wrapping_add(cycles_to_fixed(-0.2));
        assert!((fixed_to_cycles(phase) - 0.9).abs() < 1e-7);

        // The 8 lane conversions match the scalar one exactly
        let values = [0.0_f32, 0.0123, 0.5, 0.999_999_9, 1.25, -0.3, -7.125, 123.456];
        let want: Vec<u32> = values.iter().map(|v| cycles_to_fixed(*v)).collect();
        assert_eq!(lookup::portable::cycles_to_fixed8(f32x8::new(values)).to_array().to_vec(), want);
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            use std::arch::x86_64::*;
            let mut got = [0_u32; 8];
            // SAFETY: AVX2 and FMA were just detected.
            unsafe {
                let fixed = lookup::avx2::cycles_to_fixed8(_mm256_loadu_ps(values.as_ptr()));
                _mm256_storeu_si256(got.as_mut_ptr() as *mut __m256i, fixed);
            }
            assert_eq!(got.to_vec(), want);
        }
    }

    /// A one voice at a time version of [`FramePair::render_block()`], for checking the
    /// kernels against. The deltas ramp in fixed point like the kernels' do, so the phases
    /// have to match exactly.
    fn render_block_reference(
        frames: &FramePair,
        voice: &mut SuperVoice,
        params: &BlockParams,
        output: &mut BlockOutput,
        mode: Interpolation,
    ) {
        let (mut delta, mut gain) = (params.delta, params.gain);
        let factor = |i: usize| lane(params.detune_factors, i);
        let mut deltas: Vec<u32> = (0..params.voices)
            .map(|i| cycles_to_fixed(params.delta * factor(i)))
            .collect();
        let delta_steps: Vec<u32> = (0..params.voices)
            .map(|i| cycles_to_fixed(params.delta_step * factor(i)))
            .collect();
        for n in 0..output.left.len() {
            let (mut fm_delta, mut offset, mut sample_gain) = (None, 0_u32, gain);
            if let Some(modulation) = &params.modulation {
                let input = modulation.input[n];
                if modulation.fm > 0.0 {
                    fm_delta = Some(wrap_cycles_signed(delta * (1.0 + input * modulation.fm)));
                }
                if modulation.pm > 0.0 {
                    offset = cycles_to_fixed(input * modulation.pm);
                }
                if modulation.am > 0.0 {
                    sample_gain *= lerp(1.0, (input + 1.0) / 2.0, modulation.am);
                }
            }
            let (mut mono, mut left, mut right) = (0.0, 0.0, 0.0);
            for i in 0..params.voices {
                match fm_delta {
                    Some(fm_delta) => deltas[i] = cycles_to_fixed(fm_delta * factor(i)),
                    None => deltas[i] = deltas[i].wrapping_add(delta_steps[i]),
                }
                let phase = lane_u32_mut(&mut voice.voice_phases, i);
                // The step is added after this sample's delta is used
                let step = if fm_delta.is_none() { delta_steps[i] } else { 0 };
                *phase = phase.wrapping_add(deltas[i].wrapping_sub(step));
                let value = read_fixed_mode(frames, phase.wrapping_add(offset), mode);
                mono += value * lane(params.mask, i);
                left += value * lane(params.pan_left, i);
                right += value * lane(params.pan_right, i);
            }
            if let Some(right_out) = output.right.as_deref_mut() {
                output.left[n] += left * sample_gain;
                right_out[n] += right * sample_gain;
            } else {
                output.left[n] += mono * sample_gain;
            }
            if let Some((mono_out, scale)) = &mut output.mono {
                mono_out[n] = mono * *scale;
            }
            delta += params.delta_step;
            gain += params.gain_step;
        }
    }

    /// The outputs of rendering a few blocks: left, right, mono, and the final phases.
    type Rendered = (Vec<f32>, Vec<f32>, Vec<f32>, SuperVoice);

    #[test]
    fn block_kernels_match_the_reference() {
        let len = 256;
        let sine = Wavetable::new(
            (0..len)
                .map(|i| (2.0 * PI * i as f32 / len as f32).sin())
                .collect(),
        );
        let saw = Wavetable::new((0..len).map(|i| i as f32 / len as f32 * 2.0 - 1.0).collect());
        let rng = fastrand::Rng::with_seed(42);
        let modulator: Vec<f32> = (0..80).map(|_| rng.f32() * 2.0 - 1.0).collect();
        // Blocks of different lengths, to check that a short block works too
        let blocks = [(0, 32), (32, 49), (49, 80)];

        let mut levels = vec![SimdLevel::Portable];
        #[cfg(target_arch = "x86_64")]
        if SimdLevel::detect() != SimdLevel::Portable {
            levels.push(SimdLevel::Avx2);
            levels.push(SimdLevel::Avx2Gather);
        }

        for mode in MODES {
            for mix in [0.0, 0.3] {
                let frames = FramePair {
                    a: &sine,
                    b: &saw,
                    mix,
                };
                for voices in [1, 3, 8, 13, 32] {
                    let mut osc = OscillatorParams {
                        voice_count: voices as u8,
                        voices_detune: 0.3,
                        ..Default::default()
                    };
                    osc.update_detune_factors();
                    osc.update_pan_gains(0.8);
                    let start = SuperVoice::new(0.1, 0.7);

                    for (stereo, modulated, with_mono) in
                        [(false, false, false), (true, false, true), (false, true, true), (true, true, false)]
                    {
                        let render = |simd: Option<SimdLevel>| -> Rendered {
                            let mut voice = start.clone();
                            // The outputs start non-zero to check that the kernels add to them
                            let mut left = vec![0.1; 80];
                            let mut right = vec![0.2; 80];
                            let mut mono = vec![0.0; 80];
                            for (begin, end) in blocks {
                                let params = BlockParams {
                                    delta: 0.013,
                                    delta_step: 0.0001,
                                    gain: 0.5,
                                    gain_step: 0.01,
                                    modulation: modulated.then(|| Modulation {
                                        input: &modulator[begin..end],
                                        fm: 0.4,
                                        pm: 0.3,
                                        am: 0.5,
                                    }),
                                    voices,
                                    detune_factors: &osc.detune_factors,
                                    mask: &osc.voice_mask,
                                    pan_left: &osc.pan_left,
                                    pan_right: &osc.pan_right,
                                };
                                let mut output = BlockOutput {
                                    left: &mut left[begin..end],
                                    right: if stereo { Some(&mut right[begin..end]) } else { None },
                                    mono: if with_mono { Some((&mut mono[begin..end], 0.75)) } else { None },
                                };
                                match simd {
                                    None => render_block_reference(&frames, &mut voice, &params, &mut output, mode),
                                    Some(simd) => frames.render_block(&mut voice, &params, &mut output, simd, mode),
                                }
                            }
                            (left, right, mono, voice)
                        };
                        let (left, right, mono, voice) = render(None);
                        assert!(left.iter().any(|sample| (sample - 0.1).abs() > 1e-3));
                        let tolerance = 1e-3 * voices as f32;
                        let mut kernel_phases: Option<Vec<u32>> = None;
                        for simd in &levels {
                            let (got_left, got_right, got_mono, got_voice) = render(Some(*simd));
                            let case = format!("{simd:?} {mode:?} mix {mix} voices {voices} stereo {stereo} modulated {modulated}");
                            for n in 0..80 {
                                assert!((got_left[n] - left[n]).abs() < tolerance, "{case}: left {n}: {} vs {}", got_left[n], left[n]);
                                assert!((got_right[n] - right[n]).abs() < tolerance, "{case}: right {n}: {} vs {}", got_right[n], right[n]);
                                assert!((got_mono[n] - mono[n]).abs() < tolerance, "{case}: mono {n}: {} vs {}", got_mono[n], mono[n]);
                            }
                            let phases: Vec<u32> = (0..voices).map(|i| lane_u32(&got_voice.voice_phases, i)).collect();
                            for i in 0..voices {
                                let want = lane_u32(&voice.voice_phases, i);
                                assert_eq!(phases[i], want, "{case}: voice {i}: {} vs {}", fixed_to_cycles(phases[i]), voice.phase_cycles(i));
                            }
                            // Unused voices are left alone
                            for i in voices..MAX_UNISON {
                                assert_eq!(lane_u32(&got_voice.voice_phases, i), lane_u32(&start.voice_phases, i), "{case}: voice {i}");
                            }
                            // The kernels agree with each other to the bit on the phases
                            match &kernel_phases {
                                Some(first) => assert_eq!(&phases, first, "{case}"),
                                None => kernel_phases = Some(phases),
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn simd_detection_picks_an_avx2_level_on_avx2_cpus() {
        let level = SimdLevel::detect();
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            assert!(level.has_avx(), "{level:?}");
            // Both variants have to work since either can be picked
            assert!(SimdLevel::fastest_avx2().has_avx());
        }
        if !level.has_avx() {
            assert_eq!(level, SimdLevel::Portable);
        }
    }

    #[test]
    fn pulse_wavetable_has_the_expected_duty_cycle() {
        let fft = FftPlanner::new().plan_fft_inverse(2048);
        let table = Wavetable::from_spectrum(&AdditiveOsc::<2048>::pulse(0.25).spectrum(), 2048, 1024, &*fft);
        let cycle = table.cycle();
        let high = cycle.iter().filter(|sample| **sample > 0.0).count();
        assert!((high as f32 / 2048.0 - 0.25).abs() < 0.01, "{high}");
        let mean: f32 = cycle.iter().sum::<f32>() / 2048.0;
        assert!(mean.abs() < 1e-3, "{mean}");
    }

    #[test]
    fn wavetable_frames_are_resolved_from_the_position() {
        let set = WavetableSet::new(44100.0, 64);
        let single = &set.wavetables[0];
        assert_eq!(single.frame_indices(0.7), (0, 0, 0.0));
        let sweep = set.wavetables.last().unwrap();
        assert_eq!(sweep.frames.len(), PW_SWEEP_FRAMES);
        assert_eq!(sweep.frame_indices(0.0), (0, 1, 0.0));
        let (a, b, mix) = sweep.frame_indices(0.5);
        assert_eq!((a, b), (15, 16));
        assert!((mix - 0.5).abs() < 1e-5);
        assert_eq!(sweep.frame_indices(1.0), (31, 31, 0.0));
    }

    /// A cycle's spectrum renders the cycle back (minus its DC offset), at any length that holds
    /// its harmonics.
    #[test]
    fn spectrum_round_trips_a_cycle() {
        let len = 600;
        let cycle: Vec<f32> = (0..len)
            .map(|i| {
                let phase = 2.0 * PI * i as f32 / len as f32;
                0.3 + 0.5 * phase.sin() + 0.25 * (3.0 * phase + 1.0).cos() + 0.1 * (40.0 * phase).sin()
            })
            .collect();
        let mut planner = FftPlanner::new();
        let spectrum = Spectrum::from_cycle(&cycle, &*planner.plan_fft_forward(len));
        assert_eq!(spectrum.harmonics(), 299);
        assert!((spectrum.bins[1].norm() - 0.5).abs() < 1e-4);
        assert!((spectrum.bins[3].norm() - 0.25).abs() < 1e-4);
        assert!((spectrum.bins[40].norm() - 0.1).abs() < 1e-4);

        let mut rendered = vec![0.0; len];
        spectrum.render_ifft(&mut rendered, 299, &*planner.plan_fft_inverse(len));
        for (got, want) in rendered.iter().zip(&cycle) {
            assert!((got - (want - 0.3)).abs() < 1e-4, "{got} vs {want}");
        }
        // At a power of two length the same waveform comes out, resampled
        let table = Wavetable::from_spectrum(&spectrum, 1024, 299, &*planner.plan_fft_inverse(1024));
        assert!((table.generate(0.25) - (0.5 + 0.25 * (3.0 * PI / 2.0 + 1.0).cos() + 0.1 * (10.0 * PI).sin())).abs() < 1e-3);
    }

    /// Tables are as long as their harmonics need, and levels group notes to fit a budget.
    #[test]
    fn table_lengths_and_levels_follow_the_harmonics() {
        assert_eq!(table_len_for(0, 2048), MIN_TABLE_LEN);
        assert_eq!(table_len_for(4, 2048), MIN_TABLE_LEN);
        assert_eq!(table_len_for(5, 2048), 128);
        assert_eq!(table_len_for(64, 2048), 1024);
        assert_eq!(table_len_for(1000, 2048), 2048);
        assert_eq!(table_len_for(1000, 8192), 8192);

        let per_note = NoteLevels::per_note();
        assert_eq!(per_note.count(), 138);
        assert_eq!(per_note.level(60), 60);
        assert_eq!(per_note.top_note(60), 60);
        let coarse = NoteLevels::new(12);
        assert_eq!(coarse.count(), 12);
        assert_eq!(coarse.level(0), 0);
        assert_eq!(coarse.level(11), 0);
        assert_eq!(coarse.level(12), 1);
        assert_eq!(coarse.level(137), 11);
        assert_eq!(coarse.top_note(0), 11);
        assert_eq!(coarse.top_note(11), 137);

        // A single frame fits per note, 256 frames of 1024 harmonics need coarser levels
        assert_eq!(NoteLevels::fitting(WAVETABLE_MEMORY_BUDGET, 1, 1024, 44100.0, 2048).step(), 1);
        let many = NoteLevels::fitting(WAVETABLE_MEMORY_BUDGET, 256, 1024, 44100.0, 2048);
        assert!(many.step() > 1);
        assert!(many.memory_bytes(256, 1024, 44100.0, 2048) <= WAVETABLE_MEMORY_BUDGET);
        // Higher notes have fewer harmonics, so the top tables are the smallest
        let set = WavetableSet::new(44100.0, 2048);
        let saw = &set.wavetables[2];
        assert_eq!(saw.levels, NoteLevels::per_note());
        assert_eq!(saw.table(0, 0).cycle_len(), 2048);
        assert_eq!(saw.table(0, 60).cycle_len(), 2048);
        assert!(saw.table(0, 96).cycle_len() < 2048);
        assert_eq!(saw.table(0, 137).cycle_len(), MIN_TABLE_LEN);
        // The set used to take 43 MB with every table 2048 samples long
        assert!(set.memory_bytes() < 30 << 20, "{}", set.memory_bytes());
    }
}
