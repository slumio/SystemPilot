fn main() {
    // `cargo:rustc-env` only makes a value available to the compiled program; it
    // does not configure rustc. Keep CPU-specific optimisation opt-in through
    // a caller's RUSTFLAGS, so normal Cargo and
    // rust-analyzer checks remain portable.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/causal_template.html");
    println!("cargo:rerun-if-changed=.git/HEAD");
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SYSPILOT_BUILD_COMMIT={commit}");
}
