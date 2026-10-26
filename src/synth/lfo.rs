//! The LFOs, low frequency oscillators whose outputs are modulation sources (see
//! [`super::modulation`]).
//!
//! An LFO runs at a rate in Hz, or synced to the host's tempo, a cycle taking a note length (see
//! [`DIVISIONS`]). Each voice runs its own copy of every LFO (see [`LfoVoice`]), which starts
//! the note at the LFO's start phase (Retrigger), carries on from a copy that runs all the time
//! (Free, which while the host plays follows its song position when synced, so that it stays in
//! time with the music), or runs a single cycle and holds its end (One Shot). Its output can
//! fade in over the start of each note.
//!
//! Besides the periodic waves there are two random ones: a new value each cycle (Random, a
//! sample and hold) and the same values glided between (Smooth Random). The values come from a
//! hash of the cycle's number, so a copy's values follow from its cycle count alone, whether
//! it's worked out once per block or at every sample, and copies started from the free running
//! one share its values.

use wide::f32x8;

use super::fastmath::{self, x8};

/// The slowest and fastest an LFO can run, in Hz, modulated or not.
pub const LFO_MIN_HZ: f32 = 0.01;
pub const LFO_MAX_HZ: f32 = 1000.0;

/// The note lengths a synced LFO's cycle can take, from the longest to the shortest, with their
/// lengths in beats (quarter notes): bars of four beats, then dotted (`D`), straight and
/// triplet (`T`) note values.
pub const DIVISIONS: [(&str, f32); 20] = [
    ("8 bars", 32.0),
    ("4 bars", 16.0),
    ("2 bars", 8.0),
    ("1 bar", 4.0),
    ("1/2D", 3.0),
    ("1/2", 2.0),
    ("1/4D", 1.5),
    ("1/2T", 4.0 / 3.0),
    ("1/4", 1.0),
    ("1/8D", 0.75),
    ("1/4T", 2.0 / 3.0),
    ("1/8", 0.5),
    ("1/16D", 0.375),
    ("1/8T", 1.0 / 3.0),
    ("1/16", 0.25),
    ("1/32D", 0.1875),
    ("1/16T", 1.0 / 6.0),
    ("1/32", 0.125),
    ("1/32T", 1.0 / 12.0),
    ("1/64", 0.0625),
];
/// The division a synced LFO starts at: a cycle a beat.
pub const DEFAULT_DIVISION: usize = 8;
/// The tempo LFOs are synced to when the host doesn't say, in beats per minute.
pub const DEFAULT_TEMPO: f64 = 120.0;

/// An LFO's wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LfoWave {
    #[default]
    Sine,
    Triangle,
    Saw,
    /// A rectified sine.
    Exponential,
    Square,
    /// A new random value each cycle.
    Random,
    /// The random values, glided between.
    SmoothRandom,
}

impl LfoWave {
    /// Whether the wave draws random values, rather than repeating each cycle.
    pub fn is_random(self) -> bool {
        matches!(self, Self::Random | Self::SmoothRandom)
    }

    /// The wave at `phase` (in cycles, from 0 to 1) of its cycle number `cycle`, from -1 to 1.
    /// The random waves draw each cycle's value from `seed`.
    pub fn value(self, phase: f32, cycle: u32, seed: u32) -> f32 {
        match self {
            Self::Sine => fastmath::sin_cycles(phase),
            Self::Triangle => {
                if phase <= 0.5 {
                    phase * 4.0 - 1.0
                } else {
                    3.0 - phase * 4.0
                }
            }
            Self::Saw => phase * 2.0 - 1.0,
            Self::Exponential => fastmath::sin_cycles(phase).abs() * 2.0 - 1.0,
            Self::Square => {
                if phase <= 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Random => random(seed, cycle),
            Self::SmoothRandom => {
                let (from, to) = (random(seed, cycle), random(seed, cycle.wrapping_add(1)));
                // (Smoothstep, so that it eases from one value to the next)
                from + (to - from) * phase * phase * (3.0 - 2.0 * phase)
            }
        }
    }

    /// [`value()`][Self::value()] of eight phases of the periodic waves at once, with exactly
    /// the same results.
    pub fn value8(self, phase: f32x8) -> f32x8 {
        let splat = f32x8::splat;
        let half = phase.simd_le(splat(0.5));
        match self {
            Self::Sine => x8::sin_cycles(phase),
            Self::Triangle => half.select(phase * splat(4.0) - splat(1.0), splat(3.0) - phase * splat(4.0)),
            Self::Saw => phase * splat(2.0) - splat(1.0),
            Self::Exponential => x8::sin_cycles(phase).abs() * splat(2.0) - splat(1.0),
            Self::Square => half.select(splat(1.0), splat(-1.0)),
            Self::Random | Self::SmoothRandom => panic!("the random waves aren't worked out eight at a time"),
        }
    }
}

/// A random value from -1 to 1 for cycle `cycle` of the sequence `seed` draws: a hash of the
/// two (Chris Wellons' `lowbias32`).
fn random(seed: u32, cycle: u32) -> f32 {
    let mut x = seed ^ cycle.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1 << 23) as f32 - 1.0
}

/// How an LFO starts each note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LfoTrigger {
    /// At its start phase.
    #[default]
    Retrigger,
    /// Where a copy that runs all the time is (plus the start phase).
    Free,
    /// At its start phase, for a single cycle, holding the end of it.
    OneShot,
}

/// An LFO's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lfo {
    pub wave: LfoWave,
    /// The rate in Hz, unless it's synced.
    pub freq: f32,
    /// A cycle's length in beats, when it's synced to the tempo.
    pub sync: Option<f32>,
    /// Where in its cycle a note starts it, from 0 to 1.
    pub phase: f32,
    pub trigger: LfoTrigger,
    /// How long it takes to fade in at the start of each note, in seconds.
    pub fade: f32,
}

impl Default for Lfo {
    fn default() -> Self {
        Self {
            wave: LfoWave::Sine,
            freq: 5.0,
            sync: None,
            phase: 0.0,
            trigger: LfoTrigger::Retrigger,
            fade: 0.0,
        }
    }
}

impl Lfo {
    /// The rate in Hz at `tempo` beats per minute, before modulation.
    pub fn rate(&self, tempo: f64) -> f32 {
        match self.sync {
            Some(beats) => (tempo / 60.0 / beats as f64) as f32,
            None => self.freq,
        }
        .clamp(LFO_MIN_HZ, LFO_MAX_HZ)
    }

    /// How far it has faded in, `time` seconds into a note.
    pub fn fade_gain(&self, time: f32) -> f32 {
        if self.fade > 0.0 {
            (time / self.fade).min(1.0)
        } else {
            1.0
        }
    }

    /// [`fade_gain()`][Self::fade_gain()] of eight times at once, with exactly the same
    /// results.
    pub fn fade_gain8(&self, time: f32x8) -> f32x8 {
        if self.fade > 0.0 {
            (time / f32x8::splat(self.fade)).fast_min(f32x8::splat(1.0))
        } else {
            f32x8::splat(1.0)
        }
    }
}

/// A copy of an LFO, which a voice runs (or the synth, as the one that runs all the time for
/// the LFOs set to Free).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LfoVoice {
    /// The phase in its cycle, in `[0, 1)`, or 1 once a one shot LFO has finished its cycle.
    /// Double precision, since a slow LFO updated at every sample advances by steps too small
    /// for single precision to add up exactly: at 0.01 Hz it would run 3% fast.
    pub phase: f64,
    /// How many cycles it has finished, which the random waves draw a value for each of.
    pub cycle: u32,
    /// What the random waves draw their values from.
    pub seed: u32,
}

impl LfoVoice {
    /// The copy a note starts: at the LFO's start phase, or for an LFO set to Free, that far
    /// on from where the copy that runs all the time (`free`) is, with its random values.
    pub fn start(lfo: &Lfo, free: &LfoVoice, seed: u32) -> Self {
        match lfo.trigger {
            LfoTrigger::Free => {
                let phase = free.phase + lfo.phase as f64;
                let whole = phase.floor();
                Self {
                    phase: phase - whole,
                    cycle: free.cycle.wrapping_add(whole as u32),
                    seed: free.seed,
                }
            }
            LfoTrigger::Retrigger | LfoTrigger::OneShot => Self {
                phase: lfo.phase as f64,
                cycle: 0,
                seed,
            },
        }
    }

    /// The copy that runs all the time, at a song position of `beats` for a cycle of
    /// `cycle_beats`, for the synced LFOs set to Free.
    pub fn at_position(&self, beats: f64, cycle_beats: f32) -> Self {
        let cycles = beats / cycle_beats as f64;
        let whole = cycles.floor();
        Self {
            phase: cycles - whole,
            cycle: whole as i64 as u32,
            seed: self.seed,
        }
    }

    /// Advances by `delta` cycles, wrapping round into the next cycle (or, for a one shot LFO,
    /// stopping at the end of its first), and returns the new phase in single precision (which
    /// can round up to 1).
    pub fn advance(&mut self, delta: f32, one_shot: bool) -> f32 {
        let mut phase = self.phase + delta as f64;
        if one_shot {
            phase = phase.min(1.0);
        } else if phase >= 1.0 {
            // (Usually the phase is still in its cycle, and the comparison is cheaper than
            // wrapping)
            let whole = phase.floor();
            phase -= whole;
            self.cycle = self.cycle.wrapping_add(whole as u32);
        }
        self.phase = phase;
        phase as f32
    }

    /// Its output now, before any fade.
    pub fn value(&self, wave: LfoWave) -> f32 {
        wave.value(self.phase as f32, self.cycle, self.seed)
    }

    /// Its output at `offset` cycles on from its phase (without moving it on), before any fade:
    /// for the samples of a block worked out from its first. `offset` doesn't go past the end of
    /// a one shot LFO's cycle.
    pub fn value_at(&self, wave: LfoWave, offset: f32, one_shot: bool) -> f32 {
        let phase = self.phase as f32 + offset;
        if one_shot {
            return wave.value(phase.min(1.0), self.cycle, self.seed);
        }
        let whole = phase.floor();
        wave.value(phase - whole, self.cycle.wrapping_add(whole as u32), self.seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow LFO updated at every sample advances its phase by steps far smaller than the
    /// phase, which still add up to the right time.
    #[test]
    fn the_phase_keeps_time_in_tiny_steps() {
        let mut lfo = LfoVoice::default();
        let delta = 0.01 / 44100.0;
        let mut phase = 0.0;
        // Half a cycle at 0.01 Hz
        for _ in 0..44100 * 50 {
            phase = lfo.advance(delta, false);
        }
        assert!((phase - 0.5).abs() < 1e-5, "{phase}");
        // A step of more than a cycle (a fast LFO at the block rate) wraps too, counting the
        // cycles
        assert!((lfo.advance(2.25, false) - 0.75).abs() < 1e-5);
        assert_eq!(lfo.cycle, 2);
        // A one shot LFO stops at the end of its cycle
        let mut one_shot = LfoVoice::default();
        assert_eq!(one_shot.advance(0.75, true), 0.75);
        assert_eq!(one_shot.advance(0.75, true), 1.0);
        assert_eq!(one_shot.advance(0.75, true), 1.0);
        assert_eq!(one_shot.cycle, 0);
    }

    #[test]
    fn waves_go_from_minus_one_to_one() {
        let waves = [
            LfoWave::Sine,
            LfoWave::Triangle,
            LfoWave::Saw,
            LfoWave::Exponential,
            LfoWave::Square,
            LfoWave::Random,
            LfoWave::SmoothRandom,
        ];
        for wave in waves {
            let (mut low, mut high) = (f32::MAX, f32::MIN);
            for cycle in 0..200 {
                for step in 0..=16 {
                    let value = wave.value(step as f32 / 16.0, cycle, 12345);
                    low = low.min(value);
                    high = high.max(value);
                }
            }
            assert!(low >= -1.0 && high <= 1.0, "{wave:?}: {low} to {high}");
            // (and the random values spread across the range)
            assert!(low < -0.9 && high > 0.9, "{wave:?}: {low} to {high}");
        }
        // The periodic waves eight at a time match, including at the end of a cycle
        for wave in &waves[..5] {
            let phases: [f32; 8] = std::array::from_fn(|lane| lane as f32 / 7.0);
            let vector = wave.value8(f32x8::new(phases)).to_array();
            for (phase, value) in phases.into_iter().zip(vector) {
                assert_eq!(value.to_bits(), wave.value(phase, 0, 0).to_bits(), "{wave:?} at {phase}");
            }
        }
    }

    /// The random waves hold a value for each cycle, the smooth one gliding from each to the
    /// next, and a seed draws its own values.
    #[test]
    fn random_waves_draw_a_value_each_cycle() {
        let held = |phase: f32, cycle: u32| LfoWave::Random.value(phase, cycle, 7);
        assert_eq!(held(0.0, 3), held(0.99, 3));
        assert_ne!(held(0.0, 3), held(0.0, 4));
        assert_ne!(held(0.0, 3), LfoWave::Random.value(0.0, 3, 8));
        let smooth = |phase: f32, cycle: u32| LfoWave::SmoothRandom.value(phase, cycle, 7);
        assert_eq!(smooth(0.0, 3), held(0.0, 3));
        assert!((smooth(1.0, 3) - held(0.0, 4)).abs() < 1e-6);
        let middle = smooth(0.5, 3);
        assert!(middle > held(0.0, 3).min(held(0.0, 4)) && middle < held(0.0, 3).max(held(0.0, 4)));
        // A copy's values a block on are the ones it reaches by moving on
        let mut lfo = LfoVoice { phase: 0.6, cycle: 9, seed: 99 };
        let ahead = lfo.value_at(LfoWave::Random, 1.7, false);
        lfo.advance(1.7, false);
        assert_eq!(ahead, lfo.value(LfoWave::Random));
        assert_eq!(lfo.cycle, 11);
    }

    #[test]
    fn notes_start_where_the_trigger_says() {
        let free = LfoVoice { phase: 0.75, cycle: 5, seed: 42 };
        let lfo = Lfo { phase: 0.5, ..Lfo::default() };
        // Retriggered at the start phase, with the note's own random values
        assert_eq!(LfoVoice::start(&lfo, &free, 3), LfoVoice { phase: 0.5, cycle: 0, seed: 3 });
        // Free: from the free running copy, a start phase on (into its next cycle)
        let free_lfo = Lfo { trigger: LfoTrigger::Free, ..lfo };
        assert_eq!(LfoVoice::start(&free_lfo, &free, 3), LfoVoice { phase: 0.25, cycle: 6, seed: 42 });
        // Synced, the free running copy follows the song position: a quarter note cycle at
        // beat 10.5 is half way through its eleventh cycle
        assert_eq!(free.at_position(10.5, 1.0), LfoVoice { phase: 0.5, cycle: 10, seed: 42 });
        assert_eq!(free.at_position(10.5, 4.0).cycle, 2);
    }

    #[test]
    fn synced_rates_follow_the_tempo() {
        let quarter = Lfo { sync: Some(1.0), ..Lfo::default() };
        assert_eq!(quarter.rate(120.0), 2.0);
        assert_eq!(Lfo { sync: Some(DIVISIONS[3].1), ..quarter }.rate(120.0), 0.5);
        assert_eq!(Lfo { sync: Some(DIVISIONS[DEFAULT_DIVISION].1), ..quarter }.rate(90.0), 1.5);
        // Unsynced it's its own rate, and either way within the range
        assert_eq!(Lfo { freq: 7.0, ..Lfo::default() }.rate(120.0), 7.0);
        assert_eq!(Lfo { sync: Some(32.0), ..quarter }.rate(10.0), LFO_MIN_HZ);
        // The divisions go from the longest to the shortest
        assert!(DIVISIONS.windows(2).all(|pair| pair[0].1 > pair[1].1));
        assert_eq!(DIVISIONS[DEFAULT_DIVISION], ("1/4", 1.0));
    }

    #[test]
    fn the_fade_in_ramps_up_over_its_time() {
        let lfo = Lfo { fade: 2.0, ..Lfo::default() };
        assert_eq!(lfo.fade_gain(0.0), 0.0);
        assert_eq!(lfo.fade_gain(1.0), 0.5);
        assert_eq!(lfo.fade_gain(3.0), 1.0);
        assert_eq!(Lfo::default().fade_gain(0.0), 1.0);
        let times = f32x8::new([0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 0.1]);
        for (time, gain) in times.to_array().into_iter().zip(lfo.fade_gain8(times).to_array()) {
            assert_eq!(gain.to_bits(), lfo.fade_gain(time).to_bits());
        }
    }
}
