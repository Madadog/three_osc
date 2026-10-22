//! Runs Three Osc as a standalone application (outside of a plugin host) using JACK or the
//! system's audio and MIDI APIs. Only built with the `standalone` feature:
//!
//! ```shell
//! cargo run --release --features standalone
//! ```
//!
//! Run with `--help` to see the audio backend options.

use nih_plug::prelude::*;
use three_osc::ThreeOscPlugin;

fn main() {
    nih_export_standalone::<ThreeOscPlugin>();
}
