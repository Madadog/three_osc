//! Oversampling: the voices can run at two, four or eight times the sample rate, so that the
//! sidebands of frequency, phase and amplitude modulation and the filters' distortion above the
//! Nyquist frequency are filtered out rather than folding back down as aliasing. (The
//! wavetables themselves are bandlimited to the sample rate's Nyquist frequency, so on their
//! own they sound the same either way.)
//!
//! The voices' output comes back down to the sample rate through a cascade of halfband
//! decimators, a halving at a time: polyphase IIR filters made of two chains of first order
//! allpass filters, after Laurent de Soras's HIIR library. They keep the band up to 20 kHz (at
//! 44.1 kHz, and proportionally at other rates) flat, keep anything that would fold back into
//! it at least 100 dB down, and delay it by 1.5 (from twice the rate) to 2.4 samples (from eight
//! times).

use wide::f32x4;

/// How many times the sample rate the voices run at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Oversampling {
    #[default]
    Off,
    Two,
    Four,
    Eight,
}

impl Oversampling {
    pub fn factor(self) -> usize {
        1 << self.halvings()
    }
    /// The number of halving stages that bring the voices' rate back down.
    fn halvings(self) -> usize {
        match self {
            Self::Off => 0,
            Self::Two => 1,
            Self::Four => 2,
            Self::Eight => 3,
        }
    }
}

/// The band kept flat, as a fraction of the sample rate: 20 kHz at 44.1 kHz.
const PASSBAND: f64 = 20000.0 / 44100.0;
/// The coefficients of each halving stage, the last (from twice the sample rate, which needs
/// the steepest filter) first: enough for every stage to keep what would fold back into the
/// passband at least 100 dB down. Even numbers, so that the two chains of filters pair up.
const STAGE_COEFFICIENTS: [usize; 3] = [10, 6, 6];
/// The most pairs of filters a stage has.
const MAX_PAIRS: usize = 5;

/// The coefficients of a polyphase IIR halfband filter with `count` coefficients, whose
/// transition band is `transition` (as a fraction of its input rate) either side of a quarter
/// of its input rate. This is the elliptic design of Valenzuela and Constantinides, as HIIR
/// works it out.
pub fn halfband_coefficients(count: usize, transition: f64) -> Vec<f64> {
    use std::f64::consts::PI;
    let k = ((1.0 - transition * 2.0) * PI / 4.0).tan().powi(2);
    let root = (1.0 - k * k).powf(0.25);
    let e = 0.5 * (1.0 - root) / (1.0 + root);
    let e4 = e.powi(4);
    let q = e * (1.0 + e4 * (2.0 + e4 * (15.0 + 150.0 * e4)));
    let order = (count * 2 + 1) as f64;
    // The two sums of the elliptic functions' series, until their terms vanish
    let series = |term: &dyn Fn(i32) -> f64, first: i32| {
        let mut sum = 0.0;
        for i in first.. {
            let value = term(i);
            sum += value;
            if value.abs() <= 1e-100 {
                break;
            }
        }
        sum
    };
    (1..=count)
        .map(|c| {
            let c = c as f64;
            let sign = |i: i32| if i % 2 == 0 { 1.0 } else { -1.0 };
            let numerator = series(
                &|i| q.powi(i * (i + 1)) * ((i * 2 + 1) as f64 * c * PI / order).sin() * sign(i),
                0,
            ) * q.powf(0.25);
            let denominator =
                series(&|i| q.powi(i * i) * (i as f64 * 2.0 * c * PI / order).cos() * sign(i), 1) + 0.5;
            let w = (numerator / denominator).powi(2);
            let x = ((1.0 - w * k) * (1.0 - w / k)).sqrt() / (1.0 + w);
            (1.0 - x) / (1.0 + x)
        })
        .collect()
}

/// One halving of both channels: the later sample of each pair through one chain of allpass
/// filters and the earlier through the other (each filter `(c + z⁻¹) / (1 + c z⁻¹)` at the
/// lower rate, with the coefficients taking turns between the chains), averaged. The four
/// chains (two per channel) run side by side in the lanes of a vector, as
/// `[left later, left earlier, right later, right earlier]`.
#[derive(Debug, Clone, Copy)]
struct Halfband {
    /// Each pair of filters' coefficients: the first chain's in the even lanes, the second's in
    /// the odd ones.
    coefficients: [f32x4; MAX_PAIRS],
    pairs: usize,
    /// Each pair of filters' previous inputs and outputs.
    x: [f32x4; MAX_PAIRS],
    y: [f32x4; MAX_PAIRS],
}

impl Halfband {
    /// A stage with `count` coefficients, an even number.
    fn new(count: usize, transition: f64) -> Self {
        let designed = halfband_coefficients(count, transition);
        Self {
            coefficients: std::array::from_fn(|pair| match designed.get(2 * pair..2 * pair + 2) {
                Some(&[first, second]) => f32x4::new([first as f32, second as f32, first as f32, second as f32]),
                _ => f32x4::ZERO,
            }),
            pairs: count / 2,
            x: [f32x4::ZERO; MAX_PAIRS],
            y: [f32x4::ZERO; MAX_PAIRS],
        }
    }

    /// The outputs of the four chains for their inputs (see [`Halfband`]).
    #[inline]
    fn process(&mut self, input: f32x4) -> f32x4 {
        let mut value = input;
        for pair in 0..self.pairs {
            let output = (value - self.y[pair]) * self.coefficients[pair] + self.x[pair];
            self.x[pair] = value;
            self.y[pair] = output;
            value = output;
        }
        value
    }

    fn reset(&mut self) {
        self.settle(f32x4::ZERO);
    }

    /// Settles the chains as though `level` had always been coming in. Each filter passes a
    /// constant through unchanged, so its previous input and output are both that constant.
    fn settle(&mut self, level: f32x4) {
        self.x = [level; MAX_PAIRS];
        self.y = [level; MAX_PAIRS];
    }
}

/// Brings both channels down from the voices' rate to the sample rate.
#[derive(Debug, Clone, Copy)]
pub struct Decimator {
    /// The halving stages, the last first.
    stages: [Halfband; 3],
    halvings: usize,
}

impl Decimator {
    pub fn new(oversampling: Oversampling) -> Self {
        Self {
            stages: std::array::from_fn(|stage| {
                // Stage `n` halves from 2^(n + 1) times the sample rate, and must keep anything
                // that would fold back into the passband out
                let transition = 0.25 - PASSBAND / (2 << stage) as f64;
                Halfband::new(STAGE_COEFFICIENTS[stage], transition)
            }),
            halvings: oversampling.halvings(),
        }
    }

    /// Forgets the past input.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    /// Settles the filters as though the voices had always been at `left` and `right`, so that
    /// a decimator taking over from another (or from none) while notes play carries the sound
    /// on from where it was rather than from silence.
    pub fn settle(&mut self, left: f32, right: f32) {
        // (A constant comes through every stage unchanged)
        let level = f32x4::new([left, left, right, right]);
        for stage in &mut self.stages {
            stage.settle(level);
        }
    }

    /// Decimates the voices' output in `left` and `right` (a whole number of output samples'
    /// worth, the same in both) in place, and returns the number of output samples, at the
    /// start of them.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) -> usize {
        let mut len = left.len().min(right.len());
        for stage in self.stages[..self.halvings].iter_mut().rev() {
            len /= 2;
            for i in 0..len {
                let input = f32x4::new([left[2 * i + 1], left[2 * i], right[2 * i + 1], right[2 * i]]);
                let [left_later, left_earlier, right_later, right_earlier] = stage.process(input).to_array();
                left[i] = 0.5 * (left_later + left_earlier);
                right[i] = 0.5 * (right_later + right_earlier);
            }
        }
        len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decimator's gain for a sine at `frequency` (as a fraction of the output rate),
    /// measured after it has settled.
    fn gain_at(oversampling: Oversampling, frequency: f64) -> f64 {
        let factor = oversampling.factor();
        let mut decimator = Decimator::new(oversampling);
        let output_len = 4096;
        let mut input: Vec<f32> = (0..output_len * factor)
            .map(|n| (std::f64::consts::TAU * frequency * n as f64 / factor as f64).sin() as f32)
            .collect();
        // (The right channel upside down, which should come out upside down)
        let mut right: Vec<f32> = input.iter().map(|sample| -sample).collect();
        let len = decimator.process(&mut input, &mut right);
        assert_eq!(len, output_len);
        let output = &input[..len];
        assert!(output.iter().zip(&right).all(|(left, right)| *left == -right));
        // The last half, past the filters' transient, correlated with sine and cosine
        let half = output_len / 2;
        let (mut sin, mut cos) = (0.0, 0.0);
        let mut power = 0.0;
        for (n, sample) in output.iter().enumerate().skip(half) {
            let phase = std::f64::consts::TAU * frequency * n as f64;
            sin += *sample as f64 * phase.sin();
            cos += *sample as f64 * phase.cos();
            power += (*sample as f64).powi(2);
        }
        // For frequencies that fold back, the power of whatever comes out
        if frequency > 0.5 {
            return (2.0 * power / half as f64).sqrt();
        }
        2.0 * (sin * sin + cos * cos).sqrt() / half as f64
    }

    #[test]
    fn the_design_matches_hiir() {
        // 12 coefficients for a transition of 0.0232, worked out with HIIR's own design
        let expected = [
            0.025_298_446_793_4,
            0.096_316_010_113_1,
            0.200_334_940_807_6,
            0.321_345_388_215_1,
            0.444_687_111_056_3,
            0.559_871_314_987_8,
            0.661_225_854_364_9,
            0.747_101_335_795_1,
            0.818_580_320_907_5,
            0.878_318_145_198_1,
            0.929_768_003_695_1,
            0.976_811_687_656_0,
        ];
        for (designed, expected) in halfband_coefficients(12, 0.0232).iter().zip(expected) {
            assert!((designed - expected).abs() < 1e-9, "{designed} isn't {expected}");
        }
    }

    #[test]
    fn decimators_keep_the_passband_and_reject_what_would_fold_back() {
        for oversampling in [Oversampling::Two, Oversampling::Four, Oversampling::Eight] {
            // Flat up to 20 kHz (at 44.1 kHz). (Whole cycles in the 2048 samples measured)
            for cycles in [20.0, 205.0, 614.0, 921.0] {
                let frequency = cycles / 2048.0;
                let gain = gain_at(oversampling, frequency);
                assert!((gain - 1.0).abs() < 1e-3, "{oversampling:?} at {frequency}: {gain}");
            }
            // Anything that would fold back into the passband is 100 dB down, from just above
            // the output's Nyquist frequency to the input's
            let top = oversampling.factor() as f64 / 2.0;
            for frequency in [0.548, 0.6, 0.8, 1.0, 1.4, top - 0.05] {
                if frequency < top {
                    let gain = gain_at(oversampling, frequency);
                    assert!(gain < 1e-5, "{oversampling:?} at {frequency}: {} dB", 20.0 * gain.log10());
                }
            }
        }
    }

    #[test]
    fn settled_decimators_carry_a_constant_on() {
        for oversampling in [Oversampling::Two, Oversampling::Four, Oversampling::Eight] {
            let mut decimator = Decimator::new(oversampling);
            decimator.settle(0.3, -0.7);
            let (mut left, mut right) = ([0.3; 32], [-0.7; 32]);
            let len = decimator.process(&mut left, &mut right);
            assert!(left[..len].iter().all(|sample| (sample - 0.3).abs() < 1e-6), "{oversampling:?}: {left:?}");
            assert!(right[..len].iter().all(|sample| (sample + 0.7).abs() < 1e-6), "{oversampling:?}: {right:?}");
        }
    }

    #[test]
    fn decimators_bring_any_length_down() {
        let mut decimator = Decimator::new(Oversampling::Four);
        let process = |decimator: &mut Decimator, len: usize, value: f32| {
            let (mut left, mut right) = (vec![value; len], vec![value * 2.0; len]);
            let out = decimator.process(&mut left, &mut right);
            (left[..out].to_vec(), right[..out].to_vec())
        };
        assert_eq!(process(&mut decimator, 32, 1.0).0.len(), 8);
        assert_eq!(process(&mut decimator, 4, 1.0).0.len(), 1);
        // A constant comes through unchanged, once the filters have settled
        let (left, right) = process(&mut decimator, 4096, 1.0);
        assert!((left[1000] - 1.0).abs() < 1e-5 && (right[1000] - 2.0).abs() < 1e-5, "{} {}", left[1000], right[1000]);
        decimator.reset();
        let (left, right) = process(&mut decimator, 8, 0.0);
        assert!(left.iter().chain(&right).all(|sample| *sample == 0.0));
        assert_eq!(Decimator::new(Oversampling::Off).process(&mut [0.5, 0.25], &mut [0.0, 0.0]), 2);
    }

    /// The vectors give what one filter at a time does.
    #[test]
    fn the_vectors_match_one_filter_at_a_time() {
        let count = STAGE_COEFFICIENTS[0];
        let coefficients: Vec<f32> = halfband_coefficients(count, 0.0232).iter().map(|c| *c as f32).collect();
        let mut stage = Halfband::new(count, 0.0232);
        let (mut x, mut y) = (vec![0.0_f32; count], vec![0.0_f32; count]);
        for n in 0..500 {
            let (earlier, later) = (((n * 7) % 13) as f32 - 6.0, ((n * 5) % 11) as f32 - 5.0);
            // One filter at a time, the chains taking turns
            let (mut even, mut odd) = (later, earlier);
            for i in 0..count {
                let value = if i % 2 == 0 { &mut even } else { &mut odd };
                let output = (*value - y[i]) * coefficients[i] + x[i];
                x[i] = *value;
                y[i] = output;
                *value = output;
            }
            let lanes = stage.process(f32x4::new([later, earlier, -later, -earlier])).to_array();
            assert_eq!(lanes, [even, odd, -even, -odd]);
        }
    }
}
