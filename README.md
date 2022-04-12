# Three Osc

An LV2 clone of LMMS's default synth (Triple Oscillator).

## Features
* 3 oscillators which can modulate eachother with PM, FM and AM (todo)
* IIR filter with keytracking and envelope
* Unlimited polyphony
* ADSR envelopes with adjustable slopes
* Sine, triangle, exponential, saw, and pulse (50%, 25%, 12.5%) waves
* Alias-free synthesis (todo)
* Detunable super with (up to) 128 voices for each oscillator

## Sound Demo
Forthcoming.

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
* Delete LV2, switch to VST and add gui with egui