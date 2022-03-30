use std::convert::TryInto;

// Include the prelude of `lv2`. This includes the preludes of every sub-crate and you are strongly encouraged to use it, since many macros depend on it.
use lv2::{prelude::*, lv2_core::plugin};

mod synth;
use synth::{ThreeOsc, oscillator::OscWave};
use wmidi::MidiMessage;

// Most useful plugins will have ports for input and output data. In code, these ports are represented by a struct implementing the `PortCollection` trait. Internally, ports are referred to by index. These indices are assigned in ascending order, starting with 0 for the first port. The indices in `amp.ttl` have to match them.
#[derive(PortCollection)]
struct Ports {
    midi: InputPort<AtomPort>,
    out_l: OutputPort<Audio>,
    out_r: OutputPort<Audio>,
    osc1_amp: InputPort<Control>,
    osc1_semitone: InputPort<Control>,
    osc1_exponent: InputPort<Control>,
    osc1_wave: InputPort<Control>,
    osc1_mod: InputPort<Control>,
    osc1_voices: InputPort<Control>,
    osc1_super_detune: InputPort<Control>,
    osc1_phase: InputPort<Control>,
    osc1_phase_rand: InputPort<Control>,
    fil1_mode: InputPort<Control>,
    fil1_cutoff: InputPort<Control>,
    fil1_resonance: InputPort<Control>,
    fil1_slope: InputPort<Control>,
    fil1_feedback0_1: InputPort<Control>,
    fil1_feedback1_0: InputPort<Control>,
    vol_attack: InputPort<Control>,
    vol_decay: InputPort<Control>,
    vol_sustain: InputPort<Control>,
    vol_release: InputPort<Control>,
    vol_slope: InputPort<Control>,
    output_gain: InputPort<Control>,
    global_pitch: InputPort<Control>,
}

#[derive(FeatureCollection)]
pub struct Features<'a> {
    map: LV2Map<'a>,
}

#[derive(URIDCollection)]
pub struct URIDs {
    atom: AtomURIDCollection,
    midi: MidiURIDCollection,
    unit: UnitURIDCollection,
}

#[uri("https://github.com/Madadog/three_osc")]
struct Amp {
    synth: ThreeOsc,
    urids: URIDs,
}

// Every plugin struct implements the `Plugin` trait. This trait contains both the methods that are called by the hosting application and the collection types for the ports and the used host features. This plugin does not use additional host features and therefore, we set both feature collection types to `()`. Other plugins may define separate structs with their required and optional features and set it here.
impl Plugin for Amp {
    type Ports = Ports;

    type InitFeatures = Features<'static>;
    type AudioFeatures = ();

    // The `new` method is called by the plugin backend when it creates a new plugin instance. The host passes the plugin URI, sample rate, and bundle path for plugins that need to load additional resources (e.g. waveforms). The features parameter contains host-provided features defined in LV2 extensions, but this simple plugin does not use any. This method is in the “instantiation” threading class, so no other methods on this instance will be called concurrently with it.
    fn new(plugin_info: &PluginInfo, features: &mut Features<'static>) -> Option<Self> {
        println!("Sample rate was: {}", plugin_info.sample_rate());
        Some(Self {
            synth: ThreeOsc::new(plugin_info.sample_rate()),
            urids: features.map.populate_collection()?,
        })
    }
    // The `run()` method is the main process function of the plugin. It processes a block of audio in the audio context. Since this plugin is `lv2:hardRTCapable`, `run()` must be real-time safe, so blocking (e.g. with a mutex) or memory allocation are not allowed.
    fn run(&mut self, ports: &mut Ports, _features: &mut (), _sample_count: u32) {
        let coef = if *(ports.output_gain) > -90.0 {
            10.0_f32.powf(*(ports.output_gain) * 0.05)
        } else {
            0.0
        };

        // adjust master gain envelope
        self.synth.gain_envelope.attack_time = *ports.vol_attack;
        self.synth.gain_envelope.decay_time = *ports.vol_decay;
        self.synth.gain_envelope.sustain_level = *ports.vol_sustain;
        self.synth.gain_envelope.release_time = *ports.vol_release;
        self.synth.gain_envelope.slope = 2.0_f32.powf(*ports.vol_slope);
        //self.synth.gain_envelope.limits();

        self.synth.oscillators[0].amp = *ports.osc1_amp / 100.0;
        self.synth.oscillators[0].semitone = *ports.osc1_semitone + *ports.global_pitch;
        self.synth.oscillators[0].exponent = *ports.osc1_exponent as i32;
        self.synth.oscillators[0].voice_count = *ports.osc1_voices as u8;
        self.synth.oscillators[0].voices_detune = *ports.osc1_super_detune / 100.0;
        self.synth.oscillators[0].wave = match *ports.osc1_wave {
            x if x < 1.0 => OscWave::Sine,
            x if x < 2.0 => OscWave::Tri,
            x if x < 3.0 => OscWave::Saw,
            x if x < 4.0 => OscWave::Exp,
            x if x < 5.0 => OscWave::Square,
            x if x < 6.0 => OscWave::PulseQuarter,
            x if x < 7.0 => OscWave::PulseEighth,
            _ => OscWave::Sine,
        };

        self.synth.filter.input0 = *ports.fil1_mode;
        self.synth.filter.input1 = *ports.fil1_cutoff;
        self.synth.filter.feedback0 = *ports.fil1_resonance;
        self.synth.filter.feedback1 = *ports.fil1_slope;
        self.synth.filter.feedback0_1 = *ports.fil1_feedback0_1;
        self.synth.filter.feedback1_0 = *ports.fil1_feedback1_0;

        
        let control_sequence = ports
            .midi
            .read(self.urids.atom.sequence, self.urids.unit.beat)
            .unwrap();

        for (timestamp, message) in control_sequence {
            let timestamp: usize = if let Some(timestamp) = timestamp.as_frames() {
                timestamp as usize
            } else {
                continue;
            };

            let message = if let Some(message) = message.read(self.urids.midi.wmidi, ()) {
                message
            } else {
                continue;
            };

            match message {
                MidiMessage::NoteOn(_, note, velocity) => {
                    let id: u8 = note.into();
                    let velocity: u8 = velocity.into();
                    self.synth.note_on(id, velocity);
                    println!("received note_on {note:?}");
                    println!("there are {} voices...", self.synth.voices.len());
                },
                MidiMessage::NoteOff(_, note, velocity) => {
                    let id: u8 = note.into();
                    let velocity: u8 = velocity.into();
                    self.synth.note_off(id, velocity);
                    println!("received note_off {note:?}");
                    println!("there are {} voices...", self.synth.voices.len());
                },
                MidiMessage::ProgramChange(_, program) => {

                }
                _ => (),
            }
        }

        // change output volume
        self.synth.output_volume = coef;

        // run synthesiser
        self.synth.run(ports.out_l.iter_mut().zip(ports.out_r.iter_mut()));
        

    }
}
// The `lv2_descriptors` macro creates the entry point to the plugin library. It takes structs that implement `Plugin` and exposes them. The host will load the library and call a generated function to find all the plugins defined in the library.
lv2_descriptors!(Amp);
