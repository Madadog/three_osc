# Three Osc

An LV2 synthesizer based on [Triple Oscillator](https://github.com/LMMS/lmms), a polyphonic subtractive synthesizer with three oscillators that can modulate each other in various ways.

Extends the original with several useful QOL features, including bandlimited synthesis by default, unison with many detuned voices, and legato.

Currently a work in progress, but usable nevertheless. 

![alt text](/images/three_osc_v1.png "Ardour hosting a bass preset which uses the ladder filter.")


## Features

* 3 oscillators which can modulate eachother via phase, frequency, and amplitude modulation (PM, FM & AM) simultaneously
* 3 multimode filters (RC, Ladder, IIR Biquad) with keytracking and envelope
* Unlimited polyphony, with optional monophonic and legato modes
* ADSR envelopes with smoothly adjustable slopes
* Sine, triangle, exponential, saw, and square waves
* Bandlimited wave synthesis using a combination of wavetables and additive synthesis
* Detunable unison with (up to) 128 voices for each oscillator
* Integer frequency division/multiplication for each oscillator for harmonic sound effects
* No GUI

## Sound Demo
Coming soon...

## Building
0. `cd` into the three_osc directory.
1. Run `cargo build --release`. This compiles the synth and updates the .ttl metadata in `three_osc.lv2`

### Manual
2. Copy the built binary (`./target/release/libthree_osc.so` or `libthree_osc.dll` depending on your OS) into `./three_osc.lv2`
3. Copy `./three_osc.lv2` into any of the default LV2 directories (e.g. `YOUR_HOME_DIRECTORY/.lv2/`).
4. Load it into your preferred LV2 host (Ardour, Carla, LMMS) and have fun.

### Automatic
2. Run `copy_lv2.sh`. This automatically does the manual instructions, copying `./three_osc.lv2` into your home `YOUR_HOME_DIRECTORY/.lv2/` directory. *Only works on linux because the script looks for `libthree_osc.so`, but should work on windows if you edit the script and change the copied file name to `libthree_osc.dll`.*
3. Load it into your preferred LV2 host (Ardour, Carla, LMMS) and have fun.

## Tips and Tricks
* Increasing envelope slope makes it steeper, decreasing it does the opposite. Slope = 1 gives perceptually-linear (logarithmic) volume decay.
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
* Choose oscillator wavetable using delta instead of midi input pitch.
* Portamento
* Process samples in buffer if possible
* Rescale resonance control max from 10.0 to 1.0?
* Add vibrato control / LFO
* PWM
* Stereo
* Figure out how many controls can fit on the UI before Ardour adds a scroll bar
* Reduce super voices / optimize (switch to zynaddsubfx unison/chorus effect?)
* User-friendly envelope declicking
* Generate wavetables at constant length with FFT at compile time, not at load time.
* Only generate unique wavetables when necessary (i.e. every third note, and only when harmonic count changes)
* Adjust more knobs to sensible values / defaults
* Add presets that make the synth look good (current idea: reimplement/extend patches from MDA jx10, which are unreasonably nice)
* Switch between Naive, Wavetable, and Additive synthesis with a control
* Make the build system nicer
* Tools for working with the harmonic series
* Optimise
* Delete LV2, switch to VST and add gui with egui

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.