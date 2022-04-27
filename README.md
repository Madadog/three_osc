# Three Osc

An LV2 clone of [Triple Oscillator](https://github.com/LMMS/lmms), a polyphonic subtractive synthesizer with three oscillators that can modulate each other in various ways.

Currently WIP, but usable. 

## Features

* 3 oscillators which can modulate eachother with PM, FM and AM (todo: 3rd oscillator and AM)
* 3 multimode filters (RC, Ladder, IIR Biquad) with keytracking and envelope
* Unlimited polyphony
* Legato
* ADSR envelopes with adjustable slopes
* Sine, triangle, exponential, saw, and square waves
* Bandlimited wave generation using a combination of wavetables and additive synthesis.
* Detunable super with (up to) 128 voices for each oscillator
* Integer frequency division/multiplication for each oscillator for harmonic sound effects

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


## Why did you make this?
* I'm currently migrating from LMMS to Ardour. Surge and ZynAddSubFx are great synths, but...
    * Controls are spread out over multiple tabs / screens.
    * Surge is a little bit aggressive.
    * Sometimes you want to quickly make simple and predictable sounds.
* The original Triple Oscillator also has several issues:
    * A bit loud.
    * Volume envelope is optional.
    * No phase randomness.
    * No bandlimited wave generation.

## TODO
* Label / dropdown menu for filter model, polyphony, and osc wave controls in LV2 UI
* Portamento
* Bandpass and highpass filter modes for biquad filter
* Independent attack slope for ADSR envelopes
* Group controls in LV2 UI
* Add third oscillator, work out oscillator modulation interface
* PWM
* Stereo
* Reduce super voices / optimize (switch to zynaddsubfx unison/chorus effect?)
* User-friendly envelope declicking
* Fix strange PM/FM bug at wave loop point
* Generate wavetables with FFT, at compile time, rather than at load time.
* Only generate unique wavetables when necessary (i.e. every third note, and only when harmonic count changes)
* Switch between Naive, Wavetable, and Additive synthesis with a control
* Make the build system nicer
* Delete LV2, switch to VST and add gui with egui

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.