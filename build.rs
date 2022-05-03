use std::env;
use std::fs;
use std::path::Path;

/// WARNING: Do not read, this code sucks.
///
/// The following build script generates a "three_osc.ttl" file (needed by LV2
/// hosts) which is copied into the three_osc.lv2 directory at the root of this
/// project.
///
/// The file contains a list of metadata, including Control Ports which describe
/// the plugin's LV2 UI which is automatically generated from these ports by LV2
/// hosts when they load the plugin.
///
/// This build script also generates a file called "portstruct.rs", which must
/// manually be copied into "lib.rs" replacing the `Ports` struct whenever ANY
/// ports are moved or added (changing default values / range is fine). I tried
/// to automate this (not very hard, admittedly) but couldn't come up with any
/// solution better than 'manual copy' because the `include!` macro doesn't work
/// with `#[derive(PortCollection)]` which is needed by the `lv2` crate.
///
/// Yes, there is probably a much better way of doing all this, but it is not
/// supplied by the `lv2` crate. Hopefully future versions of the crate will
/// automatically handle .ttl stuff for you.

// What a better build system would look like:
// * Automatic synchronisation of YOUR_LV2.ttl and `#[derive(PortCollection)]` struct
// * Annotate ports with default values, names, ranges and properties directly inside the `#[derive(PortCollection)]` struct
fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let templates_dir = Path::new(&manifest_dir).join("build_templates");

    let ttl_header = fs::read_to_string(templates_dir.join("ttl_header")).unwrap();
    let ttl_control_divider =
        fs::read_to_string(templates_dir.join("ttl_control_divider")).unwrap();
    let ttl_end = fs::read_to_string(templates_dir.join("ttl_end")).unwrap();

    let portstruct_header = fs::read_to_string(templates_dir.join("portstruct_header")).unwrap();
    let portstruct_end = fs::read_to_string(templates_dir.join("portstruct_end")).unwrap();

    let mut ttl = ttl_header;
    let mut portstruct = portstruct_header;

    // start at 3 because of midi in + stereo output ports
    let mut port_index = 3;

    // format oscillator duplicates
    let mut oscillators = Vec::new();
    for i in 1..=3 {
        let (amp, wave) = if i == 1 {
            (100.0, 2) // first osc defaults to saw
        } else {
            (0.0, 0) // other oscs are silent
        };
        if i == 3 {
            // third osc cannot be modulated
            oscillators.push(
                PortList::oscillator_no_mod(amp, wave)
                    .prefix(&format!("osc{i}_"), &format!("Osc {i} ")),
            )
        } else {
            let modulator = format!("Osc {}", i + 1);
            oscillators.push(
                PortList::oscillator(amp, wave, &modulator)
                    .prefix(&format!("osc{i}_"), &format!("Osc {i} ")),
            )
        }
    }

    // add oscillator ports
    for control in oscillators.iter().flat_map(|x| &x.0) {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(port_index));
        port_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // filter controls
    let filter_controls = PortList::filter().prefix("fil1_", "Filter 1 ");
    let filter_envelope = PortList::filter_envelope().prefix("fil1_", "Filter 1 ");
    for control in filter_controls.0.iter().chain(filter_envelope.0.iter()) {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(port_index));
        port_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // prepare global controls
    let volume_envelope = PortList::envelope().prefix("vol_", "Vol ");
    let global_controls = PortList::global();

    // add global ports
    for control in volume_envelope.0.iter().chain(global_controls.0.iter()) {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(port_index));
        port_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // end ports
    ttl.push_str(&ttl_end);
    portstruct.push_str(&portstruct_end);

    // write files
    fs::write(Path::new(&out_dir).join("three_osc.ttl"), ttl).expect("couldn't create file");
    fs::write(Path::new(&out_dir).join("portstruct.rs"), portstruct).expect("couldn't create file");

    // copy ttl into LV2
    fs::copy(
        Path::new(&out_dir).join("three_osc.ttl"),
        Path::new(&manifest_dir)
            .join("three_osc.lv2")
            .join("three_osc.ttl"),
    )
    .unwrap();

    println!("cargo:rerun-if-changed=build.rs");
}

#[derive(Debug, Clone)]
struct ControlPort {
    symbol: String,
    name: String,
    range: ControlRange,
    properties: Vec<PortProperty>,
}
impl ControlPort {
    fn new(symbol: &str, name: &str, range: ControlRange) -> Self {
        Self {
            symbol: symbol.to_string(),
            name: name.to_string(),
            range,
            properties: vec![],
        }
    }
    #[allow(dead_code)]
    fn with_properties(mut self, properties: &[PortProperty]) -> Self {
        self.properties.extend(properties.iter().cloned());
        self
    }
    fn with_property(mut self, property: PortProperty) -> Self {
        self.properties.push(property);
        self
    }
    fn logarithmic(self) -> Self {
        self.with_property(PortProperty::Logarithmic)
    }
    fn comment(self, comment: &str) -> Self {
        self.with_property(PortProperty::Comment(comment.to_string()))
    }
    fn to_ttl(&self, index: usize) -> String {
        let mut buf = String::with_capacity(2000);
        buf.push_str(&format!("                lv2:index {index} ;\n"));
        buf.push_str(&format!("                lv2:symbol {} ;\n", self.symbol()));
        buf.push_str(&format!("                lv2:name {} ;\n", self.name()));
        buf.push_str(&format!(
            "                lv2:default {} ;\n",
            self.range.default()
        ));
        buf.push_str(&format!(
            "                lv2:minimum {} ;\n",
            self.range.min()
        ));
        buf.push_str(&format!(
            "                lv2:maximum {} ;",
            self.range.max()
        ));
        if matches!(self.range, ControlRange::Int(_, (_, _))) {
            buf.push_str("\n                lv2:portProperty lv2:integer ;")
        }
        match &self.range {
            Int(_, _) => buf.push_str("\n                lv2:portProperty lv2:integer ;"),
            ControlRange::Enum(_, entries) => {
                buf.push_str("\n                lv2:portProperty lv2:integer ;");
                buf.push_str("\n                lv2:portProperty lv2:enumeration ;");
                buf.push_str("\n                lv2:scalePoint [");
                for (i, string) in entries.iter().enumerate() {
                    if i > 0 {
                        buf.push_str(" ,\n                [")
                    };
                    buf.push_str(&format!("\n                    rdfs:label  \"{string}\" ;"));
                    buf.push_str(&format!("\n                    rdf:value {i} ;"));
                    buf.push_str("\n                ]");
                }
                buf.push_str(" ;");
            }
            _ => {}
        }
        for property in self.properties.iter() {
            #[allow(unreachable_patterns)]
            match property {
                PortProperty::Logarithmic => {
                    buf.push_str("\n                lv2:portProperty props:logarithmic ;")
                }
                PortProperty::Comment(comment) => {
                    buf.push_str(&format!("\n                rdfs:comment \"{}\" ;", comment))
                }
                x => {
                    panic!("You need to add {:?} to the .to_ttl() function", x)
                }
            }
        }
        buf
    }
    fn prefix(&mut self, symbol_prefix: &str, name_prefix: &str) {
        self.symbol.insert_str(0, symbol_prefix);
        self.name.insert_str(0, name_prefix);
    }
    fn symbol(&self) -> String {
        format!("\"{}\"", self.symbol)
    }
    fn name(&self) -> String {
        format!("\"{}\"", self.name)
    }
    fn struct_port(&self) -> String {
        format!("    {}: InputPort<Control>,", self.symbol)
    }
}

#[derive(Debug, Clone)]
enum ControlRange {
    /// (default, (min, max))
    Int(i32, (i32, i32)),
    /// (default, (min, max))
    Float(f32, (f32, f32)),
    /// (default index, enum entries)
    Enum(usize, Vec<String>),
}
impl ControlRange {
    fn default(&self) -> String {
        match self {
            ControlRange::Int(x, _) => x.to_string(),
            ControlRange::Float(x, _) => format!("{x:.3}"),
            ControlRange::Enum(x, _) => x.to_string(),
        }
    }
    fn min(&self) -> String {
        match self {
            ControlRange::Int(_, (x, _)) => x.to_string(),
            ControlRange::Float(_, (x, _)) => format!("{x:.3}"),
            ControlRange::Enum(_, _) => 0.to_string(),
        }
    }
    fn max(&self) -> String {
        match self {
            ControlRange::Int(_, (_, x)) => x.to_string(),
            ControlRange::Float(_, (_, x)) => format!("{x:.3}"),
            ControlRange::Enum(_, x) => x.len().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
enum PortProperty {
    Logarithmic,
    Comment(String),
}

use ControlRange::{Float, Int};

struct PortList(Vec<ControlPort>);

impl PortList {
    fn prefix(mut self, symbol_prefix: &str, name_prefix: &str) -> Self {
        for port in self.0.iter_mut() {
            port.prefix(symbol_prefix, name_prefix);
        }
        self
    }
    fn oscillator(default_amp: f32, default_wave: usize, modulator: &str) -> Self {
        Self(vec![
            ControlPort::new(
                "wave",
                "Wave",
                // Int(0, (0, 6)),
                ControlRange::Enum(default_wave, vec![
                    "Sine".to_string(),
                    "Triangle".to_string(),
                    "Saw".to_string(),
                    "Exponential".to_string(),
                    "Square".to_string(),
                ]),
            ),
            ControlPort::new(
                "amp",
                "Amplitude",
                Float(default_amp, (0.0, 100.0)),
            ).comment("Oscillator output volume. Doesn't affect PM, FM or AM."),
            ControlPort::new(
                "semitone",
                "Detune",
                Float(0.0, (-24.0, 24.0)),
            ).comment("Oscillator detune in semitones."),
            ControlPort::new(
                "octave",
                "Octave",
                Int(0, (-8, 8)),
            ).comment("Oscillator detune in octaves."),
            ControlPort::new(
                "multiplier",
                "Freq. Mult",
                Int(0, (-64, 64)),
            ).comment("Oscillator pitch multiplier / divider. Positive values multiply pitch, while negative values divide. A value of 0 means this control is bypassed."),
            ControlPort::new(
                "pm",
                &format!("<- {modulator} PM"),
                Float(0.0, (0.0, 1.0)),
            ).comment("Phase modulation of this oscillator by the oscillator after it."),
            ControlPort::new(
                "fm",
                &format!("<- {modulator} FM"),
                Float(0.0, (0.0, 1.0)),
            ).comment("Frequency modulation of this oscillator by the oscillator after it. Can be used for vibrato effects when the modulator is at a low pitch."),
            ControlPort::new(
                "am",
                &format!("<- {modulator} AM"),
                Float(0.0, (0.0, 1.0)),
            ).comment("Amplitude modulation of this oscillator by the oscillator after it. Also known as \'Ring Modulation\'. Can be used for telephone sounds and tremelo."),
            ControlPort::new(
                "voices",
                "Unison",
                Int(1, (1, 128)),
            ).comment("Copies of this oscillator which are overlaid at different pitches. Useful for the supersaw sound."),
            ControlPort::new(
                "super_detune",
                "Unison Detune",
                Float(21.0, (0.0, 100.0)),
            ).comment("Unison voice detune. Does nothing if unison = 1."),
            ControlPort::new(
                "phase",
                "Phase",
                Float(0.0, (0.0, 100.0)),
            ).comment("Point at which the oscillator wave starts. Does nothing when Phase Rand = 100."),
            ControlPort::new(
                "phase_rand",
                "Phase Rand.",
                Float(100.0, (0.0, 100.0)),
            ).comment("Partially or fully randomises the point where the oscillator wave starts. Keep this fairly high when using Unison."),
        ])
    }
    fn oscillator_no_mod(default_amp: f32, default_wave: usize) -> Self {
        Self(vec![
            ControlPort::new(
                "wave",
                "Wave",
                // Int(0, (0, 6)),
                ControlRange::Enum(default_wave, vec![
                    "Sine".to_string(),
                    "Triangle".to_string(),
                    "Saw".to_string(),
                    "Exponential".to_string(),
                    "Square".to_string(),
                ]),
            ),
            ControlPort::new(
                "amp",
                "Amplitude",
                Float(default_amp, (0.0, 100.0)),
            ).comment("Oscillator output volume. Doesn't affect PM, FM or AM."),
            ControlPort::new(
                "semitone",
                "Detune",
                Float(0.0, (-24.0, 24.0)),
            ).comment("Oscillator detune in semitones."),
            ControlPort::new(
                "octave",
                "Octave",
                Int(0, (-8, 8)),
            ).comment("Oscillator detune in octaves."),
            ControlPort::new(
                "multiplier",
                "Freq. Mult",
                Int(0, (-64, 64)),
            ).comment("Oscillator pitch multiplier / divider. Positive values multiply pitch, while negative values divide. A value of 0 means this control is bypassed."),
            ControlPort::new(
                "voices",
                "Unison",
                Int(1, (1, 128)),
            ).comment("Copies of this oscillator which are overlaid at different pitches. Useful for the supersaw sound."),
            ControlPort::new(
                "super_detune",
                "Unison Detune",
                Float(21.0, (0.0, 100.0)),
            ).comment("Unison voice detune. Does nothing if unison = 1."),
            ControlPort::new(
                "phase",
                "Phase",
                Float(0.0, (0.0, 100.0)),
            ).comment("Point at which the oscillator wave starts. Does nothing when Phase Rand = 100."),
            ControlPort::new(
                "phase_rand",
                "Phase Rand.",
                Float(100.0, (0.0, 100.0)),
            ).comment("Partially or fully randomises the point where the oscillator wave starts. Keep this fairly high when using Unison."),
        ])
    }
    fn global() -> PortList {
        Self(vec![
            ControlPort::new(
                "polyphony",
                "Polyphony",
                // Int(0, (0, 2)),
                ControlRange::Enum(0, vec![
                    "Polyphonic".to_string(),
                    "Monophonic".to_string(),
                    "Legato".to_string(),
                ]),
            ).comment("Polyphonic means an infinite number of notes can be played simultaneously. Monophonic means only one note can be played at a time. Legato is the same as monophonic, except notes are connected; envelopes / oscillator phases won't reset when gliding between notes."),
            ControlPort::new(
                "octave_detune",
                "Octave Drift",
                Float(0.0, (-0.04, 0.04)),
            ).comment("Stretches the octave width so that consecutive octaves are further or closer together. In 12TET (i.e. the chromatic scale), octaves are the only perfect interval, which means playing them with harsh waves (saw, square, etc) will result in a louder and harsher sound than playing any other two notes. This control is meant to fix that."),
            ControlPort::new(
                "output_gain",
                "Output Gain",
                Float(-18.0, (-90.0, 0.0)),
            ).comment("Master output volume. Adjust if the synth is too loud / too quiet."),
            ControlPort::new(
                "global_pitch",
                "Master Pitch",
                Int(0, (-24, 24)),
            ).comment("Pitch detune for the whole synth, in semitones."),
            ControlPort::new(
                "bend_range",
                "Bend Range",
                Int(2, (-24, 24)),
            ).comment("Controls the range of the MIDI pitch wheel in semitones. Useful if you have a MIDI keyboard."),
        ])
    }
    fn envelope() -> PortList {
        Self(vec![
            ControlPort::new(
                "attack",
                "Attack",
                Float(0.001, (0.001, 15.0)),
            ).logarithmic()
            .comment("Envelope start time, in seconds. This gives a \\\"fade in\\\" effect when controlling volume. (Note: This control's minimum value (0.001) actually corresponds to 0 internally. This is a GUI hack to make logarithmic values display nicely in Ardour.)"),
            ControlPort::new(
                "decay",
                "Decay",
                Float(0.25, (0.001, 15.0)),
            ).logarithmic()
            .comment("Time for envelope to reach sustain level, in seconds. This gives a \\\"pluck\\\" effect when controlling volume. Does nothing when sustain = 1. (Note: This control's minimum value (0.001) actually corresponds to 0 internally. This is a GUI hack to make logarithmic values display nicely in Ardour.)"),
            ControlPort::new(
                "sustain",
                "Sustain",
                Float(1.0, (0.0, 1.0)),
            ).comment("The level the envelope will remain at while the note is held. Used for sustained sounds, like flutes or strings. Has no effect when set to 0; the note will end when decay finishes."),
            ControlPort::new(
                "release",
                "Release",
                Float(0.005, (0.001, 15.0)),
            ).logarithmic()
            .comment("Time for the envelope to finish after the note is released. Useful for sounds which persist a while after they're played, like bells or chimes. Has mostly no effect when sustain = 0."),
            ControlPort::new(
                "slope",
                "Slope",
                Float(1.0, (-8.0, 8.0)),
            ).comment("Controls the steepness of the attack, decay and release slopes either exponentially or logarithmically. Positive slope means attack and decay will change logarithmically, resulting in punchier sounds, while negative slope will do the opposite. A slope of 0 results in exactly linear slopes."),
        ])
    }
    fn filter_envelope() -> PortList {
        Self(vec![
            ControlPort::new(
                "keytrack",
                "Keytrack",
                Float(0.0, (0.0, 1.0)),
            ).comment("Amount the filter cutoff is affected by note frequency; Keytrack of 1.0 means the filter cutoff will follow the note frequency exactly, making higher notes brighter and lower notes darker."),
            ControlPort::new(
                "env_amount",
                "Env. Amount",
                Float(0.25, (0.0, 1.0)),
            ).comment("Amount the envelope affects the filter cutoff."),
            ControlPort::new(
                "attack",
                "Attack",
                Float(0.001, (0.001, 15.0)),
            ).logarithmic()
            .comment("Envelope start time, in seconds. This gives a \\\"fade in\\\" effect when controlling volume. (Note: This control's minimum value (0.001) actually corresponds to 0 internally. This is a GUI hack to make logarithmic values display nicely in Ardour.)"),
            ControlPort::new(
                "decay",
                "Decay",
                Float(0.25, (0.001, 15.0)),
            ).logarithmic()
            .comment("Time for envelope to reach sustain level, in seconds. This gives a \\\"pluck\\\" effect when controlling volume. Does nothing when sustain = 1. (Note: This control's minimum value (0.001) actually corresponds to 0 internally. This is a GUI hack to make logarithmic values display nicely in Ardour.)"),
            ControlPort::new(
                "sustain",
                "Sustain",
                Float(0.0, (0.0, 1.0)),
            ).comment("The level the envelope will remain at while the note is held. Used for sustained sounds, like flutes or strings. Has no effect when set to 0; the note will end when decay finishes."),
            ControlPort::new(
                "release",
                "Release",
                Float(0.005, (0.001, 15.0)),
            ).logarithmic()
            .comment("Time for the envelope to finish after the note is released. Useful for sounds which persist a while after they're played, like bells or chimes. Has mostly no effect when sustain = 0."),
            ControlPort::new(
                "slope",
                "Slope",
                Float(1.0, (-8.0, 8.0)),
            ).comment("Controls the steepness of the attack, decay and release slopes either exponentially or logarithmically. Positive slope means attack and decay will change logarithmically, resulting in punchier sounds, while negative slope will do the opposite. A slope of 0 results in exactly linear slopes."),
        ])
    }
    fn filter() -> PortList {
        Self(vec![
            ControlPort::new(
                "model",
                "Model",
                // Int(0, (0, 4)),
                ControlRange::Enum(3, vec![
                    "None".to_string(),
                    "RC".to_string(),
                    "Ladder".to_string(),
                    "Digital".to_string(),
                ]),
            ).comment("There are 3 filter models: Digital is a bog-standard IIR Biquad, RC is a darker filter capable of aggressive self-resonance, and Ladder is based on a famous analog filter and sounds the best, with its code coming from janne808's Kocmoc Rack Modules project."),
            ControlPort::new(
                "type",
                "Type",
                ControlRange::Enum(0, vec![
                    "Lowpass".to_string(),
                    "Bandpass".to_string(),
                    "Highpass".to_string(),
                ]),
            ).comment("Lowpass cuts out high frequencies, Highpass cuts out low frequencies, and Bandpass allows a small band of frequencies at the cutoff point."),
            ControlPort::new(
                "cutoff",
                "Cutoff Freq.",
                Float(22000.0, (10.0, 22000.0)),
            ).logarithmic()
            .comment("Changes frequency at which the filter starts taking effect. Try it out."),
            ControlPort::new(
                "resonance",
                "Resonance",
                Float(0.7, (0.01, 10.0)),
            ).comment("Adds feedback to the filter loop, creating a volume spike at the filter's cutoff frequency. On the RC and Ladder filters, setting this high enough creates a self-sustaining sine wave."),
            ControlPort::new(
                "drive",
                "Drive",
                Float(1.0, (0.01, 10.0)),
            ).comment("Multiplies the amplitude of the filter input, creating distortion inside the RC and Ladder filters. Does not amplify when Model = None or Digital, to protect ears."),
        ])
    }
}
