//! Writes the invented test show as a `.3du` file, for trying the CLI without real drills:
//!
//!     cargo run -p three-du-core --example make_synthetic -- synthetic.3du

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "synthetic.3du".into());
    let bytes = three_du_core::synth::encode(&three_du_core::synth::sample_show());
    std::fs::write(&path, bytes).expect("write synthetic file");
    eprintln!("wrote {path}");
}
