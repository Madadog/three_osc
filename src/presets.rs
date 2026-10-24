//! Presets: a sound's settings, kept as the parameters that differ from their defaults (so a
//! preset still loads after parameters are added, which then take their defaults), plus the
//! wavetable files the oscillators play.
//!
//! The factory presets are built into the plugin (the JSON files in `presets/`), and the
//! user's own are JSON files in their presets folder (see [`crate::folders::presets()`]). The
//! GUI loads a preset by setting the parameters through the host, as moving the sliders would,
//! so that the host keeps track of them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use nih_plug::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::params::{OscillatorWave, ThreeOscParams};
use crate::synth::envelopes::AdsrEnvelope;

/// The factory presets, in the order they are listed.
const FACTORY: &[&str] = &[
    include_str!("../presets/init.json"),
    include_str!("../presets/sub_bass.json"),
    include_str!("../presets/saw_bass.json"),
    include_str!("../presets/reese.json"),
    include_str!("../presets/fm_bass.json"),
    include_str!("../presets/wobble.json"),
    include_str!("../presets/supersaw_lead.json"),
    include_str!("../presets/square_lead.json"),
    include_str!("../presets/fm_lead.json"),
    include_str!("../presets/brass.json"),
    include_str!("../presets/warm_pad.json"),
    include_str!("../presets/glass_pad.json"),
    include_str!("../presets/strings.json"),
    include_str!("../presets/shapeshifter.json"),
    include_str!("../presets/drone.json"),
    include_str!("../presets/pluck.json"),
    include_str!("../presets/ping_pong_pluck.json"),
    include_str!("../presets/fm_bell.json"),
    include_str!("../presets/electric_piano.json"),
    include_str!("../presets/organ.json"),
    include_str!("../presets/laser.json"),
];

/// The category user presets are listed under.
pub const USER_CATEGORY: &str = "User";

/// The parameters presets leave alone: how the plugin runs rather than how it sounds (like the
/// tuning), so loading a preset doesn't change them, saving one doesn't keep them, and changing
/// them doesn't count as changing the preset.
const NOT_IN_PRESETS: &[&str] = &["oversampling"];

fn in_presets(id: &str) -> bool {
    !NOT_IN_PRESETS.contains(&id)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default)]
    pub name: String,
    /// What kind of sound it is, which the factory presets are listed by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// The parameters that differ from their defaults, by id: numbers for the sliders, `true`
    /// or `false` for the switches, and the ids of the choices for the rest (as the plugin's
    /// state stores them).
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    /// The wavetable file of each oscillator that plays one (`"osc1"` to `"osc3"`), as the
    /// oscillator's File control keeps it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

/// The factory presets. Each one is tested to load.
pub fn factory() -> Vec<Preset> {
    FACTORY.iter().filter_map(|json| serde_json::from_str(json).ok()).collect()
}

/// The user's presets in a folder, named after their files and sorted by name, and the files
/// that couldn't be read as presets.
pub fn user_presets(folder: &Path) -> (Vec<Preset>, Vec<String>) {
    let mut presets = Vec::new();
    let mut broken = Vec::new();
    for file in crate::folders::list(folder, &["json"]) {
        let read = std::fs::read_to_string(folder.join(&file)).map_err(|error| error.to_string());
        match read.and_then(|json| serde_json::from_str::<Preset>(&json).map_err(|error| error.to_string())) {
            Ok(mut preset) => {
                preset.name = crate::folders::stem(&file).to_string();
                preset.category = String::from(USER_CATEGORY);
                presets.push(preset);
            }
            Err(_) => broken.push(file),
        }
    }
    (presets, broken)
}

/// Checks that a name can be a preset's file name: not empty, and without the characters
/// file systems reserve.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(String::from("The preset needs a name."));
    }
    if name.trim() != name || name.starts_with('.') {
        return Err(String::from("The name can't start or end with a space, or start with a dot."));
    }
    match name.chars().find(|c| r#"/\:*?"<>|"#.contains(*c) || c.is_control()) {
        Some(c) => Err(format!("The name can't contain \"{c}\".")),
        None => Ok(()),
    }
}

/// Where a preset of this name is saved in a folder.
pub fn path_for(folder: &Path, name: &str) -> PathBuf {
    folder.join(format!("{name}.json"))
}

/// Saves a preset in the folder, as a file named after it, replacing any preset of the name.
pub fn save(folder: &Path, preset: &Preset) -> Result<PathBuf, String> {
    check_name(&preset.name)?;
    std::fs::create_dir_all(folder).map_err(|error| format!("Can't make the presets folder: {error}"))?;
    let path = path_for(folder, &preset.name);
    let json = serde_json::to_string_pretty(preset).map_err(|error| error.to_string())?;
    std::fs::write(&path, json + "\n").map_err(|error| format!("Can't save the preset: {error}"))?;
    Ok(path)
}

/// The current settings, as a preset.
pub fn capture(params: &ThreeOscParams, name: &str) -> Preset {
    let values = params
        .param_map()
        .into_iter()
        .filter(|(id, _, _)| in_presets(id))
        // Safety: the pointers point into `params`, which outlives this
        .filter_map(|(id, ptr, _)| unsafe { saved_value(ptr) }.map(|value| (id, value)))
        .collect();
    let files = (0..3)
        .filter(|&osc| params.oscillator(osc).wave.value() == OscillatorWave::File)
        .map(|osc| (format!("osc{}", osc + 1), params.wavetable_file(osc).path()))
        .filter(|(_, path)| !path.is_empty())
        .collect();
    Preset {
        name: name.to_string(),
        category: String::new(),
        params: values,
        files,
    }
}

/// A parameter's current value as a preset keeps it, or `None` at its default.
///
/// # Safety
///
/// The parameter `ptr` points to must be alive.
unsafe fn saved_value(ptr: ParamPtr) -> Option<Value> {
    match ptr {
        ParamPtr::FloatParam(param) => {
            let param = &*param;
            let value = param.unmodulated_plain_value();
            (value != param.default_plain_value()).then(|| Value::from(round_for_saving(value)))
        }
        ParamPtr::IntParam(param) => {
            let param = &*param;
            let value = param.unmodulated_plain_value();
            (value != param.default_plain_value()).then(|| Value::from(value))
        }
        ParamPtr::BoolParam(param) => {
            let param = &*param;
            let value = param.unmodulated_plain_value();
            (value != param.default_plain_value()).then(|| Value::from(value))
        }
        ParamPtr::EnumParam(param) => {
            let param = &*param;
            let index = param.unmodulated_plain_value();
            (index != param.default_plain_value()).then(|| match param.unmodulated_plain_id() {
                Some(id) => Value::from(id),
                None => Value::from(index),
            })
        }
    }
}

/// A slider's value as presets keep it: to six significant digits (well beyond what can be
/// heard), so that the 900 Hz a slider was set to by its normalized value is saved as 900
/// rather than 900.00037.
fn round_for_saving(value: f32) -> f64 {
    let rounded = format!("{value:.5e}").parse::<f64>().unwrap_or(value as f64);
    // (Without a stray tail from widening to `f64`)
    format!("{}", rounded as f32).parse().unwrap_or(rounded)
}

/// What a preset sets the parameters to: the normalized value of every parameter presets keep,
/// with the default for those the preset leaves out, and the wavetable file of each oscillator.
pub struct Targets {
    pub values: Vec<(ParamPtr, f32)>,
    pub files: [String; 3],
    /// What in the preset couldn't be used (parameters or values that don't exist).
    pub problems: Vec<String>,
}

impl Targets {
    pub fn new(preset: &Preset, params: &ThreeOscParams, enum_ids: &EnumIds) -> Self {
        let mut problems = Vec::new();
        let map = params.param_map();
        let preset_params = upgraded(&preset.params);
        let values = map
            .iter()
            .filter(|(id, _, _)| in_presets(id))
            .map(|(id, ptr, _)| {
                // Safety: the pointers point into `params`, which outlives this
                let default = unsafe { ptr.default_normalized_value() };
                let target = match preset_params.get(id) {
                    Some(value) => unsafe { normalized(*ptr, id, value, enum_ids) }.unwrap_or_else(|| {
                        problems.push(format!("{id} can't be {value}"));
                        default
                    }),
                    None => default,
                };
                (*ptr, target)
            })
            .collect();
        for id in preset_params.keys() {
            if !map.iter().any(|(known, _, _)| known == id) {
                problems.push(format!("there's no parameter {id}"));
            }
        }
        for key in preset.files.keys() {
            if !["osc1", "osc2", "osc3"].contains(&key.as_str()) {
                problems.push(format!("there's no oscillator {key}"));
            }
        }
        let files = std::array::from_fn(|osc| {
            preset.files.get(&format!("osc{}", osc + 1)).cloned().unwrap_or_default()
        });
        Self { values, files, problems }
    }

    /// Whether the parameters (and files) are where the preset puts them.
    pub fn matches(&self, params: &ThreeOscParams) -> bool {
        // Safety: the pointers point into the parameters this was made for
        self.values
            .iter()
            .all(|(ptr, target)| (unsafe { ptr.unmodulated_normalized_value() } - target).abs() < 1e-5)
            && (0..3).all(|osc| {
                // The file only matters while the oscillator plays it
                params.oscillator(osc).wave.value() != OscillatorWave::File
                    || params.wavetable_file(osc).path() == self.files[osc]
            })
    }
}

/// A preset's parameters, with the single slope of each envelope in presets saved before each
/// stage had its own split into the three slopes that make the same curves (see
/// [`crate::params::LEGACY_SLOPES`]).
fn upgraded(params: &BTreeMap<String, Value>) -> std::borrow::Cow<'_, BTreeMap<String, Value>> {
    if !crate::params::LEGACY_SLOPES.iter().any(|(old, _)| params.contains_key(*old)) {
        return std::borrow::Cow::Borrowed(params);
    }
    let mut params = params.clone();
    for (old, new) in crate::params::LEGACY_SLOPES {
        if let Some(slope) = params.remove(old).and_then(|slope| slope.as_f64()) {
            for (id, value) in new.iter().zip(AdsrEnvelope::slopes_for(slope as f32)) {
                params.entry(id.to_string()).or_insert(Value::from(value as f64));
            }
        }
    }
    std::borrow::Cow::Owned(params)
}

/// A preset's value for a parameter, as its normalized value, or `None` if the parameter can't
/// take it.
///
/// # Safety
///
/// The parameter `ptr` points to must be alive.
unsafe fn normalized(ptr: ParamPtr, id: &str, value: &Value, enum_ids: &EnumIds) -> Option<f32> {
    match ptr {
        ParamPtr::FloatParam(param) => Some((*param).preview_normalized(value.as_f64()? as f32)),
        ParamPtr::IntParam(param) => Some((*param).preview_normalized(value.as_f64()?.round() as i32)),
        ParamPtr::BoolParam(param) => Some((*param).preview_normalized(value.as_bool()?)),
        ParamPtr::EnumParam(param) => {
            let index = match value {
                Value::String(choice) => enum_ids.index(id, choice)?,
                _ => i32::try_from(value.as_i64()?).ok()?,
            };
            ((0..(*param).len() as i32).contains(&index)).then(|| (*param).preview_normalized(index))
        }
    }
}

/// Finds the index of an enum parameter's choice from its id. nih-plug only tells a
/// parameter's id for its current value, so this keeps a set of parameters of its own to try
/// the ids on (the plugin's own can only be set through the host).
pub struct EnumIds {
    by_id: HashMap<String, ParamPtr>,
    /// What the pointers point into, kept alive (and in place) with them.
    _params: Box<ThreeOscParams>,
}

impl EnumIds {
    pub fn new() -> Self {
        let params = Box::new(ThreeOscParams::default());
        let by_id = params
            .param_map()
            .into_iter()
            .filter(|(_, ptr, _)| matches!(ptr, ParamPtr::EnumParam(_)))
            .map(|(id, ptr, _)| (id, ptr))
            .collect();
        Self { by_id, _params: params }
    }

    /// The index of the choice with the id `choice` of the enum parameter `param`.
    pub fn index(&self, param: &str, choice: &str) -> Option<i32> {
        match self.by_id.get(param)? {
            // Safety: the pointer points into `self._params`
            ParamPtr::EnumParam(inner) => unsafe {
                let inner = &**inner;
                inner.set_from_id(choice).then(|| inner.unmodulated_plain_value())
            },
            _ => None,
        }
    }
}

impl Default for EnumIds {
    fn default() -> Self {
        Self::new()
    }
}

/// A preset's values as the parameters would have them once the host had set them, for testing
/// presets without a host.
#[cfg(test)]
pub struct PresetValues(HashMap<ParamPtr, f32>);

#[cfg(test)]
impl PresetValues {
    pub fn new(preset: &Preset, params: &ThreeOscParams) -> Self {
        let targets = Targets::new(preset, params, &EnumIds::new());
        assert!(targets.problems.is_empty(), "{}: {:?}", preset.name, targets.problems);
        Self(targets.values.into_iter().collect())
    }
}

#[cfg(test)]
impl crate::params::ParamValues for PresetValues {
    fn get<P: Param>(&self, param: &P) -> P::Plain {
        match self.0.get(&param.as_ptr()) {
            Some(normalized) => param.preview_plain(*normalized),
            None => param.modulated_plain_value(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_presets_load_completely() {
        assert_eq!(factory().len(), FACTORY.len(), "a factory preset doesn't parse");
        let params = ThreeOscParams::default();
        let enum_ids = EnumIds::new();
        let presets = factory();
        assert_eq!(presets[0].name, "Init");
        assert!(presets[0].params.is_empty());
        for preset in &presets {
            assert!(!preset.name.is_empty() && !preset.category.is_empty(), "{preset:?}");
            assert_eq!(presets.iter().filter(|other| other.name == preset.name).count(), 1);
            check_name(&preset.name).unwrap();
            let targets = Targets::new(preset, &params, &enum_ids);
            assert!(targets.problems.is_empty(), "{}: {:?}", preset.name, targets.problems);
            // Every value is within its parameter's range, and not a default (which would be
            // left out)
            let map = params.param_map();
            for (id, value) in &preset.params {
                let (_, ptr, _) = map.iter().find(|(known, _, _)| known == id).unwrap();
                let (_, target) = targets.values.iter().find(|(other, _)| other == ptr).unwrap();
                let (plain, default) = unsafe { (ptr.preview_plain(*target), ptr.default_normalized_value()) };
                if let Some(number) = value.as_f64() {
                    let tolerance = 1e-4 * number.abs().max(1.0);
                    assert!((plain as f64 - number).abs() <= tolerance, "{}: {id} = {number} is out of range", preset.name);
                }
                assert!((target - default).abs() > 1e-6, "{}: {id} is at its default", preset.name);
            }
            // Factory presets can't count on files
            assert!(preset.files.is_empty());
        }
    }

    #[test]
    fn presets_capture_what_differs_from_the_defaults() {
        let params = ThreeOscParams::default();
        let preset = capture(&params, "Nothing");
        assert!(preset.params.is_empty() && preset.files.is_empty());
        // Only the choices can be set without a host, through their ids
        let ids = EnumIds::new();
        let map = params.param_map();
        let set = |id: &str, choice: &str| {
            let (_, ptr, _) = map.iter().find(|(known, _, _)| known == id).unwrap();
            let ParamPtr::EnumParam(inner) = ptr else { panic!("{id} isn't a choice") };
            assert!(unsafe { (**inner).set_from_id(choice) });
        };
        set("osc1_wave", "file");
        set("mod_source_3", "lfo2");
        set("mod_target_3", "cutoff");
        params.osc1_file.set_path(String::from("table.wav"));
        let preset = capture(&params, "Some");
        assert_eq!(preset.params.len(), 3);
        assert_eq!(preset.params["osc1_wave"], "file");
        assert_eq!(preset.params["mod_target_3"], "cutoff");
        assert_eq!(preset.files["osc1"], "table.wav");

        // ...and the preset puts them back
        let targets = Targets::new(&preset, &params, &ids);
        assert!(targets.problems.is_empty());
        assert!(targets.matches(&params));
        assert_eq!(targets.files[0], "table.wav");
        let targets = Targets::new(&factory()[0], &params, &ids);
        assert!(!targets.matches(&params));
        // The source is in the second slot of choices after Off
        assert_eq!(ids.index("mod_source_3", "lfo2"), Some(2));
        assert_eq!(ids.index("mod_source_3", "nonsense"), None);
        assert_eq!(ids.index("osc1_amp", "lfo2"), None);
    }

    #[test]
    fn presets_leave_the_oversampling_alone() {
        let params = ThreeOscParams::default();
        let ids = EnumIds::new();
        let ParamPtr::EnumParam(oversampling) = params.global.oversampling.as_ptr() else { unreachable!() };
        assert!(unsafe { (*oversampling).set_from_id("4x") });
        // It isn't saved, and doesn't keep the settings from matching a preset without it
        assert!(capture(&params, "Oversampled").params.is_empty());
        assert!(Targets::new(&factory()[0], &params, &ids).matches(&params));
        // ...and loading a preset doesn't set it, even one that has it
        let mut preset = factory()[0].clone();
        preset.params.insert(String::from("oversampling"), Value::from("2x"));
        let targets = Targets::new(&preset, &params, &ids);
        assert!(targets.problems.is_empty(), "{:?}", targets.problems);
        assert!(targets.values.iter().all(|(ptr, _)| *ptr != params.global.oversampling.as_ptr()));
        assert!(targets.matches(&params));
    }

    /// Presets saved with an envelope's single slope load with the three slopes that make the
    /// same curves.
    #[test]
    fn presets_with_one_slope_load_its_curves() {
        let params = ThreeOscParams::default();
        let mut preset = Preset {
            name: String::from("Old"),
            ..Default::default()
        };
        preset.params.insert(String::from("vol_slope"), Value::from(2.0));
        preset.params.insert(String::from("env_slope_2"), Value::from(-4.0));
        let targets = Targets::new(&preset, &params, &EnumIds::new());
        assert!(targets.problems.is_empty(), "{:?}", targets.problems);
        let map = params.param_map();
        let plain = |id: &str| {
            let (_, ptr, _) = map.iter().find(|(known, _, _)| known == id).unwrap();
            let (_, target) = targets.values.iter().find(|(other, _)| other == ptr).unwrap();
            unsafe { ptr.preview_plain(*target) }
        };
        assert_eq!([plain("vol_attack_slope"), plain("vol_decay_slope"), plain("vol_release_slope")], [0.5, 2.0, 2.0]);
        assert_eq!(
            [plain("env_attack_slope_2"), plain("env_decay_slope_2"), plain("env_release_slope_2")],
            [-4.0, -1.0, -1.0]
        );
        // The others keep their defaults
        assert_eq!([plain("fil1_attack_slope"), plain("fil1_decay_slope")], [0.25, 1.0]);
    }

    #[test]
    fn values_round_trip_through_json() {
        let params = ThreeOscParams::default();
        let ids = EnumIds::new();
        let mut preset = Preset {
            name: String::from("Round Trip"),
            ..Default::default()
        };
        preset.params.insert(String::from("fil1_cutoff"), Value::from(1234.5));
        preset.params.insert(String::from("osc2_voices"), Value::from(7));
        preset.params.insert(String::from("delay_on"), Value::from(true));
        preset.params.insert(String::from("osc3_wave"), Value::from("pw_sweep"));
        // Choices can be given as their index too
        preset.params.insert(String::from("fil1_type"), Value::from(2));
        let json = serde_json::to_string_pretty(&preset).unwrap();
        let read: Preset = serde_json::from_str(&json).unwrap();
        assert_eq!(read, preset);
        let targets = Targets::new(&read, &params, &ids);
        assert!(targets.problems.is_empty(), "{:?}", targets.problems);
        let plain = |id: &str| {
            let map = params.param_map();
            let (_, ptr, _) = map.iter().find(|(known, _, _)| known == id).unwrap();
            let (_, target) = targets.values.iter().find(|(other, _)| other == ptr).unwrap();
            unsafe { ptr.preview_plain(*target) }
        };
        assert!((plain("fil1_cutoff") - 1234.5).abs() < 0.01);
        assert_eq!(plain("osc2_voices"), 7.0);
        assert_eq!(plain("delay_on"), 1.0);
        assert_eq!(plain("osc3_wave"), OscillatorWave::PwSweep.to_index() as f32);
        assert_eq!(plain("fil1_type"), 2.0);
        // Everything else goes back to its default
        assert_eq!(plain("osc1_amp"), 100.0);

        // What doesn't fit is reported, and left at the default
        preset.params.insert(String::from("osc4_amp"), Value::from(1.0));
        preset.params.insert(String::from("osc1_wave"), Value::from("kazoo"));
        preset.params.insert(String::from("fil1_model"), Value::from(99));
        preset.params.insert(String::from("dist_on"), Value::from("yes"));
        preset.files.insert(String::from("osc9"), String::from("x.wav"));
        let targets = Targets::new(&preset, &params, &ids);
        assert_eq!(targets.problems.len(), 5, "{:?}", targets.problems);
    }

    #[test]
    fn saved_floats_are_rounded_to_six_digits() {
        assert_eq!(round_for_saving(900.00037), 900.0);
        assert_eq!(round_for_saving(0.120000005), 0.12);
        assert_eq!(round_for_saving(-17.25), -17.25);
        assert_eq!(round_for_saving(1234.5678), 1234.57);
        assert_eq!(round_for_saving(1e-6), 1e-6);
        assert_eq!(Value::from(round_for_saving(0.14999998)).to_string(), "0.15");
    }

    #[test]
    fn names_must_make_file_names() {
        assert!(check_name("Warm Pad 2").is_ok());
        assert!(check_name("Bass (dark)").is_ok());
        for bad in ["", "  ", " Pad", "Pad ", ".hidden", "a/b", "a\\b", "what?", "x:y", "tab\there"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn user_presets_are_saved_and_listed() {
        let dir = std::env::temp_dir().join(format!("three_osc_presets_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(user_presets(&dir), (Vec::new(), Vec::new()));
        let mut preset = factory()[3].clone();
        preset.name = String::from("Mine");
        let path = save(&dir, &preset).unwrap();
        assert_eq!(path, path_for(&dir, "Mine"));
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        let (presets, broken) = user_presets(&dir);
        assert_eq!(broken, vec!["broken.json"]);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Mine");
        assert_eq!(presets[0].category, USER_CATEGORY);
        assert_eq!(presets[0].params, preset.params);
        // Saving under the same name replaces it
        preset.params.clear();
        save(&dir, &preset).unwrap();
        assert!(user_presets(&dir).0[0].params.is_empty());
        assert!(save(&dir, &Preset { name: String::from("a/b"), ..Default::default() }).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
