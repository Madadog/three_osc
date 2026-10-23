//! What the audio thread shows the GUI of the notes that are playing, so that the displays can
//! follow them: where each voice is in its envelopes, and the newest voice's LFO phases,
//! modulation and filter cutoff.
//!
//! The audio thread writes it after each buffer while the GUI is open (see
//! [`ThreeOsc::publish()`][super::ThreeOsc::publish()]), and the GUI reads it as it draws. The
//! values are separate atomics, so a frame can mix values from two buffers, which doesn't
//! matter for drawing; each voice's own values are written together, so they always match.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use nih_plug::prelude::Enum;

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
    /// The newest voice's LFO phases, in cycles.
    lfo_phases: [AtomicU32; N_LFOS],
    /// The newest voice's modulation of every target.
    modulation: [AtomicU32; N_TARGETS],
    /// The newest voice's filter cutoff, in Hz.
    cutoff: AtomicU32,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            voices: std::array::from_fn(|_| AtomicU64::new(0)),
            voice_count: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            lfo_phases: std::array::from_fn(|_| AtomicU32::new(0)),
            modulation: std::array::from_fn(|_| AtomicU32::new(0)),
            cutoff: AtomicU32::new(0),
        }
    }
}

/// The newest voice's values, for [`Monitor::store()`].
pub struct NewestVoice {
    pub lfo_phases: [f32; N_LFOS],
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
            for (slot, phase) in self.lfo_phases.iter().zip(newest.lfo_phases) {
                slot.store(phase.to_bits(), Ordering::Relaxed);
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

    /// The newest voice's phase of an LFO, in cycles.
    pub fn lfo_phase(&self, lfo: usize) -> Option<f32> {
        self.newest(&self.lfo_phases[lfo])
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let newest = NewestVoice {
            lfo_phases: [0.1, 0.2, 0.3],
            modulation,
            cutoff: 1234.0,
        };
        monitor.store(voices.iter().copied(), Some(newest));
        let shown: Vec<VoiceState> = monitor.voices().collect();
        assert_eq!(shown, &voices[20 - MONITORED_VOICES..]);
        assert_eq!(monitor.lfo_phase(2), Some(0.3));
        assert_eq!(monitor.modulation(ModTarget::Cutoff), Some(-0.25));
        assert_eq!(monitor.cutoff(), Some(1234.0));
        // Once nothing plays, the newest voice's values are gone
        monitor.store(std::iter::empty(), None);
        assert_eq!(monitor.voices().count(), 0);
        assert_eq!(monitor.modulation(ModTarget::Cutoff), None);
    }
}
