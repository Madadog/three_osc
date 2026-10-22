use std::{
    convert::TryInto,
    f32::consts::{FRAC_1_SQRT_2, FRAC_PI_4, PI, SQRT_2},
};

use itertools::izip;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use wide::{f32x8, i32x8};

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
    /// Like `Avx2`, but the taps are read with gather instructions. Gathers are microcoded and
    /// slow on many Intel CPUs (since the 2023 "Downfall" mitigation), so this is kept for
    /// benchmarking rather than used by default.
    #[cfg(target_arch = "x86_64")]
    Avx2Gather,
}

impl SimdLevel {
    /// Whether this level implies AVX support (used by the ladder filter).
    pub fn has_avx(self) -> bool {
        !matches!(self, SimdLevel::Portable)
    }
    /// The best level supported by this CPU.
    pub fn detect() -> Self {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return SimdLevel::Avx2;
        }
        SimdLevel::Portable
    }
}

/// Up to this many unison voices are read one at a time instead of in SIMD vectors, since
/// filling a vector for one or two voices costs more than it saves.
const SCALAR_VOICE_LIMIT: usize = 3;

/// Maximum number of unison voices per oscillator.
pub const MAX_UNISON: usize = 32;
/// Unison voices are processed in SIMD vectors of this many lanes.
pub const UNISON_LANES: usize = 8;
/// Number of SIMD vectors needed for all unison voices.
pub const UNISON_VECTORS: usize = MAX_UNISON / UNISON_LANES;

/// The largest value below 1.0: a wrapped phase is clamped to this so that the scaled read index
/// stays below the table length.
const BELOW_ONE: f32 = 0.999_999_94;

/// Wraps a non-negative phase in cycles into `[0, 1)`.
#[inline]
pub fn wrap_cycles(phase: f32) -> f32 {
    phase - (phase as i32) as f32
}

/// Wraps a phase (or phase delta) in cycles of either sign into `[0, 1)`.
#[inline]
pub fn wrap_cycles_signed(phase: f32) -> f32 {
    let wrapped = wrap_cycles(phase);
    if wrapped < 0.0 {
        wrapped + 1.0
    } else {
        wrapped
    }
}

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
/// Implemented as a buffer of independent phases (in cycles, kept in `[0, 1)`) with a common
/// base frequency (delta) which differs for each phase according to the oscillator's detune
/// factors. The phases are stored as SIMD vectors so all voices advance at once.
pub struct SuperVoice {
    pub voice_phases: [f32x8; UNISON_VECTORS],
}
impl SuperVoice {
    pub fn new(phase: f32, phase_random: f32) -> Self {
        let rng = fastrand::Rng::new();
        let mut phases = [0.0_f32; MAX_UNISON];
        for phase_out in phases.iter_mut() {
            *phase_out = wrap_cycles(phase + rng.f32() * phase_random);
        }
        Self {
            voice_phases: std::array::from_fn(|vector| {
                let lanes = &phases[vector * UNISON_LANES..(vector + 1) * UNISON_LANES];
                f32x8::new(lanes.try_into().unwrap())
            }),
        }
    }
    /// Advances one voice by `delta` (cycles per sample) times its detune factor and returns its
    /// wavetable read position, see [`advance()`][Self::advance()].
    #[inline]
    pub fn advance_one(
        &mut self,
        voice: usize,
        delta: f32,
        detune_factor: f32,
        phase_offset: f32,
        table_len: f32,
    ) -> f32 {
        let phase = &mut self.voice_phases[voice / UNISON_LANES].as_mut_array()[voice % UNISON_LANES];
        *phase = wrap_cycles(*phase + delta * detune_factor);
        wrap_cycles_signed(*phase + phase_offset).min(BELOW_ONE) * table_len
    }
    /// Advances the first `voice_count` voices by `delta` (cycles per sample) times each voice's
    /// detune factor, then writes each voice's wavetable read position into `indices`: the phase
    /// offset by `phase_offset` cycles (for phase modulation), scaled to `table_len` samples.
    ///
    /// `delta` must be non-negative; the detune factors are positive, so the phases stay in
    /// `[0, 1)` with a cheap truncating wrap.
    #[inline]
    pub fn advance(
        &mut self,
        delta: f32,
        voice_count: usize,
        detune_factors: &[f32x8; UNISON_VECTORS],
        phase_offset: f32,
        table_len: f32,
        indices: &mut [f32x8; UNISON_VECTORS],
    ) {
        let delta = f32x8::splat(delta);
        let phase_offset = f32x8::splat(phase_offset);
        let table_len = f32x8::splat(table_len);
        let zero = f32x8::splat(0.0);
        let one = f32x8::splat(1.0);
        let below_one = f32x8::splat(BELOW_ONE);

        let vectors = voice_count.div_ceil(UNISON_LANES).min(UNISON_VECTORS);
        for vector in 0..vectors {
            let mut phase = self.voice_phases[vector] + delta * detune_factors[vector];
            phase -= phase.trunc_int().round_float();
            self.voice_phases[vector] = phase;

            // The offset can be negative, and the wrapped result can round up to exactly 1.0
            let mut read = phase + phase_offset;
            read -= read.trunc_int().round_float();
            read += read.simd_lt(zero).select(one, zero);
            indices[vector] = read.min(below_one) * table_len;
        }
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
    /// Position within the wavetable, from 0 to 1. Morphs between the frames of wavetables that
    /// have more than one.
    pub position: f32,
    /// The frames to crossfade between for the current position, see `update_frames()`.
    frame_a: usize,
    frame_b: usize,
    frame_mix: f32,

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
        let mut factors = [1.0_f32; MAX_UNISON];
        let mut mask = [0.0_f32; MAX_UNISON];
        for i in 0..voice_count {
            factors[i] = 1.0 + unison_detune_index(i) as f32 * self.voices_detune / voice_count as f32;
            mask[i] = 1.0;
        }
        self.detune_factors = to_vectors(&factors);
        self.voice_mask = to_vectors(&mask);
    }
    /// The `(left, right)` gain of a unison voice.
    pub fn pan_gain(&self, voice: usize) -> (f32, f32) {
        (
            self.pan_left[voice / UNISON_LANES].as_array()[voice % UNISON_LANES],
            self.pan_right[voice / UNISON_LANES].as_array()[voice % UNISON_LANES],
        )
    }
    /// Resolves the wavetable position to the two frames to crossfade between. Must be called
    /// before `frame_pair()` whenever the wavetable or position changed.
    pub fn update_frames(&mut self, set: &WavetableSet) {
        self.wavetable = self.wavetable.min(set.wavetables.len() - 1);
        (self.frame_a, self.frame_b, self.frame_mix) =
            set.wavetables[self.wavetable].frame_indices(self.position);
    }
    /// The wavetable frames to generate a voice from, bandlimited for the given MIDI note (see
    /// [`WavetableNotes::cycles_to_note()`]).
    #[inline]
    pub fn frame_pair<'a>(&self, set: &'a WavetableSet, note: usize) -> FramePair<'a> {
        let frames = &set.wavetables[self.wavetable].frames;
        FramePair {
            a: &frames[self.frame_a].tables[note],
            b: &frames[self.frame_b].tables[note],
            mix: self.frame_mix,
        }
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
            voices_detune: 0.1,
            unison_amp: 1.0,
            pan_left: [f32x8::splat(1.0); UNISON_VECTORS],
            pan_right: [f32x8::splat(1.0); UNISON_VECTORS],
            voice_mask: [f32x8::splat(1.0); UNISON_VECTORS],
            detune_factors: [f32x8::splat(1.0); UNISON_VECTORS],
            wavetable: 0,
            position: 0.0,
            frame_a: 0,
            frame_b: 0,
            frame_mix: 0.0,
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
/// [`TABLE_PADDING_BEFORE`]).
pub struct Wavetable {
    pub table: Vec<f32>,
}

impl Wavetable {
    pub fn new(cycle: Vec<f32>) -> Self {
        assert!(!cycle.is_empty(), "a wavetable needs at least one sample");
        let len = cycle.len();
        let mut table = Vec::with_capacity(len + TABLE_PADDING_BEFORE + TABLE_PADDING_AFTER);
        table.push(cycle[len - 1]);
        table.extend_from_slice(&cycle);
        table.push(cycle[0]);
        table.push(cycle[1 % len]);
        Self { table }
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
    /// Reads the table at a fractional sample index in `[0, cycle_len)` with cubic
    /// interpolation. Out of range indices are clamped.
    #[inline]
    pub fn read(&self, index: f32) -> f32 {
        let floor = (index as usize).min(self.cycle_len() - 1);
        let frac = index - floor as f32;
        let taps = &self.table[floor..floor + 4];
        hermite(taps[0], taps[1], taps[2], taps[3], frac)
    }
    /// Generates a sample at the given phase in cycles.
    #[inline]
    pub fn generate(&self, phase: f32) -> f32 {
        self.read(wrap_cycles_signed(phase) * self.cycle_len() as f32)
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
        Self::new(table)
    }
    pub fn from_additive_osc_ifft(
        osc: &AdditiveOsc,
        len: usize,
        harmonics: usize,
        fft: &dyn Fft<f32>,
    ) -> Self {
        let mut table: Vec<f32> = vec![0.0; len];
        osc.generate_ifft(&mut table, harmonics, fft);
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
/// second lookup.
#[derive(Debug, Clone, Copy)]
pub struct FramePair<'a> {
    pub a: &'a Wavetable,
    pub b: &'a Wavetable,
    pub mix: f32,
}

impl FramePair<'_> {
    /// Samples per cycle.
    #[inline]
    pub fn cycle_len(&self) -> usize {
        self.a.cycle_len()
    }
    /// Reads the frames at a fractional sample index in `[0, cycle_len)`.
    #[inline]
    pub fn read(&self, index: f32) -> f32 {
        let a = self.a.read(index);
        if self.mix > 0.0 {
            lerp(a, self.b.read(index), self.mix)
        } else {
            a
        }
    }
    /// Advances the first `voices` unison voices (see [`SuperVoice::advance()`]) and sums their
    /// samples, skipping the voices masked out by `mask`.
    #[inline]
    pub fn advance_and_sum(
        &self,
        voice: &mut SuperVoice,
        delta: f32,
        detune_factors: &[f32x8; UNISON_VECTORS],
        phase_offset: f32,
        voices: usize,
        mask: &[f32x8; UNISON_VECTORS],
        simd: SimdLevel,
    ) -> f32 {
        let table_len = self.cycle_len() as f32;
        if voices <= SCALAR_VOICE_LIMIT {
            let mut total = 0.0;
            for i in 0..voices {
                let index = voice.advance_one(i, delta, lane(detune_factors, i), phase_offset, table_len);
                total += self.read(index) * lane(mask, i);
            }
            return total;
        }
        match simd {
            SimdLevel::Portable => {
                let mut indices = [f32x8::splat(0.0); UNISON_VECTORS];
                voice.advance(delta, voices, detune_factors, phase_offset, table_len, &mut indices);
                lookup::portable::sum(self, &indices, voices.div_ceil(UNISON_LANES), mask)
            }
            // SAFETY: the AVX2 levels are only produced by `SimdLevel::detect()` when the CPU
            // supports AVX2 and FMA.
            #[cfg(target_arch = "x86_64")]
            SimdLevel::Avx2 => unsafe {
                lookup::avx2::advance_and_sum::<false>(
                    self, voice, delta, detune_factors, phase_offset, voices, mask,
                )
            },
            #[cfg(target_arch = "x86_64")]
            SimdLevel::Avx2Gather => unsafe {
                lookup::avx2::advance_and_sum::<true>(
                    self, voice, delta, detune_factors, phase_offset, voices, mask,
                )
            },
        }
    }
    /// Like [`advance_and_sum()`][Self::advance_and_sum()], but also pans the voices into a
    /// left and a right channel using the per voice gains (which are zero for unused voices).
    /// Returns `(mono, left, right)`, where `mono` is the plain sum of the voices.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn advance_and_sum_panned(
        &self,
        voice: &mut SuperVoice,
        delta: f32,
        detune_factors: &[f32x8; UNISON_VECTORS],
        phase_offset: f32,
        voices: usize,
        mask: &[f32x8; UNISON_VECTORS],
        pan_left: &[f32x8; UNISON_VECTORS],
        pan_right: &[f32x8; UNISON_VECTORS],
        simd: SimdLevel,
    ) -> (f32, f32, f32) {
        let table_len = self.cycle_len() as f32;
        if voices <= SCALAR_VOICE_LIMIT {
            let (mut mono, mut left, mut right) = (0.0, 0.0, 0.0);
            for i in 0..voices {
                let index = voice.advance_one(i, delta, lane(detune_factors, i), phase_offset, table_len);
                let sample = self.read(index);
                mono += sample * lane(mask, i);
                left += sample * lane(pan_left, i);
                right += sample * lane(pan_right, i);
            }
            return (mono, left, right);
        }
        match simd {
            SimdLevel::Portable => {
                let mut indices = [f32x8::splat(0.0); UNISON_VECTORS];
                voice.advance(delta, voices, detune_factors, phase_offset, table_len, &mut indices);
                lookup::portable::sum_panned(
                    self,
                    &indices,
                    voices.div_ceil(UNISON_LANES),
                    mask,
                    pan_left,
                    pan_right,
                )
            }
            // SAFETY: see `advance_and_sum()`
            #[cfg(target_arch = "x86_64")]
            SimdLevel::Avx2 => unsafe {
                lookup::avx2::advance_and_sum_panned::<false>(
                    self, voice, delta, detune_factors, phase_offset, voices, mask, pan_left, pan_right,
                )
            },
            #[cfg(target_arch = "x86_64")]
            SimdLevel::Avx2Gather => unsafe {
                lookup::avx2::advance_and_sum_panned::<true>(
                    self, voice, delta, detune_factors, phase_offset, voices, mask, pan_left, pan_right,
                )
            },
        }
    }
}

/// One voice's value from the per voice vectors.
#[inline]
fn lane(vectors: &[f32x8; UNISON_VECTORS], voice: usize) -> f32 {
    vectors[voice / UNISON_LANES].as_array()[voice % UNISON_LANES]
}

/// The hot loop: reading and interpolating all unison voices from the wavetable frames.
mod lookup {
    use super::*;

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

        /// Reads 8 voices from a table at the given fractional indices.
        #[inline]
        fn read8(table: &[f32], max_index: i32x8, indices: f32x8) -> f32x8 {
            let floor = indices.trunc_int().min(max_index).max(i32x8::splat(0));
            let frac = indices - floor.round_float();
            let floor = floor.to_array();
            let mut taps = [[0.0_f32; UNISON_LANES]; 4];
            for (lane, &floor) in floor.iter().enumerate() {
                let floor = floor as usize;
                // Clamped above, so the 4 taps are always inside the padded table
                let samples = &table[floor..floor + 4];
                taps[0][lane] = samples[0];
                taps[1][lane] = samples[1];
                taps[2][lane] = samples[2];
                taps[3][lane] = samples[3];
            }
            hermite8(
                f32x8::new(taps[0]),
                f32x8::new(taps[1]),
                f32x8::new(taps[2]),
                f32x8::new(taps[3]),
                frac,
            )
        }

        /// Reads 8 voices from the frame pair.
        #[inline]
        fn read_frames8(frames: &FramePair, max_index: i32x8, indices: f32x8) -> f32x8 {
            let a = read8(&frames.a.table, max_index, indices);
            if frames.mix > 0.0 {
                let b = read8(&frames.b.table, max_index, indices);
                a + (b - a) * f32x8::splat(frames.mix)
            } else {
                a
            }
        }

        #[inline]
        pub fn sum(
            frames: &FramePair,
            indices: &[f32x8; UNISON_VECTORS],
            vectors: usize,
            mask: &[f32x8; UNISON_VECTORS],
        ) -> f32 {
            let max_index = i32x8::splat(frames.cycle_len() as i32 - 1);
            let mut total = f32x8::splat(0.0);
            for vector in 0..vectors.min(UNISON_VECTORS) {
                total += read_frames8(frames, max_index, indices[vector]) * mask[vector];
            }
            horizontal_sum(total)
        }

        #[inline]
        pub fn sum_panned(
            frames: &FramePair,
            indices: &[f32x8; UNISON_VECTORS],
            vectors: usize,
            mask: &[f32x8; UNISON_VECTORS],
            pan_left: &[f32x8; UNISON_VECTORS],
            pan_right: &[f32x8; UNISON_VECTORS],
        ) -> (f32, f32, f32) {
            let max_index = i32x8::splat(frames.cycle_len() as i32 - 1);
            let mut mono = f32x8::splat(0.0);
            let mut left = f32x8::splat(0.0);
            let mut right = f32x8::splat(0.0);
            for vector in 0..vectors.min(UNISON_VECTORS) {
                let samples = read_frames8(frames, max_index, indices[vector]);
                mono += samples * mask[vector];
                left += samples * pan_left[vector];
                right += samples * pan_right[vector];
            }
            (
                horizontal_sum(mono),
                horizontal_sum(left),
                horizontal_sum(right),
            )
        }
    }

    /// The same lookups using AVX2 gathers and FMA. Every function here must only be called
    /// after checking that the CPU supports both.
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
        unsafe fn store(vector: &mut f32x8, value: __m256) {
            _mm256_storeu_ps(vector.as_mut_array().as_mut_ptr(), value)
        }

        /// Wraps non-negative phases into `[0, 1)`.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn wrap(phase: __m256) -> __m256 {
            _mm256_sub_ps(phase, _mm256_cvtepi32_ps(_mm256_cvttps_epi32(phase)))
        }

        /// Advances one vector of voices, see [`SuperVoice::advance()`], returning their read
        /// indices.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn advance(
            phases: &mut f32x8,
            delta: __m256,
            detune_factors: &f32x8,
            phase_offset: __m256,
            table_len: __m256,
        ) -> __m256 {
            let phase = wrap(_mm256_fmadd_ps(delta, load(detune_factors), load(phases)));
            store(phases, phase);

            // The offset can be negative, and the wrapped result can round up to exactly 1.0
            let read = wrap(_mm256_add_ps(phase, phase_offset));
            let negative = _mm256_cmp_ps::<_CMP_LT_OQ>(read, _mm256_setzero_ps());
            let read = _mm256_add_ps(read, _mm256_and_ps(negative, _mm256_set1_ps(1.0)));
            _mm256_mul_ps(_mm256_min_ps(read, _mm256_set1_ps(BELOW_ONE)), table_len)
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

        /// The 4 taps of 8 voices, each as a vector with one lane per voice.
        type Taps = (__m256, __m256, __m256, __m256);

        /// Reads the 4 taps of 8 voices with gather instructions.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn taps_gather(base: *const f32, floor: __m256i) -> Taps {
            (
                _mm256_i32gather_ps::<4>(base, floor),
                _mm256_i32gather_ps::<4>(base.add(1), floor),
                _mm256_i32gather_ps::<4>(base.add(2), floor),
                _mm256_i32gather_ps::<4>(base.add(3), floor),
            )
        }

        /// Reads the 4 taps of 8 voices with one 128-bit load per voice, then transposes the
        /// 8 rows of 4 taps into 4 vectors of 8 voices.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn taps_transpose(base: *const f32, floor: __m256i) -> Taps {
            let mut offsets = [0_i32; UNISON_LANES];
            _mm256_storeu_si256(offsets.as_mut_ptr() as *mut __m256i, floor);
            let row = |voice: usize| _mm_loadu_ps(base.add(offsets[voice] as usize));
            // Pair up rows so that each 128-bit half holds one row: (r0|r4), (r1|r5), ...
            let pair = |low: usize| _mm256_insertf128_ps::<1>(_mm256_castps128_ps256(row(low)), row(low + 4));
            let (a, b, c, d) = (pair(0), pair(1), pair(2), pair(3));
            // Interleave: t0 = r0[0] r1[0] r0[1] r1[1] | r4[0] r5[0] r4[1] r5[1], etc.
            let t0 = _mm256_unpacklo_ps(a, b);
            let t1 = _mm256_unpackhi_ps(a, b);
            let t2 = _mm256_unpacklo_ps(c, d);
            let t3 = _mm256_unpackhi_ps(c, d);
            (
                _mm256_shuffle_ps::<0x44>(t0, t2),
                _mm256_shuffle_ps::<0xEE>(t0, t2),
                _mm256_shuffle_ps::<0x44>(t1, t3),
                _mm256_shuffle_ps::<0xEE>(t1, t3),
            )
        }

        /// Reads 8 voices from a table at the given fractional indices.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn read8<const GATHER: bool>(
            table: &[f32],
            max_index: __m256i,
            indices: __m256,
        ) -> __m256 {
            let floor = _mm256_max_epi32(
                _mm256_min_epi32(_mm256_cvttps_epi32(indices), max_index),
                _mm256_setzero_si256(),
            );
            let frac = _mm256_sub_ps(indices, _mm256_cvtepi32_ps(floor));
            // SAFETY: the indices are clamped to the cycle, and the table is padded with the
            // neighbouring samples, so all 4 taps are inside it.
            let base = table.as_ptr();
            let (y0, y1, y2, y3) = if GATHER {
                taps_gather(base, floor)
            } else {
                taps_transpose(base, floor)
            };
            hermite8(y0, y1, y2, y3, frac)
        }

        /// Reads 8 voices from the frame pair.
        #[inline]
        #[target_feature(enable = "avx2,fma")]
        unsafe fn read_frames8<const GATHER: bool>(
            frames: &FramePair,
            max_index: __m256i,
            indices: __m256,
        ) -> __m256 {
            let a = read8::<GATHER>(&frames.a.table, max_index, indices);
            if frames.mix > 0.0 {
                let b = read8::<GATHER>(&frames.b.table, max_index, indices);
                _mm256_fmadd_ps(_mm256_sub_ps(b, a), _mm256_set1_ps(frames.mix), a)
            } else {
                a
            }
        }

        #[target_feature(enable = "avx2,fma")]
        pub unsafe fn advance_and_sum<const GATHER: bool>(
            frames: &FramePair,
            voice: &mut SuperVoice,
            delta: f32,
            detune_factors: &[f32x8; UNISON_VECTORS],
            phase_offset: f32,
            voices: usize,
            mask: &[f32x8; UNISON_VECTORS],
        ) -> f32 {
            let max_index = _mm256_set1_epi32(frames.cycle_len() as i32 - 1);
            let table_len = _mm256_set1_ps(frames.cycle_len() as f32);
            let delta = _mm256_set1_ps(delta);
            let phase_offset = _mm256_set1_ps(phase_offset);
            let mut total = _mm256_setzero_ps();
            for vector in 0..voices.div_ceil(UNISON_LANES).min(UNISON_VECTORS) {
                let indices = advance(
                    &mut voice.voice_phases[vector],
                    delta,
                    &detune_factors[vector],
                    phase_offset,
                    table_len,
                );
                let samples = read_frames8::<GATHER>(frames, max_index, indices);
                total = _mm256_fmadd_ps(samples, load(&mask[vector]), total);
            }
            horizontal_sum(total)
        }

        #[target_feature(enable = "avx2,fma")]
        #[allow(clippy::too_many_arguments)]
        pub unsafe fn advance_and_sum_panned<const GATHER: bool>(
            frames: &FramePair,
            voice: &mut SuperVoice,
            delta: f32,
            detune_factors: &[f32x8; UNISON_VECTORS],
            phase_offset: f32,
            voices: usize,
            mask: &[f32x8; UNISON_VECTORS],
            pan_left: &[f32x8; UNISON_VECTORS],
            pan_right: &[f32x8; UNISON_VECTORS],
        ) -> (f32, f32, f32) {
            let max_index = _mm256_set1_epi32(frames.cycle_len() as i32 - 1);
            let table_len = _mm256_set1_ps(frames.cycle_len() as f32);
            let delta = _mm256_set1_ps(delta);
            let phase_offset = _mm256_set1_ps(phase_offset);
            let mut mono = _mm256_setzero_ps();
            let mut left = _mm256_setzero_ps();
            let mut right = _mm256_setzero_ps();
            for vector in 0..voices.div_ceil(UNISON_LANES).min(UNISON_VECTORS) {
                let indices = advance(
                    &mut voice.voice_phases[vector],
                    delta,
                    &detune_factors[vector],
                    phase_offset,
                    table_len,
                );
                let samples = read_frames8::<GATHER>(frames, max_index, indices);
                mono = _mm256_fmadd_ps(samples, load(&mask[vector]), mono);
                left = _mm256_fmadd_ps(samples, load(&pan_left[vector]), left);
                right = _mm256_fmadd_ps(samples, load(&pan_right[vector]), right);
            }
            (
                horizontal_sum(mono),
                horizontal_sum(left),
                horizontal_sum(right),
            )
        }
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
    /// Returns the index of the lowest note whose table is bandlimited enough for a phase delta
    /// of `delta` cycles per sample, i.e. `index` from `440.0 * 2.0_f32.powf(index / 12.0) =
    /// delta * sample_rate`, rounded up.
    pub fn cycles_to_note(delta: f32, sample_rate: f32) -> usize {
        (((sample_rate * delta / 440.0).log2() * 12.0 + 69.0).ceil() as usize).clamp(0, 137)
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
        fft: &dyn Fft<f32>,
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
                Wavetable::from_additive_osc_ifft(osc, table_length, harmonics, fft)
            })
            .collect();
        Self {
            tables: tables.try_into().unwrap(),
        }
    }
}

/// Number of frames in the pulse width sweep wavetable.
pub const PW_SWEEP_FRAMES: usize = 32;

/// A wavetable in the wavetable synth sense: one or more frames (waveforms, each bandlimited per
/// note) that an oscillator can morph between with its position control.
pub struct WavetableFrames {
    /// Display name, which matches the oscillators' wave parameter.
    #[allow(dead_code)] // only read by the tests and benchmarks so far
    pub name: &'static str,
    pub frames: Vec<WavetableNotes>,
}
impl WavetableFrames {
    /// Resolves a position (0 to 1) to the two frames to crossfade between and the mix amount
    /// between them.
    pub fn frame_indices(&self, position: f32) -> (usize, usize, f32) {
        let last = self.frames.len().saturating_sub(1);
        let scaled = position.clamp(0.0, 1.0) * last as f32;
        let a = (scaled as usize).min(last);
        let b = (a + 1).min(last);
        (a, b, scaled - a as f32)
    }
    /// Size of the wavetable's sample data.
    #[allow(dead_code)] // used by the benchmarks
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
    /// Generates the wavetables for the given sample rate. The order matches the oscillators'
    /// wave parameter.
    pub fn new(sample_rate: f32, table_len: usize) -> Self {
        // One FFT plan is shared by all tables; planning is comparatively expensive.
        let fft = FftPlanner::new().plan_fft_inverse(table_len);
        let notes = |osc: &AdditiveOsc| {
            WavetableNotes::from_additive_osc_ifft(osc, sample_rate, table_len, &*fft)
        };
        let single = |name, osc: AdditiveOsc| WavetableFrames {
            name,
            frames: vec![notes(&osc)],
        };
        let pulse = |name, duty| single(name, AdditiveOsc::pulse(duty));

        // From a square wave down to a very narrow pulse
        let sweep = WavetableFrames {
            name: "PW Sweep",
            frames: (0..PW_SWEEP_FRAMES)
                .map(|frame| {
                    let duty = 0.5 * (1.0 - frame as f32 / PW_SWEEP_FRAMES as f32);
                    notes(&AdditiveOsc::pulse(duty))
                })
                .collect(),
        };

        Self {
            wavetables: vec![
                single("Sine", AdditiveOsc::sine()),
                single("Triangle", AdditiveOsc::triangle()),
                single("Saw", AdditiveOsc::saw()),
                single("Exponential", AdditiveOsc::fake_exp()),
                single("Square", AdditiveOsc::square()),
                pulse("Pulse 40%", 0.4),
                pulse("Pulse 30%", 0.3),
                pulse("Pulse 20%", 0.2),
                pulse("Pulse 10%", 0.1),
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
    /// Renders one cycle of the first `harmonics` partials into `output` using an inverse FFT.
    /// `fft` must be an inverse FFT plan with the same length as `output`, which must be at least
    /// twice `harmonics` to keep the result bandlimited.
    pub fn generate_ifft(&self, output: &mut [f32], harmonics: usize, fft: &dyn Fft<f32>) {
        assert_eq!(output.len(), fft.len());
        assert!(harmonics <= output.len() / 2);

        // Bin 0 is the DC offset, which is always zero.
        let mut buffer = vec![Complex32::new(0.0, 0.0); output.len()];
        for (bin, (amplitude, phase)) in buffer
            .iter_mut()
            .skip(1)
            .zip(self.amplitudes.iter().zip(self.phases.iter()))
            .take(harmonics)
        {
            // `i * amplitude * e^(i * phase)`, which the inverse FFT turns into
            // `-amplitude * sin(n * theta + phase)`.
            let (sin, cos) = phase.sin_cos();
            *bin = Complex32::new(-amplitude * sin, amplitude * cos);
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
        for i in 0..len {
            assert_eq!(table.read(i as f32), cycle[i]);
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

    #[test]
    fn simd_lookups_match_the_scalar_reads() {
        let len = 256;
        let sine = Wavetable::new(
            (0..len)
                .map(|i| (2.0 * PI * i as f32 / len as f32).sin())
                .collect(),
        );
        let saw = Wavetable::new((0..len).map(|i| i as f32 / len as f32 * 2.0 - 1.0).collect());
        let rng = fastrand::Rng::with_seed(42);

        let mut osc = OscillatorParams {
            voice_count: 13,
            voices_detune: 0.3,
            ..Default::default()
        };
        osc.update_detune_factors();
        osc.update_pan_gains(0.8);
        let voices = osc.unison_voices();

        let mut levels = vec![SimdLevel::Portable];
        #[cfg(target_arch = "x86_64")]
        if SimdLevel::detect() != SimdLevel::Portable {
            levels.push(SimdLevel::Avx2);
            levels.push(SimdLevel::Avx2Gather);
        }

        for mix in [0.0, 0.3] {
            let frames = FramePair {
                a: &sine,
                b: &saw,
                mix,
            };
            let start = SuperVoice::new(0.1, 0.7);
            // Reference: one voice at a time, through the scalar path
            let mut reference = start.clone();
            let mut expected = Vec::new();
            for step in 0..20 {
                let delta = 0.013 * (1.0 + step as f32 * 0.1);
                let offset = (rng.f32() - 0.5) * 3.0;
                let (mut mono, mut left, mut right) = (0.0, 0.0, 0.0);
                for i in 0..voices {
                    let index = reference.advance_one(i, delta, lane(&osc.detune_factors, i), offset, len as f32);
                    let sample = frames.read(index);
                    let (gain_left, gain_right) = osc.pan_gain(i);
                    mono += sample;
                    left += sample * gain_left;
                    right += sample * gain_right;
                }
                expected.push((delta, offset, mono, left, right));
            }

            for simd in &levels {
                let mut voice = start.clone();
                let mut voice_panned = start.clone();
                for (delta, offset, mono, left, right) in &expected {
                    let got = frames.advance_and_sum(
                        &mut voice, *delta, &osc.detune_factors, *offset, voices, &osc.voice_mask, *simd,
                    );
                    assert!((got - mono).abs() < 1e-3, "{simd:?} mix {mix}: {got} vs {mono}");
                    let (got_mono, got_left, got_right) = frames.advance_and_sum_panned(
                        &mut voice_panned, *delta, &osc.detune_factors, *offset, voices,
                        &osc.voice_mask, &osc.pan_left, &osc.pan_right, *simd,
                    );
                    assert!((got_mono - mono).abs() < 1e-3, "{simd:?} mix {mix}: {got_mono} vs {mono}");
                    assert!((got_left - left).abs() < 1e-3, "{simd:?} mix {mix}: {got_left} vs {left}");
                    assert!((got_right - right).abs() < 1e-3, "{simd:?} mix {mix}: {got_right} vs {right}");
                }
                // The phases of the voices in use advanced identically
                for i in 0..voices {
                    let a = lane(&reference.voice_phases, i);
                    let b = lane(&voice.voice_phases, i);
                    assert!((a - b).abs() < 1e-5, "{simd:?} voice {i}: {a} vs {b}");
                }
            }

            // Few voices take the scalar path
            let mut few = start.clone();
            let mut few_reference = start.clone();
            let got = frames.advance_and_sum(&mut few, 0.01, &osc.detune_factors, 0.2, 2, &osc.voice_mask, levels[0]);
            let mut want = 0.0;
            for i in 0..2 {
                want += frames.read(few_reference.advance_one(i, 0.01, lane(&osc.detune_factors, i), 0.2, len as f32));
            }
            assert!((got - want).abs() < 1e-4);
        }
    }

    #[test]
    fn pulse_wavetable_has_the_expected_duty_cycle() {
        let fft = FftPlanner::new().plan_fft_inverse(2048);
        let table = Wavetable::from_additive_osc_ifft(&AdditiveOsc::pulse(0.25), 2048, 1024, &*fft);
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
}
