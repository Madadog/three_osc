# Three Osc

A CLAP / VST3 synthesizer based on [Triple Oscillator](https://github.com/LMMS/lmms), a polyphonic subtractive synthesizer with three oscillators that can modulate each other in various ways. Built with [nih-plug](https://github.com/robbert-vdh/nih-plug).

Extends the original with several useful QOL features, including bandlimited synthesis by default, unison with many detuned voices, and legato.

Currently a work in progress, but usable nevertheless. 

![alt text](/images/three_osc_v1.png "Ardour hosting a bass preset which uses the ladder filter (this screenshot is of the older LV2 version's generic host UI).")


## Features

* 3 oscillators which can modulate eachother via phase, frequency, and amplitude modulation (PM, FM & AM) simultaneously
* Choose between 3 multimode filter models (RC, Ladder, Simper SVF) with keytracking and envelope
* Unlimited polyphony, with optional monophonic and legato modes
* ADSR envelopes with slopes smoothly adjustable from exponential to logarithmic.
* Wavetable oscillators with sine, triangle, absolute sine, saw, square and pulse waves, plus a pulse width sweep wavetable that each oscillator's position control morphs through
* Bandlimited wave synthesis using wavetables computed via FFT (harmonics extend up to the Nyquist frequency, with no unexpected drop-off)
* Detunable unison with (up to) 32 voices for each oscillator (i.e. yes it can supersaw), with a stereo width control that spreads the voices across the stereo field
* Integer frequency division/multiplication for each oscillator for harmonic sound effects
* Vibrato, tremolo and modulation control with an LFO
* Portamento and adjustable initial pitch slide for kickdrum synthesis.
* MIDI pitch bend with an adjustable range
* A simple GUI (made with [egui](https://github.com/emilk/egui)) with every control on one page. Hover a control's label for a description of what it does.
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
Rough throughput numbers for the synth engine (time per sample for a few patches, and wavetable generation time and memory):

```shell
cargo test --release --lib benchmark -- --ignored --nocapture --test-threads=1
```

### Control rate
Envelopes, pitch (including portamento and vibrato), the LFO, keytracking and the filter coefficients are recalculated once per control rate block of `CONTROL_BLOCK_SAMPLES` samples (in `src/synth.rs`, 32 by default) rather than every sample. Gains and pitch are ramped linearly across the block and the Digital filter ramps its coefficients, so the main audible effects of the block size are on very fast envelopes and on the LFO, which can't go faster than half the control rate. Change the constant to try other sizes.

### Previous LV2 version
Older versions of Three Osc were LV2 plugins. nih-plug has no LV2 export, so the LV2 build is gone; if you need it, check out a commit from before the nih-plug port.

## Tips and Tricks
* The absolute sine / exponential wave is like a saw wave where the harmonics decrease volume at -12dB per octave instead of -6 dB per octave (i.e. it's a saw wave tracked by a soft filter). Similarly, the triangle wave is like a square wave where the harmonics diminish at -12dB per octave instead of -6dB.
* Increasing envelope slope makes it steeper, decreasing it does the opposite. Slope = 0 gives perfectly linear slopes, which are not perceptually linear. Slope = 1 gives perceptually-linear (logarithmic) volume decay.
* The Ladder and RC filter models are both capable of self-resonance at resonance >= 9.0. Underdriving the filters (i.e. drive below 1) and sweeping them very slowly gives a 'harmonic snap' effect.
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
* Add presets that make the synth look good (current idea: reimplement/extend patches from MDA jx10, which are unreasonably nice)
* Make the build system nicer
* Tools for working with the harmonic series
* Use audio buffering for more optimisations
* Optimise
* Extract DSP to internal crate
* Sinc interpolation for wavetables instead of Lerp
* SIMD accelerate stereo paths
* Approximate/cache expensive computations (i.e. any uses of `powf`)
* Fix buffer size to reduce expensive modulation calculations at the expense of sample-accurate automation (make const so that you can play around with buffer lengths, see what is good)
* Come up with better name for "Initial Pitch Slide" control
* Improve the UI
    * Knobs instead of sliders, envelope / waveform displays
    * 2nd page for oversampling and other unimportant things
* Load Scala .scl and .kbm files
* Publish

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.