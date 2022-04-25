use std::env;
use std::f32::consts::PI;
use std::fs;
use std::path::Path;

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

    // format oscillator duplicates
    let mut oscillators = Vec::new();
    for i in 1..=2 {
        oscillators.push(PortList::oscillator().prefix(&format!("osc{i}_"), &format!("Osc{i} ")))
    }


    // format oscillator duplicates
    let mut oscillators = Vec::new();
    for i in 1..=2 {
        oscillators.push(PortList::oscillator().prefix(&format!("osc{i}_"), &format!("Osc{i} ")))
    }
    
    // start at 3 because of midi in + stereo output ports
    let mut ttl_index = 3; 
    // add oscillator ports
    for control in oscillators.iter().map(|x| &x.0).flatten() {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(ttl_index));
        ttl_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // filter controls
    let mut filter_controls = PortList::filter().prefix("fil1_", "Filter 1 ");
    let mut filter_envelope = PortList::filter_envelope().prefix("fil1_", "Filter 1 ");
    for control in filter_controls.0.iter().chain(filter_envelope.0.iter()) {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(ttl_index));
        ttl_index += 1;
        portstruct.push_str(&format!("\n{}", control.struct_port()));
    }

    // prepare global controls
    let mut volume_envelope = PortList::envelope().prefix("vol_", "Vol ");
    let global_controls = PortList::global();

    // add global ports
    for control in volume_envelope.0.iter().chain(global_controls.0.iter()) {
        ttl.push_str(&ttl_control_divider);
        ttl.push_str(&control.to_ttl(ttl_index));
        ttl_index += 1;
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
        let mut buf = String::with_capacity(1000);
        buf.push_str(&format!("                lv2:index {index} ;\n"));
        buf.push_str(&format!("                lv2:symbol {} ;\n", self.symbol()));
        buf.push_str(&format!("                lv2:name {} ;\n", self.name()));
        buf.push_str(&format!("                lv2:default {} ;\n", self.range.default()));
        buf.push_str(&format!("                lv2:minimum {} ;\n", self.range.min()));
        buf.push_str(&format!("                lv2:maximum {} ;", self.range.max()));
        if matches!(self.range, ControlRange::Int(_, (_, _))) {
            buf.push_str("\n                lv2:portProperty lv2:integer ;")
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
    Float(f32, (f32, f32))
}
impl ControlRange {
    fn default(&self) -> String {
        match self {
            ControlRange::Int(x, _) => x.to_string(),
            ControlRange::Float(x, _) => format!("{x:.3}"),
        }
    }
    fn min(&self) -> String {
        match self {
            ControlRange::Int(_, (x, _)) => x.to_string(),
            ControlRange::Float(_, (x, _)) => format!("{x:.3}"),
        }
    }
    fn max(&self) -> String {
        match self {
            ControlRange::Int(_, (_, x)) => x.to_string(),
            ControlRange::Float(_, (_, x)) => format!("{x:.3}"),
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
    fn oscillator() -> Self {
        Self(vec![
            ControlPort::new(
                "amp",
                "Amplitude",
                Float(100.0, (0.0, 100.0)),
            ),
            ControlPort::new(
                "semitone",
                "Semitone",
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
                "wave",
                "Wave",
                Int(0, (0, 6)),
            ),
            ControlPort::new(
                "pm",
                "PM",
                Float(0.0, (0.0, 20.0)),
            ),
            ControlPort::new(
                "fm",
                "FM",
                Float(0.0, (0.0, 20.0)),
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
                Float(0.0, (0.0, PI * 2.0)),
            ),
            ControlPort::new(
                "phase_rand",
                "Phase Rand.",
                Float(PI * 2.0, (0.0, PI * 2.0)),
            ),
        ])
    }
    fn global() -> PortList {
        Self(vec![
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
                Float(0.0, (-3.0, 8.0)),
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
                Float(0.0, (-3.0, 8.0)),
            ),
        ])
    }
    fn filter() -> PortList {
        Self(vec![
            ControlPort::new(
                "model",
                "Model",
                Int(0, (0, 4)),
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
                Float(0.1, (0.01, 50.0)),
            ),
        ])
    }
}