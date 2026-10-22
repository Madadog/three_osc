use std::{
    convert::TryInto,
    f32::consts::{FRAC_1_SQRT_2, FRAC_PI_4, PI, SQRT_2},
};

use itertools::izip;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use wide::f32x8;

use super::lerp;

/// Maximum number of unison voices per oscillator.
pub const MAX_UNISON: usize = 32;
/// Unison voices are processed in SIMD vectors of this many lanes.
pub const UNISON_LANES: usize = 8;
/// Number of SIMD vectors needed for all unison voices.
pub const UNISON_VECTORS: usize = MAX_UNISON / UNISON_LANES;

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
        // The largest value below 1.0, so that the scaled index stays below the table length.
        let below_one = f32x8::splat(0.999_999_94);

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
    /// `(left, right)` gain of each unison voice, see `update_pan_gains()`.
    pub pan_gains: [(f32, f32); MAX_UNISON],
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
    /// Updates each unison voice's phase delta relative to the base delta: the centre voice is
    /// 1, the others spread out above and below it by the unison detune.
    pub fn update_detune_factors(&mut self) {
        let voice_count = (self.voice_count as usize).clamp(1, MAX_UNISON);
        let mut factors = [1.0_f32; MAX_UNISON];
        for (i, factor) in factors.iter_mut().enumerate().take(voice_count) {
            *factor = 1.0 + unison_detune_index(i) as f32 * self.voices_detune / voice_count as f32;
        }
        self.detune_factors = std::array::from_fn(|vector| {
            let lanes = &factors[vector * UNISON_LANES..(vector + 1) * UNISON_LANES];
            f32x8::new(lanes.try_into().unwrap())
        });
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
        let voice_count = (self.voice_count as usize).clamp(1, self.pan_gains.len());

        for (i, gains) in self.pan_gains.iter_mut().enumerate().take(voice_count) {
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
            *gains = (cos * SQRT_2, sin * SQRT_2);
        }
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
            pan_gains: [(1.0, 1.0); MAX_UNISON],
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

#[derive(Debug, Clone)]
/// One cycle of a waveform, followed by a copy of its first sample so that reads can interpolate
/// across the end of the cycle without any wrapping logic.
pub struct Wavetable {
    pub table: Vec<f32>,
}

impl Wavetable {
    pub fn new(mut table: Vec<f32>) -> Self {
        assert!(!table.is_empty(), "a wavetable needs at least one sample");
        let first = table[0];
        table.push(first);
        Self { table }
    }
    /// Samples per cycle.
    #[inline]
    pub fn cycle_len(&self) -> usize {
        self.table.len() - 1
    }
    /// Reads the table at a fractional sample index in `[0, cycle_len)` with linear
    /// interpolation.
    /// Out of range indices are clamped to the last sample.
    #[inline]
    pub fn read(&self, index: f32) -> f32 {
        let floor = (index as usize).min(self.table.len().saturating_sub(2));
        // SAFETY: `floor + 1` is at most `table.len() - 1`, and the table always holds at least
        // two samples (a cycle plus the padding sample).
        let (from, to) = unsafe {
            (
                *self.table.get_unchecked(floor),
                *self.table.get_unchecked(floor + 1),
            )
        };
        lerp(from, to, index - floor as f32)
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
        // Room for the padding sample, so `new()` doesn't have to reallocate
        let mut table: Vec<f32> = Vec::with_capacity(len + 1);
        table.resize(len, 0.0);
        osc.generate_ifft(&mut table, harmonics, fft);
        Self::new(table)
    }
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
    /// Sums the first `count` unison voices, read at the given indices.
    #[inline]
    pub fn sum(&self, indices: &[f32x8; UNISON_VECTORS], count: usize) -> f32 {
        let mut total = 0.0;
        for voice in 0..count.min(MAX_UNISON) {
            total += self.read(indices[voice / UNISON_LANES].as_array()[voice % UNISON_LANES]);
        }
        total
    }
    /// Like [`sum()`][Self::sum()], but also pans the voices into a left and a right channel
    /// using `gains` (one `(left, right)` gain pair per voice). Returns `(mono, left, right)`,
    /// where `mono` is the plain sum of the voices.
    #[inline]
    pub fn sum_panned(
        &self,
        indices: &[f32x8; UNISON_VECTORS],
        count: usize,
        gains: &[(f32, f32); MAX_UNISON],
    ) -> (f32, f32, f32) {
        let (mut mono, mut left, mut right) = (0.0, 0.0, 0.0);
        for voice in 0..count.min(MAX_UNISON) {
            let sample = self.read(indices[voice / UNISON_LANES].as_array()[voice % UNISON_LANES]);
            let (gain_left, gain_right) = gains[voice];
            mono += sample;
            left += sample * gain_left;
            right += sample * gain_right;
        }
        (mono, left, right)
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
        assert!((osc.pan_gains[0].0 - 1.0).abs() < 1e-5);
        assert!((osc.pan_gains[0].1 - 1.0).abs() < 1e-5);

        // Detune indices per voice: 0, -1, 2, -3, 4, -5, 6, -7
        osc.voice_count = 8;
        osc.update_pan_gains(1.0);
        let pans: Vec<f32> = osc.pan_gains[..8].iter().map(|&g| pan_from_gains(g)).collect();
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
        let pans: Vec<f32> = osc.pan_gains[..3].iter().map(|&g| pan_from_gains(g)).collect();
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
        let table = Wavetable::new(vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(table.cycle_len(), 4);
        assert_eq!(table.read(0.0), 0.0);
        assert_eq!(table.read(1.5), 1.5);
        // Between the last sample and the start of the next cycle
        assert_eq!(table.read(3.5), 1.5);
        // Out of range reads are clamped instead of reading out of bounds
        assert!(table.read(4.0).is_finite());
        assert!(table.read(1e9).is_finite());
        assert!(table.read(-1.0).is_finite());
        assert_eq!(table.generate(0.375), 1.5);
        assert_eq!(table.generate(-0.625), 1.5);
    }

    #[test]
    fn unison_voices_advance_and_wrap() {
        let mut voice = SuperVoice::new(0.0, 0.0);
        let mut osc = OscillatorParams {
            voice_count: 2,
            voices_detune: 0.5,
            ..Default::default()
        };
        osc.update_detune_factors();
        let mut indices = [f32x8::splat(0.0); UNISON_VECTORS];
        // Voice 0 advances by 0.3 cycles, voice 1 by 0.3 * (1 - 0.5 / 2)
        for _ in 0..4 {
            voice.advance(0.3, 2, &osc.detune_factors, 0.0, 100.0, &mut indices);
        }
        let phases = voice.voice_phases[0].to_array();
        assert!((phases[0] - 0.2).abs() < 1e-5, "{phases:?}");
        assert!((phases[1] - 0.9).abs() < 1e-5, "{phases:?}");
        let read = indices[0].to_array();
        assert!((read[0] - 20.0).abs() < 1e-3, "{read:?}");
        // A negative phase offset wraps too
        voice.advance(0.0, 2, &osc.detune_factors, -0.5, 100.0, &mut indices);
        let read = indices[0].to_array();
        assert!((read[0] - 70.0).abs() < 1e-3, "{read:?}");
        assert!((read[1] - 40.0).abs() < 1e-3, "{read:?}");
        assert!(read.iter().all(|index| *index < 100.0));
    }

    #[test]
    fn pulse_wavetable_has_the_expected_duty_cycle() {
        let fft = FftPlanner::new().plan_fft_inverse(2048);
        let table = Wavetable::from_additive_osc_ifft(&AdditiveOsc::pulse(0.25), 2048, 1024, &*fft);
        let cycle = &table.table[..table.cycle_len()];
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
