//! Compiles the Windows resources (icon and version info) into the exe.
//!
//! Done with `windres` directly rather than a build-script crate, because the
//! cross-compile already depends on mingw's binutils being present and this
//! avoids another dependency for something purely cosmetic.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=assets/voice-notepad.rc");
    println!("cargo:rerun-if-changed=assets/voice-notepad.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let object = out_dir.join("voice-notepad-resources.o");

    let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
        if target.contains("gnu") {
            "x86_64-w64-mingw32-windres".to_string()
        } else {
            "windres".to_string()
        }
    });

    let result = Command::new(&windres)
        .args(["-I", "assets", "assets/voice-notepad.rc", "-o"])
        .arg(&object)
        .status();

    match result {
        Ok(status) if status.success() => {
            // Only the binary gets the resources; test harnesses do not need
            // them and linking them there would be noise.
            println!("cargo:rustc-link-arg-bins={}", object.display());
        }
        Ok(status) => println!(
            "cargo:warning={windres} failed ({status}); the exe will have no icon"
        ),
        Err(err) => println!(
            "cargo:warning=could not run {windres} ({err}); the exe will have no icon"
        ),
    }
}
