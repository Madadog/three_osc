use std::{fs::File, io::Write};

fn main() {
    let ttl_header = "@prefix atom: <http://lv2plug.in/ns/ext/atom#> .
@prefix doap:  <http://usefulinc.com/ns/doap#> .
@prefix lv2:   <http://lv2plug.in/ns/lv2core#> .
@prefix midi: <http://lv2plug.in/ns/ext/midi#> .
@prefix props: <http://lv2plug.in/ns/ext/port-props#> .
@prefix rdf:   <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs:  <http://www.w3.org/2000/01/rdf-schema#> .
@prefix units: <http://lv2plug.in/ns/extensions/units#> .
@prefix urid: <http://lv2plug.in/ns/ext/urid#> .

<https://github.com/Madadog/three_osc>
        a lv2:Plugin ,
                lv2:InstrumentPlugin ;
        lv2:project <https://github.com/Madadog/three_osc> ;

        doap:name \"Three Osc\" ;
        doap:license <https://www.gnu.org/licenses/gpl-3.0.html> ;
        lv2:requiredFeature urid:map ;
        lv2:optionalFeature lv2:hardRTCapable ;

        lv2:port [
                a lv2:InputPort ,
                    atom:AtomPort ;
                atom:bufferType atom:Sequence ;
                atom:supports midi:MidiEvent ;
                lv2:designation lv2:control ;
                lv2:index 0 ;
                lv2:symbol \"midi\" ;
                lv2:name \"Midi In\"
        ] , [
                a lv2:AudioPort ,
                        lv2:OutputPort ;
                lv2:index 1 ;
                lv2:symbol \"out_l\" ;
                lv2:name \"Output L\"
        ] , [
                a lv2:AudioPort ,
                        lv2:OutputPort ;
                lv2:index 2 ;
                lv2:symbol \"out_r\" ;
                lv2:name \"Output R\"";
    let ttl_divider = "
        ] , [
                a lv2:InputPort ,
                    lv2:ControlPort ;
";
    let ttl_end = "
        ] .";
    let mut ttl = ttl_header.to_string();

    let mut struct_header = "struct Ports {
    midi: InputPort<AtomPort>,
    out_l: OutputPort<Audio>,
    out_r: OutputPort<Audio>,";
    let mut struct_end = "
}";
    let mut struct_ports = struct_header.to_string();

    // format oscillator duplicates
    let mut oscillators = Vec::new();

    for i in 1..=1 {
        oscillators.push(PortList::oscillator().prefix(&format!("osc{i}_"), &format!("Osc{i} ")))
    }
    
    // start at 3 because of midi in + stereo output ports
    let mut ttl_index = 3; 
    // add oscillator ports
    for control in oscillators.iter().map(|x| &x.0).flatten() {
        ttl.push_str(ttl_divider);
        ttl.push_str(&control.to_ttl(ttl_index));
        ttl_index += 1;
        struct_ports.push_str(&format!("\n{}", control.struct_port()));
    }

    // filter controls
    let mut filter_controls = PortList::filter().prefix("fil1_", "Filter 1 ");
    for control in filter_controls.0.iter() {
        ttl.push_str(ttl_divider);
        ttl.push_str(&control.to_ttl(ttl_index));
        ttl_index += 1;
        struct_ports.push_str(&format!("\n{}", control.struct_port()));
    }

    // prepare global controls
    let mut volume_envelope = PortList::envelope().prefix("vol_", "Vol ");
    let global_controls = PortList::global();

    // add global ports
    for control in volume_envelope.0.iter().chain(global_controls.0.iter()) {
        ttl.push_str(ttl_divider);
        ttl.push_str(&control.to_ttl(ttl_index));
        ttl_index += 1;
        struct_ports.push_str(&format!("\n{}", control.struct_port()));
    }


    // end ports
    ttl.push_str(ttl_end);
    struct_ports.push_str(struct_end);

    let mut file = File::create("three_osc.ttl").expect("couldn't create file");
    file.write_all(ttl.as_bytes()).expect("Couldn't write everything to file");

    let mut file = File::create("struct_ports.txt").expect("couldn't create file");
    file.write_all(struct_ports.as_bytes()).expect("Couldn't write everything to file");
    
    // println!("{struct_ports}");
}

#[derive(Debug, Clone)]
struct ControlPort {
    symbol: String,
    name: String,
    range: ControlRange,
}
impl ControlPort {
    fn new(symbol: &str, name: &str, range: ControlRange) -> Self { Self { symbol: symbol.to_string(), name: name.to_string(), range } }
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
                "exponent",
                "Freq. Exponent",
                Int(0, (-16, 16)),
            ),
            ControlPort::new(
                "wave",
                "Wave",
                Int(0, (0, 6)),
            ),
            ControlPort::new(
                "mod",
                "Mod",
                Int(0, (0, 4)),
            ),
            ControlPort::new(
                "voices",
                "Voices",
                Int(1, (1, 128)),
            ),
            ControlPort::new(
                "super_detune",
                "Super Detune",
                Float(0.0, (0.0, 100.0)),
            ),
            ControlPort::new(
                "phase",
                "Phase",
                Float(0.0, (0.0, 360.0)),
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
                "output_gain",
                "Output Gain",
                Float(-18.0, (-90.0, 0.0)),
            ),
            ControlPort::new(
                "global_pitch",
                "Master Pitch",
                Int(0, (-24, 24)),
            ),
        ])
    }
    fn envelope() -> PortList {
        Self(vec![
            ControlPort::new(
                "attack",
                "Attack",
                Float(0.001, (0.0, 15.0)),
            ),
            ControlPort::new(
                "decay",
                "Decay",
                Float(0.25, (0.0, 15.0)),
            ),
            ControlPort::new(
                "sustain",
                "Sustain",
                Float(1.0, (0.0, 1.0)),
            ),
            ControlPort::new(
                "release",
                "Release",
                Float(0.005, (0.001, 15.0)),
            ),
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
                "mode",
                "Mode",
                Float(0.0, (-2.0, 2.0)),
            ),
            ControlPort::new(
                "cutoff",
                "Cutoff",
                Float(0.0, (-2.0, 2.0)),
            ),
            ControlPort::new(
                "resonance",
                "Feedback0",
                Float(0.0, (-1.0, 1.0)),
            ),
            ControlPort::new(
                "slope",
                "Feedback1",
                Float(0.0, (-1.0, 1.0)),
            ),
            ControlPort::new(
                "feedback0_1",
                "Feedback01",
                Float(0.0, (-1.0, 1.0)),
            ),
            ControlPort::new(
                "feedback1_0",
                "Feedback10",
                Float(0.0, (-1.0, 1.0)),
            ),
        ])
    }
}