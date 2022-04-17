# Three Osc

An LV2 clone of LMMS's default synth (Triple Oscillator).

Currently WIP, but usable. 

## Features

* 3 oscillators which can modulate eachother with PM, FM and AM (todo)
* IIR filter with keytracking and envelope
* Unlimited polyphony
* ADSR envelopes with adjustable slopes
* Sine, triangle, exponential, saw, and pulse (50%, 25%, 12.5%) waves
* Alias-free wave generation (todo)
* Detunable super with (up to) 128 voices for each oscillator
* Integer frequency division/multiplication for each oscillator for harmonic sound effects

## Sound Demo
Forthcoming.

## Building
0. `cd` into the three_osc directory.
1. Run `cargo build --release`. This compiles the synth and updates the .ttl metadata in `three_osc.lv2`

### Automatic
2. Run `copy_lv2.sh`. This copies the compiled shared library into `./three_osc.lv2`, then copies `./three_osc.lv2` into your home `/.lv2/` directory. *Only works on linux because the script looks for `libthree_osc.so`, but should work on windows if you edit the script and change the copied file name to `libthree_osc.dll`.*
3. Load it into your preferred LV2 host (Ardour, Carla, LMMS) and have fun.

### Manual
2. Copy the built binary (`./target/release/libthree_osc.so` or `./target/release/libthree_osc.dll` depending on your OS) into `./three_osc.lv2`
3. Copy `./three_osc.lv2` into any of the default LV2 directories (e.g. `YOUR_HOME_DIRECTORY/.lv2`).
4. Load it into your preferred LV2 host (Ardour, Carla, LMMS) and have fun.


## Why did you make this?
* I'm currently migrating from LMMS to Ardour. Surge and ZynAddSubFx are great synths, but...
    * Controls are spread out over multiple tabs / screens.
    * Surge is a little bit aggressive.
    * Sometimes you want to quickly make simple and predictable sounds.
* The original Triple Oscillator also has several issues:
    * A bit loud.
    * Volume envelope is off by default. *Click click click*.
    * Emulating super is a bit verbose, and without phase randomness it doesn't work very well.
    * No bandlimited wave generation.

## TODO
* Better bandlimited waves
* Legato and portamento
* Bandpass and highpass filter modes
* Ladder filter
* Add Odin2's SEM12 filter, add LMMS RC 24dB lowpass filter
* Independent attack slope for ADSR envelopes
* Smooth filter parameters
* Add third oscillator, work out oscillator modulation interface
* Stereo
* Reduce super voices / optimize (switch to zynaddsubfx unison/chorus effect?)
* Envelope declicking
* Fix strange PM/FM bug with naive wave generators
* Make the build system nicer
* Delete LV2, switch to VST and add gui with egui

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.