# Three Osc

An LV2 clone of LMMS's default synth (Triple Oscillator).

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

## Why?
* I'm currently migrating from LMMS to Ardour. Surge and ZynAddSubFx are great synths, but...
    * Controls are spread out over multiple tabs / screens.
    * Surge is a little bit aggressive.
    * Sometimes you want to make simple, predictable sounds quickly.
* The original Triple Oscillator also had several issues:
    * A bit loud.
    * Volume envelope was off by default. *Click click click*.
    * Emulating super was a bit verbose, and without phase randomness it didn't work very well.

## TODO
* Bandlimited waves
* Legato and portamento
* Bandpass and highpass filter modes
* Ladder filter
* Independent attack slope for ADSR envelopes
* Smooth filter parameters
* Add third oscillator, work out oscillator modulation interface
* Stereo
* Reduce super voices / optimize (switch to zynaddsubfx unison/chorus effect?)
* Readjust super detune control to reasonable values
* Envelope declicking
* Fix strange PM/FM bug with naive wave generators
* Make the build system nicer
* Delete LV2, switch to VST and add gui with egui

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.