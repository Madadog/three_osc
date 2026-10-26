//! What each control does, shown when it is hovered, in either theme.

/// A description of an effect's control, followed by what modulating it does.
macro_rules! effect {
    ($text:literal) => {
        concat!(
            $text,
            " Modulation dropped here follows the newest note's sources, since the effects \
             process all the notes together."
        )
    };
}

pub const ADVANCED: &str = "Settings for quality against CPU (the oversampling, how the wavetables are read and how \
     often the modulation is worked out), and the folder of wavetable files.";

pub const OVERSAMPLING: &str = "Runs the voices at 2, 4 or 8 times the sample rate and filters them back down, so that the \
     sidebands of FM, PM and AM and the filters' distortion above the Nyquist frequency are \
     filtered out instead of folding back down as aliasing (the waves themselves are alias-free \
     either way). The voices cost about that many times the CPU (the Ladder filter less, since it \
     runs oversampled on its own); the effects run at the sample rate. Presets leave it as it is.";
pub const INTERPOLATION: &str = "How the oscillators read their wavetables in between samples. Cubic is the cleanest. Linear \
     is a little cheaper and adds some aliasing noise on high notes. None (no interpolation) is \
     the cheapest and the grittiest: it steps between the samples, which is clearly audible on \
     all but the lowest notes.";
pub const MODULATION_RATE: &str = "How often the envelopes, LFOs, modulation routes, pitch and filter settings are worked out. \
     Block works them out every 32 samples and glides gains, pitch and the Digital filter in \
     between, which is plenty for most sounds. Sample works them out at every sample, for the \
     snappiest envelopes, and LFOs and oscillators modulating at audio rates. It costs little \
     extra while nothing moves, and up to two or three times the CPU while modulation does.";

pub const DISTORTION: &str = "Soft clipping (a tanh curve) of the synth's output, anti-aliased so that high notes stay \
     clean.";
pub const DISTORTION_DRIVE: &str = effect!("How hard the sound is pushed into the clipping, in decibels.");
pub const DISTORTION_MIX: &str = effect!("From the clean sound at 0% to only the distorted one at 100%.");
pub const CHORUS: &str = "Mixes in a copy of the sound whose delay sweeps back and forth, a quarter of a cycle apart in \
     the two channels, for a wider, shimmering sound.";
pub const CHORUS_RATE: &str = effect!("How fast the delay sweeps.");
pub const CHORUS_DEPTH: &str = effect!("How far the delay sweeps: up to 6 ms either way of 12 ms.");
pub const CHORUS_MIX: &str =
    effect!("From the dry sound at 0% to only the delayed copy at 100%; 50% is the classic chorus.");
pub const DELAY: &str = "Echoes of the sound.";
pub const DELAY_TIME: &str = effect!("The time between echoes. Changing it glides the echoes' pitch, like tape.");
pub const DELAY_FEEDBACK: &str = effect!("How much of each echo comes back as the next one.");
pub const DELAY_MIX: &str = effect!("From the dry sound at 0% to only the echoes at 100%.");
pub const DELAY_PING_PONG: &str = "The echoes bounce from one channel to the other, starting on the left.";
pub const REVERB: &str = "A room around the sound (Freeverb: eight combs into four allpasses per channel).";
pub const REVERB_SIZE: &str = effect!("How long the reverb rings.");
pub const REVERB_DAMPING: &str = effect!("How much faster the highs die away than the lows.");
pub const REVERB_MIX: &str = effect!("From the dry sound at 0% to only the reverb at 100%.");

pub const OSC_WAVE: &str = "The wavetable the oscillator plays. The basic shapes and fixed pulse widths are single \
     frames; PW Sweep and Basic Shapes have several frames that Position morphs between. File \
     plays a wavetable loaded from a .wav file (see Wavetable Files on the Advanced tab). All \
     waves are bandlimited (alias-free).";
pub const OSC_POSITION: &str = "Position within the wavetable. Only has an effect on wavetables with more than one frame: for \
     PW Sweep it sweeps the pulse width from 50% (a square wave) down to a very narrow pulse, and \
     Basic Shapes morphs from a sine through the triangle, saw, exponential and square to ever \
     narrower pulses.";
pub const OSC_AMP: &str = "Oscillator output volume. Doesn't affect PM, FM or AM.";
pub const OSC_PAN: &str = "Where the oscillator sits in the stereo field, from fully left to fully right. Its unison \
     voices spread out around it by the Stereo Width, less so the further it is panned, so that \
     they stay within the field. Modulation moves it once per control rate block.";
pub const OSC_SEMITONE: &str = "Oscillator detune in semitones.";
pub const OSC_OCTAVE: &str = "Oscillator detune in octaves. Modulation moves it in whole octaves.";
pub const OSC_MULTIPLIER: &str = "Oscillator pitch multiplier / divider. Positive values multiply pitch, while negative values \
     divide. A value of 0 means this control is bypassed. Modulation moves it in whole steps, \
     through the harmonics (x2, x3...) or subharmonics (/2, /3...).";
pub const OSC_PM: &str = "Phase modulation of this oscillator by the oscillator after it.";
pub const OSC_FM: &str = "Frequency modulation of this oscillator by the oscillator after it. Can be used for vibrato \
     effects when the modulator is at a low pitch.";
pub const OSC_AM: &str = "Amplitude modulation of this oscillator by the oscillator after it. Also known as 'Ring \
     Modulation'. Can be used for telephone sounds and tremolo.";
pub const OSC_VOICES: &str = "Copies of this oscillator which are overlaid at different pitches. Useful for the supersaw \
     sound.";
pub const OSC_DETUNE: &str = "Unison voice detune. Does nothing if unison = 1.";
pub const OSC_PHASE: &str = "Point at which the oscillator wave starts. Does nothing when Phase Rand = 100.";
pub const OSC_PHASE_RAND: &str = "Partially or fully randomises the point where the oscillator wave starts. Keep this fairly \
     high when using Unison.";

pub const WAVETABLE_FILE: &str = "The .wav file this oscillator plays as a wavetable: one or more single cycles one after the \
     other, 2048 samples each unless the file says otherwise. Pick one from the wavetable folder \
     here, or type a path into Wavetable Files on the Advanced tab.";
pub const WAVETABLE_FOLDER: &str = "Put .wav wavetables in this folder and set an oscillator's Wave to File to play them. The \
     folder is Documents/Three Osc/Wavetables in your home directory, or wherever the \
     THREE_OSC_WAVETABLES environment variable points.";
pub const WAVETABLE_PATH: &str = "A file outside the folder, loaded into an oscillator with the buttons.";
pub const WAVETABLE_LOAD: &str = "Loads the file into this oscillator, and sets its Wave to File.";

pub const LFO_WAVE: &str = "The LFO's waveform. Random draws a new value each cycle and holds it (a sample and hold), and \
     Smooth Random glides from each value to the next. Each note draws its own random values, \
     except with the Free trigger, where the notes share the free running LFO's.";
pub const LFO_RATE: &str = "How fast the LFO oscillates, up to 1 kHz (or, with Sync on, how long a cycle takes, as a note \
     length at the host's tempo). At the Block modulation rate (see the Advanced tab), \
     modulation is worked out every 32 samples, so an LFO much faster than a few hundred Hz \
     steps and aliases there: audio-rate LFOs want the Sample rate.";
pub const LFO_DIVISION: &str = "How long a cycle of the LFO takes, as a note length at the host's tempo (120 beats a \
     minute when the host doesn't say): from 8 bars down to a 64th note, with dotted (D) and \
     triplet (T) lengths. Modulating the rate still scales it, by up to 4 octaves at 100%.";
pub const LFO_SYNC: &str = "Syncs the LFO to the host's tempo: its rate becomes a note length. Set to Free as well, it \
     follows the host's song position while it plays, so its cycles line up with the bars.";
pub const LFO_PHASE: &str = "Where in its cycle each note starts the LFO (for Free, how far ahead of the free running LFO \
     the notes are), from 0 to 360 degrees: 90 starts a sine at its top.";
pub const LFO_TRIGGER: &str = "How each note starts the LFO. Retrigger starts it at its phase, so every note's modulation \
     starts the same. Free carries on from an LFO that runs all the time, so the notes move \
     together, in time with the song when synced. One Shot runs through the cycle once, from its \
     phase, and holds the end: an extra envelope (a Saw rises, a Triangle falls).";
pub const LFO_FADE: &str = "How long the LFO takes to fade in at the start of each note, from nothing to its full swing: \
     vibrato that comes in as a note is held, say.";

pub const FILTER_MODEL: &str = "There are 3 filter models: Digital is an SVF identical to an IIR biquad filter (except stable \
     over large cutoff changes), RC is a darker filter capable of aggressive self-resonance, and \
     Ladder is based on a famous analog filter and sounds the best, with its code coming from \
     janne808's Kocmoc Rack Modules project.";
pub const FILTER_TYPE: &str = "Lowpass cuts out high frequencies, Highpass cuts out low frequencies, and Bandpass allows a \
     small band of frequencies at the cutoff point.";
pub const FILTER_CUTOFF: &str = "Changes the frequency at which the filter starts taking effect. Try it out.";
pub const FILTER_RESONANCE: &str = "Adds feedback to the filter loop, creating a volume spike at the filter's cutoff frequency. \
     On the RC and Ladder filters, setting this high enough creates a self-sustaining sine wave.";
pub const FILTER_DRIVE: &str = "Multiplies the amplitude of the filter input, creating distortion inside the RC and Ladder \
     filters. Does not amplify when Model = None or Digital, to keep volume equal between \
     filters.";
pub const FILTER_KEYTRACK: &str = "Amount the filter cutoff is affected by note frequency; Keytrack of 100% means the filter \
     cutoff will follow the note frequency exactly, making higher notes brighter and lower notes \
     darker.";
pub const FILTER_ENV_AMOUNT: &str = "Amount the envelope affects the filter cutoff.";

pub const ENV_ATTACK: &str = "Envelope start time. This gives a \"fade in\" effect when controlling volume.";
pub const ENV_DECAY: &str = "Time for envelope to reach sustain level. This gives a \"pluck\" effect when controlling \
     volume. Does nothing when sustain = 100%.";
pub const ENV_SUSTAIN: &str = "The level the envelope will remain at while the note is held. Used for sustained sounds, like \
     flutes or strings. When set to 0 the note ends when decay finishes.";
pub const ENV_RELEASE: &str = "Time for the envelope to finish after the note is released. Useful for sounds which persist a \
     while after they're played, like bells or chimes. Has mostly no effect when sustain = 0.";

pub const POLYPHONY: &str = "Polyphonic means an unlimited number of notes can be played simultaneously. Monophonic means \
     only one note can be played at a time. Legato is the same as monophonic, except notes are \
     connected; envelopes / oscillator phases won't reset when gliding between notes.";
pub const PORTAMENTO: &str = "How slowly note pitch will glide to a new note. In Polyphonic mode, this only affects Pitch \
     Slide.";
pub const PITCH_SLIDE: &str = "Initial pitch shift at the start of a note.";
pub const OCTAVE_DRIFT: &str = "Stretches the octave width so that consecutive octaves are further or closer together. In \
     12TET (i.e. the chromatic scale), octaves are the only perfect interval, which means playing \
     them with harsh waves (saw, square, etc) will result in a louder and harsher sound than \
     playing any other two notes. This control is meant to fix that.";
pub const OUTPUT_GAIN: &str = "Master output volume. Adjust if the synth is too loud / too quiet. Modulation dropped here \
     controls the volume of each note (Volume (all) in the matrix).";
pub const STEREO_WIDTH: &str = "Spreads each oscillator's unison voices across the stereo field around the oscillator's Pan, \
     with the lowest pitched voice furthest left and the highest furthest right. At 0 all of an \
     oscillator's voices are where its Pan puts it (in the centre, unless panned). An oscillator \
     with Unison = 1 is always just where its Pan puts it.";
pub const GLOBAL_PITCH: &str = "Pitch detune for the whole synth, in semitones. Modulation dropped here controls the pitch of \
     every oscillator (Pitch (all) in the matrix).";
pub const BEND_RANGE: &str = "Controls the range of the MIDI pitch wheel in semitones. Useful if you have a MIDI keyboard.";

pub const MOD_AMOUNT: &str = "How much of the source is applied to the target, from -100% to 100%.";
pub const MOD_POLARITY: &str = "Whether the route swings both ways around the control (±, from -1 to 1) or only one way \
     (+, from 0 to 1), whatever its source does by itself. The LFOs, the oscillators, the key \
     and the random value swing both ways by themselves, the rest one way.";
pub const MOD_CURVE: &str = "Bends the source on its way to the amount: above 0 it stays near the bottom of its range for \
     longer and rises late (up to the 4th power), below 0 it rises early and stays near the top.";
pub const MOD_STEPS: &str = "Steps the source into this many levels across its range (Off for smooth): 2 flips between its \
     ends, and 13 on a one way route to a pitch at 100% (an octave) plays the semitones.";
pub const MOD_VIA: &str = "Another source that scales the route, from nothing at its bottom (or, for one that swings \
     both ways, at its lowest) to all of it at its top: the mod wheel bringing in an LFO's \
     vibrato, say.";
pub const CLEAR_ROUTE: &str = "Clears this row.";
pub const REMOVE_ROUTE: &str = "Removes this modulation route.";
