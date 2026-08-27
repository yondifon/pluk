use std::process::Command;

fn main() {
    // Stamp version from VERSION file and git commit into the binary.
    // Both are exposed as env vars at compile time (PLUK_VERSION, PLUK_COMMIT)
    // and via tauri.conf.json > version (sourced from Cargo.toml workspace version).
    // The Makefile keeps VERSION and Cargo.toml in sync before bundling.

    let version = std::fs::read_to_string("../../VERSION")
        .or_else(|_| std::fs::read_to_string("VERSION"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());

    // Git commit at bundle time; fallback is "unknown" so dev builds without git still compile.
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    // Short commit for display
    let commit_short = if commit.len() > 7 { commit[..7].to_string() } else { commit.clone() };

    println!("cargo:rustc-env=PLUK_VERSION={version}");
    println!("cargo:rustc-env=PLUK_COMMIT={commit}");
    println!("cargo:rustc-env=PLUK_COMMIT_SHORT={commit_short}");
    println!("cargo:rerun-if-changed=../../VERSION");
    println!("cargo:rerun-if-changed=VERSION");
    // Also trigger rebuild when git HEAD moves
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads/main");

    tauri_build::build()
}
