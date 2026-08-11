//! Puts the input manifest where a freshly built binary will actually find it.
//!
//! SteamVR is handed an absolute path to `input/ward_actions.json`, resolved from the directory
//! the executable is running in. That works for an installed copy because the installer puts the
//! folder there, and it would not work for anything Cargo builds — so this does for the manifest
//! what `openvr-sys` already does for `openvr_api.dll`, and for the same reason.
//!
//! Without it, a freshly built Ward starts, finds no manifest, and reports that the panel cannot
//! be grabbed. That is an honest message about a file the developer never knew had to be moved.

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let source = Path::new(&manifest_dir).join("input");

    println!("cargo:rerun-if-changed=input");

    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        println!("cargo:warning=no OUT_DIR, so the input manifest was not copied");
        return;
    };

    // OUT_DIR is <profile>/build/<pkg>-<hash>/out, so the profile directory is three up.
    let Some(profile_dir) = Path::new(&out_dir).ancestors().nth(3) else {
        println!("cargo:warning=could not locate the profile directory from OUT_DIR");
        return;
    };

    // The application in `<profile>` and test binaries in `<profile>/deps`, because a test that
    // brings a session up needs the manifest exactly as much as the application does.
    for dest_dir in [profile_dir.join("input"), profile_dir.join("deps/input")] {
        if let Err(error) = std::fs::create_dir_all(&dest_dir) {
            println!(
                "cargo:warning=could not create {}: {error}",
                dest_dir.display()
            );
            continue;
        }

        let Ok(entries) = std::fs::read_dir(&source) else {
            println!("cargo:warning=no input manifest at {}", source.display());
            return;
        };

        for entry in entries.flatten() {
            let dest = dest_dir.join(entry.file_name());

            // Track the copy as well as the source, so one swept out with a tidied target
            // directory is put back rather than reported as up to date.
            println!("cargo:rerun-if-changed={}", dest.display());

            if let Err(error) = std::fs::copy(entry.path(), &dest) {
                println!(
                    "cargo:warning=could not copy {} to {}: {error}",
                    entry.path().display(),
                    dest.display()
                );
            }
        }
    }
}
