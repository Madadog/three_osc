//! Microtuning with Scala files. A scale (`.scl`) lists the pitches of its degrees above its
//! root, the last of which is its period (usually the octave, 2/1), and a keyboard mapping
//! (`.kbm`) says which key plays which degree, and which key is tuned to which frequency.
//!
//! Without a mapping, the scale's root is middle C at its usual pitch (261.63 Hz), and the
//! keys go up and down the scale a degree at a time. Without a scale, a mapping retunes the
//! usual twelve-tone equal temperament (to move A away from 440 Hz, say).
//!
//! The formats are described at <https://www.huygens-fokker.org/scala/scl_format.html> and in
//! the keyboard mapping section of <https://www.huygens-fokker.org/scala/help.htm>. Scales can
//! also be worked out from a few numbers, see [`generate`].

pub mod generate;

/// MIDI key numbers.
pub const KEYS: usize = 128;

/// The frequency of middle C (key 60) in twelve-tone equal temperament, which a scale's root
/// is tuned to when no keyboard mapping says otherwise.
pub const MIDDLE_C_HZ: f64 = 261.625_565_300_598_6;

/// Tuned pitches are kept within these MIDI note numbers (about 0.5 Hz to 1.1 MHz), however
/// wide a scale's steps are.
const LOWEST_NOTE: f64 = -48.0;
const HIGHEST_NOTE: f64 = 176.0;

/// The most degrees a scale, and the most keys a mapping's pattern, may have.
pub const MAX_DEGREES: usize = 4096;

/// A pitch above a scale's root, as a `.scl` file gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pitch {
    Cents(f64),
    /// A frequency ratio: numerator over denominator, both above zero.
    Ratio(u128, u128),
}

impl Pitch {
    pub fn cents(self) -> f64 {
        match self {
            Self::Cents(cents) => cents,
            Self::Ratio(numerator, denominator) => {
                1200.0 * (numerator as f64 / denominator as f64).log2()
            }
        }
    }

    /// Reads a pitch: cents if it has a decimal point, otherwise a ratio (`3/2`, or `2` for
    /// 2/1).
    pub fn parse(word: &str) -> Option<Self> {
        if word.contains('.') {
            return word.parse::<f64>().ok().filter(|cents| cents.is_finite()).map(Self::Cents);
        }
        let (numerator, denominator) = word.split_once('/').unwrap_or((word, "1"));
        let numerator = numerator.parse::<u128>().ok()?;
        let denominator = denominator.parse::<u128>().ok()?;
        (numerator > 0 && denominator > 0).then_some(Self::Ratio(numerator, denominator))
    }

    /// The pitch as a `.scl` file writes it.
    pub fn to_scl(self) -> String {
        match self {
            Self::Cents(cents) => format_cents(cents),
            Self::Ratio(numerator, denominator) => format!("{numerator}/{denominator}"),
        }
    }
}

/// Cents to six decimal places at most, always with a decimal point (which is what makes them
/// cents in a `.scl` file).
pub fn format_cents(cents: f64) -> String {
    let text = format!("{cents:.6}");
    let text = text.trim_end_matches('0');
    if text.ends_with('.') {
        format!("{text}0")
    } else {
        text.to_string()
    }
}

/// A scale: the pitches of its degrees above its root.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    pub description: String,
    /// Degrees 1 to the period (degree 0, the root, goes without saying), as given.
    pub pitches: Vec<Pitch>,
    /// The same in cents.
    cents: Vec<f64>,
}

impl Scale {
    /// The scale of these pitches above the root, the last of which is the period it repeats
    /// at.
    pub fn new(description: impl Into<String>, pitches: Vec<Pitch>) -> Result<Self, String> {
        if pitches.is_empty() {
            return Err(String::from("the scale has no pitches"));
        }
        if pitches.len() > MAX_DEGREES {
            return Err(format!("the scale has more than {MAX_DEGREES} pitches"));
        }
        let cents: Vec<f64> = pitches.iter().map(|pitch| pitch.cents()).collect();
        if cents[cents.len() - 1] <= 0.0 {
            return Err(String::from(
                "the last pitch is the period the scale repeats at (usually 2/1), so it has to \
                 be above the root",
            ));
        }
        Ok(Self {
            description: description.into(),
            pitches,
            cents,
        })
    }

    /// Twelve-tone equal temperament, the usual tuning.
    pub fn equal_temperament() -> Self {
        let pitches = (1..12)
            .map(|step| Pitch::Cents(step as f64 * 100.0))
            .chain([Pitch::Ratio(2, 1)])
            .collect();
        Self::new("12-tone equal temperament", pitches).expect("a valid scale")
    }

    /// Reads a `.scl` file.
    pub fn parse(text: &str) -> Result<Self, String> {
        // Lines starting with '!' are comments. The first other line is the description (which
        // can be empty), then comes the number of pitches, and then the pitches
        let mut lines = text.lines().map(str::trim).filter(|line| !line.starts_with('!'));
        let description = lines.next().ok_or("the file is empty")?.to_string();
        let mut lines = lines.filter(|line| !line.is_empty());
        let count_line = lines.next().ok_or("the number of pitches is missing")?;
        let count: usize = first_word(count_line)
            .parse()
            .map_err(|_| format!("\"{count_line}\" isn't a number of pitches"))?;
        if count == 0 {
            return Err(String::from("the scale has no pitches"));
        }
        if count > MAX_DEGREES {
            return Err(format!("the scale has more than {MAX_DEGREES} pitches"));
        }
        let pitches = lines.take(count).map(parse_pitch).collect::<Result<Vec<Pitch>, String>>()?;
        if pitches.len() < count {
            return Err(format!(
                "the scale says it has {count} pitches, but lists {}",
                pitches.len()
            ));
        }
        Self::new(description, pitches)
    }

    /// The scale as a `.scl` file called `file_name`.
    pub fn to_scl(&self, file_name: &str) -> String {
        // (The description has to be one line, which isn't a comment)
        let description = self.description.replace(['\r', '\n'], " ");
        let description = description.trim_start_matches('!');
        let mut text = format!("! {file_name}\n!\n{description}\n {}\n!\n", self.pitches.len());
        for pitch in &self.pitches {
            text += &format!(" {}\n", pitch.to_scl());
        }
        text
    }

    /// Degrees 1 to the period, in cents above the root.
    pub fn cents(&self) -> &[f64] {
        &self.cents
    }

    /// The interval the scale repeats at.
    pub fn period(&self) -> Pitch {
        self.pitches[self.pitches.len() - 1]
    }

    /// The number of degrees in a period.
    pub fn len(&self) -> usize {
        self.cents.len()
    }

    /// The pitch of a degree, in cents above the root. Past the period (or below the root)
    /// the scale repeats a period higher (or lower).
    pub fn degree_cents(&self, degree: i64) -> f64 {
        let len = self.cents.len() as i64;
        let periods = degree.div_euclid(len) as f64;
        let step = degree.rem_euclid(len) as usize;
        let within = if step == 0 { 0.0 } else { self.cents[step - 1] };
        periods * self.cents[self.cents.len() - 1] + within
    }
}

/// A `.scl` pitch line: anything after the first word is a comment.
fn parse_pitch(line: &str) -> Result<Pitch, String> {
    let word = first_word(line);
    match Pitch::parse(word) {
        Some(pitch) => Ok(pitch),
        // Whole numbers, one of them zero
        None if word.split('/').all(|part| part.parse::<u128>().is_ok()) => {
            Err(format!("\"{line}\" isn't a positive ratio"))
        }
        None => Err(format!("\"{line}\" isn't a pitch: cents like 701.955, or a ratio like 3/2")),
    }
}

fn first_word(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

/// The next of a `.kbm` file's whole numbers.
fn next_number<'a>(words: &mut impl Iterator<Item = &'a str>, what: &str) -> Result<i64, String> {
    let word = words.next().ok_or_else(|| format!("the {what} is missing"))?;
    word.parse().map_err(|_| format!("the {what} (\"{word}\") isn't a whole number"))
}

/// Which key plays which degree of a scale, and the frequency of one key.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardMapping {
    /// The number of keys in the pattern that repeats along the keyboard, or 0 for a degree
    /// per key.
    pub size: usize,
    /// The keys outside of these are left silent.
    pub first_key: i32,
    pub last_key: i32,
    /// The key that plays the pattern's first entry (degree 0, the root, for a degree per
    /// key).
    pub middle_key: i32,
    /// The key that is tuned to `reference_hz`.
    pub reference_key: i32,
    pub reference_hz: f64,
    /// The degree each repeat of the pattern moves up by, or 0 for the scale's period.
    pub octave_degree: usize,
    /// The degree each key of the pattern plays, `None` for a key left silent. Keys past the
    /// end are silent too.
    pub degrees: Vec<Option<i64>>,
}

impl KeyboardMapping {
    /// A degree per key, with the root on middle C at its usual pitch.
    pub fn standard() -> Self {
        Self {
            size: 0,
            first_key: 0,
            last_key: KEYS as i32 - 1,
            middle_key: 60,
            reference_key: 60,
            reference_hz: MIDDLE_C_HZ,
            octave_degree: 0,
            degrees: Vec::new(),
        }
    }

    /// Reads a `.kbm` file.
    pub fn parse(text: &str) -> Result<Self, String> {
        // Lines starting with '!' are comments, and only each line's first word counts
        let mut words = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('!'))
            .map(first_word);
        let size = next_number(&mut words, "map size")?;
        let first_key = next_number(&mut words, "first key")?;
        let last_key = next_number(&mut words, "last key")?;
        let middle_key = next_number(&mut words, "middle key")?;
        let reference_key = next_number(&mut words, "reference key")?;
        let reference_word = words.next().ok_or("the reference frequency is missing")?;
        let reference_hz: f64 = reference_word
            .parse()
            .ok()
            .filter(|hz: &f64| hz.is_finite() && *hz > 0.0)
            .ok_or_else(|| format!("the reference frequency (\"{reference_word}\") isn't a frequency"))?;
        let octave_degree = next_number(&mut words, "octave degree")?;
        if !(0..=MAX_DEGREES as i64).contains(&size) {
            return Err(format!("the map size ({size}) isn't between 0 and {MAX_DEGREES}"));
        }
        let key = |key: i64, what: &str| -> Result<i32, String> {
            if (0..KEYS as i64).contains(&key) {
                Ok(key as i32)
            } else {
                Err(format!("the {what} ({key}) isn't a MIDI key from 0 to 127"))
            }
        };
        let mapping = Self {
            size: size as usize,
            first_key: key(first_key, "first key")?,
            last_key: key(last_key, "last key")?,
            middle_key: key(middle_key, "middle key")?,
            reference_key: key(reference_key, "reference key")?,
            reference_hz,
            octave_degree: usize::try_from(octave_degree)
                .ok()
                .filter(|degree| *degree <= MAX_DEGREES)
                .ok_or_else(|| format!("the octave degree ({octave_degree}) is out of range"))?,
            // The entries, which may stop early, the rest of the keys being silent
            degrees: words
                .take(size as usize)
                .map(|word| {
                    if word.eq_ignore_ascii_case("x") {
                        Ok(None)
                    } else {
                        word.parse::<i64>()
                            .ok()
                            .filter(|degree| (0..=MAX_DEGREES as i64 * 128).contains(degree))
                            .map(Some)
                            .ok_or_else(|| format!("\"{word}\" isn't a scale degree or x"))
                    }
                })
                .collect::<Result<_, String>>()?,
        };
        Ok(mapping)
    }

    /// The pitch a key plays, in cents above the root on the middle key, or `None` if the
    /// mapping leaves it silent. The first and last keys aren't checked here.
    fn key_cents(&self, scale: &Scale, key: i32) -> Option<f64> {
        let offset = (key - self.middle_key) as i64;
        if self.size == 0 {
            return Some(scale.degree_cents(offset));
        }
        let size = self.size as i64;
        let repeats = offset.div_euclid(size) as f64;
        let degree = (*self.degrees.get(offset.rem_euclid(size) as usize)?)?;
        let octave = if self.octave_degree == 0 {
            scale.len()
        } else {
            self.octave_degree
        };
        Some(scale.degree_cents(degree) + repeats * scale.degree_cents(octave as i64))
    }
}

/// The pitch of every key: a MIDI note number (69 is A at 440 Hz, and 1 is a semitone of
/// twelve-tone equal temperament), fractional between the usual notes, or `None` for a key the
/// mapping leaves silent.
#[derive(Debug, Clone, PartialEq)]
pub struct Tuning {
    pub keys: [Option<f32>; KEYS],
}

impl Tuning {
    /// Twelve-tone equal temperament with A at 440 Hz: every key at its own note number.
    pub fn standard() -> Self {
        Self {
            keys: std::array::from_fn(|key| Some(key as f32)),
        }
    }

    pub fn new(scale: &Scale, mapping: &KeyboardMapping) -> Result<Self, String> {
        let reference_cents = mapping.key_cents(scale, mapping.reference_key).ok_or_else(|| {
            format!(
                "the reference key ({}) doesn't play a degree of the scale",
                mapping.reference_key
            )
        })?;
        // The note number of the reference frequency, which every key is tuned from
        let reference_note = 69.0 + 12.0 * (mapping.reference_hz / 440.0).log2();
        let keys = std::array::from_fn(|key| {
            let key = key as i32;
            if key < mapping.first_key || key > mapping.last_key {
                return None;
            }
            let cents = mapping.key_cents(scale, key)?;
            let note = reference_note + (cents - reference_cents) / 100.0;
            Some(note.clamp(LOWEST_NOTE, HIGHEST_NOTE) as f32)
        });
        Ok(Self { keys })
    }

    /// Reads a scale and a mapping (either of which may be left out for the usual one) and
    /// tunes the keys with them.
    pub fn from_files(scale: Option<&str>, mapping: Option<&str>) -> Result<Self, String> {
        let scale = match scale {
            Some(text) => Scale::parse(text).map_err(|error| format!("Scale: {error}"))?,
            None => Scale::equal_temperament(),
        };
        let mapping = match mapping {
            Some(text) => KeyboardMapping::parse(text).map_err(|error| format!("Mapping: {error}"))?,
            None => KeyboardMapping::standard(),
        };
        Self::new(&scale, &mapping)
    }
}

impl Default for Tuning {
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f64) {
        assert!(
            (actual as f64 - expected).abs() < 1e-4,
            "{actual} isn't {expected}"
        );
    }

    #[test]
    fn the_usual_scale_and_mapping_tune_every_key_to_its_note() {
        let tuning = Tuning::new(&Scale::equal_temperament(), &KeyboardMapping::standard()).unwrap();
        for (key, pitch) in tuning.keys.iter().enumerate() {
            assert_close(pitch.unwrap(), key as f64);
        }
        assert_eq!(Tuning::from_files(None, None).unwrap().keys.map(|pitch| pitch.map(f32::round)), Tuning::standard().keys);
    }

    #[test]
    fn scales_read_cents_and_ratios() {
        let text = "! meantone.scl\r\n!\r\nQuarter-comma meantone, partly\r\n 4\r\n!\r\n 76.04900 cents\r\n5/4\r\n\r\n 3/2 fifth\r\n2\r\n";
        let scale = Scale::parse(text).unwrap();
        assert_eq!(scale.description, "Quarter-comma meantone, partly");
        assert_eq!(scale.len(), 4);
        let expected = [76.049, 386.3137, 701.955, 1200.0];
        for (cents, expected) in scale.cents().iter().zip(expected) {
            assert!((cents - expected).abs() < 1e-3, "{cents} isn't {expected}");
        }
        assert_eq!(scale.degree_cents(0), 0.0);
        assert!((scale.degree_cents(6) - (1200.0 + 386.3137)).abs() < 1e-3);
        assert!((scale.degree_cents(-1) - (701.955 - 1200.0)).abs() < 1e-3);
        // A description can be empty
        assert_eq!(Scale::parse("\n1\n2/1").unwrap().cents(), [1200.0]);
        assert_eq!(scale.pitches[1], Pitch::Ratio(5, 4));
        assert_eq!(scale.period(), Pitch::Ratio(2, 1));
    }

    #[test]
    fn scales_write_out_as_scl_files() {
        assert_eq!(format_cents(63.157_894_736), "63.157895");
        assert_eq!(format_cents(1200.0), "1200.0");
        assert_eq!(format_cents(-3.5), "-3.5");
        let scale = Scale::new(
            "Mixed\nup",
            vec![Pitch::Cents(150.0), Pitch::Ratio(5, 4), Pitch::Cents(701.955), Pitch::Ratio(2, 1)],
        )
        .unwrap();
        let text = scale.to_scl("mixed.scl");
        assert!(text.starts_with("! mixed.scl\n!\nMixed up\n 4\n"), "{text}");
        let read = Scale::parse(&text).unwrap();
        assert_eq!(read.pitches, scale.pitches);
        assert_eq!(read.description, "Mixed up");
        let standard = Scale::equal_temperament();
        assert_eq!(Scale::parse(&standard.to_scl("12.scl")).unwrap(), standard);
        // A description can't start a comment
        let odd = Scale::new("!!Loud", vec![Pitch::Ratio(2, 1)]).unwrap();
        assert_eq!(Scale::parse(&odd.to_scl("odd.scl")).unwrap().description, "Loud");
    }

    #[test]
    fn broken_scales_are_refused() {
        for (text, reason) in [
            ("", "empty"),
            ("Nothing\n", "number of pitches is missing"),
            ("Bad\nmany\n", "isn't a number"),
            ("None\n0\n", "no pitches"),
            ("Short\n3\n100.0\n2/1\n", "lists 2"),
            ("Negative\n1\n-3/2\n", "isn't a pitch"),
            ("Zero\n1\n0/1\n", "positive ratio"),
            ("Words\n1\nfive\n", "isn't a pitch"),
            ("Flat\n2\n100.0\n-100.0\n", "above the root"),
        ] {
            let error = Scale::parse(text).unwrap_err();
            assert!(error.contains(reason), "{text:?}: {error}");
        }
    }

    #[test]
    fn equal_divisions_spread_their_steps_over_the_keys() {
        // 19 equal steps to the octave: 19 keys up from middle C is the octave above
        let steps: String = (1..=19).map(|step| format!("{:.6}\n", step as f64 * 1200.0 / 19.0)).collect();
        let scale = Scale::parse(&format!("19-EDO\n19\n{steps}")).unwrap();
        let tuning = Tuning::new(&scale, &KeyboardMapping::standard()).unwrap();
        assert_close(tuning.keys[60].unwrap(), 60.0);
        assert_close(tuning.keys[61].unwrap(), 60.0 + 12.0 / 19.0);
        assert_close(tuning.keys[79].unwrap(), 72.0);
        assert_close(tuning.keys[41].unwrap(), 48.0);
    }

    #[test]
    fn mappings_move_the_reference_and_skip_keys() {
        // The usual scale with A at 432 Hz
        let a432 = "! a432.kbm\n0\n0\n127\n60\n69\n432.0\n12\n";
        let tuning = Tuning::from_files(None, Some(a432)).unwrap();
        let shift = 12.0 * (432.0_f64 / 440.0).log2();
        assert_close(tuning.keys[69].unwrap(), 69.0 + shift);
        assert_close(tuning.keys[57].unwrap(), 57.0 + shift);

        // A seven note scale on the white keys only, with C at 256 Hz and the keys from C1 to
        // C7
        let scale = "C major, just\n7\n9/8\n5/4\n4/3\n3/2\n5/3\n15/8\n2/1\n";
        let white_keys = "12\n24\n96\n60\n60\n256.0\n7\n0\nx\n1\nx\n2\n3\nx\n4\nx\n5\nx\n6\n";
        let tuning = Tuning::from_files(Some(scale), Some(white_keys)).unwrap();
        let c = 69.0 + 12.0 * (256.0_f64 / 440.0).log2();
        assert_close(tuning.keys[60].unwrap(), c);
        assert_eq!(tuning.keys[61], None);
        assert_close(tuning.keys[67].unwrap(), c + 12.0 * 1.5_f64.log2());
        assert_close(tuning.keys[72].unwrap(), c + 12.0);
        assert_close(tuning.keys[59].unwrap(), c - 12.0 + 12.0 * (15.0_f64 / 8.0).log2());
        assert_close(tuning.keys[24].unwrap(), c - 36.0);
        assert_eq!(tuning.keys[23], None);
        assert_eq!(tuning.keys[97], None);

        // A pattern that stops early leaves the rest of its keys silent
        let short = "12\n0\n127\n60\n60\n261.6\n12\n0\n1\n";
        let tuning = Tuning::from_files(None, Some(short)).unwrap();
        assert!(tuning.keys[61].is_some() && tuning.keys[62].is_none() && tuning.keys[72].is_some());
    }

    #[test]
    fn broken_mappings_are_refused() {
        for (text, reason) in [
            ("12\n0\n127\n60\n", "reference key is missing"),
            ("12\n0\n128\n60\n69\n440\n12\n", "isn't a MIDI key"),
            ("12\n0\n127\n60\n69\nloud\n12\n", "isn't a frequency"),
            ("12\n0\n127\n60\n69\n-440\n12\n", "isn't a frequency"),
            ("2\n0\n127\n60\n69\n440\n12\n0\ny\n", "isn't a scale degree"),
            ("-1\n0\n127\n60\n69\n440\n12\n", "map size"),
        ] {
            let error = KeyboardMapping::parse(text).unwrap_err();
            assert!(error.contains(reason), "{text:?}: {error}");
        }
        // The reference key has to play something
        let error = Tuning::from_files(None, Some("2\n0\n127\n60\n61\n440\n12\n0\nx\n")).unwrap_err();
        assert!(error.contains("reference key"), "{error}");
        let error = Tuning::from_files(Some("Bad\n1\n"), None).unwrap_err();
        assert!(error.starts_with("Scale: "), "{error}");
    }

    #[test]
    fn huge_steps_stay_within_reach() {
        // A tenth of the octave... times a hundred
        let tuning = Tuning::from_files(Some("Wide\n1\n100000.0\n"), None).unwrap();
        assert_close(tuning.keys[60].unwrap(), 60.0);
        assert_close(tuning.keys[127].unwrap(), HIGHEST_NOTE);
        assert_close(tuning.keys[0].unwrap(), LOWEST_NOTE);
    }
}
