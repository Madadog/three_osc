//! Scales worked out from a few numbers rather than read from a file: equal divisions of an
//! interval (an octave, or any other), stretches of the harmonic and subharmonic series, stacks
//! of a generator interval (Pythagorean tuning, meantone and the like) and some historical
//! temperaments.

use super::{Pitch, Scale, MAX_DEGREES};

/// A generated scale, and a short name for it (which is also the file name it suggests, so it
/// holds no slashes).
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    pub name: String,
    pub scale: Scale,
}

/// Reads an interval as typed in: a ratio (`3/2`, or `3` for 3/1), or cents (`701.955`, or
/// with a `c` after it, `700c`).
pub fn parse_interval(text: &str) -> Result<Pitch, String> {
    let text = text.trim();
    let cents = text
        .strip_suffix("cents")
        .or_else(|| text.strip_suffix("cent"))
        .or_else(|| text.strip_suffix('c'));
    let pitch = match cents {
        Some(number) => number.trim().parse::<f64>().ok().filter(|cents| cents.is_finite()).map(Pitch::Cents),
        None => Pitch::parse(text).map(reduce),
    };
    match pitch {
        Some(pitch) if pitch.cents() > 0.0 => Ok(pitch),
        Some(_) => Err(format!("{text} isn't above the root")),
        None => Err(format!("\"{text}\" isn't an interval: a ratio like 3/2, or cents like 701.955")),
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A ratio in its lowest terms.
fn ratio(numerator: u128, denominator: u128) -> Pitch {
    let divisor = gcd(numerator, denominator).max(1);
    Pitch::Ratio(numerator / divisor, denominator / divisor)
}

fn reduce(pitch: Pitch) -> Pitch {
    match pitch {
        Pitch::Ratio(numerator, denominator) => ratio(numerator, denominator),
        cents => cents,
    }
}

/// Cents for reading, to three decimal places at most: `78`, `696.578`.
fn short_cents(cents: f64) -> String {
    let text = format!("{cents:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// An interval in a name: `3` for 3/1, `5-4` for 5/4 (names can't hold slashes), `78c` for
/// cents.
fn label(pitch: Pitch) -> String {
    match pitch {
        Pitch::Ratio(numerator, 1) => numerator.to_string(),
        Pitch::Ratio(numerator, denominator) => format!("{numerator}-{denominator}"),
        Pitch::Cents(cents) => format!("{}c", short_cents(cents)),
    }
}

/// An interval in a description: `3/2`, `78 cents`.
fn describe(pitch: Pitch) -> String {
    match pitch {
        Pitch::Ratio(numerator, denominator) => format!("{numerator}/{denominator}"),
        Pitch::Cents(cents) => format!("{} cents", short_cents(cents)),
    }
}

/// `steps` equal steps of an interval: an octave (2/1) for the usual EDOs, a tritave (3/1)
/// for Bohlen-Pierce (13 steps), a fifth (3/2) for Carlos's alpha (9 steps), or a step of so
/// many cents with one division.
pub fn equal_divisions(steps: u32, interval: Pitch) -> Result<Generated, String> {
    let interval = reduce(interval);
    let total = interval.cents();
    if steps == 0 || steps as usize > MAX_DEGREES {
        return Err(format!("the number of steps has to be from 1 to {MAX_DEGREES}"));
    }
    if total <= 0.0 {
        return Err(String::from("the interval has to be above the root"));
    }
    let step = total / steps as f64;
    let pitches = (1..steps)
        .map(|n| Pitch::Cents(step * n as f64))
        .chain([interval])
        .collect();
    let name = match interval {
        Pitch::Ratio(2, 1) => format!("{steps}-EDO"),
        Pitch::Ratio(3, 1) => format!("{steps}-EDT"),
        Pitch::Ratio(3, 2) => format!("{steps}-EDF"),
        Pitch::Cents(cents) if steps == 1 => format!("{}c-ET", short_cents(cents)),
        _ => format!("{steps}-ED{}", label(interval)),
    };
    let description = format!(
        "{steps} equal divisions of {} ({} cents a step)",
        describe(interval),
        short_cents(step)
    );
    Ok(Generated {
        name,
        scale: Scale::new(description, pitches)?,
    })
}

/// The harmonics from `low` to `high` of a fundamental, repeating at `high/low`: from 8 to 16
/// is mode 8 of the harmonic series, an octave of eight steps that shrink as they go up.
pub fn harmonic_series(low: u32, high: u32) -> Result<Generated, String> {
    check_series(low, high, "harmonic")?;
    let (low, high) = (low as u128, high as u128);
    let pitches = (low + 1..=high).map(|harmonic| ratio(harmonic, low)).collect();
    let description = if high == 2 * low {
        format!("Mode {low} of the harmonic series: harmonics {low} to {high}")
    } else {
        format!("Harmonics {low} to {high}, repeating at {}", describe(ratio(high, low)))
    };
    Ok(Generated {
        name: format!("Harmonics {low}-{high}"),
        scale: Scale::new(description, pitches)?,
    })
}

/// The subharmonics from `high` down to `low`, the harmonic series upside down: steps that
/// grow as they go up, repeating at `high/low`.
pub fn subharmonic_series(high: u32, low: u32) -> Result<Generated, String> {
    check_series(low, high, "subharmonic")?;
    let (low, high) = (low as u128, high as u128);
    let pitches = (low..high).rev().map(|subharmonic| ratio(high, subharmonic)).collect();
    let description = if high == 2 * low {
        format!("Subharmonics {high} down to {low} of the subharmonic series")
    } else {
        format!("Subharmonics {high} down to {low}, repeating at {}", describe(ratio(high, low)))
    };
    Ok(Generated {
        name: format!("Subharmonics {high}-{low}"),
        scale: Scale::new(description, pitches)?,
    })
}

fn check_series(low: u32, high: u32, kind: &str) -> Result<(), String> {
    if low == 0 {
        return Err(format!("the lowest {kind} is the first, 1"));
    }
    if high <= low {
        return Err(format!("the highest {kind} has to be above the lowest"));
    }
    if (high - low) as usize > MAX_DEGREES {
        return Err(format!("that's more than {MAX_DEGREES} notes"));
    }
    Ok(())
}

/// A stack of `notes` generators, `down` of them below the root and the rest above, each
/// moved by whole periods to within a period above the root. Pure fifths (3/2) in an octave
/// make Pythagorean scales, fifths narrowed by a quarter of a syntonic comma (696.578 cents)
/// quarter-comma meantone, and 7 notes with 1 down the major scale. With ratios for both, the
/// notes are worked out as exact ratios as long as their numbers fit.
pub fn generator_stack(generator: Pitch, period: Pitch, notes: u32, down: u32) -> Result<Generated, String> {
    let (generator, period) = (reduce(generator), reduce(period));
    let (step, span) = (generator.cents(), period.cents());
    if notes == 0 || notes as usize > MAX_DEGREES {
        return Err(format!("the number of notes has to be from 1 to {MAX_DEGREES}"));
    }
    if down >= notes {
        return Err(String::from("at least the root has to be left out of the generators below it"));
    }
    if span <= 0.0 {
        return Err(String::from("the period has to be above the root"));
    }
    let mut degrees: Vec<(f64, Pitch)> = (-(down as i64)..(notes - down) as i64)
        .filter(|&k| k != 0)
        .map(|k| {
            let periods = (k as f64 * step / span).floor();
            let within = k as f64 * step - periods * span;
            let exact = exact_power(generator, period, k, -periods as i64);
            (within, exact.unwrap_or(Pitch::Cents(within)))
        })
        .collect();
    degrees.sort_by(|a, b| a.0.total_cmp(&b.0));
    // The stack can come round to a note it already has (the root, or the period)
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let repeats = degrees.windows(2).any(|pair| close(pair[0].0, pair[1].0))
        || degrees.first().is_some_and(|first| close(first.0, 0.0))
        || degrees.last().is_some_and(|last| close(last.0, span));
    if repeats {
        return Err(format!(
            "{} comes back round to a note it already has before {notes} notes",
            describe(generator)
        ));
    }
    let pitches = degrees.into_iter().map(|(_, pitch)| pitch).chain([period]).collect();
    let mut name = format!("{notes}-note {} stack", label(generator));
    if period != Pitch::Ratio(2, 1) {
        name += &format!(" per {}", label(period));
    }
    let description = format!(
        "{notes} notes of a stack of {}, {down} below the root, within {}",
        describe(generator),
        describe(period)
    );
    Ok(Generated {
        name,
        scale: Scale::new(description, pitches)?,
    })
}

/// `generator` to the power of `k` times `period` to the power of `m`, as a ratio, if both are
/// ratios and the numbers fit.
fn exact_power(generator: Pitch, period: Pitch, k: i64, m: i64) -> Option<Pitch> {
    let (Pitch::Ratio(gn, gd), Pitch::Ratio(pn, pd)) = (generator, period) else {
        return None;
    };
    let mut numerator: u128 = 1;
    let mut denominator: u128 = 1;
    for (top, bottom, power) in [(gn, gd, k), (pn, pd, m)] {
        let (top, bottom) = if power >= 0 { (top, bottom) } else { (bottom, top) };
        let power = u32::try_from(power.unsigned_abs()).ok()?;
        numerator = numerator.checked_mul(top.checked_pow(power)?)?;
        denominator = denominator.checked_mul(bottom.checked_pow(power)?)?;
    }
    Some(ratio(numerator, denominator))
}

/// Historical temperaments of the twelve notes, and twelve note just intonation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temperament {
    Pythagorean,
    QuarterCommaMeantone,
    WerckmeisterIii,
    KirnbergerIii,
    Vallotti,
    Young,
    Just,
}

/// The intervals the temperaments are made from, in cents.
fn fifth() -> f64 {
    Pitch::Ratio(3, 2).cents()
}
fn pythagorean_comma() -> f64 {
    Pitch::Ratio(531441, 524288).cents()
}
fn syntonic_comma() -> f64 {
    Pitch::Ratio(81, 80).cents()
}
fn schisma() -> f64 {
    Pitch::Ratio(32805, 32768).cents()
}

impl Temperament {
    pub const ALL: [Self; 7] = [
        Self::Pythagorean,
        Self::QuarterCommaMeantone,
        Self::WerckmeisterIii,
        Self::KirnbergerIii,
        Self::Vallotti,
        Self::Young,
        Self::Just,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Pythagorean => "Pythagorean",
            Self::QuarterCommaMeantone => "Quarter-comma meantone",
            Self::WerckmeisterIii => "Werckmeister III",
            Self::KirnbergerIii => "Kirnberger III",
            Self::Vallotti => "Vallotti",
            Self::Young => "Young",
            Self::Just => "Just intonation",
        }
    }

    /// What the temperament is, for its `.scl` file's description and the GUI.
    pub fn description(self) -> &'static str {
        match self {
            Self::Pythagorean => "Pythagorean tuning: pure fifths (3/2) from Eb to G#, the wolf between G# and Eb",
            Self::QuarterCommaMeantone => {
                "Quarter-comma meantone: fifths a quarter of a syntonic comma narrow, for pure major \
                 thirds, from Eb to G#"
            }
            Self::WerckmeisterIii => {
                "Werckmeister III (1691): the fifths C-G-D-A and B-F# a quarter of a Pythagorean \
                 comma narrow, the rest pure"
            }
            Self::KirnbergerIii => {
                "Kirnberger III (1779): the fifths C-G-D-A-E a quarter of a syntonic comma narrow, \
                 F#-C# a schisma narrow, the rest pure"
            }
            Self::Vallotti => "Vallotti: the fifths F-C-G-D-A-E-B a sixth of a Pythagorean comma narrow, the rest pure",
            Self::Young => "Young (1800): the fifths C-G-D-A-E-B-F# a sixth of a Pythagorean comma narrow, the rest pure",
            Self::Just => "Just intonation: twelve notes from C in ratios of 2, 3 and 5",
        }
    }

    pub fn generate(self) -> Generated {
        let (p, quarter_syntonic, quarter_pythagorean, sixth_pythagorean) = (
            fifth(),
            fifth() - syntonic_comma() / 4.0,
            fifth() - pythagorean_comma() / 4.0,
            fifth() - pythagorean_comma() / 6.0,
        );
        let pitches = match self {
            Self::Pythagorean => {
                generator_stack(Pitch::Ratio(3, 2), Pitch::Ratio(2, 1), 12, 3).map(|stack| stack.scale.pitches)
            }
            Self::QuarterCommaMeantone => generator_stack(Pitch::Cents(quarter_syntonic), Pitch::Ratio(2, 1), 12, 3)
                .map(|stack| stack.scale.pitches),
            // The fifths from Eb up to G#
            Self::WerckmeisterIii => Ok(chain_of_fifths({
                let t = quarter_pythagorean;
                [p, p, p, t, t, t, p, p, t, p, p]
            })),
            Self::KirnbergerIii => Ok(chain_of_fifths({
                let (t, s) = (quarter_syntonic, p - schisma());
                [p, p, p, t, t, t, t, p, p, s, p]
            })),
            Self::Vallotti => Ok(chain_of_fifths({
                let t = sixth_pythagorean;
                [p, p, t, t, t, t, t, t, p, p, p]
            })),
            Self::Young => Ok(chain_of_fifths({
                let t = sixth_pythagorean;
                [p, p, p, t, t, t, t, t, t, p, p]
            })),
            Self::Just => Ok([(16, 15), (9, 8), (6, 5), (5, 4), (4, 3), (45, 32), (3, 2), (8, 5), (5, 3), (9, 5), (15, 8), (2, 1)]
                .map(|(numerator, denominator)| Pitch::Ratio(numerator, denominator))
                .to_vec()),
        };
        let pitches = pitches.expect("the temperaments are valid stacks");
        Generated {
            name: self.name().to_string(),
            scale: Scale::new(self.description(), pitches).expect("the temperaments are valid scales"),
        }
    }
}

/// The twelve notes above C reached by a chain of fifths of the given sizes in cents, from Eb
/// up to G# (the fifth from G# back round to Eb is whatever is left of the circle), and the
/// octave.
fn chain_of_fifths(fifths: [f64; 11]) -> Vec<Pitch> {
    // Eb Bb F C G D A E B F# C# G#, with C at 0
    let mut notes = [0.0; 12];
    for i in (0..3).rev() {
        notes[i] = notes[i + 1] - fifths[i];
    }
    for i in 4..12 {
        notes[i] = notes[i - 1] + fifths[i - 1];
    }
    let mut cents: Vec<f64> = notes.iter().map(|note| note.rem_euclid(1200.0)).collect();
    cents.sort_by(f64::total_cmp);
    // The first is C, the root
    cents[1..]
        .iter()
        .map(|&cents| Pitch::Cents(cents))
        .chain([Pitch::Ratio(2, 1)])
        .collect()
}

/// A line about a scale's steps: their size if they're all the same, otherwise the smallest
/// and largest.
pub fn summarize_steps(scale: &Scale) -> String {
    let mut previous = 0.0;
    let steps: Vec<f64> = scale
        .cents()
        .iter()
        .map(|&cents| {
            let step = cents - previous;
            previous = cents;
            step
        })
        .collect();
    let smallest = steps.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = steps.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let notes = match scale.len() {
        1 => String::from("1 note"),
        len => format!("{len} notes"),
    };
    // (Equal to the thousandth of a cent shown, which a .scl file's six decimals can be off by)
    if largest - smallest < 5e-4 {
        format!("{notes}, {} cents apart", short_cents(smallest))
    } else {
        format!(
            "{notes}, from {} to {} cents apart",
            short_cents(smallest),
            short_cents(largest)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cents_of(generated: &Generated) -> Vec<f64> {
        generated.scale.cents().to_vec()
    }

    fn assert_cents(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() < 0.001, "{actual} isn't {expected} in {actual:?}");
        }
    }

    #[test]
    fn intervals_are_ratios_or_cents() {
        assert_eq!(parse_interval("3/2"), Ok(Pitch::Ratio(3, 2)));
        assert_eq!(parse_interval(" 4/2 "), Ok(Pitch::Ratio(2, 1)));
        assert_eq!(parse_interval("3"), Ok(Pitch::Ratio(3, 1)));
        assert_eq!(parse_interval("701.955"), Ok(Pitch::Cents(701.955)));
        assert_eq!(parse_interval("700c"), Ok(Pitch::Cents(700.0)));
        assert_eq!(parse_interval("88 cents"), Ok(Pitch::Cents(88.0)));
        assert!(parse_interval("1/1").unwrap_err().contains("above the root"));
        assert!(parse_interval("-100c").is_err());
        assert!(parse_interval("fifth").unwrap_err().contains("isn't an interval"));
        assert!(parse_interval("0/1").is_err());
    }

    #[test]
    fn equal_divisions_of_any_interval() {
        let edo = equal_divisions(19, Pitch::Ratio(2, 1)).unwrap();
        assert_eq!(edo.name, "19-EDO");
        assert_eq!(edo.scale.len(), 19);
        assert!((edo.scale.cents()[0] - 1200.0 / 19.0).abs() < 1e-9);
        assert_eq!(edo.scale.period(), Pitch::Ratio(2, 1));
        assert!(edo.scale.description.starts_with("19 equal divisions of 2/1"));

        let bohlen_pierce = equal_divisions(13, Pitch::Ratio(3, 1)).unwrap();
        assert_eq!(bohlen_pierce.name, "13-EDT");
        assert!((bohlen_pierce.scale.cents()[0] - 146.304).abs() < 0.001);
        assert_eq!(equal_divisions(9, Pitch::Ratio(3, 2)).unwrap().name, "9-EDF");
        assert_eq!(equal_divisions(7, Pitch::Ratio(5, 4)).unwrap().name, "7-ED5-4");
        assert_eq!(equal_divisions(1, Pitch::Cents(88.0)).unwrap().name, "88c-ET");
        assert_eq!(equal_divisions(2, Pitch::Cents(150.5)).unwrap().name, "2-ED150.5c");
        assert!(equal_divisions(0, Pitch::Ratio(2, 1)).is_err());
        assert!(equal_divisions(5000, Pitch::Ratio(2, 1)).is_err());
        // The names make file names
        for generated in [bohlen_pierce, equal_divisions(7, Pitch::Ratio(5, 4)).unwrap()] {
            crate::presets::check_name(&generated.name).unwrap();
        }
    }

    #[test]
    fn harmonic_series_modes() {
        let mode8 = harmonic_series(8, 16).unwrap();
        assert_eq!(mode8.name, "Harmonics 8-16");
        assert_eq!(
            mode8.scale.pitches,
            [(9, 8), (5, 4), (11, 8), (3, 2), (13, 8), (7, 4), (15, 8), (2, 1)].map(|(n, d)| Pitch::Ratio(n, d))
        );
        assert!(mode8.scale.description.starts_with("Mode 8"));
        // Not an octave: repeating at 3/2
        let fifth = harmonic_series(4, 6).unwrap();
        assert_eq!(fifth.scale.period(), Pitch::Ratio(3, 2));
        assert!(harmonic_series(0, 4).is_err());
        assert!(harmonic_series(8, 8).is_err());

        let subharmonics = subharmonic_series(16, 8).unwrap();
        assert_eq!(subharmonics.name, "Subharmonics 16-8");
        assert_eq!(
            subharmonics.scale.pitches,
            [(16, 15), (8, 7), (16, 13), (4, 3), (16, 11), (8, 5), (16, 9), (2, 1)].map(|(n, d)| Pitch::Ratio(n, d))
        );
        assert!(subharmonic_series(8, 16).is_err());
    }

    #[test]
    fn generator_stacks_make_pythagorean_and_meantone_scales() {
        // The major scale: F below C, then G D A E B
        let major = generator_stack(Pitch::Ratio(3, 2), Pitch::Ratio(2, 1), 7, 1).unwrap();
        assert_eq!(major.name, "7-note 3-2 stack");
        assert_eq!(
            major.scale.pitches,
            [(9, 8), (81, 64), (4, 3), (3, 2), (27, 16), (243, 128), (2, 1)].map(|(n, d)| Pitch::Ratio(n, d))
        );
        // Meantone, in cents: pure major thirds
        let meantone = generator_stack(Pitch::Cents(696.578_428), Pitch::Ratio(2, 1), 12, 3).unwrap();
        assert!((meantone.scale.cents()[3] - 386.314).abs() < 0.001);
        // A generator that comes round to the root can't give more notes than that
        assert!(generator_stack(Pitch::Cents(700.0), Pitch::Ratio(2, 1), 12, 0).is_ok());
        assert!(generator_stack(Pitch::Cents(700.0), Pitch::Ratio(2, 1), 13, 0).unwrap_err().contains("comes back round"));
        assert!(generator_stack(Pitch::Ratio(2, 1), Pitch::Ratio(2, 1), 2, 0).is_err());
        assert!(generator_stack(Pitch::Ratio(3, 2), Pitch::Ratio(2, 1), 5, 5).is_err());
        // Other periods
        let tritave = generator_stack(Pitch::Ratio(9, 7), Pitch::Ratio(3, 1), 9, 4).unwrap();
        assert_eq!(tritave.name, "9-note 9-7 stack per 3");
        assert_eq!(tritave.scale.period(), Pitch::Ratio(3, 1));
        // Ratios too big to hold come out in cents
        let long = generator_stack(Pitch::Ratio(3, 2), Pitch::Ratio(2, 1), 200, 0).unwrap();
        assert!(long.scale.pitches.iter().any(|pitch| matches!(pitch, Pitch::Cents(_))));
        assert!(long.scale.pitches.iter().any(|pitch| matches!(pitch, Pitch::Ratio(..))));
    }

    #[test]
    fn historical_temperaments_match_their_published_cents() {
        let check = |temperament: Temperament, expected: [f64; 11]| {
            let generated = temperament.generate();
            assert_eq!(generated.name, temperament.name());
            let mut expected = expected.to_vec();
            expected.push(1200.0);
            assert_cents(&cents_of(&generated), &expected);
        };
        check(
            Temperament::WerckmeisterIii,
            [90.225, 192.180, 294.135, 390.225, 498.045, 588.270, 696.090, 792.180, 888.270, 996.090, 1092.180],
        );
        check(
            Temperament::KirnbergerIii,
            [90.225, 193.157, 294.135, 386.314, 498.045, 590.224, 696.578, 792.180, 889.735, 996.090, 1088.269],
        );
        check(
            Temperament::Vallotti,
            [94.135, 196.090, 298.045, 392.180, 501.955, 592.180, 698.045, 796.090, 894.135, 1000.000, 1090.225],
        );
        check(
            Temperament::Young,
            [90.225, 196.090, 294.135, 392.180, 498.045, 588.270, 698.045, 792.180, 894.135, 996.090, 1090.225],
        );
        check(
            Temperament::QuarterCommaMeantone,
            [76.049, 193.157, 310.265, 386.314, 503.422, 579.471, 696.578, 772.627, 889.735, 1006.843, 1082.892],
        );
        let pythagorean = Temperament::Pythagorean.generate();
        assert_eq!(pythagorean.scale.pitches[0], Pitch::Ratio(2187, 2048));
        assert_eq!(pythagorean.scale.pitches[2], Pitch::Ratio(32, 27));
        assert_eq!(Temperament::Just.generate().scale.pitches[5], Pitch::Ratio(45, 32));
        for temperament in Temperament::ALL {
            let generated = temperament.generate();
            assert_eq!(generated.scale.len(), 12);
            crate::presets::check_name(&generated.name).unwrap();
            assert!(generated.scale.description.is_ascii());
        }
    }

    #[test]
    fn steps_are_summed_up() {
        // Read back from a .scl file, whose cents are rounded
        let read = Scale::parse(&equal_divisions(19, Pitch::Ratio(2, 1)).unwrap().scale.to_scl("19.scl")).unwrap();
        assert_eq!(summarize_steps(&read), "19 notes, 63.158 cents apart");
        assert_eq!(summarize_steps(&equal_divisions(19, Pitch::Ratio(2, 1)).unwrap().scale), "19 notes, 63.158 cents apart");
        assert_eq!(
            summarize_steps(&harmonic_series(8, 16).unwrap().scale),
            "8 notes, from 111.731 to 203.91 cents apart"
        );
        assert_eq!(summarize_steps(&equal_divisions(1, Pitch::Cents(88.0)).unwrap().scale), "1 note, 88 cents apart");
    }
}
