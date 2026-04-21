// Bake git SHA into the nit binary at build time, so `nit --version` can show
// which commit it was built from. Needed by the `rebuild-nit` trigger in
// dotfiles/triggers.toml — it compares the installed nit's SHA against the
// pin in `.nit-version` to decide whether to rebuild.
//
// Falls back to "unknown" if git isn't available or we're building outside
// the repo (e.g., from a packaged crate on crates.io). Trigger script treats
// "unknown" as "rebuild, we can't verify".

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=NIT_GIT_SHA={}", sha);

    // Re-run when HEAD moves or refs change — cargo's default rebuild triggers
    // don't catch git state changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}
