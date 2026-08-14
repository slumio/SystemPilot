fn main() {
    // `cargo:rustc-env` only makes a value available to the compiled program; it
    // does not configure rustc. Keep CPU-specific optimisation opt-in through
    // a caller's RUSTFLAGS, so normal Cargo and
    // rust-analyzer checks remain portable.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/causal_template.html");
}
