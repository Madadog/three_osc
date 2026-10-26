# Three Osc

A CLAP / VST3 synthesizer based on [Triple Oscillator](https://github.com/LMMS/lmms), a polyphonic subtractive synthesizer with three oscillators that can modulate each other in various ways. Built with [nih-plug](https://github.com/robbert-vdh/nih-plug).

Extends the original with several useful QOL features, including bandlimited synthesis by default, unison with many detuned voices, and legato.

Currently a work in progress, but usable nevertheless. 

![alt text](/images/three_osc_v1.png "Ardour hosting a bass preset which uses the ladder filter (this screenshot is of the older LV2 version's generic host UI).")


## Features

* 3 oscillators which can modulate eachother via phase, frequency, and amplitude modulation (PM, FM & AM) simultaneously
* Choose between 3 multimode filter models (RC, Ladder, Simper SVF) with keytracking and envelope. The Digital (SVF) and Ladder filters run eight voices at a time (with AVX where available).
* Unlimited polyphony, with optional monophonic and legato modes
* ADSR envelopes with a slope for each stage, smoothly adjustable from exponential to logarithmic, set by dragging the envelope displays' points and handles.
* Wavetable oscillators with sine, triangle, absolute sine, saw, square and pulse waves, a pulse width sweep wavetable and a Basic Shapes wavetable (those nine shapes in a row) that each oscillator's position control morphs through, and wavetables loaded from `.wav` files (single cycles or multi-frame tables, including the `clm` frame size chunk Serum-style tables carry)
* Bandlimited wave synthesis using wavetables computed via FFT (harmonics extend up to the Nyquist frequency, with no unexpected drop-off), read with cubic, linear or no interpolation (a global control, for trading quality against CPU)
* Oversampling: the voices can run at 2, 4 or 8 times the sample rate, which takes the aliasing out of strong PM, FM and AM and the filters' distortion
* Detunable unison with (up to) 32 voices for each oscillator (i.e. yes it can supersaw), with a stereo width control (at its widest by default) that spreads the voices across the stereo field around each oscillator's pan
* Integer frequency division/multiplication for each oscillator for harmonic sound effects
* A modulation matrix: three LFOs, two spare envelopes, the volume and filter envelopes, velocity, key position, mod wheel, aftertouch, a per-note random value and the oscillators themselves can each modulate pitch, octave, frequency multiplier, wavetable position, amplitude, pan, unison detune, PM/FM/AM amounts, filter cutoff, resonance and drive, note volume, the LFO rates and the effects' controls, through 16 routes. Each route can swing one way or both ways whatever its source does, and reshape its source with a curve, steps and another source to scale it by (the mod wheel bringing in vibrato, say). Drag a source's handle onto a control in the GUI to route it. Modulation is worked out every 32 samples, or at every sample with the Modulation Rate control.
* LFOs with sine, triangle, saw, exponential, square, random (sample and hold) and smooth random waves, from 0.01 Hz up to 1 kHz or synced to the host's tempo (8 bars down to 1/64, with dotted and triplet lengths), a start phase, a fade-in, and three ways for notes to start them: from their phase (Retrigger), carrying on from one that runs all the time (Free, which follows the song position when synced) or once through and holding the end (One Shot).
* Effects after the voices: distortion (anti-aliased soft clipping), stereo chorus, delay (with ping-pong) and reverb
* Portamento and adjustable initial pitch slide for kickdrum synthesis.
* MIDI pitch bend with an adjustable range
* Presets: 21 factory presets (basses, leads, pads, plucks, keys and effects), and your own saved as JSON files
* Microtuning with Scala files (any `.scl` scale, with an optional `.kbm` keyboard mapping), and a scale generator for equal divisions of the octave or any other interval, the harmonic and subharmonic series, stacks of a generator interval and historical temperaments, with a view of the scale and `.scl` export
* A GUI (made with [egui](https://github.com/emilk/egui)) with seven themes, each a layout of its own: Studio, knobs grouped in cards that fill the window, with the modulation sources always in view to drag onto them and pages for the effects, the modulation routes and the advanced settings; Classic, the original layout with every control on one page as a slider; Flow, the sound's path drawn as a blueprint; Rack, modules with patch cables; Focus, one part at a time, explained; Compact, the whole synth in a small window; and Vintage, a polysynth's front panel with faders. All have the presets and the tuning in the title bar, displays of each oscillator's wave, the LFOs, the envelopes (which can be dragged), the filter's frequency response and the effects, which follow the notes as they play, and a spectrogram and level meter of the output. Hover a control (or a display) for a description of what it does.
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
Rough throughput numbers for the synth engine (time per sample for a few patches, at both modulation rates and at each oversampling, and wavetable generation time and memory), and how much aliasing each oversampling leaves:

```shell
cargo test --release --lib benchmark -- --ignored --nocapture --test-threads=1
```

### Themes
The GUI comes in seven themes, each its own layout of the same controls with its own look. Switch between them with the Theme menu at the right of the title bar (hover a theme in it for what it's like), in the Settings window, or on Studio's Advanced page. The choice is a preference rather than part of a project: it's kept for every project and every instance of the plugin, in `Documents/Three Osc/Settings.json` in your home directory (or the file the `THREE_OSC_SETTINGS` environment variable names).

**Studio**, the default, lays the controls out as knobs grouped in cards, and stretches to fill the window (a window narrower than 1180 pixels scrolls). The header has the presets, the tuning, the theme and four pages: Synth (the oscillators, the filter and the voice settings), Effects, Matrix (every modulation route, to add, change and remove, with how modulation works beside them) and Advanced (the quality settings, each explained under it, the wavetable folder, with buttons to play its files on an oscillator, and the theme). Whichever page is shown, the modulation sources stay in view below it: the four envelopes side by side, the three LFOs beside the output's spectrogram and meters, and the MIDI sources along the bottom. An effect's knobs dim while it's switched off, and arrows between the effects show the order they process the sound in.

**Classic** is the original layout: every control on one page as a slider, with the modulation matrix's 16 rows always in view (each with its polarity and shape beside its amount), and tabs for the effects and the advanced settings in the oscillators' place.

**Flow** draws the controls as a blueprint, in the order the sound flows through them from left to right: the oscillators stacked, with each one's phase, frequency and amplitude modulation of the next on the dashed arrow between them (Osc 3 into Osc 2, Osc 2 into Osc 1), into the mixer's levels and pans, then the filter (with its envelope's amount on the arrow up into it), the amp (the volume envelope, with the voice settings under it) and the effects in the order they come, down into the output. The other modulation sources are in a lane along the bottom.

**Rack** puts every part of the synth in a module, on aluminium panels in two rows of a rack like modular hardware: the oscillators, the filter, the amp, the filter envelope and the voice settings above the other modulation sources, the effects and the output. Every modulation source has an output jack to drag onto a knob, and every route is drawn as a patch cable from its source's jack to its knob, in the source's colour (hovering a jack picks its cables out; the Cables box in the header hides them all).

**Focus** shows one part of the synth at a time, large and explained, on a light page. A sidebar lists every part with what it's set to (its modulation sources can be dragged onto the knobs from there), and searches for any control by its name or what it does. The part picked shows what it does, its display, its choices and large knobs, what the control last hovered does (rather than a tooltip), and its modulation: the routes into it and, for a source, out of it, each with its amount to change, its polarity and shape, and a button to remove it.

**Compact** fits the whole synth in a small window, about 1000 by 600 pixels: every control at once as a small knob with a short name, in sections that flow across the window, green on black like a terminal. One display under them, as large as the window allows, shows the section last pressed on (or the one picked from the tabs over it), so the sections needn't have displays of their own.

**Vintage** is the front panel of an old polysynth, between wooden end cheeks: three tiers of sections under coloured strips, where every control is a fader and the choices are lit buttons, or lit displays (the oscillators' waves and the filter's model) stepped through with the arrows either side of them or picked from a list with a click. The faders line up across each tier, and get longer, and the panel wider, to fill the window (a window narrower than about 1230 pixels scrolls). A scope at the bottom right shows the section last pressed on, whose strip's light is lit.

In every theme but Classic:

* Every modulation source has a handle (an orange chip with dots, or Rack's jacks), which also shows how many routes the source drives. Drag it onto a knob to route the source there: the knobs that can be modulated light up while it's dragged. Hover it to light up the knobs it modulates.
* A knob turns as it's dragged up (or to the right), ten times finer with Shift held (a fader's cap follows the pointer); the mouse wheel steps it; a double-click (or Ctrl/Cmd+click) resets it; and clicking its value lets a value be typed in. Right-click a knob to reset it or type a value in, and, for one that can be modulated, to add modulation from a source (straight under the other entries while nothing modulates it) or, under a thin line, to change its routes' amounts, flip them between swinging one way and both ways, open their shapes (see [Modulation](#modulation)) and remove them. An arc from zero (or the bottom of its range) shows the knob's value, and a modulated knob has an orange arc around it for how far its routes can move it (a bar beside a fader), with a dot where the newest note's modulation has it. Rack, Compact and Vintage show a knob's value in place of its name while it's hovered or turned, as they have no room to print it under the knob.
* Hovering a modulated knob shows its routes as small sliders just above it, each with its source's name (or its amount, while it's hovered), on a transparent background: they appear at once, can be dragged (Shift for finer, a double-click for none), and go as soon as the pointer leaves the knob and them.
* The filter's display can be dragged too: across to move the cutoff, up and down for the resonance. Double-click it to reset them.
* The themes without pages of their own for them have Routes and Settings buttons in the title bar, for windows with every modulation route (to add, change and remove) and with the quality settings, the wavetable folder and the theme.

### Modulation
Every modulation route goes from a source to a target with an amount from -100% to 100%. At 100% a source's full swing moves a slider target across its whole length (an oscillator's pan from the left to the right), a pitch target by an octave (the Fine Pitch target by a semitone), the filter cutoff by 10 octaves, the resonance by 10 times, the drive by 32 times, an LFO's rate and the chorus rate by 4 octaves and the delay time by 2; a target's routes add up and the result is clamped to the control's range. The Octave and Freq. Mult targets move in whole steps (an octave is 6.25%, a multiplier step about 0.8%), so an LFO or the random value on them jumps between octaves or harmonics. The LFOs, the random value and the oscillators are bipolar (-1 to 1), the envelopes and MIDI sources go from 0 to 1.

An oscillator as a source is its wave, with its unison voices averaged, before its Amplitude, so a silent oscillator can be a modulator (it plays whenever a route uses it). The modulation is worked out before the oscillators play, so an oscillator's wave reaches the routes a control rate block (32 samples) late: a fixed delay, which shifts its phase against the oscillators it modulates and puts an oscillator modulating its own pitch through a short feedback loop. At the Block modulation rate a source is only read once per block, so oscillators (and LFOs faster than a few hundred Hz) want the Sample rate, where they modulate at audio rates: the filter's cutoff by an oscillator, say, or one oscillator's wavetable position by another. At slow pitches (an octave or a division far down), an oscillator also makes a keytracked LFO. A silent oscillator on the cutoff costs about as much as playing it, plus the filter coefficients at every sample at the Sample rate (90 and 126 ns per sample for the default patch's 8 notes, against about 60). Each note runs its own copies of the LFOs and envelopes, so the routes are evaluated per voice at the control rate (see below). The effects process all the notes together, so their controls follow the newest note's sources (and hold where they were once the notes have finished). In the GUI, drag a source's handle onto any control with a drop highlight to fill in a free row of the matrix (the effects take the oscillators' place, so the sources stay in view). In every theme but Classic a modulated knob shows how far its routes can move it as an orange arc around it (or a bar beside a fader), and where the newest note's modulation has it as a dot, and right-clicking it changes, removes or adds routes (see [Themes](#themes)). In the Classic theme a modulated slider shows an orange marker, its routes as small sliders over it while it's hovered, and while notes play, an orange line from the slider's value to where the newest note's modulation takes it.

On its way to the amount, a route can reshape its source; the picture beside a route's amount (in the right-click menus, the matrix and the routes windows) opens its shape, which it draws. The source's place in its range, from 0 to 1, is bent by the Curve (above 0 it stays low for longer, up to its 4th power, below 0 it rises early), stepped into a number of levels by Steps (2 flips between the ends; 13 on a one way route to a pitch target at 100% plays the semitones of an octave, both ends included), and swung either from 0 to 1 or from -1 to 1 by the Polarity, which by default is the source's own (the ± or + button beside the amount flips it). Then Scale By multiplies it by another source, from nothing at that source's bottom (a bipolar source's lowest) to all of it at its top: the mod wheel bringing in an LFO's vibrato, say, or velocity setting how far an envelope opens the filter.

The LFOs run at a rate from 0.01 Hz to 1 kHz, or, with Sync on, a cycle takes a note length at the host's tempo (120 beats a minute if the host doesn't say), from 8 bars to a 64th note with dotted and triplet lengths; modulating a synced LFO's rate still scales it, by up to 4 octaves at 100%. Each note runs its own copy of every LFO, which by default starts at the LFO's Phase (Retrigger, so every note's modulation starts the same); with the Free trigger the notes carry on from a copy that runs all the time, so they move together (and when synced, it follows the host's song position while it plays, so its cycles line up with the bars); and One Shot runs through a single cycle from its phase and holds the end, as an extra envelope. Fade In brings the LFO in from nothing over the start of each note. Random draws a new value each cycle and holds it, and Smooth Random glides from each value to the next; the values come from a hash of the cycle's number, so they don't depend on the modulation rate, each note draws its own (the Free trigger's notes share theirs), and the display draws the newest note's.

### Effects
Four effects process the synth's output (after the Output Gain) in this order, each switched on separately (in the Studio theme they're on the Effects page, and in the Classic theme on the FX tab, in the oscillators' place):

* Distortion: `tanh` soft clipping with a drive of up to 36 dB, anti-aliased with the first order antiderivative method (the output is the average of `tanh` between consecutive inputs, worked out from its antiderivative `ln(cosh(x))`), which takes most of the aliasing out of high notes for half a sample of delay and no oversampling.
* Chorus: a copy of the sound delayed by 6 to 18 ms, swept by an LFO a quarter of a cycle apart in the two channels.
* Delay: up to 2 seconds, with feedback and an optional ping-pong between the channels. Changing the time glides the echoes' pitch, like tape.
* Reverb: Jezar's Freeverb, eight damped comb filters into four allpasses per channel.

Every setting is smoothed sample by sample, so automation and modulation (applied once per control rate block) don't step, and an effect that is switched on starts from silence. All four together take about 130 ns per sample on the machine below, a little over twice what 8 notes of the default patch take.

### Presets
The title bar's preset menu lists the factory presets by category, then your own; the arrows step through them, and a `*` after the name means the controls have been moved since the preset was loaded. Save stores the current settings as a preset of your own in `Documents/Three Osc/Presets` in your home directory (or the folder the `THREE_OSC_PRESETS` environment variable names), named after the file. The tuning and the oversampling aren't part of a preset: they stay as they are while you browse.

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
The title bar's Tuning button opens a window for retuning the keys with [Scala](https://www.huygens-fokker.org/scala/) scales and keyboard mappings. It has three tabs.

**Files** lists the `.scl` and `.kbm` files in `Documents/Three Osc/Tunings` (or the folder `THREE_OSC_TUNINGS` names), and loads them from any other path typed in:

* A scale (`.scl`) lists the pitches of its steps above its root, in cents or as ratios, the last being the period it repeats at (usually the octave, `2/1`). On its own, its root is middle C at its usual pitch (261.63 Hz), and each key goes up the scale a step.
* A keyboard mapping (`.kbm`) says which key plays which step (keys can be left silent, to put a 7 note scale on the white keys, say), which key is tuned to which frequency, and which keys are retuned at all (the others stay silent). With the standard scale, it can just move A away from 440 Hz.

**Generate** makes a scale from a few numbers, shows its steps, and plays it (keeping the keyboard mapping):

* Equal divisions: so many equal steps of an interval, given as a ratio or in cents. Of the octave they're the EDOs (19, 31, 53...), of the tritave (3/1) Bohlen-Pierce, of the fifth (3/2) Wendy Carlos's alpha, and one division of so many cents is a scale of equal steps that never comes back to the octave.
* Harmonic series: the harmonics from one to another, such as 8 to 16, an octave of steps that shrink as they go up.
* Subharmonic series: the same upside down, from a subharmonic down to another.
* Generator stack: a chain of one interval, each note moved by whole periods into one period of the root, with some of the chain going down from the root. Pure fifths (3/2) make Pythagorean tuning (12 notes, 3 down) or the major scale (7 notes, 1 down), and fifths of 696.578 cents quarter-comma meantone. With ratios, the notes come out as exact ratios.
* Historical temperament: Pythagorean, quarter-comma meantone, Werckmeister III, Kirnberger III, Vallotti and Young, worked out from the sizes of their fifths, and 5-limit just intonation.

**Scale** shows the scale being played: a ruler of its steps over the twelve-tone semitones, and a table of each degree's pitch in cents, the step up to it, its ratio (when the scale gives one) and the nearest twelve-tone note with how far off it is. It saves the scale as a `.scl` file in the tunings folder, for other synths (or later), or copies the file's text.

The files' contents (a generated scale's too) are saved with the plugin's state, so a project sounds the same on a computer without them. Only the notes' pitches change: pitch bend, the oscillators' Detune and Octave controls and the pitch modulation still move in ordinary semitones, the filter's keytracking follows the tuned pitch, and the Key Track modulation source still follows the key's place on the keyboard.

### Wavetable files
Set an oscillator's Wave to File to play a wavetable from a `.wav` file. The File row that appears lists the `.wav` files in the wavetable folder, `Documents/Three Osc/Wavetables` in your home directory (or the folder the `THREE_OSC_WAVETABLES` environment variable names); the Wavetable Files box on the Advanced page (the Advanced tab in the Classic theme, and the Settings window in the other themes) shows the folder, rescans it, and loads a file from any other path into an oscillator (in every theme but Classic it also lists the folder's files, with buttons to play each on an oscillator). The chosen path is saved with the plugin's state.

A file holds one or more frames, single cycles one after the other. The frame length comes from the file's `clm ` chunk when it has one (wavetable editors such as Serum write it), otherwise frames are taken to be 2048 samples long when the file divides evenly into them, and any other file up to 65536 samples is one cycle. PCM (8 to 32-bit) and float files are read, stereo files are mixed to mono, and files with more than 256 frames are thinned out to 256. Position morphs through the frames like it does for PW Sweep.

The file is read and analysed on a background thread: each frame's spectrum is kept, and the bandlimited tables for every note are generated from it for the current sample rate (and regenerated when it changes), so a loaded wavetable is as alias-free as the built in ones. Until the file is loaded, or when none is set, the oscillator plays a sine.

Each note's table is only as long as its harmonics need (a power of two of at least 16 samples per harmonic, from 64 up to 2048 samples), and a wavetable with many frames shares one table between neighbouring notes when it would otherwise take more than 24 MB. The built in wavetables take 26 MB.

### Displays
The displays are drawn with the synth's own code, so they show what is played. An oscillator's display is one cycle of its wavetable at the current position (rendered from the same spectra as the tables it plays, loaded files included), starting where its notes start, so the wave moves along with the Phase, with the stretch Phase Rand lets notes start in shaded; it is dimmed while the oscillator's amplitude is 0. The LFO displays show one cycle from the phase the notes start at (a One Shot's single run, and four cycles of a random wave's values, the newest note's own while notes play). The envelope displays draw attack, decay down to the sustain point (where held notes wait) and release, each stage as wide as the square root of its time, so that a 2 ms attack is still visible next to a 2 second release.

The envelope displays are also how the envelopes' slopes are set, and a quicker way to set their times. Drag the top of the attack, the sustain point (across for the decay time, up and down for the sustain level) or the end of the release; while a point is dragged the time scale stays put, so it follows the pointer, and the display fits itself to the width again when it's let go. The small handle in the middle of each stage bends its curve: up for a punchier attack, down for a punchier decay or release (each stage's slope goes from -8 to 8, 0 being a straight line, and a step of 1 doubles the curve's exponent). When a decay or release falls too little to bend (a sustain near the top or the bottom), its handle works over the whole height, and shows that curve faintly while it's hovered. Double-click a point or handle to reset it. Presets and projects saved with the single Slope control envelopes had before load with the three slopes that give the same curves. The filter display is the frequency response from 20 Hz to 20 kHz, measured by running an impulse through the filter model itself at the rate the voices run at, the host's sample rate times the oversampling (a quiet impulse, so the nonlinear models show their small-signal response; the drive's distortion doesn't show), with the response at the peak of the filter envelope drawn more faintly. In the Studio theme it can be dragged to set the cutoff and the resonance (see [Themes](#themes)). The effects show the distortion's curve, the chorus's sweep, the delay's echoes and the reverb's decay (its lows and, faster under the damping, its highs).

While notes play, the displays follow them: a dot for each of the newest 16 notes moves through the envelopes (a released note carries on from wherever it was, along its own release), a dot on each LFO shows the newest note's phase, the filter display draws the newest note's response over the controls', moving with its cutoff (envelope, keytracking and modulation) and resonance (it's measured again when the cutoff moves a quarter of a semitone or more, which takes 0.1 ms for the Digital filter and up to half a millisecond for the others), and the waves and the effects' displays show the newest note's modulated position and settings. After each buffer the audio thread writes what the GUI shows into a handful of atomics (only while the GUI is open), and the GUI reads them as it draws.

The Output section (beside the LFOs in the Studio theme, at the top right in the Classic one, and with the output's controls, or on the one display, in the others) shows what the synth plays, after the effects. The spectrogram covers the last few seconds, the newest at the right, with frequency going up the side from 20 Hz to 20 kHz on a log scale and brightness showing level from -90 to 0 dB (hover it to read a frequency off it): each column is the FFT of about 90 ms of the output under a Hann window, a quarter of a window after the last. Under it, each channel's meter has a bright bar for the RMS level (over 300 ms), a dim one for the peaks, which fall back at 20 dB a second, and a line for the highest peak of the last second and a half, with that peak's level beside them, in red once the output has clipped (click the meter to clear it). The audio thread copies its output into a ring of samples (only while the GUI is open), and the GUI does the rest.

### Control rate
Envelopes, pitch (including portamento), the LFOs, the modulation matrix, keytracking and the filter coefficients are recalculated once per control rate block of `CONTROL_BLOCK_SAMPLES` samples (in `src/synth.rs`, 32 by default) rather than every sample. Gains and pitch are ramped linearly across the block and the Digital filter ramps its coefficients, so the main audible effects of the block size are on very fast envelopes and on the LFOs, which can't go faster than half the control rate (689 Hz at 44.1 kHz). Change the constant to try other sizes.

The Modulation Rate control (on the Advanced tab) switches between that (Block, the default) and working everything out for every sample (Sample), where a 2 ms attack follows its curve rather than a few straight lines and an LFO can run at audio rates. The control update then works a whole block's values out at once, eight samples at a time (in `src/synth/per_sample.rs`): the envelopes, LFOs and matrix, then every sample's pitch, gains, wavetable position, modulation depths and filter coefficients, with 8-lane versions of the fast math below that give exactly the same results as one value at a time. The oscillator kernels, the filters (the Digital and Ladder filters still in lockstep across voices) and the mixing take each sample's values instead of ramps, and still render the block in one go. Whatever doesn't change within a block (an envelope in its sustain, a route from velocity or the mod wheel, an oscillator nothing modulates, filter coefficients) is only worked out once and rendered as at the block rate without a ramp, which gives exactly what working out every sample's values would. Three things still change once per block: the bandlimited table an oscillator plays (picked for the highest pitch in the block), the unison detune and the pan. The LFOs keep their phase in double precision, since at the sample rate a slow LFO's steps are too small for single precision to add up: a 0.01 Hz LFO would run 3% fast.

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
The Interpolation control on the Advanced tab picks how the wavetables are read in between their samples: Cubic (4-point Hermite, the default and cleanest), Linear, or None (the sample below the read position, which steps audibly on all but the lowest notes). Each mode has its own specialised lookup kernels, so there is no per-sample cost to the choice. On an 8 note, 3 x 8 voice supersaw, Linear is about 20% cheaper than Cubic and None about 30% cheaper.

### Oversampling
The Oversampling menu on the Advanced tab runs the voices at 2, 4 or 8 times the sample rate (it is Off by default). The wavetables are bandlimited to the host's Nyquist frequency either way, so a plain note sounds the same at every setting (its level stays within 0.05 dB); what oversampling takes out is the aliasing of what the voices make above the Nyquist frequency: the sidebands of phase, frequency and amplitude modulation, and the filters' distortion. The voices' output comes back down to the sample rate through a cascade of halfband decimators, one per halving: polyphase IIR filters made of two chains of first order allpasses, designed as in Laurent de Soras's HIIR library, with both chains of both channels running in one 4-lane vector. They keep up to 20 kHz flat (at 44.1 kHz, and in proportion at other rates), keep anything that would fold back into that band at least 100 dB down, and delay the output by 1.5 (2x) to 2.4 samples (8x). The effects run at the sample rate, after the decimators. A voice still works its controls out once per 32 of its own samples, so oversampled, envelopes and modulation are worked out two, four or eight times as often. The setting is a quality and CPU choice, like the tuning, so presets leave it alone. It can change while notes play, and they carry on where they were: the new decimators start from the voices' last output rather than from silence, so the sound doesn't drop out.

How much aliasing is left, from `benchmark_aliasing`: the share of a held note's output that isn't at a harmonic of the note (or at 0 Hz), from an FFT of three quarters of a second of it under a Blackman-Harris window, at 44.1 kHz. Oscillator 2 is the modulator in the modulation cases, a sine (and silent itself) unless said otherwise.

| Patch | Off | 2x | 4x | 8x |
|---|---|---|---|---|
| Saw, no filter, C7 | -89 dB | -89 dB | -89 dB | -91 dB |
| Sine, phase modulated (50%) by a sine, C6 | -2 dB | -90 dB | -90 dB | -90 dB |
| Sine, phase modulated (50%) by a sine, C7 | -1 dB | -2 dB | -89 dB | -89 dB |
| Sine, frequency modulated (30%) by a sine, C7 | -8 dB | -91 dB | -91 dB | -91 dB |
| Sine, ring modulated (100%) by a saw, C6 | -34 dB | -89 dB | -89 dB | -89 dB |
| Saw through the Ladder filter at 5 kHz, drive 4, C6 | -67 dB | -72 dB | -72 dB | -87 dB |
| Saw through the Ladder filter at 5 kHz, drive 10, C6 | -49 dB | -50 dB | -50 dB | -73 dB |

The bandlimited saw has nothing to lose, and the strongly phase modulated C7, with sidebands up to about 80 kHz, needs 4x. The Ladder filter runs oversampled on its own, at three times the sample rate (see Tips and Tricks), and only as much on top of the voices' oversampling as it needs to stay at least that fast: 2x on top of 2x, and nothing on top of 4x or 8x. So at 2x and 4x it runs at four times the sample rate, which is clean up to a drive of about 2 (-88 dB at all three settings), while a hard driven Ladder needs 8x. (Running it 3x on top of every setting would take the drive 10 case to -60, -84 and -88 dB, for half as much again of its CPU at 2x and three times as much at 4x and 8x.)

The cost, from `benchmark_oversampling` (8 notes at 44.1 kHz on a 2.1 GHz Xeon, time per sample of output, with how many times the cost without oversampling):

| Patch | Off | 2x | 4x | 8x |
|---|---|---|---|---|
| Default (1 saw, Digital filter) | 53 ns | 110 ns (2.1x) | 222 ns (4.2x) | 447 ns (8.5x) |
| Default, no filter | 41 ns | 87 ns (2.1x) | 174 ns (4.3x) | 355 ns (8.7x) |
| Default, Ladder filter | 159 ns | 253 ns (1.6x) | 337 ns (2.1x) | 643 ns (4.0x) |
| Phase modulation (2 sines) | 98 ns | 203 ns (2.1x) | 427 ns (4.4x) | 857 ns (8.8x) |
| 3 x saw, unison 8 | 287 ns | 565 ns (2.0x) | 1162 ns (4.0x) | 2251 ns (7.8x) |
| 3 x saw, unison 8, stereo | 289 ns | 595 ns (2.1x) | 1299 ns (4.5x) | 2594 ns (9.0x) |
| 3 x saw, unison 8, Ladder filter | 391 ns | 674 ns (1.7x) | 1371 ns (3.5x) | 2765 ns (7.1x) |

Nearly all of the cost is the voices rendering two, four or eight times as many samples; the decimators themselves take 5.3, 14 and 30 ns per sample of stereo output at 2x, 4x and 8x. The Ladder costs less than that, since it drops its own oversampling as the voices' goes up (with it 3x on top of every setting, the default patch through the Ladder took 319, 655 and 1297 ns). So 2x, at twice the CPU, takes care of most modulation, and 8x is for the extreme cases.

### Oscillator kernels and SIMD
Each oscillator renders a whole control rate block in one kernel call: the unison voices' phases advance as vectors, every sample reads and interpolates the wavetable at each voice and sums the voices into the block (with AVX2, eight samples' sums across the voices are worked out together), and a modulated oscillator reads its modulator's block sample for sample (the oscillators are rendered from the last to the first, so the modulation is still instantaneous). With fewer than 8 unison voices the vectors run across time instead: each holds 8 consecutive samples of one voice, whose phases have a closed form (or are added up one by one under frequency modulation), so nothing has to be summed across a vector's lanes at every sample, and the gain ramp and the additions to the output go a vector at a time too. That makes a single voice more than three times cheaper than with the vectors across voices. Phases are 32-bit fixed point: one cycle is 2^32, so they wrap for free and pitch resolves to 2^-32 cycles per sample, and a phase splits into a table index and a fraction with a shift and a mask (wavetable cycles are powers of two long).

The reads run with AVX2 and FMA where the CPU has them, falling back to a portable path otherwise. There are two AVX2 variants, one reading each voice's taps with a 128-bit load and transposing them, the other using gather instructions; which is faster depends on the CPU (gathers are slow on the Intel generations with the "Downfall" mitigation, Skylake to Tiger Lake), so the two are timed against each other when the synth starts. The filters run several voices in lockstep (a filter's recurrence is a chain of dependent operations, so one voice on its own leaves most of the CPU idle): the Digital filter eight at a time with AVX (or in portable SIMD vectors without it), producing exactly the same output as the one-voice-at-a-time version, and the ladder eight at a time with AVX2 and FMA, whose fused multiply-adds round differently from the one-voice version (staying within 0.01% of the signal level). The Digital filter works out its next state directly rather than through the paper's intermediate values, which takes the chain from one sample to the next from five dependent operations down to three. Everything is detected at runtime, so one build runs on any x86-64 CPU.

The lookups are not memory bound: shrinking the tables 8 times or tripling the number of notes changes the cost per voice by a few percent, so a 16-bit table format would buy little.

### Previous LV2 version
Older versions of Three Osc were LV2 plugins. nih-plug has no LV2 export, so the LV2 build is gone; if you need it, check out a commit from before the nih-plug port.

## Tips and Tricks
* The absolute sine / exponential wave is like a saw wave where the harmonics decrease volume at -12dB per octave instead of -6 dB per octave (i.e. it's a saw wave tracked by a soft filter). Similarly, the triangle wave is like a square wave where the harmonics diminish at -12dB per octave instead of -6dB.
* Each envelope stage has its own slope, set with the handle in its middle: a higher slope makes the stage steeper at its start, a lower one the opposite. A slope of 0 gives a perfectly linear stage, which is not perceptually linear; a decay or release slope of 1 (the default) gives perceptually-linear (logarithmic) volume decay.
* The Ladder and RC filter models are both capable of self-resonance at resonance >= 9.0. Underdriving the filters (i.e. drive below 1) and sweeping them very slowly gives a 'harmonic snap' effect.
* The Ladder filter runs 3x oversampled, with its input properly upsampled and its output decimated so that its distortion doesn't alias. Its cutoff can't go higher than about 12.6 kHz at a 44.1 kHz sample rate (13.7 kHz at 48 kHz), so even fully open it softens the top octave (15 kHz comes out about 11 dB below 1 kHz at 44.1 kHz). At 88.2 kHz and above it opens up fully. With the voices oversampled it only runs as much faster again as it needs to stay at three times the sample rate or more: 2x on top of the voices' 2x, and not at all on top of 4x or 8x. Its cutoff then reaches 16.8 kHz at 2x and 4x, and opens up fully at 8x. It runs in single precision, like the original module's 32-bit build: its output is within about -110 dB of a double precision version.
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
* A file browser for wavetable files outside the wavetable folder (paths can be typed in for now)
* Publish

## LICENSE
The Three Osc project is licensed under the GNU General Public Licence version 3.