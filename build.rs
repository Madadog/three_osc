use std::env;
use std::f32::consts::PI;
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
    let dest_path = Path::new(&out_dir).join("hello.rs");
    let templates_dir = Path::new(&manifest_dir).join("build_templates");

    let ttl_header = fs::read_to_string(templates_dir.join("ttl_header")).unwrap();
    let ttl_control_divider = fs::read_to_string(templates_dir.join("ttl_control_divider")).unwrap();
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
            oscillators.push(PortList::oscillator_no_mod(amp, wave).prefix(&format!("osc{i}_"), &format!("Osc {i} ")))
        } else {
            oscillators.push(PortList::oscillator(amp, wave).prefix(&format!("osc{i}_"), &format!("Osc {i} ")))
        }
    }

    // add oscillator ports
    for control in oscillators.iter().map(|x| &x.0).flatten() {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(port_index));
        port_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // filter controls
    let mut filter_controls = PortList::filter().prefix("fil1_", "Filter 1 ");
    let mut filter_envelope = PortList::filter_envelope().prefix("fil1_", "Filter 1 ");
    for control in filter_controls.0.iter().chain(filter_envelope.0.iter()) {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(port_index));
        port_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // prepare global controls
    let mut volume_envelope = PortList::envelope().prefix("vol_", "Vol ");
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
    fs::copy(Path::new(&out_dir).join("three_osc.ttl"), Path::new(&manifest_dir).join("three_osc.lv2").join("three_osc.ttl")).unwrap();

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
    fn new(symbol: &str, name: &str, range: ControlRange) -> Self { Self { symbol: symbol.to_string(), name: name.to_string(), range, properties: vec!() } }
    fn with_properties(self, properties: Vec<PortProperty>) -> Self { Self {properties, ..self}}
    fn logarithmic(self) -> Self { self.with_properties(vec![PortProperty::Logarithmic]) }
    fn to_ttl(&self, index: usize) -> String {
        let mut buf = String::with_capacity(2000);
        buf.push_str(&format!("                lv2:index {index} ;\n"));
        buf.push_str(&format!("                lv2:symbol {} ;\n", self.symbol()));
        buf.push_str(&format!("                lv2:name {} ;\n", self.name()));
        buf.push_str(&format!("                lv2:default {} ;\n", self.range.default()));
        buf.push_str(&format!("                lv2:minimum {} ;\n", self.range.min()));
        buf.push_str(&format!("                lv2:maximum {} ;", self.range.max()));
        if matches!(self.range, ControlRange::Int(_, (_, _))) {
            buf.push_str("\n                lv2:portProperty lv2:integer ;")
        }
        match &self.range {
            Int(_, _) => {
                buf.push_str("\n                lv2:portProperty lv2:integer ;")
            },
            ControlRange::Enum(_, entries) => {
                buf.push_str("\n                lv2:portProperty lv2:integer ;");
                buf.push_str("\n                lv2:portProperty lv2:enumeration ;");
                buf.push_str("\n                lv2:scalePoint [");
                for (i, string) in entries.iter().enumerate() {
                    if i > 0 {buf.push_str(" ,\n                [")};
                    buf.push_str(&format!("\n                    rdfs:label  \"{string}\" ;"));
                    buf.push_str(&format!("\n                    rdf:value {i} ;"));
                    buf.push_str("\n                ]");
                }
                buf.push_str(" ;");
            },
            _ => {},
        }
        for property in self.properties.iter() {
            match property {
                PortProperty::Logarithmic => {buf.push_str("\n                lv2:portProperty props:logarithmic ;")},
                x => {panic!("You need to add {:?} to the .to_ttl() function", x)}
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

fn prefix_ports(ports: &mut [ControlPort], symbol_prefix: &str, name_prefix: &str) {
    for port in ports {
        port.prefix(symbol_prefix, name_prefix);
    }
}

#[derive(Debug, Clone)]
enum PortProperty {
    Logarithmic,
}

use ControlRange::{Int, Float};

struct PortList(Vec<ControlPort>);

impl PortList {
    fn prefix(mut self, symbol_prefix: &str, name_prefix: &str) -> Self {
        for port in self.0.iter_mut() {
            port.prefix(symbol_prefix, name_prefix);
        }
        self
    }
    fn oscillator(default_amp: f32, default_wave: usize) -> Self {
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
            ),
            ControlPort::new(
                "semitone",
                "Detune",
                Float(0.0, (-24.0, 24.0)),
            ),
            ControlPort::new(
                "octave",
                "Octave",
                Int(0, (-8, 8)),
            ),
            ControlPort::new(
                "multiplier",
                "Freq. Mult",
                Int(0, (-64, 64)),
            ),
            ControlPort::new(
                "pm",
                "PM",
                Float(0.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "fm",
                "FM",
                Float(0.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "am",
                "AM",
                Float(0.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "voices",
                "Voices",
                Int(1, (1, 128)),
            ),
            ControlPort::new(
                "super_detune",
                "Super Detune",
                Float(21.0, (0.0, 100.0)),
            ),
            ControlPort::new(
                "phase",
                "Phase",
                Float(0.0, (0.0, 100.0)),
            ),
            ControlPort::new(
                "phase_rand",
                "Phase Rand.",
                Float(100.0, (0.0, 100.0)),
            ),
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
            ),
            ControlPort::new(
                "semitone",
                "Detune",
                Float(0.0, (-24.0, 24.0)),
            ),
            ControlPort::new(
                "octave",
                "Octave",
                Int(0, (-8, 8)),
            ),
            ControlPort::new(
                "multiplier",
                "Freq. Mult",
                Int(0, (-64, 64)),
            ),
            ControlPort::new(
                "voices",
                "Voices",
                Int(1, (1, 128)),
            ),
            ControlPort::new(
                "super_detune",
                "Super Detune",
                Float(21.0, (0.0, 100.0)),
            ),
            ControlPort::new(
                "phase",
                "Phase",
                Float(0.0, (0.0, 100.0)),
            ),
            ControlPort::new(
                "phase_rand",
                "Phase Rand.",
                Float(100.0, (0.0, 100.0)),
            ),
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
            ),
            ControlPort::new(
                "octave_detune",
                "Octave Detune",
                Float(0.0, (-0.05, 0.05)),
            ),
            ControlPort::new(
                "output_gain",
                "Output Gain",
                Float(-18.0, (-90.0, 0.0)),
            ),
            ControlPort::new(
                "global_pitch",
                "Master Pitch",
                Int(0, (-24, 24)),
            ),
            ControlPort::new(
                "bend_range",
                "Bend Range",
                Int(2, (-24, 24)),
            ),
        ])
    }
    fn envelope() -> PortList {
        Self(vec![
            ControlPort::new(
                "attack",
                "Attack",
                Float(0.001, (0.0, 15.0)),
            ).logarithmic(),
            ControlPort::new(
                "decay",
                "Decay",
                Float(0.25, (0.0, 15.0)),
            ).logarithmic(),
            ControlPort::new(
                "sustain",
                "Sustain",
                Float(1.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "release",
                "Release",
                Float(0.005, (0.001, 15.0)),
            ).logarithmic(),
            ControlPort::new(
                "slope",
                "Slope",
                Float(0.0, (-1.0, 1.0)),
            ),
        ])
    }
    fn filter_envelope() -> PortList {
        Self(vec![
            ControlPort::new(
                "keytrack",
                "Keytrack",
                Float(0.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "env_amount",
                "Env. Amount",
                Float(0.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "attack",
                "Attack",
                Float(0.001, (0.0, 15.0)),
            ).logarithmic(),
            ControlPort::new(
                "decay",
                "Decay",
                Float(0.25, (0.0, 15.0)),
            ).logarithmic(),
            ControlPort::new(
                "sustain",
                "Sustain",
                Float(1.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "release",
                "Release",
                Float(0.005, (0.001, 15.0)),
            ).logarithmic(),
            ControlPort::new(
                "slope",
                "Slope",
                Float(0.0, (-1.0, 1.0)),
            ),
        ])
    }
    fn filter() -> PortList {
        Self(vec![
            ControlPort::new(
                "model",
                "Model",
                // Int(0, (0, 4)),
                ControlRange::Enum(0, vec![
                    "None".to_string(),
                    "RC".to_string(),
                    "Ladder".to_string(),
                    "Digital".to_string(),
                ]),
            ),
            ControlPort::new(
                "type",
                "Type",
                Float(0.0, (0.0, 3.0)),
            ),
            ControlPort::new(
                "cutoff",
                "Cutoff",
                Float(22000.0, (1.0, 22000.0)),
            ).logarithmic(),
            ControlPort::new(
                "resonance",
                "Resonance",
                Float(0.1, (0.01, 10.0)),
            ),
            ControlPort::new(
                "drive",
                "Drive",
                Float(1.0, (0.01, 10.0)),
            ),
        ])
    }
}