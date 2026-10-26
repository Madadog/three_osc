//! The output section: a spectrogram of the synth's output and a level meter for each channel,
//! worked out in the GUI from what the audio thread writes to the [`OutputTap`].

use std::sync::Arc;

use nih_plug_egui::egui::{
    self, Color32, ColorImage, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::synth::monitor::OutputTap;

/// The spectrogram's frequency range, in Hz (up to the Nyquist frequency at low rates).
const LOWEST_HZ: f32 = 20.0;
const HIGHEST_HZ: f32 = 20000.0;
/// The quietest level the spectrogram shows, in dB; anything quieter is the background.
const FLOOR_DB: f32 = -90.0;
/// How long each analysis window is, in seconds (rounded to a power of two samples), and how
/// many windows overlap.
const WINDOW_SECONDS: f32 = 0.085;
const OVERLAP: usize = 4;
/// The most columns the spectrogram adds in one frame, after a stall.
const MAX_COLUMNS_PER_FRAME: usize = 64;

/// The meter's range, in dBFS, and how much more room it gives the loud end (see
/// [`meter_fraction()`]).
const METER_FLOOR_DB: f32 = -60.0;
const METER_TOP_DB: f32 = 6.0;
const METER_CURVE: f32 = 1.6;
/// How fast the meter's bar falls from a peak, in dB per second.
const METER_FALL_DB_PER_SECOND: f32 = 20.0;
/// How long the meter holds its highest peak, in seconds.
const PEAK_HOLD_SECONDS: f64 = 1.5;
/// The time the RMS level is averaged over, in seconds.
const RMS_SECONDS: f32 = 0.3;

/// The output section's help.
pub const HELP: &str = "What the synth plays, after the effects. The spectrogram shows the last \
     few seconds, the newest at the right: frequency goes up the side (20 Hz to 20 kHz), and the \
     brighter, the louder. Hover it to read a frequency. Under it is the level of each channel.";
const METER_HELP: &str = "The level of each channel, in dB below full scale: the bright bar is \
     the RMS level (averaged over 300 ms), the dim one the peaks, and the line the highest \
     peak lately. The number is that highest peak; it turns red when the output clips (goes \
     over 0 dB), until it's clicked.";

/// What the output section keeps from frame to frame.
pub struct Analyser {
    /// How much of the output has been read.
    position: u64,
    /// The new samples of each channel, read this frame.
    left: Vec<f32>,
    right: Vec<f32>,
    meters: [Meter; 2],
    /// Whether the output has clipped since the meter was last clicked.
    clipped: bool,
    spectrogram: Option<Spectrogram>,
}

impl Analyser {
    pub fn new() -> Self {
        Self {
            position: 0,
            left: Vec::new(),
            right: Vec::new(),
            meters: [Meter::default(), Meter::default()],
            clipped: false,
            spectrogram: None,
        }
    }

    /// Reads what the synth has played since the last frame, `dt` seconds ago.
    fn update(&mut self, tap: &OutputTap, sample_rate: f32, time: f64, dt: f32) {
        self.left.clear();
        self.right.clear();
        self.position = tap.read(self.position, &mut self.left, &mut self.right);
        for (meter, samples) in self.meters.iter_mut().zip([&self.left, &self.right]) {
            meter.update(samples, sample_rate, time, dt);
            self.clipped |= meter.peak > 1.0;
        }
        let spectrogram = match &mut self.spectrogram {
            Some(spectrogram) if spectrogram.sample_rate == sample_rate => spectrogram,
            spectrogram => spectrogram.insert(Spectrogram::new(sample_rate)),
        };
        spectrogram.push(self.left.iter().zip(&self.right).map(|(left, right)| (left + right) * 0.5));
    }
}

impl Default for Analyser {
    fn default() -> Self {
        Self::new()
    }
}

/// One channel's level.
#[derive(Default)]
struct Meter {
    /// The peak level of the newest samples, linear.
    peak: f32,
    /// The level the bar shows, which falls back slowly from the peaks.
    level: f32,
    /// The highest peak lately, and when it was.
    hold: f32,
    hold_time: f64,
    /// The mean of the squared samples, over about [`RMS_SECONDS`].
    mean_square: f32,
}

impl Meter {
    fn update(&mut self, samples: &[f32], sample_rate: f32, time: f64, dt: f32) {
        self.peak = samples.iter().fold(0.0, |peak, sample| peak.max(sample.abs()));
        let fall = db_to_gain(-METER_FALL_DB_PER_SECOND * dt);
        self.level = self.peak.max(self.level * fall);
        if self.peak >= self.hold || time - self.hold_time > PEAK_HOLD_SECONDS {
            self.hold = self.peak.max(self.level);
            self.hold_time = time;
        }
        let rate = 1.0 - (-1.0 / (RMS_SECONDS * sample_rate)).exp();
        for sample in samples {
            self.mean_square += rate * (sample * sample - self.mean_square);
        }
        // Not to be stuck at a denormal level after the output goes quiet
        if self.mean_square < 1e-20 {
            self.mean_square = 0.0;
        }
    }
}

/// How far along the meter a level in dBFS is: the scale is stretched at the loud end, where
/// the detail matters.
fn meter_fraction(db: f32) -> f32 {
    ((db - METER_FLOOR_DB) / (METER_TOP_DB - METER_FLOOR_DB)).clamp(0.0, 1.0).powf(METER_CURVE)
}

fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-12).log10()
}

/// The spectrogram's analysis and the image it draws.
struct Spectrogram {
    sample_rate: f32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Scales a bin's magnitude so that a full scale sine shows at 0 dB.
    scale: f32,
    hop: usize,
    /// Samples waiting to be analysed: the end of the last window and what has come since.
    pending: Vec<f32>,
    buffer: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// The newest windows' levels per bin in dB, oldest first, waiting to be drawn.
    columns: Vec<Vec<f32>>,
    /// The image, one column per window, the newest at the right, and its texture.
    image: ColorImage,
    texture: Option<TextureHandle>,
    /// For each row of the image (from the top), the bins it covers.
    rows: Vec<RowBins>,
    palette: [Color32; 256],
}

/// The bins a row of the spectrogram shows: the loudest of `first..=last`, or (for rows
/// narrower than a bin) a blend of the two bins around its frequency.
#[derive(Clone, Copy)]
enum RowBins {
    Range { first: usize, last: usize },
    Between { bin: usize, fraction: f32 },
}

impl Spectrogram {
    fn new(sample_rate: f32) -> Self {
        let size = ((sample_rate * WINDOW_SECONDS) as usize).next_power_of_two().max(256);
        // A Hann window
        let window: Vec<f32> = (0..size)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / size as f32).cos())
            .collect();
        let fft = FftPlanner::new().plan_fft_forward(size);
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        Self {
            sample_rate,
            scale: 2.0 / window.iter().sum::<f32>(),
            window,
            fft,
            hop: size / OVERLAP,
            // Starts with silence, so that the first column comes after one hop
            pending: vec![0.0; size - size / OVERLAP],
            buffer: vec![Complex32::default(); size],
            scratch,
            columns: Vec::new(),
            image: ColorImage::new([0, 0], Color32::BLACK),
            texture: None,
            rows: Vec::new(),
            palette: std::array::from_fn(|i| heat(i as f32 / 255.0)),
        }
    }

    /// Adds samples, analysing every window that completes.
    fn push(&mut self, samples: impl Iterator<Item = f32>) {
        self.pending.extend(samples);
        let size = self.window.len();
        // After a stall, only the newest windows are worth analysing
        let most = size + self.hop * MAX_COLUMNS_PER_FRAME;
        if self.pending.len() > most {
            self.pending.drain(..self.pending.len() - most);
        }
        while self.pending.len() >= size {
            for ((value, sample), weight) in self.buffer.iter_mut().zip(&self.pending).zip(&self.window) {
                *value = Complex32::new(sample * weight, 0.0);
            }
            self.fft.process_with_scratch(&mut self.buffer, &mut self.scratch);
            let column = self.buffer[..size / 2 + 1]
                .iter()
                .map(|bin| gain_to_db(bin.norm() * self.scale))
                .collect();
            self.columns.push(column);
            self.pending.drain(..self.hop);
        }
        // (They are drawn every frame, but just in case they weren't)
        if self.columns.len() > MAX_COLUMNS_PER_FRAME {
            self.columns.drain(..self.columns.len() - MAX_COLUMNS_PER_FRAME);
        }
    }

    /// Works out which bins each of `height` rows shows.
    fn layout_rows(&mut self, height: usize) {
        let size = self.window.len();
        let bin_hz = self.sample_rate / size as f32;
        let highest = HIGHEST_HZ.min(self.sample_rate * 0.5);
        let ratio = LOWEST_HZ / highest;
        // The frequency at a fraction of the way down from the top
        let frequency = |fraction: f32| highest * ratio.powf(fraction);
        let last_bin = size / 2;
        self.rows = (0..height)
            .map(|row| {
                let top = frequency(row as f32 / height as f32) / bin_hz;
                let bottom = frequency((row + 1) as f32 / height as f32) / bin_hz;
                let (first, last) = (bottom.ceil() as usize, (top.floor() as usize).min(last_bin));
                if first <= last {
                    RowBins::Range { first, last }
                } else {
                    let centre = frequency((row as f32 + 0.5) / height as f32) / bin_hz;
                    let bin = (centre.floor() as usize).min(last_bin - 1);
                    RowBins::Between { bin, fraction: centre - bin as f32 }
                }
            })
            .collect();
    }

    /// The colour of each row for a column of levels.
    fn colors<'a>(&'a self, column: &'a [f32]) -> impl Iterator<Item = Color32> + 'a {
        self.rows.iter().map(move |row| {
            let db = match *row {
                RowBins::Range { first, last } => column[first..=last].iter().copied().fold(f32::MIN, f32::max),
                RowBins::Between { bin, fraction } => column[bin] + (column[bin + 1] - column[bin]) * fraction,
            };
            let level = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
            self.palette[(level * 255.0) as usize]
        })
    }

    /// Draws the new columns into the image, which is resized to `width` by `height` pixels
    /// (and cleared) when the display changes size, and returns its texture.
    fn texture(&mut self, ctx: &egui::Context, width: usize, height: usize) -> &TextureHandle {
        if self.image.size != [width, height] {
            self.image = ColorImage::new([width, height], self.palette[0]);
            self.layout_rows(height);
        }
        let columns = std::mem::take(&mut self.columns);
        let new = columns.len().min(width);
        if new > 0 {
            // Scroll left, and draw the new columns at the right
            for row in self.image.pixels.chunks_exact_mut(width) {
                row.copy_within(new.., 0);
            }
            for (x, column) in (width - new..width).zip(&columns[columns.len() - new..]) {
                let colors: Vec<Color32> = self.colors(column).collect();
                for (y, color) in colors.into_iter().enumerate() {
                    self.image.pixels[y * width + x] = color;
                }
            }
        }
        match &mut self.texture {
            Some(texture) => {
                if new > 0 {
                    texture.set(self.image.clone(), TextureOptions::NEAREST);
                }
            }
            None => {
                self.texture = Some(ctx.load_texture("spectrogram", self.image.clone(), TextureOptions::NEAREST));
            }
        }
        self.texture.as_ref().unwrap()
    }

    /// The frequency at a fraction of the way down the display.
    fn frequency_at(&self, fraction: f32) -> f32 {
        let highest = HIGHEST_HZ.min(self.sample_rate * 0.5);
        highest * (LOWEST_HZ / highest).powf(fraction)
    }
}

/// The spectrogram's colours, from silence (the display's background) through blue and the
/// modulation orange to white.
fn heat(level: f32) -> Color32 {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [12.0, 14.0, 22.0]),
        (0.35, [24.0, 58.0, 135.0]),
        (0.6, [70.0, 150.0, 240.0]),
        (0.82, [255.0, 170.0, 60.0]),
        (1.0, [255.0, 248.0, 225.0]),
    ];
    let upper = STOPS.iter().position(|(at, _)| *at >= level).unwrap_or(STOPS.len() - 1).max(1);
    let (from, to) = (STOPS[upper - 1], STOPS[upper]);
    let t = ((level - from.0) / (to.0 - from.0)).clamp(0.0, 1.0);
    let channel = |i: usize| (from.1[i] + (to.1[i] - from.1[i]) * t).round() as u8;
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

/// Draws the spectrogram and the meters into an area `width` wide and `height` tall, after
/// reading what the synth has played since the last frame.
pub fn output(ui: &mut Ui, analyser: &mut Analyser, tap: &OutputTap, sample_rate: f32, width: f32, height: f32) {
    let (time, dt) = ui.input(|input| (input.time, input.stable_dt.min(0.25)));
    analyser.update(tap, sample_rate, time, dt);
    // The meters take two rows and their scale under the spectrogram
    let meter_height = 46.0;
    let spectrogram_height = (height - meter_height - ui.spacing().item_spacing.y).max(40.0);
    spectrogram(ui, analyser, width, spectrogram_height);
    meters(ui, analyser, width, meter_height);
}

fn spectrogram(ui: &mut Ui, analyser: &mut Analyser, width: f32, height: f32) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let spectrogram = analyser.spectrogram.as_mut().expect("updated before drawing");
    let pixels_per_point = ui.ctx().pixels_per_point();
    let inner = rect.shrink(1.0);
    let texture = spectrogram.texture(
        ui.ctx(),
        (inner.width() * pixels_per_point).round().max(1.0) as usize,
        (inner.height() * pixels_per_point).round().max(1.0) as usize,
    );
    let painter = ui.painter_at(rect);
    painter.image(texture.id(), inner, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, super::displays::palette(ui.ctx()).edge), StrokeKind::Inside);

    // Guides at 100 Hz, 1 kHz and 10 kHz
    let highest = HIGHEST_HZ.min(spectrogram.sample_rate * 0.5);
    let y_of = |hz: f32| inner.top() + inner.height() * (highest / hz).ln() / (highest / LOWEST_HZ).ln();
    for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        if hz < highest {
            let y = y_of(hz);
            painter.line_segment(
                [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
                Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
            );
            // On a dark patch, to read over the brightest parts
            let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(10.0), Color32::from_white_alpha(170));
            let text = Rect::from_min_size(Pos2::new(inner.left() + 3.0, y - 1.0 - galley.size().y), galley.size());
            painter.rect_filled(text.expand2(egui::vec2(2.0, 0.0)), 2.0, Color32::from_black_alpha(150));
            painter.galley(text.min, galley, Color32::WHITE);
        }
    }
    // What frequency the pointer is at
    if let Some(pos) = response.hover_pos() {
        let hz = spectrogram.frequency_at(((pos.y - inner.top()) / inner.height()).clamp(0.0, 1.0));
        let text = if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") };
        response.on_hover_text_at_pointer(text);
    }
}

fn meters(ui: &mut Ui, analyser: &mut Analyser, width: f32, height: f32) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
    if response.clicked() {
        analyser.clipped = false;
        for meter in analyser.meters.iter_mut() {
            meter.hold = 0.0;
        }
    }
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let screen = super::displays::palette(ui.ctx()).screen;
    let label_width = 12.0;
    let readout_width = 38.0;
    let scale = Rect::from_min_max(
        Pos2::new(rect.left() + label_width, rect.top()),
        Pos2::new(rect.right() - readout_width, rect.bottom()),
    );
    let x_of = |db: f32| scale.left() + meter_fraction(db) * scale.width();
    let bar_height = 9.0;
    let bar_color = |db: f32| {
        if db > 0.0 {
            Color32::from_rgb(235, 70, 60)
        } else if db > -6.0 {
            Color32::from_rgb(255, 170, 60)
        } else {
            Color32::from_rgb(70, 150, 240)
        }
    };
    for (i, (meter, name)) in analyser.meters.iter().zip(["L", "R"]).enumerate() {
        let top = rect.top() + 2.0 + i as f32 * (bar_height + 3.0);
        let bar = Rect::from_min_max(Pos2::new(scale.left(), top), Pos2::new(scale.right(), top + bar_height));
        painter.text(
            Pos2::new(rect.left(), bar.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(10.0),
            visuals.weak_text_color(),
        );
        painter.rect_filled(bar, 2.0, screen);
        let peak_db = gain_to_db(meter.level);
        let rms_db = gain_to_db(meter.mean_square.sqrt());
        if peak_db > METER_FLOOR_DB {
            let peak = Rect::from_min_max(bar.left_top(), Pos2::new(x_of(peak_db), bar.bottom()));
            painter.rect_filled(peak, 2.0, bar_color(peak_db).gamma_multiply(0.45));
        }
        if rms_db > METER_FLOOR_DB {
            let rms = Rect::from_min_max(bar.left_top(), Pos2::new(x_of(rms_db), bar.bottom()));
            painter.rect_filled(rms, 2.0, bar_color(rms_db));
        }
        let hold_db = gain_to_db(meter.hold);
        if hold_db > METER_FLOOR_DB {
            let x = x_of(hold_db);
            painter.line_segment([Pos2::new(x, bar.top()), Pos2::new(x, bar.bottom())], Stroke::new(1.5_f32, bar_color(hold_db)));
        }
        // 0 dB
        let x = x_of(0.0);
        painter.line_segment([Pos2::new(x, bar.top()), Pos2::new(x, bar.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(60)));
    }

    // The scale under the bars
    let ticks_top = rect.top() + 2.0 + 2.0 * (bar_height + 3.0);
    for db in [-48.0, -24.0, -12.0, -6.0, 0.0] {
        let x = x_of(db);
        painter.line_segment([Pos2::new(x, ticks_top), Pos2::new(x, ticks_top + 3.0)], Stroke::new(1.0_f32, visuals.weak_text_color()));
        // (-6 is too close to its neighbours for a number)
        if db != -6.0 {
            painter.text(
                Pos2::new(x, ticks_top + 3.0),
                egui::Align2::CENTER_TOP,
                format!("{}", db.abs() as i32),
                egui::FontId::proportional(9.0),
                visuals.weak_text_color(),
            );
        }
    }

    // The highest peak of the two channels, red once the output has clipped
    let hold = analyser.meters.iter().fold(0.0_f32, |hold, meter| hold.max(meter.hold));
    let readout = Rect::from_min_max(Pos2::new(rect.right() - readout_width + 4.0, rect.top() + 2.0), Pos2::new(rect.right(), rect.top() + 2.0 + 2.0 * bar_height + 3.0));
    if analyser.clipped {
        painter.rect_filled(readout, 2.0, Color32::from_rgb(170, 40, 35));
    }
    let text = if hold > 0.0 && gain_to_db(hold) > METER_FLOOR_DB {
        format!("{:.1}", gain_to_db(hold))
    } else {
        String::from("-inf")
    };
    painter.text(
        readout.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::monospace(10.0),
        if analyser.clipped { Color32::WHITE } else { visuals.text_color() },
    );
    response.on_hover_text(METER_HELP);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meters_follow_the_level() {
        let mut meter = Meter::default();
        let sine: Vec<f32> = (0..4800).map(|i| 0.5 * (i as f32 * 0.1).sin()).collect();
        meter.update(&sine, 48000.0, 0.0, 0.1);
        assert!((meter.peak - 0.5).abs() < 1e-3);
        assert_eq!(meter.level, meter.peak);
        // A tenth of a second of a sine is a third of the way to its RMS level
        let rms = meter.mean_square.sqrt();
        assert!(rms > 0.1 && rms < 0.5 / 2.0_f32.sqrt(), "{rms}");
        // Silence lets the bar fall at its rate, and the hold stays for a while
        meter.update(&[0.0; 480], 48000.0, 0.5, 0.5);
        assert!((gain_to_db(meter.level) - (gain_to_db(0.5) - METER_FALL_DB_PER_SECOND * 0.5)).abs() < 0.01);
        assert_eq!(meter.hold, 0.5);
        meter.update(&[0.0; 480], 48000.0, 2.5, 0.5);
        assert!(meter.hold < 0.5);
    }

    #[test]
    fn spectrograms_find_a_sine_at_its_level() {
        let sample_rate = 48000.0;
        let mut spectrogram = Spectrogram::new(sample_rate);
        let size = spectrogram.window.len();
        assert_eq!(size, 4096);
        let hz = 1000.0;
        let sine = (0..size * 2).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / sample_rate).sin());
        spectrogram.push(sine);
        assert!(spectrogram.columns.len() >= 4);
        let column = spectrogram.columns.last().unwrap().clone();
        let (loudest, db) = column
            .iter()
            .enumerate()
            .fold((0, f32::MIN), |best, (bin, db)| if *db > best.1 { (bin, *db) } else { best });
        assert_eq!(loudest, (hz * size as f32 / sample_rate).round() as usize);
        // Half of full scale is -6 dB, give or take the window's scalloping
        assert!((db - gain_to_db(0.5)).abs() < 1.5, "{db}");

        // The rows cover the range top to bottom, and the loudest row is near 1 kHz
        spectrogram.layout_rows(200);
        let colors: Vec<Color32> = spectrogram.colors(&column).collect();
        let brightest = (0..colors.len()).max_by_key(|&row| colors[row].r() as u32 + colors[row].g() as u32 + colors[row].b() as u32).unwrap();
        let found = spectrogram.frequency_at((brightest as f32 + 0.5) / 200.0);
        assert!((found / hz).log2().abs() < 0.1, "{found}");
    }

    #[test]
    fn the_heat_palette_runs_from_dark_to_light() {
        let brightness = |color: Color32| color.r() as u32 + color.g() as u32 + color.b() as u32;
        let palette: Vec<u32> = (0..=10).map(|i| brightness(heat(i as f32 / 10.0))).collect();
        assert!(palette.windows(2).all(|pair| pair[0] <= pair[1]), "{palette:?}");
        assert_eq!(heat(0.0), Color32::from_rgb(12, 14, 22));
    }

    #[test]
    fn the_meter_gives_the_loud_end_more_room() {
        assert_eq!(meter_fraction(METER_FLOOR_DB - 10.0), 0.0);
        assert_eq!(meter_fraction(METER_TOP_DB + 10.0), 1.0);
        let step = |from: f32| meter_fraction(from + 6.0) - meter_fraction(from);
        assert!(step(-6.0) > step(-30.0) && step(-30.0) > step(-54.0));
    }
}
