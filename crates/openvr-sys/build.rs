//! Generates the OpenVR bindings, and points the linker and the loader at the vendored library.
//!
//! The interface is a function-pointer table of several hundred slots, where a wrong slot is
//! silent memory corruption rather than a compile error. Generated, never transcribed.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let vendor =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("vendor");
    let header = vendor.join("openvr_capi.h");
    let lib_dir = vendor.join("win64");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    // Emitting any rerun-if-changed replaces Cargo's rerun-on-any-change default, so everything
    // this script depends on has to be named. The header alone was not enough: a Valve
    // redistributable refresh that left the header untouched would link against the new import
    // library while loading whatever DLL was copied last, which is the quiet version drift the
    // vendoring is meant to make impossible.
    println!("cargo:rerun-if-changed={}", header.display());
    println!(
        "cargo:rerun-if-changed={}",
        lib_dir.join("openvr_api.dll").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        lib_dir.join("openvr_api.lib").display()
    );
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=openvr_api");

    let patched = patch_entry_points(&header, &out_dir);

    let bindings = bindgen::Builder::default()
        .header(patched.to_str().expect("out path is not UTF-8"))
        // On Windows S_API expands to `extern "C" __declspec(dllimport)`, which is C++ syntax
        // and does not parse as C. NODLL collapses it to nothing. The import library supplies
        // the thunks either way, so dropping dllimport costs an optimization hint and no more.
        .clang_arg("-DOPENVR_API_NODLL")
        // Plain constants rather than Rust enums. The C enums carry values OpenVR may extend,
        // and a Rust enum with an unlisted discriminant is undefined behavior rather than an
        // unknown value. Journal decoding fails soft elsewhere for the same reason.
        .default_enum_style(bindgen::EnumVariation::Consts)
        // Valve's variant names already carry the enum name, so bindgen's default doubles it
        // into ETextureType_ETextureType_TextureType_Vulkan. Off, and the constants then read
        // exactly as the header and every piece of OpenVR documentation writes them.
        .prepend_enum_name(false)
        .derive_default(true)
        .generate()
        .expect("bindgen could not parse openvr_capi.h");

    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("could not write the generated bindings");

    copy_runtime_dll(&lib_dir, &out_dir);
}

/// Flips the `#if 0` that Valve wraps the global entry points in, and returns the patched copy.
///
/// `openvr_capi.h` declares `VR_InitInternal`, `VR_GetGenericInterface` and their siblings inside
/// `#if 0`, because the flat header is written for binding generators that are expected to supply
/// the entry points themselves. Without them nothing can start OpenVR at all.
///
/// The alternative was declaring the seven by hand, which would put a
/// transcription step back into exactly the file that exists to avoid one. A text substitution is
/// mechanical, picks up any change Valve makes, and costs one assertion: if the marker ever moves,
/// this fails the build rather than silently generating nothing.
fn patch_entry_points(header: &Path, out_dir: &Path) -> PathBuf {
    const GUARD: &str = "#if 0\n// Global entry points";
    const OPENED: &str = "#if 1\n// Global entry points";

    let source = std::fs::read_to_string(header).expect("could not read the vendored header");

    // The vendored header is CRLF as Valve ships it, and .gitattributes keeps it that way, so
    // match on either ending rather than assuming one.
    let (needle, replacement) = if source.contains(GUARD) {
        (GUARD.to_string(), OPENED.to_string())
    } else {
        (GUARD.replace('\n', "\r\n"), OPENED.replace('\n', "\r\n"))
    };

    assert!(
        source.contains(&needle),
        "openvr_capi.h no longer guards its global entry points with `#if 0` immediately before \
         `// Global entry points`. The vendored header changed shape; look at it and update this \
         substitution rather than working around the result."
    );

    let patched_source = source.replace(&needle, &replacement);
    let patched = out_dir.join("openvr_capi_patched.h");
    std::fs::write(&patched, patched_source).expect("could not write the patched header");
    patched
}

/// Puts `openvr_api.dll` where a freshly built binary will actually find it.
///
/// Windows resolves a DLL from the directory of the running executable, so an import library
/// alone links but does not run. Confirmed by running a binary with PATH stripped to System32:
/// it fails with 0xC0000135 without a copy alongside and succeeds with one.
///
/// Cargo puts binaries in three places, and all three need it: the app in `<profile>`, test
/// binaries in `<profile>/deps`, and examples in `<profile>/examples`. The directories are
/// created rather than skipped when absent, because a clean build runs this script before Cargo
/// has made them.
///
/// Without this, every tier 2 test dies at load time with an error naming nothing that suggests
/// the real cause.
fn copy_runtime_dll(lib_dir: &Path, out_dir: &Path) {
    let dll = lib_dir.join("openvr_api.dll");

    // OUT_DIR is <profile>/build/<pkg>-<hash>/out, so the profile directory is three up.
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        println!("cargo:warning=could not locate the profile directory from OUT_DIR");
        return;
    };

    let destinations = [
        profile_dir.to_path_buf(),
        profile_dir.join("deps"),
        profile_dir.join("examples"),
    ];

    for dest_dir in destinations {
        if let Err(error) = std::fs::create_dir_all(&dest_dir) {
            println!(
                "cargo:warning=could not create {}: {error}",
                dest_dir.display()
            );
            continue;
        }
        let dest = dest_dir.join("openvr_api.dll");
        // Track the copy, not only the source. Cargo counts a missing tracked file as changed, so
        // a copy that gets quarantined by antivirus or swept out with a tidied target directory
        // brings the script back to replace it. Without this the build reports success and every
        // binary dies at load with 0xC0000135, which is the failure this function exists to
        // prevent. Naming a file the script writes is safe here because Cargo compares it against
        // a fingerprint written after the copy.
        println!("cargo:rerun-if-changed={}", dest.display());
        if let Err(error) = std::fs::copy(&dll, &dest) {
            println!(
                "cargo:warning=could not copy {} to {}: {error}",
                dll.display(),
                dest.display()
            );
        }
    }
}
