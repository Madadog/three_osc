//! Rough throughput benchmarks for the synth engine.
//!
//! These are ignored by default since they take a while, and their numbers are only meaningful
//! in release mode:
//!
//! ```shell
//! cargo test --release --lib benchmark -- --ignored --nocapture --test-threads=1
//! ```

use std::time::Instant;

use crate::params::OscillatorWave;
use crate::synth::filter::{Filter, FilterModel, LadderFilter};
use crate::synth::oscillator::{OscillatorParams, SimdLevel};
use crate::synth::ThreeOsc;
use crate::ThreeOscPlugin;

const SAMPLE_RATE: f64 = 44100.0;
const BLOCK_SIZE: usize = 512;
const SECONDS: f64 = 4.0;
/// The notes held down for every scenario.
const CHORD: [u8; 8] = [36, 43, 48, 52, 55, 60, 64, 67];

/// Renders `SECONDS` of audio with the given patch and prints the time per sample.
fn measure(name: &str, setup: impl Fn(&mut ThreeOscPlugin)) {
    let mut plugin = ThreeOscPlugin::default();
    plugin.apply_params();
    setup(&mut plugin);
    for note in CHORD {
        plugin.synth.note_on(note, 100);
    }

    // The number of unison voices rendered per sample, across all notes and oscillators.
    let unison_voices: usize = plugin
        .synth
        .oscillators
        .iter()
        .map(|osc| osc.voice_count as usize)
        .sum::<usize>()
        * CHORD.len();

    let mut left = vec![0.0; BLOCK_SIZE];
    let mut right = vec![0.0; BLOCK_SIZE];
    for _ in 0..16 {
        plugin.render(std::iter::empty(), &mut left, &mut right);
    }

    let blocks = (SECONDS * SAMPLE_RATE / BLOCK_SIZE as f64) as usize;
    let start = Instant::now();
    for _ in 0..blocks {
        plugin.render(std::iter::empty(), &mut left, &mut right);
    }
    let elapsed = start.elapsed();

    let samples = blocks * BLOCK_SIZE;
    let ns_per_sample = elapsed.as_nanos() as f64 / samples as f64;
    let realtime = (samples as f64 / SAMPLE_RATE) / elapsed.as_secs_f64();
    println!(
        "{name:<34} {ns_per_sample:>8.1} ns/sample {:>6.1} ns/sample/unison voice {realtime:>7.1}x realtime",
        ns_per_sample / unison_voices as f64,
    );
    assert!(left.iter().all(|sample| sample.is_finite()));
}

/// Three audible saw oscillators with 8 unison voices each.
fn supersaw(osc: &mut OscillatorParams) {
    osc.amp = 1.0;
    osc.voice_count = 8;
    osc.voices_detune = 0.01;
    select_saw(osc);
}

fn select_saw(osc: &mut OscillatorParams) {
    osc.wavetable = OscillatorWave::Saw.wavetable_index();
}

/// A 40% pulse wave.
fn select_pulse(osc: &mut OscillatorParams) {
    osc.wavetable = OscillatorWave::Pulse40.wavetable_index();
}

/// The pulse width sweep wavetable, in between two frames so both are read.
fn select_sweep(osc: &mut OscillatorParams) {
    osc.wavetable = OscillatorWave::PwSweep.wavetable_index();
    osc.position = 0.37;
}

#[test]
#[ignore]
fn benchmark_synth() {
    println!("\nwavetable lookups: {:?}", SimdLevel::detect());
    measure("default patch (1 saw, 8 notes)", |_| {});
    measure("3 x saw, unison 8", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(supersaw);
    });
    measure("3 x saw, unison 8, portable lookups", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(supersaw);
        plugin.synth.simd = SimdLevel::Portable;
    });
    #[cfg(target_arch = "x86_64")]
    if SimdLevel::detect() != SimdLevel::Portable {
        measure("3 x saw, unison 8, AVX2 gather lookups", |plugin| {
            plugin.synth.oscillators.iter_mut().for_each(supersaw);
            plugin.synth.simd = SimdLevel::Avx2Gather;
        });
    }
    measure("3 x saw, unison 8, stereo width 1", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(supersaw);
        plugin.synth.stereo_width = 1.0;
    });
    measure("3 x 40% pulse, unison 8", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(|osc| {
            supersaw(osc);
            select_pulse(osc);
        });
    });
    measure("3 x PW sweep (between frames), unison 8", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(|osc| {
            supersaw(osc);
            select_sweep(osc);
        });
    });
    measure("3 x saw, unison 8, ladder filter", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(supersaw);
        plugin.synth.filter_controller.filter_model = FilterModel::LadderFilter;
    });
    measure("3 x saw, unison 8, ladder, scalar", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(supersaw);
        plugin.synth.filter_controller.filter_model = FilterModel::LadderFilter;
        plugin.synth.simd = SimdLevel::Portable;
    });
    measure("default patch, ladder filter", |plugin| {
        plugin.synth.filter_controller.filter_model = FilterModel::LadderFilter;
    });
    measure("3 x saw, unison 8, PM + FM", |plugin| {
        plugin.synth.oscillators.iter_mut().for_each(|osc| {
            supersaw(osc);
            osc.pm = 0.1;
            osc.fm = 0.01;
        });
    });
}

/// The ladder filter on its own, and without its two oversampling related costs.
#[test]
#[ignore]
fn benchmark_ladder() {
    type Setup = fn(&mut LadderFilter);
    let input: Vec<f32> = (0..4096).map(|i| (i % 100) as f32 / 50.0 - 1.0).collect();
    let variants: [(&str, Setup); 3] = [
        ("ladder filter (3x oversampled, 16th order decimator)", |_| {}),
        ("ladder filter without the decimator", |filter| filter.set_decimator_order(0)),
        ("ladder filter without oversampling", |filter| filter.oversampling_factor = 1),
    ];
    println!();
    for (name, setup) in variants {
        let mut filter = LadderFilter::default();
        setup(&mut filter);
        filter.set_params(SAMPLE_RATE as f32, 2000.0, 3.0);
        let mut sink = 0.0;
        for sample in &input {
            sink += filter.process(*sample);
        }
        let iterations = 200;
        let start = Instant::now();
        for _ in 0..iterations {
            for sample in &input {
                sink += filter.process(*sample);
            }
        }
        let ns_per_sample = start.elapsed().as_nanos() as f64 / (iterations * input.len()) as f64;
        println!("{name:<52} {ns_per_sample:>7.1} ns/sample");
        assert!(sink.is_finite());
    }
}

#[test]
#[ignore]
fn benchmark_wavetable_generation() {
    let start = Instant::now();
    let synth = ThreeOsc::new(SAMPLE_RATE);
    let elapsed = start.elapsed();
    let frames: usize = synth
        .waves
        .wavetables
        .iter()
        .map(|wavetable| wavetable.frames.len())
        .sum();
    println!(
        "\nwavetable generation: {} wavetables, {} frames, {:.1} MB, {:.0} ms",
        synth.waves.wavetables.len(),
        frames,
        synth.waves.memory_bytes() as f64 / 1e6,
        elapsed.as_secs_f64() * 1000.0
    );
}
