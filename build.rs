//! Compiles the Windows resources (icon and version info) into the exe.
//!
//! Done with `windres` directly rather than a build-script crate, because the
//! cross-compile already depends on mingw's binutils being present and this
//! avoids another dependency for something purely cosmetic.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    // Shown in Help > About, so a bug report says which build it came from.
    println!("cargo:rustc-env=VOICE_NOTEPAD_TARGET={target}");

    if !target.contains("windows") {
        return;
    }

    println!("cargo:rerun-if-changed=assets/voice-notepad.rc");
    println!("cargo:rerun-if-changed=assets/voice-notepad.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let object = out_dir.join("voice-notepad-resources.o");

    // Two different resource compilers, because the two Windows toolchains
    // use different ones: windres for MinGW, llvm-rc (or rc.exe) for MSVC.
    let msvc = target.contains("msvc");
    let tool = std::env::var("WINDRES").unwrap_or_else(|_| {
        if msvc {
            "llvm-rc".to_string()
        } else {
            "x86_64-w64-mingw32-windres".to_string()
        }
    });

    let result = if msvc {
        // llvm-rc emits a .res, which the MSVC linker takes directly.
        let res = out_dir.join("voice-notepad-resources.res");
        let status = Command::new(&tool)
            .arg("/I")
            .arg("assets")
            .arg("/FO")
            .arg(&res)
            .arg("assets/voice-notepad.rc")
            .status();
        if matches!(&status, Ok(s) if s.success()) {
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        status
    } else {
        Command::new(&tool)
            .args(["-I", "assets", "assets/voice-notepad.rc", "-o"])
            .arg(&object)
            .status()
    };

    match result {
        Ok(status) if status.success() => {
            if !msvc {
                // Only the binary gets the resources; test harnesses do not
                // need them and linking them there would be noise.
                println!("cargo:rustc-link-arg-bins={}", object.display());
            }
        }
        Ok(status) => {
            println!("cargo:warning={tool} failed ({status}); the exe will have no icon")
        }
        Err(err) => {
            println!("cargo:warning=could not run {tool} ({err}); the exe will have no icon")
        }
    }
}
