//! The tuning window, opened from the title bar: the Scala files in the tunings folder, scales
//! worked out from a few numbers (see [`generate`]), and a look at the scale the synth plays,
//! with a way to save it as a `.scl` file.

use std::path::{Path, PathBuf};

use nih_plug_egui::egui::{self, Color32, RichText, Stroke, StrokeKind, Ui};

use super::MOD_COLOR;
use crate::folders;
use crate::params::{TextFile, ThreeOscParams, TuningFiles};
use crate::presets::check_name;
use crate::synth::tuning::generate::{self, Generated, Temperament};
use crate::synth::tuning::{KeyboardMapping, Pitch, Scale, MAX_DEGREES};

const ERROR_COLOR: Color32 = Color32::from_rgb(255, 120, 100);
const WINDOW_WIDTH: f32 = 400.0;
/// The note names counted from C, for the scale's table.
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Files,
    Generate,
    Scale,
}

/// The kinds of scale the Generate tab makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    EqualDivisions,
    Harmonics,
    Subharmonics,
    Stack,
    Historical,
}

impl Kind {
    const ALL: [Self; 5] = [Self::EqualDivisions, Self::Harmonics, Self::Subharmonics, Self::Stack, Self::Historical];

    fn name(self) -> &'static str {
        match self {
            Self::EqualDivisions => "Equal divisions",
            Self::Harmonics => "Harmonic series",
            Self::Subharmonics => "Subharmonic series",
            Self::Stack => "Generator stack",
            Self::Historical => "Historical temperament",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::EqualDivisions => {
                "Equal steps that add up to an interval: an octave (2/1) for the EDOs, a tritave \
                 (3/1) for Bohlen-Pierce, a fifth (3/2), or one step of so many cents."
            }
            Self::Harmonics => {
                "The harmonics of a note, from one to another: 8 to 16 is an octave of eight steps \
                 that shrink as they go up."
            }
            Self::Subharmonics => "The harmonic series upside down: steps that grow as they go up.",
            Self::Stack => {
                "A chain of one interval (the generator), each note moved by whole periods to \
                 within a period of the root: pure fifths give Pythagorean scales, slightly \
                 narrow ones meantone."
            }
            Self::Historical => "Well temperaments and meantone of the 17th and 18th centuries, and just intonation.",
        }
    }
}

/// What the tuning window shows and is doing.
pub struct TuningWindow {
    open: bool,
    tab: Tab,
    /// Where the window first opens: under the title bar's button.
    anchor: egui::Pos2,

    // The Files tab
    folder: PathBuf,
    scales: Vec<String>,
    mappings: Vec<String>,
    scanned: bool,
    /// A path typed in, to load a file from anywhere else.
    custom_path: String,
    /// What went wrong loading or making the last tuning.
    message: Option<String>,

    // The Generate tab
    kind: Kind,
    divisions: u32,
    /// The interval the divisions add up to, as typed in.
    divided: String,
    /// The lowest and highest harmonics.
    harmonics: (u32, u32),
    /// The highest and lowest subharmonics.
    subharmonics: (u32, u32),
    generator: String,
    period: String,
    notes: u32,
    /// How many of the generators go down from the root.
    down: u32,
    temperament: Temperament,

    // The Scale tab
    export_name: String,
    /// The scale file the export name was filled in from.
    export_for: Option<String>,
    /// What came of the last export, and whether it went wrong.
    export_message: Option<(String, bool)>,
}

impl TuningWindow {
    pub fn new() -> Self {
        Self {
            open: false,
            tab: Tab::Files,
            anchor: egui::Pos2::new(400.0, 30.0),
            folder: folders::tunings(),
            scales: Vec::new(),
            mappings: Vec::new(),
            scanned: false,
            custom_path: String::new(),
            message: None,
            kind: Kind::EqualDivisions,
            divisions: 19,
            divided: String::from("2/1"),
            harmonics: (8, 16),
            subharmonics: (16, 8),
            generator: String::from("3/2"),
            period: String::from("2/1"),
            notes: 12,
            down: 3,
            temperament: Temperament::WerckmeisterIii,
            export_name: String::new(),
            export_for: None,
            export_message: None,
        }
    }

    fn rescan(&mut self) {
        self.scales = folders::list(&self.folder, &["scl"]);
        self.mappings = folders::list(&self.folder, &["kbm"]);
        self.scanned = true;
    }

    /// Tunes the synth with a scale (`.scl`) or keyboard mapping (`.kbm`) file, keeping the
    /// other one.
    fn load(&mut self, params: &ThreeOscParams, path: &Path) {
        let mapping = path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("kbm"));
        let result = TextFile::read(path).and_then(|file| {
            let mut files = params.tuning.files();
            if mapping {
                files.mapping = Some(file);
            } else {
                files.scale = Some(file);
            }
            params.tuning.try_set(files)
        });
        self.message = result.err().map(|error| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            format!("{name}: {error}")
        });
    }

    fn set(&mut self, params: &ThreeOscParams, files: TuningFiles) {
        self.message = params.tuning.try_set(files).err();
    }

    /// The scale the Generate tab's settings make.
    fn generated(&self) -> Result<Generated, String> {
        let interval = |text: &str, what: &str| generate::parse_interval(text).map_err(|error| format!("{what}: {error}"));
        match self.kind {
            Kind::EqualDivisions => generate::equal_divisions(self.divisions, interval(&self.divided, "Interval")?),
            Kind::Harmonics => generate::harmonic_series(self.harmonics.0, self.harmonics.1),
            Kind::Subharmonics => generate::subharmonic_series(self.subharmonics.0, self.subharmonics.1),
            Kind::Stack => generate::generator_stack(
                interval(&self.generator, "Generator")?,
                interval(&self.period, "Period")?,
                self.notes,
                self.down,
            ),
            Kind::Historical => Ok(self.temperament.generate()),
        }
    }

    /// Tunes the synth with a generated scale, keeping the keyboard mapping.
    fn play(&mut self, params: &ThreeOscParams, generated: &Generated) {
        let file_name = format!("{}.scl", generated.name);
        let scale = TextFile {
            text: generated.scale.to_scl(&file_name),
            name: file_name,
        };
        self.set(params, TuningFiles { scale: Some(scale), ..params.tuning.files() });
    }
}

impl Default for TuningWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// What the tuning is called: the scale's file name, and the mapping's when there is one.
pub fn tuning_name(files: &TuningFiles) -> String {
    let scale = files.scale.as_ref().map_or("12-TET", |file| folders::stem(&file.name));
    match &files.mapping {
        Some(mapping) => format!("{scale}, {}", folders::stem(&mapping.name)),
        None => scale.to_string(),
    }
}

/// The title bar's button, which opens and closes the window.
pub fn button(ui: &mut Ui, params: &ThreeOscParams, window: &mut TuningWindow) {
    let mut name = tuning_name(&params.tuning.files());
    if name.chars().count() > 28 {
        name = name.chars().take(27).collect::<String>() + "…";
    }
    let button = ui
        .selectable_label(window.open, format!("Tuning: {name}"))
        .on_hover_text(
            "How the keys are tuned: the usual twelve-tone equal temperament, or a Scala scale \
             and keyboard mapping, from a file or made here. Click to open the tuning window.",
        );
    window.anchor = button.rect.left_bottom() + egui::vec2(0.0, 6.0);
    if button.clicked() {
        window.open = !window.open;
        if window.open && !window.scanned {
            window.rescan();
        }
    }
}

/// The window itself, while it's open.
pub fn show(ctx: &egui::Context, params: &ThreeOscParams, window: &mut TuningWindow) {
    let mut open = window.open;
    egui::Window::new("Tuning")
        .open(&mut open)
        .default_pos(window.anchor)
        .default_width(WINDOW_WIDTH)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.set_width(WINDOW_WIDTH);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut window.tab, Tab::Files, RichText::new("Files").strong())
                    .on_hover_text("The scales and keyboard mappings in the tunings folder.");
                ui.selectable_value(&mut window.tab, Tab::Generate, RichText::new("Generate").strong())
                    .on_hover_text("Make a scale from a few numbers.");
                ui.selectable_value(&mut window.tab, Tab::Scale, RichText::new("Scale").strong())
                    .on_hover_text("The scale the synth plays, step by step, and saving it as a .scl file.");
            });
            ui.separator();
            match window.tab {
                Tab::Files => files_tab(ui, params, window),
                Tab::Generate => generate_tab(ui, params, window),
                Tab::Scale => scale_tab(ui, params, window),
            }
            if let Some(message) = &window.message {
                ui.add(egui::Label::new(RichText::new(message).small().color(ERROR_COLOR)).wrap());
            }
            if let Some(error) = params.tuning.error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "The saved tuning doesn't work ({error}), so the keys play the standard tuning."
                        ))
                        .small()
                        .color(ERROR_COLOR),
                    )
                    .wrap(),
                );
            }
        });
    window.open = open;
}

/// The scales and mappings in the tunings folder, and loading a file from elsewhere.
fn files_tab(ui: &mut Ui, params: &ThreeOscParams, window: &mut TuningWindow) {
    let files = params.tuning.files();
    let mut load = None;
    ui.label(RichText::new("Scale").strong()).on_hover_text(
        "A Scala .scl file: the pitches of the scale's steps. The keys go up the scale a step at \
         a time, from its root on middle C (at 261.63 Hz) unless a keyboard mapping says \
         otherwise. Pitch bend, the oscillators' Detune and Octave, and the Pitch modulation \
         still move the pitch in ordinary semitones.",
    );
    let current_scale = files.scale.as_ref().map(|file| file.name.as_str());
    egui::ScrollArea::vertical().id_salt("scales").max_height(150.0).show(ui, |ui| {
        if ui.selectable_label(current_scale.is_none(), "12-tone equal temperament (standard)").clicked() {
            window.set(params, TuningFiles { scale: None, ..files.clone() });
        }
        for scale in &window.scales {
            if ui.selectable_label(current_scale == Some(scale.as_str()), folders::stem(scale)).clicked() {
                load = Some(window.folder.join(scale));
            }
        }
        // A file loaded from elsewhere, or made in the Generate tab
        if let Some(name) = current_scale.filter(|name| !window.scales.iter().any(|scale| scale == name)) {
            let _ = ui.selectable_label(true, folders::stem(name));
        }
    });
    if window.scales.is_empty() {
        ui.label(RichText::new("No .scl files in the tunings folder").small().weak());
    }

    ui.add_space(4.0);
    ui.label(RichText::new("Keyboard Mapping").strong()).on_hover_text(
        "A Scala .kbm file: which key plays which step of the scale (some may be left silent), \
         and which key is tuned to which frequency. It can retune the standard scale too, to \
         move A away from 440 Hz, say.",
    );
    let current_mapping = files.mapping.as_ref().map(|file| file.name.as_str());
    egui::ScrollArea::vertical().id_salt("mappings").max_height(90.0).show(ui, |ui| {
        if ui
            .selectable_label(current_mapping.is_none(), "A step per key, root on middle C (standard)")
            .clicked()
        {
            window.set(params, TuningFiles { mapping: None, ..files.clone() });
        }
        for mapping in &window.mappings {
            if ui.selectable_label(current_mapping == Some(mapping.as_str()), folders::stem(mapping)).clicked() {
                load = Some(window.folder.join(mapping));
            }
        }
        if let Some(name) = current_mapping.filter(|name| !window.mappings.iter().any(|mapping| mapping == name)) {
            let _ = ui.selectable_label(true, folders::stem(name));
        }
    });
    if window.mappings.is_empty() {
        ui.label(RichText::new("No .kbm files in the tunings folder").small().weak());
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Tunings folder").small()).on_hover_text(
            "Documents/Three Osc/Tunings in your home directory, or wherever the \
             THREE_OSC_TUNINGS environment variable points.",
        );
        if ui.small_button("Rescan").clicked() {
            window.rescan();
        }
    });
    ui.add(egui::Label::new(RichText::new(window.folder.to_string_lossy()).small().weak()).wrap());
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut window.custom_path)
                .hint_text("Path to a .scl or .kbm file")
                .desired_width(WINDOW_WIDTH - 60.0),
        );
        if ui.button("Load").clicked() && !window.custom_path.trim().is_empty() {
            load = Some(PathBuf::from(window.custom_path.trim()));
        }
    });
    if let Some(path) = load {
        window.load(params, &path);
    }
}

/// The settings for each kind of generated scale, a look at what they make, and playing it.
fn generate_tab(ui: &mut Ui, params: &ThreeOscParams, window: &mut TuningWindow) {
    ui.horizontal(|ui| {
        ui.label("Make");
        egui::ComboBox::from_id_salt("generator kind")
            .selected_text(window.kind.name())
            .width(200.0)
            .show_ui(ui, |ui| {
                for kind in Kind::ALL {
                    ui.selectable_value(&mut window.kind, kind, kind.name()).on_hover_text(kind.help());
                }
            });
    });
    ui.add(egui::Label::new(RichText::new(window.kind.help()).small().weak()).wrap());
    ui.add_space(4.0);

    let max = MAX_DEGREES as u32;
    let interval_help = "A ratio (3/2, or 3 for 3/1), or cents (701.955, or 700c).";
    egui::Grid::new("generator settings").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        match window.kind {
            Kind::EqualDivisions => {
                ui.label("Steps");
                ui.add(egui::DragValue::new(&mut window.divisions).range(1..=max));
                ui.end_row();
                ui.label("Of").on_hover_text(interval_help);
                ui.add(egui::TextEdit::singleline(&mut window.divided).desired_width(110.0))
                    .on_hover_text(interval_help);
                ui.end_row();
            }
            Kind::Harmonics => {
                ui.label("From harmonic");
                ui.add(egui::DragValue::new(&mut window.harmonics.0).range(1..=max));
                ui.end_row();
                ui.label("Up to");
                ui.add(egui::DragValue::new(&mut window.harmonics.1).range(2..=2 * max));
                ui.end_row();
            }
            Kind::Subharmonics => {
                ui.label("From subharmonic");
                ui.add(egui::DragValue::new(&mut window.subharmonics.0).range(2..=2 * max));
                ui.end_row();
                ui.label("Down to");
                ui.add(egui::DragValue::new(&mut window.subharmonics.1).range(1..=max));
                ui.end_row();
            }
            Kind::Stack => {
                ui.label("Generator").on_hover_text(interval_help);
                ui.add(egui::TextEdit::singleline(&mut window.generator).desired_width(110.0))
                    .on_hover_text(interval_help);
                ui.end_row();
                ui.label("Period").on_hover_text("The interval the scale repeats at, usually an octave (2/1).");
                ui.add(egui::TextEdit::singleline(&mut window.period).desired_width(110.0))
                    .on_hover_text(interval_help);
                ui.end_row();
                ui.label("Notes");
                ui.add(egui::DragValue::new(&mut window.notes).range(1..=max));
                ui.end_row();
                ui.label("Below the root").on_hover_text(
                    "How many generators go down from the root rather than up: with fifths, 1 \
                     makes the major scale out of 7 notes, and 3 the notes from Eb to G# out of 12.",
                );
                window.down = window.down.min(window.notes.saturating_sub(1));
                ui.add(egui::DragValue::new(&mut window.down).range(0..=window.notes.saturating_sub(1)));
                ui.end_row();
            }
            Kind::Historical => {
                ui.label("Temperament");
                egui::ComboBox::from_id_salt("temperament")
                    .selected_text(window.temperament.name())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for temperament in Temperament::ALL {
                            ui.selectable_value(&mut window.temperament, temperament, temperament.name())
                                .on_hover_text(temperament.description());
                        }
                    });
                ui.end_row();
            }
        }
    });

    // Some well known settings to start from (the temperaments are all well known)
    if window.kind != Kind::Historical {
        examples(ui, window);
    }

    ui.separator();
    match window.generated() {
        Ok(generated) => {
            ui.label(RichText::new(&generated.name).strong());
            ui.add(egui::Label::new(RichText::new(&generated.scale.description).small().weak()).wrap());
            ui.label(RichText::new(generate::summarize_steps(&generated.scale)).small());
            ruler(ui, &generated.scale);
            ui.horizontal(|ui| {
                if ui.button("Play this scale").on_hover_text("Tunes the synth to this scale.").clicked() {
                    window.play(params, &generated);
                }
                if let Some(mapping) = &params.tuning.files().mapping {
                    ui.label(
                        RichText::new(format!("through the mapping {}", folders::stem(&mapping.name)))
                            .small()
                            .weak(),
                    );
                }
            });
        }
        Err(error) => {
            ui.add(egui::Label::new(RichText::new(error).color(ERROR_COLOR)).wrap());
        }
    }
}

/// Buttons that fill in well known settings for the kind of scale being made.
fn examples(ui: &mut Ui, window: &mut TuningWindow) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Try").small().weak());
        let example = |ui: &mut Ui, label: &str, help: &str| ui.small_button(label).on_hover_text(help).clicked();
        match window.kind {
            Kind::EqualDivisions => {
                for steps in [12, 19, 22, 24, 31, 41, 53] {
                    if example(ui, &format!("{steps}-EDO"), "Equal divisions of the octave.") {
                        (window.divisions, window.divided) = (steps, String::from("2/1"));
                    }
                }
                if example(ui, "Bohlen-Pierce", "13 equal divisions of the tritave (3/1).") {
                    (window.divisions, window.divided) = (13, String::from("3/1"));
                }
                if example(ui, "Carlos alpha", "9 equal divisions of the fifth (3/2), 78 cents a step.") {
                    (window.divisions, window.divided) = (9, String::from("3/2"));
                }
            }
            Kind::Harmonics => {
                for low in [4, 8, 12, 16] {
                    if example(ui, &format!("{low} to {}", 2 * low), "An octave of the harmonic series.") {
                        window.harmonics = (low, 2 * low);
                    }
                }
            }
            Kind::Subharmonics => {
                for high in [8, 16, 24] {
                    if example(ui, &format!("{high} to {}", high / 2), "An octave of the subharmonic series.") {
                        window.subharmonics = (high, high / 2);
                    }
                }
            }
            Kind::Stack => {
                let examples = [
                    ("Pythagorean", "Twelve pure fifths, Eb to G#.", "3/2", 12, 3),
                    ("Major", "Seven pure fifths: the major scale.", "3/2", 7, 1),
                    ("Pentatonic", "Five pure fifths.", "3/2", 5, 0),
                    ("Meantone", "Twelve quarter-comma meantone fifths, Eb to G#.", "696.578", 12, 3),
                    ("Golden", "Golden meantone: fifths of 696.214 cents, whose whole and half steps are in the golden ratio.", "696.214", 12, 3),
                ];
                for (label, help, generator, notes, down) in examples {
                    if example(ui, label, help) {
                        window.generator = String::from(generator);
                        window.period = String::from("2/1");
                        (window.notes, window.down) = (notes, down);
                    }
                }
            }
            Kind::Historical => {}
        }
    });
}

/// The scale the synth plays, step by step, and saving it as a `.scl` file.
fn scale_tab(ui: &mut Ui, params: &ThreeOscParams, window: &mut TuningWindow) {
    let files = params.tuning.files();
    let (file_name, text) = match &files.scale {
        Some(file) => (file.name.clone(), file.text.clone()),
        None => {
            let name = String::from("12-TET.scl");
            let text = Scale::equal_temperament().to_scl(&name);
            (name, text)
        }
    };
    let scale = match Scale::parse(&text) {
        Ok(scale) => scale,
        Err(error) => {
            ui.label(RichText::new(format!("{file_name}: {error}")).color(ERROR_COLOR));
            return;
        }
    };
    let mapping = files
        .mapping
        .as_ref()
        .and_then(|file| KeyboardMapping::parse(&file.text).ok())
        .unwrap_or_else(KeyboardMapping::standard);

    ui.label(RichText::new(folders::stem(&file_name)).strong());
    if !scale.description.is_empty() {
        ui.add(egui::Label::new(RichText::new(&scale.description).small().weak()).wrap());
    }
    let root = mapping.middle_key;
    let root_pitch = params.tuning.pitches()[root as usize];
    let root_text = if root_pitch.is_nan() {
        format!("root on {}", key_name(root))
    } else {
        format!("root on {}, {:.2} Hz", key_name(root), 440.0 * ((root_pitch - 69.0) / 12.0).exp2())
    };
    ui.label(
        RichText::new(format!(
            "{}, repeating at {}; {root_text}",
            generate::summarize_steps(&scale),
            scale.period().to_scl()
        ))
        .small(),
    );
    ruler(ui, &scale);

    // The degrees, a row each (only the rows in view are laid out, for scales of thousands)
    let font = egui::TextStyle::Monospace;
    let row_height = ui.text_style_height(&font);
    let row = |degree: &str, cents: &str, step: &str, ratio: &str, nearest: &str| {
        format!("{degree:>4} {cents:>10} {step:>9} {ratio:>12} {nearest:>10}")
    };
    ui.label(RichText::new(row("#", "cents", "step", "ratio", "12-TET")).monospace().weak())
        .on_hover_text(
            "Each degree's pitch above the root in cents, the step up to it from the degree \
             before, its ratio (when the scale gives one) and the nearest note of twelve-tone \
             equal temperament counting from the root's key, with how far off it is in cents.",
        );
    let cents = scale.cents();
    egui::ScrollArea::vertical()
        .id_salt("degrees")
        .max_height(170.0)
        .auto_shrink([false, true])
        .show_rows(ui, row_height, scale.len() + 1, |ui, rows| {
            for degree in rows {
                let (pitch, step, ratio) = match degree {
                    0 => (0.0, String::new(), String::from("1/1")),
                    _ => {
                        let previous = if degree == 1 { 0.0 } else { cents[degree - 2] };
                        let ratio = match scale.pitches[degree - 1] {
                            Pitch::Ratio(..) => scale.pitches[degree - 1].to_scl(),
                            Pitch::Cents(_) => String::new(),
                        };
                        (cents[degree - 1], format!("{:.3}", cents[degree - 1] - previous), ratio)
                    }
                };
                let ratio = if ratio.chars().count() > 12 { String::from("…") } else { ratio };
                let text = row(&degree.to_string(), &format!("{pitch:.3}"), &step, &ratio, &nearest(root, pitch));
                ui.label(RichText::new(text).monospace());
            }
        });

    // Saving it
    ui.separator();
    if window.export_for.as_deref() != Some(file_name.as_str()) {
        window.export_name = folders::stem(&file_name).to_string();
        window.export_for = Some(file_name.clone());
        window.export_message = None;
    }
    let name = window.export_name.trim().to_string();
    let problem = check_name(&name).err();
    let exists = problem.is_none() && window.folder.join(format!("{name}.scl")).exists();
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut window.export_name).desired_width(170.0))
            .on_hover_text("The name of the .scl file to save, in the tunings folder.");
        ui.label(".scl");
        let save = ui
            .add_enabled(problem.is_none(), egui::Button::new(if exists { "Replace" } else { "Save" }))
            .on_hover_text("Saves the scale as a .scl file in the tunings folder.");
        if save.clicked() {
            let text = renamed(&text, &format!("{name}.scl"));
            let saved = std::fs::create_dir_all(&window.folder)
                .and_then(|_| std::fs::write(window.folder.join(format!("{name}.scl")), &text));
            window.export_message = Some(match saved {
                Ok(()) => {
                    window.rescan();
                    // The synth's scale is now that file
                    let scale = TextFile { name: format!("{name}.scl"), text };
                    window.set(params, TuningFiles { scale: Some(scale), ..files.clone() });
                    window.export_for = Some(format!("{name}.scl"));
                    (format!("Saved {name}.scl in the tunings folder."), false)
                }
                Err(error) => (format!("Can't save it: {error}"), true),
            });
        }
        if ui.button("Copy").on_hover_text("Copies the .scl file's text, to paste elsewhere.").clicked() {
            ui.ctx().copy_text(renamed(&text, &format!("{name}.scl")));
            window.export_message = Some((String::from("Copied the .scl text."), false));
        }
    });
    if let Some(problem) = problem.filter(|_| !window.export_name.is_empty()) {
        ui.label(RichText::new(problem).small().color(ERROR_COLOR));
    }
    if let Some((message, error)) = &window.export_message {
        let text = RichText::new(message).small();
        ui.label(if *error { text.color(ERROR_COLOR) } else { text.weak() });
    }
}

/// A `.scl` file's text with its first comment line naming it `file_name`, as Scala's own
/// files do (when it starts with a line like that).
fn renamed(text: &str, file_name: &str) -> String {
    match text.split_once('\n') {
        Some((first, rest)) if first.starts_with('!') && first.trim_end().to_lowercase().ends_with(".scl") => {
            format!("! {file_name}\n{rest}")
        }
        _ => text.to_string(),
    }
}

/// A key's note name and octave, middle C being C4.
fn key_name(key: i32) -> String {
    format!("{}{}", NOTE_NAMES[key.rem_euclid(12) as usize], key.div_euclid(12) - 1)
}

/// The note of twelve-tone equal temperament nearest a pitch `cents` above the root key, and
/// how far off it is.
fn nearest(root: i32, cents: f64) -> String {
    let semitones = (cents / 100.0).round();
    let off = cents - semitones * 100.0;
    // (No negative zero)
    let off = if off.abs() < 0.05 { 0.0 } else { off };
    format!("{} {off:+.1}", key_name(root + semitones as i32))
}

/// A line across one period of a scale with a mark at each degree (orange), over the semitones
/// of twelve-tone equal temperament (grey).
fn ruler(ui: &mut Ui, scale: &Scale) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
    painter.rect_stroke(rect, 3.0, visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
    let line = rect.shrink2(egui::vec2(8.0, 6.0));
    let period = scale.period().cents();
    let x = |cents: f64| line.left() + line.width() * (cents / period) as f32;
    painter.line_segment([line.left_center(), line.right_center()], Stroke::new(1.0_f32, Color32::from_gray(70)));
    let semitones = (period / 100.0).floor() as usize;
    for semitone in 0..=semitones.min(1000) {
        let x = x(semitone as f64 * 100.0);
        let y = line.center().y;
        painter.line_segment([egui::pos2(x, y - 4.0), egui::pos2(x, y + 4.0)], Stroke::new(1.0_f32, Color32::from_gray(90)));
    }
    for (i, &cents) in [0.0].iter().chain(scale.cents()).enumerate() {
        let color = if i == 0 || i == scale.len() { Color32::WHITE } else { MOD_COLOR };
        let x = x(cents);
        painter.line_segment([egui::pos2(x, line.top()), egui::pos2(x, line.bottom())], Stroke::new(1.5_f32, color));
    }
    response.on_hover_text(
        "The scale's degrees across one period (orange, the root and the period white), over \
         the semitones of twelve-tone equal temperament (grey).",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunings_are_named_after_their_files() {
        let file = |name: &str| Some(TextFile { name: name.to_string(), text: String::new() });
        assert_eq!(tuning_name(&TuningFiles::default()), "12-TET");
        assert_eq!(tuning_name(&TuningFiles { scale: file("19-EDO.scl"), mapping: None }), "19-EDO");
        assert_eq!(tuning_name(&TuningFiles { scale: None, mapping: file("a432.kbm") }), "12-TET, a432");
    }

    #[test]
    fn notes_are_named_from_the_root() {
        assert_eq!(key_name(60), "C4");
        assert_eq!(key_name(69), "A4");
        assert_eq!(key_name(-1), "B-2");
        assert_eq!(nearest(60, 0.0), "C4 +0.0");
        assert_eq!(nearest(60, 386.314), "E4 -13.7");
        assert_eq!(nearest(60, 1901.955), "G5 +2.0");
        assert_eq!(nearest(69, 63.158), "Bb4 -36.8");
    }

    #[test]
    fn exports_name_themselves() {
        assert_eq!(renamed("! old.scl\n!\nDesc\n 1\n 2/1\n", "new.scl"), "! new.scl\n!\nDesc\n 1\n 2/1\n");
        // A file that doesn't name itself is left as it is
        assert_eq!(renamed("Desc\n 1\n 2/1\n", "new.scl"), "Desc\n 1\n 2/1\n");
    }

    #[test]
    fn generated_scales_play_and_keep_the_mapping() {
        let params = ThreeOscParams::default();
        let mut window = TuningWindow::new();
        let mapping = TextFile { name: String::from("a432.kbm"), text: String::from("0\n0\n127\n60\n69\n432.0\n0\n") };
        params.tuning.try_set(TuningFiles { scale: None, mapping: Some(mapping) }).unwrap();
        // 24-EDO: quarter tones
        window.kind = Kind::EqualDivisions;
        (window.divisions, window.divided) = (24, String::from("2/1"));
        let generated = window.generated().unwrap();
        window.play(&params, &generated);
        assert_eq!(window.message, None);
        let files = params.tuning.files();
        assert_eq!(files.scale.as_ref().unwrap().name, "24-EDO.scl");
        assert_eq!(files.mapping.as_ref().unwrap().name, "a432.kbm");
        let shift = 12.0 * (432.0_f32 / 440.0).log2();
        assert!((params.tuning.pitches()[70] - (69.5 + shift)).abs() < 1e-4);
        // Every kind makes something with its starting settings
        for kind in Kind::ALL {
            window.kind = kind;
            assert!(window.generated().is_ok(), "{kind:?}");
        }
        window.kind = Kind::Stack;
        window.generator = String::from("nonsense");
        assert!(window.generated().unwrap_err().starts_with("Generator: "));
    }
}
