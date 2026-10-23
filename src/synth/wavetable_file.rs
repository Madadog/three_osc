//! Wavetables loaded from WAV files.
//!
//! A file holds one or more frames (single cycles) one after the other, the way wavetable
//! editors export them: the frame length comes from the file's `clm ` chunk when it has one,
//! otherwise frames are assumed to be 2048 samples long when the file divides evenly into
//! them, and a short file without either is taken to be a single cycle. Each frame's spectrum
//! is kept (see [`LoadedSpectra`]), and the bandlimited per note tables are generated from it
//! for whatever sample rate the synth runs at.

use std::path::{Path, PathBuf};

use rustfft::FftPlanner;

use super::oscillator::{CycleRenderer, Spectrum, WavePreview, WavetableFrames};
use super::wav;

/// The usual length of a wavetable frame, assumed for files that don't say.
pub const DEFAULT_FRAME_LEN: usize = 2048;
/// The most frames a wavetable keeps; longer files are thinned out to this many.
pub const MAX_FRAMES: usize = 256;
/// The longest file that is taken to be a single cycle when its frame length is unknown.
pub const MAX_SINGLE_CYCLE: usize = 65536;
/// The longest table generated for a loaded wavetable.
pub const MAX_TABLE_LEN: usize = 2048;
/// Harmonics this far below a frame's strongest one (-100 dB) are dropped: they are below the
/// noise floor of most files, and would only make the tables longer.
pub const HARMONIC_FLOOR: f32 = 1e-5;

/// The spectra of a wavetable file's frames: everything needed to generate its tables for any
/// sample rate.
#[derive(Debug, Clone)]
pub struct LoadedSpectra {
    /// The file's name, without the directory.
    pub name: String,
    pub spectra: Vec<Spectrum>,
    /// Samples per frame in the file.
    pub frame_len: usize,
}

impl LoadedSpectra {
    /// Reads and analyses a wavetable file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("can't read the file: {error}"))?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_wav(name, &wav::parse(&bytes)?)
    }
    /// Splits decoded WAV data into frames and takes their spectra.
    pub fn from_wav(name: String, data: &wav::WavData) -> Result<Self, String> {
        let samples = &data.samples;
        if samples.len() < 2 {
            return Err(String::from("the file has no samples"));
        }
        let frame_len = match data.frame_len {
            Some(len) if len >= 2 && len <= samples.len() => len,
            Some(_) => {
                return Err(String::from("the file's frame length doesn't fit its samples"))
            }
            None if samples.len().is_multiple_of(DEFAULT_FRAME_LEN) => DEFAULT_FRAME_LEN,
            None if samples.len() <= MAX_SINGLE_CYCLE => samples.len(),
            None => {
                return Err(format!(
                    "can't tell the frame length: the file isn't a multiple of \\
                     {DEFAULT_FRAME_LEN} samples and is too long for a single cycle"
                ))
            }
        };
        let frame_count = samples.len() / frame_len;
        let fft = FftPlanner::new().plan_fft_forward(frame_len);
        // Long files are thinned out to MAX_FRAMES evenly spaced frames
        let kept = frame_count.min(MAX_FRAMES);
        let spectra = (0..kept)
            .map(|i| {
                let frame = if kept == frame_count {
                    i
                } else {
                    i * (frame_count - 1) / (kept - 1).max(1)
                };
                let mut spectrum =
                    Spectrum::from_cycle(&samples[frame * frame_len..(frame + 1) * frame_len], &*fft);
                spectrum.trim(HARMONIC_FLOOR);
                spectrum
            })
            .collect();
        Ok(Self {
            name,
            spectra,
            frame_len,
        })
    }
    /// Generates the bandlimited tables for a sample rate.
    pub fn generate(&self, sample_rate: f32) -> WavetableFrames {
        WavetableFrames::from_spectra(
            self.name.clone(),
            &self.spectra,
            sample_rate,
            MAX_TABLE_LEN,
            &mut CycleRenderer::new(),
        )
    }
    /// The file's frames, for the GUI to draw.
    pub fn preview(&self) -> WavePreview {
        WavePreview::from_spectra(&self.spectra, &mut CycleRenderer::new())
    }
    /// A one line description for the GUI.
    pub fn describe(&self, frames: &WavetableFrames) -> String {
        format!(
            "{} frame{} of {} samples, {:.1} MB",
            self.spectra.len(),
            if self.spectra.len() == 1 { "" } else { "s" },
            self.frame_len,
            frames.memory_bytes() as f64 / 1e6
        )
    }
}

/// The folder wavetable files are listed from: `Documents/Three Osc/Wavetables` in the user's
/// home directory, unless the `THREE_OSC_WAVETABLES` environment variable names another one.
pub fn default_folder() -> PathBuf {
    crate::folders::wavetables()
}

/// Resolves a saved path: relative paths (plain file names, as the GUI saves for files picked
/// from the folder) are looked up in the default folder.
pub fn resolve(path: &str) -> PathBuf {
    crate::folders::resolve(&default_folder(), path)
}

/// The WAV files in a folder, by name, sorted.
pub fn list_folder(folder: &Path) -> Vec<String> {
    crate::folders::list(folder, &["wav"])
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    /// Two frames: a sine, then a 3rd harmonic.
    fn two_frame_data(frame_len: usize, clm: bool) -> wav::WavData {
        let mut samples = Vec::new();
        for harmonic in [1.0, 3.0] {
            samples.extend(
                (0..frame_len).map(|i| (2.0 * PI * harmonic * i as f32 / frame_len as f32).sin()),
            );
        }
        wav::parse(&wav::write_float(44100, 1, &samples, clm.then_some(frame_len))).unwrap()
    }

    #[test]
    fn frames_are_found_and_analysed() {
        // Frame length from the clm chunk
        let loaded = LoadedSpectra::from_wav(String::from("a.wav"), &two_frame_data(600, true)).unwrap();
        assert_eq!(loaded.frame_len, 600);
        assert_eq!(loaded.spectra.len(), 2);
        assert!((loaded.spectra[0].bins[1].norm() - 1.0).abs() < 1e-4);
        assert!((loaded.spectra[1].bins[3].norm() - 1.0).abs() < 1e-4);
        // The spectra are trimmed to their content
        assert_eq!(loaded.spectra[0].harmonics(), 1);
        assert_eq!(loaded.spectra[1].harmonics(), 3);

        // Frames of 2048 without a chunk
        let loaded = LoadedSpectra::from_wav(String::new(), &two_frame_data(2048, false)).unwrap();
        assert_eq!(loaded.frame_len, 2048);
        assert_eq!(loaded.spectra.len(), 2);

        // Anything else short is a single cycle
        let loaded = LoadedSpectra::from_wav(String::new(), &two_frame_data(300, false)).unwrap();
        assert_eq!(loaded.frame_len, 600);
        assert_eq!(loaded.spectra.len(), 1);

        // Too long to guess
        let long = wav::WavData {
            sample_rate: 44100,
            channels: 1,
            samples: vec![0.0; MAX_SINGLE_CYCLE + 1],
            frame_len: None,
        };
        assert!(LoadedSpectra::from_wav(String::new(), &long).is_err());
        let empty = wav::WavData {
            samples: vec![],
            ..long.clone()
        };
        assert!(LoadedSpectra::from_wav(String::new(), &empty).is_err());
        let bad_chunk = wav::WavData {
            frame_len: Some(1 << 20),
            ..long
        };
        assert!(LoadedSpectra::from_wav(String::new(), &bad_chunk).is_err());
    }

    #[test]
    fn long_files_are_thinned_out() {
        let frame_len = 64;
        let samples: Vec<f32> = (0..frame_len * 1000)
            .map(|i| (2.0 * PI * i as f32 / frame_len as f32).sin())
            .collect();
        let data = wav::parse(&wav::write_float(44100, 1, &samples, Some(frame_len))).unwrap();
        let loaded = LoadedSpectra::from_wav(String::new(), &data).unwrap();
        assert_eq!(loaded.spectra.len(), MAX_FRAMES);
    }

    #[test]
    fn loaded_wavetables_generate_playable_tables() {
        let loaded = LoadedSpectra::from_wav(String::from("test.wav"), &two_frame_data(600, true)).unwrap();
        let frames = loaded.generate(44100.0);
        assert_eq!(frames.frames.len(), 2);
        assert_eq!(frames.name, "test.wav");
        // The first frame is a sine...
        let table = frames.table(0, 60);
        assert!((table.generate(0.25) - 1.0).abs() < 1e-3);
        assert!(table.generate(0.5).abs() < 1e-3);
        // ...and the second its third harmonic, which the top notes can't hold
        let table = frames.table(1, 60);
        assert!((table.generate(1.0 / 12.0) - 1.0).abs() < 1e-3);
        assert!(frames.table(1, 130).generate(1.0 / 12.0).abs() < 1e-3);
        assert!(loaded.describe(&frames).starts_with("2 frames of 600 samples"));
    }

    #[test]
    fn files_are_loaded_from_disk_and_listed() {
        let dir = std::env::temp_dir().join(format!("three_osc_wavetables_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = two_frame_data(600, true);
        let path = dir.join("b.wav");
        std::fs::write(&path, wav::write_float(44100, 1, &data.samples, Some(600))).unwrap();
        std::fs::write(dir.join("a.WAV"), wav::write_float(44100, 1, &data.samples, None)).unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a wave").unwrap();

        let loaded = LoadedSpectra::load(&path).unwrap();
        assert_eq!(loaded.name, "b.wav");
        assert_eq!(loaded.spectra.len(), 2);
        assert!(LoadedSpectra::load(&dir.join("notes.txt")).is_err());
        assert!(LoadedSpectra::load(&dir.join("missing.wav")).is_err());
        assert_eq!(list_folder(&dir), vec!["a.WAV", "b.wav"]);
        assert!(list_folder(&dir.join("missing")).is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relative_paths_resolve_to_the_folder() {
        assert_eq!(resolve("saw.wav"), default_folder().join("saw.wav"));
        let absolute = std::env::temp_dir().join("x.wav");
        assert_eq!(resolve(absolute.to_str().unwrap()), absolute);
    }
}
