//! nih-plug's bundler, exposed as `cargo xtask`. Run `cargo xtask bundle three-osc --release` to
//! build the CLAP and VST3 bundles into `target/bundled/`.

fn main() -> nih_plug_xtask::Result<()> {
    nih_plug_xtask::main()
}
