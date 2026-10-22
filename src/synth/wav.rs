//! A small reader for the WAV files wavetables come in.
//!
//! It handles what wavetable files use in practice: PCM (8, 16, 24 and 32-bit) and IEEE float
//! (32 and 64-bit) samples, plain and `WAVE_FORMAT_EXTENSIBLE` headers, any number of channels
//! (which are averaged), and the `clm ` chunk some wavetable editors write to record the number
//! of samples per frame.

/// The contents of a WAV file, mixed down to one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct WavData {
    pub sample_rate: u32,
    pub channels: u16,
    /// The samples of the first channel, or the average of all channels, from -1 to 1.
    pub samples: Vec<f32>,
    /// Samples per wavetable frame, if the file's `clm ` chunk says so.
    pub frame_len: Option<usize>,
}

/// Parses a WAV file.
pub fn parse(bytes: &[u8]) -> Result<WavData, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(String::from("not a WAV file"));
    }
    let mut format: Option<Format> = None;
    let mut data: Option<&[u8]> = None;
    let mut frame_len = None;

    // Walk the chunks. Each one is an id, a little endian size, then the data, padded to an
    // even length.
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        // A truncated final chunk is read as far as it goes
        let end = (start + size).min(bytes.len());
        let chunk = &bytes[start..end];
        match id {
            b"fmt " => format = Some(Format::parse(chunk)?),
            b"data" => data = Some(chunk),
            b"clm " => frame_len = parse_clm(chunk),
            _ => {}
        }
        offset = start + size + (size % 2);
    }

    let format = format.ok_or("WAV file has no format chunk")?;
    let data = data.ok_or("WAV file has no data chunk")?;
    let channels = format.channels.max(1) as usize;
    let bytes_per_sample = (format.bits_per_sample as usize).div_ceil(8);
    if bytes_per_sample == 0 {
        return Err(String::from("WAV file has zero bits per sample"));
    }
    let frame_bytes = bytes_per_sample * channels;
    let frames = data.len() / frame_bytes;
    let mut samples = Vec::with_capacity(frames);
    for frame in data.chunks_exact(frame_bytes) {
        let mut sum = 0.0;
        for sample in frame.chunks_exact(bytes_per_sample) {
            sum += format.decode(sample)?;
        }
        samples.push(sum / channels as f32);
    }
    Ok(WavData {
        sample_rate: format.sample_rate,
        channels: format.channels,
        samples,
        frame_len,
    })
}

/// The `fmt ` chunk.
struct Format {
    /// 1 for PCM, 3 for IEEE float.
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xfffe;

impl Format {
    fn parse(chunk: &[u8]) -> Result<Self, String> {
        if chunk.len() < 16 {
            return Err(String::from("WAV format chunk is too short"));
        }
        let u16_at = |at: usize| u16::from_le_bytes([chunk[at], chunk[at + 1]]);
        let mut tag = u16_at(0);
        let channels = u16_at(2);
        let sample_rate = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let bits_per_sample = u16_at(14);
        if tag == FORMAT_EXTENSIBLE {
            // The real format tag is the start of the sub format GUID, which follows the
            // extension size, the valid bits and the channel mask.
            if chunk.len() < 26 {
                return Err(String::from("WAV extensible format chunk is too short"));
            }
            tag = u16_at(24);
        }
        match (tag, bits_per_sample) {
            (FORMAT_PCM, 8 | 16 | 24 | 32) | (FORMAT_FLOAT, 32 | 64) => Ok(Self {
                tag,
                channels,
                sample_rate,
                bits_per_sample,
            }),
            _ => Err(format!(
                "unsupported WAV format: format tag {tag}, {bits_per_sample} bits per sample"
            )),
        }
    }
    /// Decodes one sample to a float from -1 to 1.
    fn decode(&self, bytes: &[u8]) -> Result<f32, String> {
        Ok(match (self.tag, self.bits_per_sample) {
            (FORMAT_PCM, 8) => (bytes[0] as f32 - 128.0) / 128.0,
            (FORMAT_PCM, 16) => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0,
            (FORMAT_PCM, 24) => {
                i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) as f32 / 2_147_483_648.0
            }
            (FORMAT_PCM, 32) => {
                i32::from_le_bytes(bytes.try_into().unwrap()) as f32 / 2_147_483_648.0
            }
            (FORMAT_FLOAT, 32) => f32::from_le_bytes(bytes.try_into().unwrap()),
            (FORMAT_FLOAT, 64) => f64::from_le_bytes(bytes.try_into().unwrap()) as f32,
            _ => return Err(String::from("unsupported WAV sample format")),
        })
    }
}

/// Reads the frame length from a `clm ` chunk, which looks like
/// `<!>2048 01000000 wavetable (www.serum.com)`.
fn parse_clm(chunk: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(chunk).ok()?;
    let digits: String = text
        .trim_start_matches("<!>")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok().filter(|&len| len > 0)
}

/// Writes a WAV file with one or more channels of 32-bit float samples, interleaved, with an
/// optional `clm ` chunk. Only used by the tests and tools.
#[cfg(test)]
pub fn write_float(sample_rate: u32, channels: u16, samples: &[f32], frame_len: Option<usize>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&0_u32.to_le_bytes()); // patched below
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_FLOAT.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 4 * channels as u32).to_le_bytes());
    out.extend_from_slice(&(4 * channels).to_le_bytes());
    out.extend_from_slice(&32_u16.to_le_bytes());
    if let Some(frame_len) = frame_len {
        let text = format!("<!>{frame_len} 01000000 wavetable (www.serum.com)");
        out.extend_from_slice(b"clm ");
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        if text.len() % 2 == 1 {
            out.push(0);
        }
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&((samples.len() * 4) as u32).to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    let riff_size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PCM file, written by hand, with the given bits per sample.
    fn pcm_file(bits: u16, extensible: bool, samples: &[i32]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        let fmt_len: u32 = if extensible { 40 } else { 16 };
        out.extend_from_slice(&fmt_len.to_le_bytes());
        let tag = if extensible { FORMAT_EXTENSIBLE } else { FORMAT_PCM };
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&44100_u32.to_le_bytes());
        out.extend_from_slice(&(44100 * bits as u32 / 8).to_le_bytes());
        out.extend_from_slice(&(bits / 8).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        if extensible {
            out.extend_from_slice(&22_u16.to_le_bytes());
            out.extend_from_slice(&bits.to_le_bytes());
            out.extend_from_slice(&0_u32.to_le_bytes());
            out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
            out.extend_from_slice(&[0; 14]);
        }
        // An unknown chunk with an odd size, which is padded
        out.extend_from_slice(b"junk");
        out.extend_from_slice(&3_u32.to_le_bytes());
        out.extend_from_slice(&[1, 2, 3, 0]);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(samples.len() as u32 * bits as u32 / 8).to_le_bytes());
        for &sample in samples {
            match bits {
                8 => out.push((sample + 128) as u8),
                16 => out.extend_from_slice(&(sample as i16).to_le_bytes()),
                24 => out.extend_from_slice(&sample.to_le_bytes()[..3]),
                _ => out.extend_from_slice(&sample.to_le_bytes()),
            }
        }
        out
    }

    #[test]
    fn reads_pcm_formats() {
        for (bits, extensible, full_scale) in [
            (8, false, 128.0),
            (16, false, 32768.0),
            (24, true, 8_388_608.0),
            (32, true, 2_147_483_648.0),
        ] {
            let values = [0, 1, -1, (full_scale / 2.0) as i32, -(full_scale as i32)];
            let data = parse(&pcm_file(bits, extensible, &values)).unwrap();
            assert_eq!(data.sample_rate, 44100);
            assert_eq!(data.channels, 1);
            assert_eq!(data.frame_len, None);
            let expected: Vec<f32> = values.iter().map(|&v| v as f32 / full_scale).collect();
            assert_eq!(data.samples, expected, "{bits} bits");
        }
    }

    #[test]
    fn reads_float_files_with_channels_and_frame_length() {
        let samples = [0.5, -0.5, 0.25, 0.75, -1.0, 1.0];
        let bytes = write_float(48000, 2, &samples, Some(2048));
        let data = parse(&bytes).unwrap();
        assert_eq!(data.sample_rate, 48000);
        assert_eq!(data.channels, 2);
        assert_eq!(data.frame_len, Some(2048));
        assert_eq!(data.samples, vec![0.0, 0.5, 0.0]);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(parse(b"not a wav file at all").is_err());
        assert!(parse(&write_float(44100, 1, &[], None)[..20]).is_err());
        let mut bytes = write_float(44100, 1, &[0.0], None);
        // 12-bit samples aren't supported
        bytes[34] = 12;
        assert!(parse(&bytes).is_err());
    }

    #[test]
    fn parses_clm_chunks() {
        assert_eq!(parse_clm(b"<!>2048 01000000 wavetable (www.serum.com)"), Some(2048));
        assert_eq!(parse_clm(b"<!>600"), Some(600));
        assert_eq!(parse_clm(b"<!>0"), None);
        assert_eq!(parse_clm(b"nonsense"), None);
    }
}
