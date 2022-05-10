use std::f32::consts::PI;

use lv2::prelude::*;

mod synth;
use synth::{
    filter::{FilterModel, FilterType},
    oscillator::OscWave,
    Polyphony, ThreeOsc,
};
use wmidi::MidiMessage;

/// Control ports for the synth's LV2 UI.
///
/// This struct must match the output file `portstruct.rs` generated by `build.rs`, and it must be kept up-to-date manually.
/// I tried to automate it with `include!(concat!(env!("OUT_DIR"), "/portstruct.rs"));` but apparently it didn't work with derive macro expansion
/// for `#[derive(PortCollection)]`.
#[derive(PortCollection)]
struct Ports {
    midi: InputPort<AtomPort>,
    out_l: OutputPort<Audio>,
    out_r: OutputPort<Audio>,
    osc1_wave: InputPort<Control>,
    osc1_amp: InputPort<Control>,
    osc1_semitone: InputPort<Control>,
    osc1_octave: InputPort<Control>,
    osc1_multiplier: InputPort<Control>,
    osc1_pm: InputPort<Control>,
    osc1_fm: InputPort<Control>,
    osc1_am: InputPort<Control>,
    osc1_voices: InputPort<Control>,
    osc1_super_detune: InputPort<Control>,
    osc1_phase: InputPort<Control>,
    osc1_phase_rand: InputPort<Control>,
    osc2_wave: InputPort<Control>,
    osc2_amp: InputPort<Control>,
    osc2_semitone: InputPort<Control>,
    osc2_octave: InputPort<Control>,
    osc2_multiplier: InputPort<Control>,
    osc2_pm: InputPort<Control>,
    osc2_fm: InputPort<Control>,
    osc2_am: InputPort<Control>,
    osc2_voices: InputPort<Control>,
    osc2_super_detune: InputPort<Control>,
    osc2_phase: InputPort<Control>,
    osc2_phase_rand: InputPort<Control>,
    osc3_wave: InputPort<Control>,
    osc3_amp: InputPort<Control>,
    osc3_semitone: InputPort<Control>,
    osc3_octave: InputPort<Control>,
    osc3_multiplier: InputPort<Control>,
    osc3_pwm: InputPort<Control>,
    osc3_voices: InputPort<Control>,
    osc3_super_detune: InputPort<Control>,
    osc3_phase: InputPort<Control>,
    osc3_phase_rand: InputPort<Control>,
    fil1_model: InputPort<Control>,
    fil1_type: InputPort<Control>,
    fil1_cutoff: InputPort<Control>,
    fil1_resonance: InputPort<Control>,
    fil1_drive: InputPort<Control>,
    fil1_keytrack: InputPort<Control>,
    fil1_env_amount: InputPort<Control>,
    fil1_attack: InputPort<Control>,
    fil1_decay: InputPort<Control>,
    fil1_sustain: InputPort<Control>,
    fil1_release: InputPort<Control>,
    fil1_slope: InputPort<Control>,
    vol_attack: InputPort<Control>,
    vol_decay: InputPort<Control>,
    vol_sustain: InputPort<Control>,
    vol_release: InputPort<Control>,
    vol_slope: InputPort<Control>,
    lfo_target: InputPort<Control>,
    lfo_wave: InputPort<Control>,
    lfo_freq: InputPort<Control>,
    lfo_freq_mod: InputPort<Control>,
    lfo_amp_mod: InputPort<Control>,
    lfo_mod_mod: InputPort<Control>,
    lfo_filter_mod: InputPort<Control>,
    polyphony: InputPort<Control>,
    portamento_rate: InputPort<Control>,
    pitch_offset: InputPort<Control>,
    octave_detune: InputPort<Control>,
    output_gain: InputPort<Control>,
    global_pitch: InputPort<Control>,
    bend_range: InputPort<Control>,
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
struct SynthLv2 {
    synth: ThreeOsc,
    urids: URIDs,
}

impl Plugin for SynthLv2 {
    type Ports = Ports;

    type InitFeatures = Features<'static>;
    type AudioFeatures = ();

    /// Create the plugin. Does initial setup (i.e. necessary allocation to stay realtime safe)
    fn new(plugin_info: &PluginInfo, features: &mut Features<'static>) -> Option<Self> {
        println!("Sample rate was: {}", plugin_info.sample_rate());
        Some(Self {
            synth: ThreeOsc::new(plugin_info.sample_rate()),
            urids: features.map.populate_collection()?,
        })
    }

    /// Read parameters from LV2 control ports, update actual synth parameters,
    /// then generate audio.
    fn run(&mut self, ports: &mut Ports, _features: &mut (), _sample_count: u32) {
        let coef = if *(ports.output_gain) > -90.0 {
            10.0_f32.powf(*(ports.output_gain) * 0.05)
        } else {
            0.0
        };
        self.synth.output_volume = coef;
        self.synth.bend_range = *ports.bend_range;
        self.synth.polyphony = match *ports.polyphony {
            x if x < 1.0 => Polyphony::Polyphonic,
            x if x < 2.0 => Polyphony::Monophonic,
            _ => Polyphony::Legato,
        };
        // Scaling: This is a lerp, and must be proportional to the sample rate
        // ... the '0.002' is just user-friendly control scaling.
        // TODO: test and make sure this actually keeps time constant across sample rates
        self.synth.portamento_rate = 1.0 - ports.portamento_rate.powf(0.002 * self.synth.sample_rate as f32 / 44100.0);
        self.synth.portamento_offset = *ports.pitch_offset;

        // multiplies delta: smaller = higher pitch
        self.synth.octave_detune = 1.0 - *ports.octave_detune;

        // adjust master gain envelope

        // Attack and decay's minimum value of 0.001 is manually set to 0.0. This is a workaround to
        // make logarithmic values display nicely in Ardour (which ignores the 'logarithmic' port
        // property when the port's minimum value is 0) while still allowing instant attack times.
        // Naturally we don't do this for the release port.
        // TODO: decide if the decay control needs this at all
        self.synth.gain_envelope.attack_time = if *ports.vol_attack <= 0.001 {
            0.0
        } else {
            *ports.vol_attack
        };
        self.synth.gain_envelope.decay_time = if *ports.vol_decay <= 0.001 {
            0.0
        } else {
            *ports.vol_decay
        };
        self.synth.gain_envelope.sustain_level = *ports.vol_sustain;
        self.synth.gain_envelope.release_time = *ports.vol_release;
        self.synth.gain_envelope.set_slope(*ports.vol_slope);

        self.synth.filter_controller.envelope_amount = (*ports.fil1_env_amount).powi(2) * 22000.0;
        self.synth.filter_controller.keytrack = *ports.fil1_keytrack;
        self.synth.filter_controller.cutoff_envelope.attack_time = if *ports.fil1_attack <= 0.001 {
            0.0
        } else {
            *ports.fil1_attack
        };
        self.synth.filter_controller.cutoff_envelope.decay_time = if *ports.fil1_decay <= 0.001 {
            0.0
        } else {
            *ports.fil1_decay
        };
        self.synth.filter_controller.cutoff_envelope.sustain_level = *ports.fil1_sustain;
        self.synth.filter_controller.cutoff_envelope.release_time = *ports.fil1_release;
        self.synth
            .filter_controller
            .cutoff_envelope
            .set_slope(*ports.fil1_slope);
        self.synth.filter_controller.target_cutoff = *ports.fil1_cutoff;
        self.synth.filter_controller.resonance = *ports.fil1_resonance;
        self.synth.filter_controller.drive = *ports.fil1_drive;
        self.synth.filter_controller.filter_type = match *ports.fil1_type {
            x if x < 1.0 => FilterType::Lowpass,
            x if x < 2.0 => FilterType::Bandpass,
            x if x <= 3.0 => FilterType::Highpass,
            _ => FilterType::Highpass,
        };
        self.synth.filter_controller.filter_model = match *ports.fil1_model {
            x if x < 1.0 => FilterModel::None,
            x if x < 2.0 => FilterModel::RcFilter,
            x if x < 3.0 => FilterModel::LadderFilter,
            x if x <= 4.0 => FilterModel::BiquadFilter,
            _ => FilterModel::None,
        };

        // lfo
        self.synth.lfo_params.target_osc = match *ports.lfo_target {
            x if x < 1.0 => None,
            x if x < 2.0 => Some(0),
            x if x < 3.0 => Some(1),
            x if x < 4.0 => Some(2),
            _ => Some(3),
        };
        self.synth.lfo_params.wave = OscWave::from_index(*ports.lfo_wave);
        self.synth.lfo_params.freq = *ports.lfo_freq;
        self.synth.lfo_params.freq_mod = ports.lfo_freq_mod.powi(2);
        self.synth.lfo_params.amp_mod = *ports.lfo_amp_mod;
        self.synth.lfo_params.mod_mod = *ports.lfo_mod_mod;
        self.synth.lfo_params.filter_mod = *ports.lfo_filter_mod;

        // apply oscillator ports
        // ... TODO: write a macro for all this
        {
            // osc1
            self.synth.oscillators[0].amp = *ports.osc1_amp / 100.0;
            self.synth.oscillators[0].semitone = *ports.osc1_semitone + *ports.global_pitch;
            self.synth.oscillators[0].octave = *ports.osc1_octave as i32;
            // Frequency multiplication if Freq. Mult is positive, frequency division if negative.
            // (This is because negative multiplication would just reverse the wave, which is not very useful)
            self.synth.oscillators[0].pitch_multiplier = if ports.osc1_multiplier.is_sign_positive() {
                1.0 + *ports.osc1_multiplier
            } else {
                1.0 / (1.0 - *ports.osc1_multiplier)
            };
            self.synth.oscillators[0].voice_count = *ports.osc1_voices as u8;
            self.synth.oscillators[0].voices_detune = (*ports.osc1_super_detune / 100.0).powi(3);
            self.synth.oscillators[0].phase = *ports.osc1_phase * 2.0 * PI / 100.0;
            self.synth.oscillators[0].phase_rand = *ports.osc1_phase_rand * 2.0 * PI / 100.0;
            self.synth.oscillators[0].wave = OscWave::from_index(*ports.osc1_wave);
            self.synth.oscillators[0].pm = (*ports.osc1_pm).powi(2);
            self.synth.oscillators[0].fm = (*ports.osc1_fm).powi(2);
            self.synth.oscillators[0].am = (*ports.osc1_am).powi(2);
            
            // osc2
            self.synth.oscillators[1].amp = *ports.osc2_amp / 100.0;
            self.synth.oscillators[1].semitone = *ports.osc2_semitone + *ports.global_pitch;
            self.synth.oscillators[1].octave = *ports.osc2_octave as i32;
            self.synth.oscillators[1].pitch_multiplier = if ports.osc2_multiplier.is_sign_positive() {
                1.0 + *ports.osc2_multiplier
            } else {
                1.0 / (1.0 - *ports.osc2_multiplier)
            };
            self.synth.oscillators[1].voice_count = *ports.osc2_voices as u8;
            self.synth.oscillators[1].voices_detune = (*ports.osc2_super_detune / 100.0).powi(3);
            self.synth.oscillators[1].phase = *ports.osc2_phase * 2.0 * PI / 100.0;
            self.synth.oscillators[1].phase_rand = *ports.osc2_phase_rand * 2.0 * PI / 100.0;
            self.synth.oscillators[1].wave = OscWave::from_index(*ports.osc2_wave);
            self.synth.oscillators[1].pm = (*ports.osc2_pm).powi(2);
            self.synth.oscillators[1].fm = (*ports.osc2_fm).powi(2);
            self.synth.oscillators[1].am = (*ports.osc2_am).powi(2);
            
            // osc3
            self.synth.oscillators[2].amp = *ports.osc3_amp / 100.0;
            self.synth.oscillators[2].semitone = *ports.osc3_semitone + *ports.global_pitch;
            self.synth.oscillators[2].octave = *ports.osc3_octave as i32;
            self.synth.oscillators[2].pitch_multiplier = if ports.osc3_multiplier.is_sign_positive() {
                1.0 + *ports.osc3_multiplier
            } else {
                1.0 / (1.0 - *ports.osc3_multiplier)
            };
            self.synth.oscillators[2].voice_count = *ports.osc3_voices as u8;
            self.synth.oscillators[2].voices_detune = (*ports.osc3_super_detune / 100.0).powi(3);
            self.synth.oscillators[2].phase = *ports.osc3_phase * 2.0 * PI / 100.0;
            self.synth.oscillators[2].phase_rand = *ports.osc3_phase_rand * 2.0 * PI / 100.0;
            self.synth.oscillators[2].wave = OscWave::from_index_pulse(*ports.osc3_wave);
            self.synth.oscillators[2].pulse_width = *ports.osc3_pwm * 2.0 * PI / 100.0;
        }
        
        let control_sequence = ports
        .midi
        .read(self.urids.atom.sequence, self.urids.unit.beat)
        .unwrap();
        
        for (timestamp, message) in control_sequence {
            let _timestamp: usize = if let Some(timestamp) = timestamp.as_frames() {
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
                    println!("received note_on {note:?} (vel: {velocity:?})");
                    println!("there are {} voices...", self.synth.voices.len());
                }
                MidiMessage::NoteOff(_, note, velocity) => {
                    let id: u8 = note.into();
                    let velocity: u8 = velocity.into();
                    self.synth.note_off(id, velocity);
                    println!("received note_off {note:?}");
                    println!("there are {} voices...", self.synth.voices.len());
                }
                MidiMessage::PitchBendChange(_, bend) => {
                    self.synth.pitch_bend(bend.into());
                }
                _ => (),
            }
        }

        // run synthesiser
        self.synth.run(&mut ports.out_l, &mut ports.out_r);
    }
}
// The `lv2_descriptors` macro creates the entry point to the plugin library. It takes structs that implement `Plugin` and exposes them. The host will load the library and call a generated function to find all the plugins defined in the library.
lv2_descriptors!(SynthLv2);
