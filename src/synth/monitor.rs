//! What the audio thread shows the GUI of the notes that are playing, so that the displays can
//! follow them: where each voice is in its envelopes, and the newest voice's LFOs, modulation
//! and filter cutoff. And the synth's output itself, for the meter and spectrogram
//! (see [`OutputTap`]).
//!
//! The audio thread writes it after each buffer while the GUI is open (see
//! [`ThreeOsc::publish()`][super::ThreeOsc::publish()]), and the GUI reads it as it draws. The
//! values are separate atomics, so a frame can mix values from two buffers, which doesn't
//! matter for drawing; each voice's own values are written together, so they always match.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use nih_plug::prelude::Enum;

use super::lfo::LfoVoice;
use super::modulation::{ModTarget, Targets, N_LFOS, N_TARGETS};

/// How many of the newest voices are shown.
pub const MONITORED_VOICES: usize = 16;

/// A voice as the GUI sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceState {
    /// Samples since the note started.
    pub runtime: u32,
    /// The sample the note was released at, if it has been.
    pub released: Option<u32>,
}

impl VoiceState {
    /// Both values in one word, the release as one more than its sample (0 while held).
    fn pack(self) -> u64 {
        let released = self.released.map_or(0, |released| released as u64 + 1);
        (released << 32) | self.runtime as u64
    }
    fn unpack(packed: u64) -> Self {
        let released = (packed >> 32) as u32;
        Self {
            runtime: packed as u32,
            released: released.checked_sub(1),
        }
    }
}

/// See the [module documentation][self].
pub struct Monitor {
    voices: [AtomicU64; MONITORED_VOICES],
    voice_count: AtomicU32,
    /// Whether a voice is playing, so that the newest voice's values below mean something.
    playing: AtomicBool,
    /// The newest voice's copies of the LFOs (their phases, cycles and seeds), and their
    /// outputs.
    lfo_phases: [AtomicU32; N_LFOS],
    lfo_cycles: [AtomicU32; N_LFOS],
    lfo_seeds: [AtomicU32; N_LFOS],
    lfo_values: [AtomicU32; N_LFOS],
    /// The newest voice's modulation of every target.
    modulation: [AtomicU32; N_TARGETS],
    /// The newest voice's filter cutoff, in Hz.
    cutoff: AtomicU32,
    /// The synth's output.
    pub output: OutputTap,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            voices: std::array::from_fn(|_| AtomicU64::new(0)),
            voice_count: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            lfo_phases: std::array::from_fn(|_| AtomicU32::new(0)),
            lfo_cycles: std::array::from_fn(|_| AtomicU32::new(0)),
            lfo_seeds: std::array::from_fn(|_| AtomicU32::new(0)),
            lfo_values: std::array::from_fn(|_| AtomicU32::new(0)),
            modulation: std::array::from_fn(|_| AtomicU32::new(0)),
            cutoff: AtomicU32::new(0),
            output: OutputTap::default(),
        }
    }
}

/// The newest voice's values, for [`Monitor::store()`].
pub struct NewestVoice {
    pub lfos: [LfoVoice; N_LFOS],
    /// Its LFOs' outputs, faded in as far as they have.
    pub lfo_values: [f32; N_LFOS],
    pub modulation: Targets,
    pub cutoff: f32,
}

impl Monitor {
    /// Stores the shown voices (the newest last) and the newest voice's values, if any voice is
    /// playing.
    pub fn store(&self, voices: impl ExactSizeIterator<Item = VoiceState>, newest: Option<NewestVoice>) {
        let count = voices.len().min(MONITORED_VOICES);
        let older = voices.len() - count;
        for (slot, voice) in self.voices.iter().zip(voices.skip(older)) {
            slot.store(voice.pack(), Ordering::Relaxed);
        }
        self.voice_count.store(count as u32, Ordering::Relaxed);
        self.playing.store(newest.is_some(), Ordering::Relaxed);
        if let Some(newest) = newest {
            for (lfo, voice) in newest.lfos.iter().enumerate() {
                self.lfo_phases[lfo].store((voice.phase as f32).to_bits(), Ordering::Relaxed);
                self.lfo_cycles[lfo].store(voice.cycle, Ordering::Relaxed);
                self.lfo_seeds[lfo].store(voice.seed, Ordering::Relaxed);
            }
            for (slot, value) in self.lfo_values.iter().zip(newest.lfo_values) {
                slot.store(value.to_bits(), Ordering::Relaxed);
            }
            for (slot, modulation) in self.modulation.iter().zip(newest.modulation) {
                slot.store(modulation.to_bits(), Ordering::Relaxed);
            }
            self.cutoff.store(newest.cutoff.to_bits(), Ordering::Relaxed);
        }
    }

    /// The shown voices, the newest last.
    pub fn voices(&self) -> impl Iterator<Item = VoiceState> + '_ {
        let count = (self.voice_count.load(Ordering::Relaxed) as usize).min(MONITORED_VOICES);
        self.voices[..count]
            .iter()
            .map(|slot| VoiceState::unpack(slot.load(Ordering::Relaxed)))
    }

    /// Reads one of the newest voice's values, if a voice is playing.
    fn newest(&self, slot: &AtomicU32) -> Option<f32> {
        self.playing
            .load(Ordering::Relaxed)
            .then(|| f32::from_bits(slot.load(Ordering::Relaxed)))
    }

    /// The newest voice's output of an LFO, faded in as far as it has.
    pub fn lfo_value(&self, lfo: usize) -> Option<f32> {
        self.newest(&self.lfo_values[lfo])
    }

    /// The newest voice's copy of an LFO (with its phase in single precision).
    pub fn lfo(&self, lfo: usize) -> Option<LfoVoice> {
        self.newest(&self.lfo_phases[lfo]).map(|phase| LfoVoice {
            phase: phase as f64,
            cycle: self.lfo_cycles[lfo].load(Ordering::Relaxed),
            seed: self.lfo_seeds[lfo].load(Ordering::Relaxed),
        })
    }

    /// The newest voice's modulation of a target.
    pub fn modulation(&self, target: ModTarget) -> Option<f32> {
        self.newest(&self.modulation[target.to_index()])
    }

    /// The newest voice's filter cutoff, in Hz.
    pub fn cutoff(&self) -> Option<f32> {
        self.newest(&self.cutoff)
    }
}

/// The samples of each channel the output tap keeps: a third of a second at 192 kHz, which is
/// many frames of the GUI.
pub const OUTPUT_SAMPLES: usize = 1 << 16;

/// The synth's output, for the GUI's meter and spectrogram: the audio thread writes each buffer
/// into a ring of samples, and the GUI reads what is new as it draws.
pub struct OutputTap {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    /// The number of samples written so far, stored after the samples themselves.
    written: AtomicU64,
}

impl Default for OutputTap {
    fn default() -> Self {
        let ring = || (0..OUTPUT_SAMPLES).map(|_| AtomicU32::new(0)).collect();
        Self {
            left: ring(),
            right: ring(),
            written: AtomicU64::new(0),
        }
    }
}

impl OutputTap {
    /// Adds a buffer of the synth's output.
    pub fn write(&self, left: &[f32], right: &[f32]) {
        let start = self.written.load(Ordering::Relaxed);
        let len = left.len().min(right.len());
        for (i, (left, right)) in left.iter().zip(right).enumerate() {
            let at = (start as usize + i) % OUTPUT_SAMPLES;
            self.left[at].store(left.to_bits(), Ordering::Relaxed);
            self.right[at].store(right.to_bits(), Ordering::Relaxed);
        }
        self.written.store(start + len as u64, Ordering::Release);
    }

    /// Appends the samples written since `from` (a count of samples written, as this returns)
    /// to `left` and `right`, and returns the count to read on from next time. After a long
    /// break, only the most recent half of the ring is read (the rest could be overwritten
    /// while it is read).
    pub fn read(&self, from: u64, left: &mut Vec<f32>, right: &mut Vec<f32>) -> u64 {
        let end = self.written.load(Ordering::Acquire);
        let start = from.clamp(end.saturating_sub(OUTPUT_SAMPLES as u64 / 2), end);
        for position in start..end {
            let at = (position % OUTPUT_SAMPLES as u64) as usize;
            left.push(f32::from_bits(self.left[at].load(Ordering::Relaxed)));
            right.push(f32::from_bits(self.right[at].load(Ordering::Relaxed)));
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_tap_hands_on_what_is_new() {
        let tap = OutputTap::default();
        let (mut left, mut right) = (Vec::new(), Vec::new());
        assert_eq!(tap.read(0, &mut left, &mut right), 0);
        tap.write(&[1.0, 2.0, 3.0], &[-1.0, -2.0, -3.0]);
        let position = tap.read(0, &mut left, &mut right);
        assert_eq!((position, &left[..], &right[..]), (3, &[1.0, 2.0, 3.0][..], &[-1.0, -2.0, -3.0][..]));
        left.clear();
        right.clear();
        tap.write(&[4.0], &[-4.0]);
        assert_eq!(tap.read(position, &mut left, &mut right), 4);
        assert_eq!(left, [4.0]);

        // Round the ring, more than it can keep: the newest half of it is read
        let long: Vec<f32> = (0..OUTPUT_SAMPLES + 100).map(|i| i as f32).collect();
        tap.write(&long, &long);
        left.clear();
        right.clear();
        let end = tap.read(4, &mut left, &mut right);
        assert_eq!(end, 4 + long.len() as u64);
        assert_eq!(left.len(), OUTPUT_SAMPLES / 2);
        assert_eq!(left.last(), long.last());
        assert_eq!(right[0], long[long.len() - OUTPUT_SAMPLES / 2]);
    }

    #[test]
    fn monitor_keeps_the_newest_voices() {
        let monitor = Monitor::default();
        assert_eq!(monitor.voices().count(), 0);
        assert_eq!(monitor.cutoff(), None);
        let voices: Vec<VoiceState> = (0..20)
            .map(|i| VoiceState {
                runtime: 1000 + i,
                released: (i % 2 == 0).then_some(500 + i),
            })
            .collect();
        let mut modulation = [0.0; N_TARGETS];
        modulation[ModTarget::Cutoff.to_index()] = -0.25;
        let lfo = |phase: f64, cycle: u32| LfoVoice { phase, cycle, seed: 7 };
        let newest = NewestVoice {
            lfos: [lfo(0.125, 0), lfo(0.25, 1), lfo(0.375, 2)],
            lfo_values: [-0.5, 0.0, 0.75],
            modulation,
            cutoff: 1234.0,
        };
        monitor.store(voices.iter().copied(), Some(newest));
        let shown: Vec<VoiceState> = monitor.voices().collect();
        assert_eq!(shown, &voices[20 - MONITORED_VOICES..]);
        assert_eq!(monitor.lfo(2), Some(lfo(0.375, 2)));
        assert_eq!(monitor.lfo_value(0), Some(-0.5));
        assert_eq!(monitor.modulation(ModTarget::Cutoff), Some(-0.25));
        assert_eq!(monitor.cutoff(), Some(1234.0));
        // Once nothing plays, the newest voice's values are gone
        monitor.store(std::iter::empty(), None);
        assert_eq!(monitor.voices().count(), 0);
        assert_eq!(monitor.modulation(ModTarget::Cutoff), None);
    }
}
