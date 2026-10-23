# Three Osc

A CLAP / VST3 synthesizer based on [Triple Oscillator](https://github.com/LMMS/lmms), a polyphonic subtractive synthesizer with three oscillators that can modulate each other in various ways. Built with [nih-plug](https://github.com/robbert-vdh/nih-plug).

Extends the original with several useful QOL features, including bandlimited synthesis by default, unison with many detuned voices, and legato.

Currently a work in progress, but usable nevertheless. 

![alt text](/images/three_osc_v1.png "Ardour hosting a bass preset which uses the ladder filter (this screenshot is of the older LV2 version's generic host UI).")


## Features

* 3 oscillators which can modulate eachother via phase, frequency, and amplitude modulation (PM, FM & AM) simultaneously
* Choose between 3 multimode filter models (RC, Ladder, Simper SVF) with keytracking and envelope. The Digital (SVF) and Ladder filters run eight voices at a time (with AVX where available).
* Unlimited polyphony, with optional monophonic and legato modes
* ADSR envelopes with slopes smoothly adjustable from exponential to logarithmic.
* Wavetable oscillators with sine, triangle, absolute sine, saw, square and pulse waves, a pulse width sweep wavetable and a Basic Shapes wavetable (those nine shapes in a row) that each oscillator's position control morphs through, and wavetables loaded from `.wav` files (single cycles or multi-frame tables, including the `clm` frame size chunk Serum-style tables carry)
* Bandlimited wave synthesis using wavetables computed via FFT (harmonics extend up to the Nyquist frequency, with no unexpected drop-off), read with cubic, linear or no interpolation (a global control, for trading quality against CPU)
* Detunable unison with (up to) 32 voices for each oscillator (i.e. yes it can supersaw), with a stereo width control (at its widest by default) that spreads the voices across the stereo field around each oscillator's pan
* Integer frequency division/multiplication for each oscillator for harmonic sound effects
* A modulation matrix: three LFOs, two spare envelopes, the volume and filter envelopes, velocity, key position, mod wheel, aftertouch and a per-note random value can each modulate pitch, wavetable position, amplitude, pan, unison detune, PM/FM/AM amounts, filter cutoff, resonance and drive, note volume, the LFO rates and the effects' controls, through 16 routes. Drag a source's handle onto a slider in the GUI to route it. Modulation is worked out every 32 samples, or at every sample with the Modulation Rate control.
* Effects after the voices: distortion (anti-aliased soft clipping), stereo chorus, delay (with ping-pong) and reverb
* Portamento and adjustable initial pitch slide for kickdrum synthesis.
* MIDI pitch bend with an adjustable range
* Presets: 21 factory presets (basses, leads, pads, plucks, keys and effects), and your own saved as JSON files
* Microtuning with Scala files: any `.scl` scale, with an optional `.kbm` keyboard mapping
* A simple GUI (made with [egui](https://github.com/emilk/egui)) with every control on one page (the effects on a tab in place of the oscillators), the presets and the tuning in the title bar, displays of each oscillator's wave, the LFOs, the envelopes, the filter's frequency response and the effects, which follow the notes as they play, and a spectrogram and level meter of the output. Hover a control's label (or a display) for a description of what it does.
* Sample-accurate note timing

## Sound Demo
Coming soon...

## Building
Three Osc is built with [nih-plug](https://github.com/robbert-vdh/nih-plug), which exports it as a CLAP plugin and a VST3 plugin. You need a [Rust toolchain](https://rustup.rs/); on Linux you also need the X11 and OpenGL development libraries for the GUI, the same as for any other nih-plug plugin. On Debian/Ubuntu:

```shell
sudo apt-get install libgl-dev libx11-xcb-dev libxcb1-dev libxcb-dri2-0-dev libxcb-icccm4-dev libxcursor-dev libxkbcommon-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

0. `cd` into the three_osc directory.
1. Run `cargo xtask bundle three-osc --release`. This compiles the synth and creates the plugin bundles in `./target/bundled/`: `Three Osc.clap` and `Three Osc.vst3`.
2. Copy the bundle(s) into a directory your host looks in:
    * CLAP: `~/.clap/` on Linux, `~/Library/Audio/Plug-Ins/CLAP/` on macOS, `C:\Program Files\Common Files\CLAP\` on Windows.
    * VST3: `~/.vst3/` on Linux, `~/Library/Audio/Plug-Ins/VST3/` on macOS, `C:\Program Files\Common Files\VST3\` on Windows.
3. Load it into your preferred host (Ardour, Bitwig, Carla, Reaper, ...) and have fun.

### Standalone
The synth can also run on its own, outside of a plugin host, using JACK or your system's audio and MIDI APIs. This needs a few more dependencies (on Debian/Ubuntu: `libasound2-dev libjack-jackd2-dev`), so it is behind a feature flag:

```shell
cargo run --release --features standalone
# or, to list the audio backend / device options:
cargo run --release --features standalone -- --help
```

### Benchmarks
Rough throughput numbers for the synth engine (time per sample for a few patches, at both modulation rates, and wavetable generation time and memory):

```shell
cargo test --release --lib benchmark -- --ignored --nocapture --test-threads=1
```

### Modulation
Every modulation route goes from a source to a target with an amount from -100% to 100%. At 100% a source's full swing moves a slider target across its whole length (an oscillator's pan from the left to the right), a pitch target by an octave (the Fine Pitch target by a semitone), the filter cutoff by 10 octaves, the resonance by 10 times, the drive by 32 times, an LFO's rate and the chorus rate by 4 octaves and the delay time by 2; a target's routes add up and the result is clamped to the control's range. The LFOs and the random value are bipolar (-1 to 1), the envelopes and MIDI sources go from 0 to 1. Each note runs its own copies of the LFOs and envelopes, so the routes are evaluated per voice at the control rate (see below). The effects process all the notes together, so their controls follow the newest note's sources (and hold where they were once the notes have finished). In the GUI, drag the handle next to a source's title (or in the "More Sources" box) onto any slider with a drop highlight to fill in a free row of the matrix (the effects' tab takes the oscillators' place, so the sources stay in view). Modulated sliders show an orange marker that lists their routes, and while notes play, an orange line from the slider's value to where the newest note's modulation takes it.

### Effects
The FX tab, in the oscillators' place, has four effects that process the synth's output (after the Output Gain) in this order, each switched on separately:

* Distortion: `tanh` soft clipping with a drive of up to 36 dB, anti-aliased with the first order antiderivative method (the output is the average of `tanh` between consecutive inputs, worked out from its antiderivative `ln(cosh(x))`), which takes most of the aliasing out of high notes for half a sample of delay and no oversampling.
* Chorus: a copy of the sound delayed by 6 to 18 ms, swept by an LFO a quarter of a cycle apart in the two channels.
* Delay: up to 2 seconds, with feedback and an optional ping-pong between the channels. Changing the time glides the echoes' pitch, like tape.
* Reverb: Jezar's Freeverb, eight damped comb filters into four allpasses per channel.

Every setting is smoothed sample by sample, so automation and modulation (applied once per control rate block) don't step, and an effect that is switched on starts from silence. All four together take about 130 ns per sample on the machine below, a little over twice what 8 notes of the default patch take.

### Presets
The title bar's preset menu lists the factory presets by category, then your own; the arrows step through them, and a `*` after the name means the controls have been moved since the preset was loaded. Save stores the current settings as a preset of your own in `Documents/Three Osc/Presets` in your home directory (or the folder the `THREE_OSC_PRESETS` environment variable names), named after the file. The tuning isn't part of a preset: it stays as it is while you browse.

A preset is a JSON file that lists the parameters that differ from their defaults, by their IDs (the same as in the plugin's saved state), plus the wavetable file of each oscillator that plays one:

```json
{
  "name": "Wobble",
  "category": "Bass",
  "params": {
    "fil1_cutoff": 300.0,
    "fil1_model": "ladder",
    "lfo_wave_1": "triangle",
    "mod_source_1": "lfo1",
    "mod_target_1": "cutoff",
    "mod_amount_1": 0.35,
    "osc1_voices": 3
  }
}
```

Sliders are numbers in the units they show (Hz, seconds, semitones, decibels, and 0 to 1 for percentages, except for the oscillators' Amplitude, Unison Detune, Phase and Phase Rand., which go from 0 to 100), switches are `true` or `false`, and choices are the IDs of their options. Anything a preset leaves out gets its default, so presets keep loading as parameters are added. Loading a preset sets each parameter through the host, as moving its slider would, so the host keeps track of them. The factory presets are the files in `presets/`, built into the plugin.

### Tuning
The title bar's Tuning menu retunes the keys with [Scala](https://www.huygens-fokker.org/scala/) files from `Documents/Three Osc/Tunings` (or the folder `THREE_OSC_TUNINGS` names), or from any path typed in:

* A scale (`.scl`) lists the pitches of its steps above its root, in cents or as ratios, the last being the period it repeats at (usually the octave, `2/1`). On its own, its root is middle C at its usual pitch (261.63 Hz), and each key goes up the scale a step.
* A keyboard mapping (`.kbm`) says which key plays which step (keys can be left silent, to put a 7 note scale on the white keys, say), which key is tuned to which frequency, and which keys are retuned at all (the others stay silent). With the standard scale, it can just move A away from 440 Hz.

The files' contents are saved with the plugin's state, so a project sounds the same on a computer without them. Only the notes' pitches change: pitch bend, the oscillators' Detune and Octave controls and the pitch modulation still move in ordinary semitones, the filter's keytracking follows the tuned pitch, and the Key Track modulation source still follows the key's place on the keyboard.

### Wavetable files
Set an oscillator's Wave to File to play a wavetable from a `.wav` file. The File row that appears lists the `.wav` files in the wavetable folder, `Documents/Three Osc/Wavetables` in your home directory (or the folder the `THREE_OSC_WAVETABLES` environment variable names); the Wavetable Files box shows the folder, rescans it, and loads a file from any other path into an oscillator. The chosen path is saved with the plugin's state.

A file holds one or more frames, single cycles one after the other. The frame length comes from the file's `clm ` chunk when it has one (wavetable editors such as Serum write it), otherwise frames are taken to be 2048 samples long when the file divides evenly into them, and any other file up to 65536 samples is one cycle. PCM (8 to 32-bit) and float files are read, stereo files are mixed to mono, and files with more than 256 frames are thinned out to 256. Position morphs through the frames like it does for PW Sweep.

The file is read and analysed on a background thread: each frame's spectrum is kept, and the bandlimited tables for every note are generated from it for the current sample rate (and regenerated when it changes), so a loaded wavetable is as alias-free as the built in ones. Until the file is loaded, or when none is set, the oscillator plays a sine.

Each note's table is only as long as its harmonics need (a power of two of at least 16 samples per harmonic, from 64 up to 2048 samples), and a wavetable with many frames shares one table between neighbouring notes when it would otherwise take more than 24 MB. The built in wavetables take 26 MB.

### Displays
The displays are drawn with the synth's own code, so they show what is played. An oscillator's display is one cycle of its wavetable at the current position (rendered from the same spectra as the tables it plays, loaded files included), with a line where the wave starts when Phase Rand is below 100%; it is dimmed while the oscillator's amplitude is 0. The LFO displays show one cycle. The envelope displays draw attack, decay, a stretch of sustain and release, each stage as wide as the square root of its time, so that a 2 ms attack is still visible next to a 2 second release. The filter display is the frequency response from 20 Hz to 20 kHz, measured by running an impulse through the filter model itself at the host's sample rate (a quiet one, so the nonlinear models show their small-signal response; the drive's distortion doesn't show), with the response at the peak of the filter envelope drawn more faintly. The effects show the distortion's curve, the chorus's sweep, the delay's echoes and the reverb's decay (its lows and, faster under the damping, its highs).

While notes play, the displays follow them: a dot for each of the newest 16 notes moves through the envelopes, a dot on each LFO shows the newest note's phase, the filter display marks the newest note's cutoff (with its envelope, keytracking and modulation), and the waves and the effects' displays show the newest note's modulated position and settings. After each buffer the audio thread writes what the GUI shows into a handful of atomics (only while the GUI is open), and the GUI reads them as it draws.

The Output section at the top right shows what the synth plays, after the effects. The spectrogram covers the last few seconds, the newest at the right, with frequency going up the side from 20 Hz to 20 kHz on a log scale and brightness showing level from -90 to 0 dB (hover it to read a frequency off it): each column is the FFT of about 90 ms of the output under a Hann window, a quarter of a window after the last. Under it, each channel's meter has a bright bar for the RMS level (over 300 ms), a dim one for the peaks, which fall back at 20 dB a second, and a line for the highest peak of the last second and a half, with that peak's level beside them, in red once the output has clipped (click the meter to clear it). The audio thread copies its output into a ring of samples (only while the GUI is open), and the GUI does the rest.

### Control rate
Envelopes, pitch (including portamento), the LFOs, the modulation matrix, keytracking and the filter coefficients are recalculated once per control rate block of `CONTROL_BLOCK_SAMPLES` samples (in `src/synth.rs`, 32 by default) rather than every sample. Gains and pitch are ramped linearly across the block and the Digital filter ramps its coefficients, so the main audible effects of the block size are on very fast envelopes and on the LFOs, which can't go faster than half the control rate (689 Hz at 44.1 kHz). Change the constant to try other sizes.

The global Modulation Rate control switches between that (Block, the default) and working everything out for every sample (Sample), where a 2 ms attack follows its curve rather than a few straight lines and an LFO can run at audio rates. The control update then works a whole block's values out at once, eight samples at a time (in `src/synth/per_sample.rs`): the envelopes, LFOs and matrix, then every sample's pitch, gains, wavetable position, modulation depths and filter coefficients, with 8-lane versions of the fast math below that give exactly the same results as one value at a time. The oscillator kernels, the filters (the Digital and Ladder filters still in lockstep across voices) and the mixing take each sample's values instead of ramps, and still render the block in one go. Whatever doesn't change within a block (an envelope in its sustain, a route from velocity or the mod wheel, an oscillator nothing modulates, filter coefficients) is only worked out once and rendered as at the block rate without a ramp, which gives exactly what working out every sample's values would. Three things still change once per block: the bandlimited table an oscillator plays (picked for the highest pitch in the block), the unison detune and the pan. The LFOs keep their phase in double precision, since at the sample rate a slow LFO's steps are too small for single precision to add up: a 0.01 Hz LFO would run 3% fast.

The cost, from `benchmark_modulation_rate` (8 notes at 44.1 kHz on a 2.1 GHz Xeon, time per sample of output, mostly with the envelopes in their sustain):

| Patch | Block | Sample | |
|---|---|---|---|
| Default (1 saw, Digital filter) | 60 ns | 63 ns | 1.1x |
| Default, no filter | 46 ns | 55 ns | 1.2x |
| Default, Ladder filter | 180 ns | 192 ns | 1.1x |
| Default, 4 modulation routes | 65 ns | 124 ns | 1.9x |
| 3 x saw, unison 8 | 327 ns | 343 ns | 1.05x |
| 3 x saw, unison 8, Ladder filter | 451 ns | 468 ns | 1.04x |
| 3 x saw, unison 8, 4 modulation routes | 382 ns | 565 ns | 1.5x |

What moves within a block costs more: each sample's LFO values, pitch factors (`2^x`), envelope levels (`x^y`) and, with a moving cutoff, filter coefficients, under SSE2 on x86-64 (the baseline, so two 4-lane halves per vector). Over the first quarter second of the notes, while their envelopes decay, the default patch takes about 1.6 times the block rate's instructions and the one with 4 routes about 2.6 times.

The control rate code uses polynomial approximations of `2^x`, `log2`, `x^y`, `sin` and `tan` (in `src/synth/fastmath.rs`), accurate to around single precision, instead of the standard library's functions, which cost more than rendering the block's samples.

### Interpolation
The global Interpolation control picks how the wavetables are read in between their samples: Cubic (4-point Hermite, the default and cleanest), Linear, or None (the sample below the read position, which steps audibly on all but the lowest notes). Each mode has its own specialised lookup kernels, so there is no per-sample cost to the choice. On an 8 note, 3 x 8 voice supersaw, Linear is about 20% cheaper than Cubic and None about 30% cheaper.

### Oscillator kernels and SIMD
Each oscillator renders a whole control rate block in one kernel call: the unison voices' phases advance as vectors, every sample reads and interpolates the wavetable at each voice and sums the voices into the block (with AVX2, eight samples' sums across the voices are worked out together), and a modulated oscillator reads its modulator's block sample for sample (the oscillators are rendered from the last to the first, so the modulation is still instantaneous). With fewer than 8 unison voices the vectors run across time instead: each holds 8 consecutive samples of one voice, whose phases have a closed form (or are added up one by one under frequency modulation), so nothing has to be summed across a vector's lanes at every sample, and the gain ramp and the additions to the output go a vector at a time too. That makes a single voice more than three times cheaper than with the vectors across voices. Phases are 32-bit fixed point: one cycle is 2^32, so they wrap for free and pitch resolves to 2^-32 cycles per sample, and a phase splits into a table index and a fraction with a shift and a mask (wavetable cycles are powers of two long).

The reads run with AVX2 and FMA where the CPU has them, falling back to a portable path otherwise. There are two AVX2 variants, one reading each voice's taps with a 128-bit load and transposing them, the other using gather instructions; which is faster depends on the CPU (gathers are slow on the Intel generations with the "Downfall" mitigation, Skylake to Tiger Lake), so the two are timed against each other when the synth starts. The filters run several voices in lockstep (a filter's recurrence is a chain of dependent operations, so one voice on its own leaves most of the CPU idle): the Digital filter eight at a time with AVX (or in portable SIMD vectors without it), producing exactly the same output as the one-voice-at-a-time version, and the ladder eight at a time with AVX2 and FMA, whose fused multiply-adds round differently from the one-voice version (staying within 0.01% of the signal level). The Digital filter works out its next state directly rather than through the paper's intermediate values, which takes the chain from one sample to the next from five dependent operations down to three. Everything is detected at runtime, so one build runs on any x86-64 CPU.

The lookups are not memory bound: shrinking the tables 8 times or tripling the number of notes changes the cost per voice by a few percent, so a 16-bit table format would buy little.

### Previous LV2 version
Older versions of Three Osc were LV2 plugins. nih-plug has no LV2 export, so the LV2 build is gone; if you need it, check out a commit from before the nih-plug port.

## Tips and Tricks
* The absolute sine / exponential wave is like a saw wave where the harmonics decrease volume at -12dB per octave instead of -6 dB per octave (i.e. it's a saw wave tracked by a soft filter). Similarly, the triangle wave is like a square wave where the harmonics diminish at -12dB per octave instead of -6dB.
* Increasing envelope slope makes it steeper, decreasing it does the opposite. Slope = 0 gives perfectly linear slopes, which are not perceptually linear. Slope = 1 gives perceptually-linear (logarithmic) volume decay.
* The Ladder and RC filter models are both capable of self-resonance at resonance >= 9.0. Underdriving the filters (i.e. drive below 1) and sweeping them very slowly gives a 'harmonic snap' effect.
* The Ladder filter runs 3x oversampled, with its input properly upsampled and its output decimated so that its distortion doesn't alias. Its cutoff can't go higher than about 12.6 kHz at a 44.1 kHz sample rate (13.7 kHz at 48 kHz), so even fully open it softens the top octave (15 kHz comes out about 11 dB below 1 kHz at 44.1 kHz). At 88.2 kHz and above it opens up fully. It runs in single precision, like the original module's 32-bit build: its output is within about -110 dB of a double precision version.
* Setting octave detune to -0.0028 gives near perfect fifths, while 0.0342 gives near perfect major thirds.
* FM changes frequency with the modulator's waveform, PM changes frequency with the derivative of the modulator's waveform. (I.E. PM by triangle == FM by square wave)

## Why did you make this?
* I wanted to write a synthesiser.
* I'm currently migrating from LMMS to Ardour. Surge and ZynAddSubFx are great synths, but...
    * Controls are spread out over multiple tabs / screens.
    * Surge is a little bit aggressive.
    * Sometimes you want to quickly make simple and predictable sounds.
* The original Triple Oscillator mostly solves these problems, but has issues of its own:
    * LMMS exclusive.
    * A bit loud.
    * Volume envelope is optional.
    * No phase randomness.
    * Originally had no bandlimited wave generation.

## TODO
* Use naive wave generation for modulation between oscillators? (stop ringing artifacts)
* Add oversampling with a control (for FM / PM)
* Switch between Naive, Wavetable, and Additive synthesis with a control
* Only generate unique wavetables when necessary (i.e. every third note, and only when harmonic count changes)
* Adjust more knobs to sensible values / defaults
* More presets (current idea: reimplement/extend patches from MDA jx10, which are unreasonably nice)
* Make the build system nicer
* Tools for working with the harmonic series
* Use audio buffering for more optimisations
* Optimise
* Extract DSP to internal crate
* Better wavetable interpolation (sinc or Niemitalo's optimal interpolators; currently 4-point cubic Hermite at best)
* Fix buffer size to reduce expensive modulation calculations at the expense of sample-accurate automation (make const so that you can play around with buffer lengths, see what is good)
* Come up with better name for "Initial Pitch Slide" control
* Improve the UI
    * Knobs instead of sliders, envelope / waveform displays
    * 2nd page for oversampling and other unimportant things
* A file browser for wavetable files outside the wavetable folder (paths can be typed in for now)
* Publish

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.