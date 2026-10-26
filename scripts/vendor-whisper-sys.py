#!/usr/bin/env python3
"""Vendor whisper-rs-sys from the local cargo registry and patch it so it
cross-compiles to x86_64-pc-windows-gnu.

Why this is needed
------------------
whisper.cpp's ggml/CMakeLists.txt strips the "lib" prefix from static
libraries on WIN32 ("remove the lib prefix on win32 mingw"), so cmake emits
ggml.a / ggml-base.a / ggml-cpu.a.  That matches what MSVC's rustc looks for
(ggml.lib), but rustc on the *-windows-gnu target resolves
`-l static=ggml` to `libggml.a`, so the link fails with
"could not find native static library `ggml`".

whisper-rs-sys has no rename step, so we add one: after the cmake build we
drop `libX.a` copies next to every `X.a`.  Nothing else about the build
changes, and on non-mingw targets the patch is inert.

Idempotent: safe to re-run.
"""
import os, shutil, sys, glob

CRATE = "whisper-rs-sys-0.15.0"
ANCHOR = "    let destination = config.build();"

STDLIB_ANCHOR = '        println!("cargo:rustc-link-lib=dylib={}", cpp_stdlib);'

STDLIB_PATCH = """        // --- VoiceNotepad: static C++ runtime on MinGW ---
        // A dynamic libstdc++ would make the exe need libstdc++-6.dll beside
        // it; the deliverable is a single self-contained binary. rustc has no
        // idea where mingw's gcc keeps libstdc++.a, so ask the compiler and
        // hand the directory over.
        if target.contains("windows-gnu") {
            let cxx = env::var("CXX_x86_64_pc_windows_gnu")
                .or_else(|_| env::var("CXX"))
                .unwrap_or_else(|_| "x86_64-w64-mingw32-g++-posix".to_string());
            match std::process::Command::new(&cxx)
                .arg("-print-file-name=libstdc++.a")
                .output()
            {
                Ok(out) => {
                    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    match std::path::Path::new(&printed).parent() {
                        Some(dir) if dir.is_dir() => {
                            println!("cargo:rustc-link-search=native={}", dir.display());
                        }
                        _ => println!(
                            "cargo:warning={} could not locate libstdc++.a (said {:?})",
                            cxx, printed
                        ),
                    }
                }
                Err(e) => println!("cargo:warning=could not run {}: {}", cxx, e),
            }
            println!("cargo:rustc-link-lib=static={}", cpp_stdlib);
        } else {
            println!("cargo:rustc-link-lib=dylib={}", cpp_stdlib);
        }"""

PATCH = '''
    // --- VoiceNotepad: MinGW static-library naming compatibility ---
    // ggml's CMakeLists strips the "lib" prefix on WIN32, but rustc on
    // *-windows-gnu resolves `static=ggml` to `libggml.a`. Drop prefixed
    // copies alongside the originals so both spellings resolve.
    if env::var("TARGET").unwrap_or_default().contains("windows-gnu") {
        vn_add_lib_prefix_aliases(&out);
    }
'''

HELPER = '''
/// Create `libX.a` copies of any `X.a` produced by the cmake build, so that
/// rustc's *-windows-gnu static-library lookup finds them. Added by
/// VoiceNotepad's scripts/vendor-whisper-sys.py.
fn vn_add_lib_prefix_aliases(out: &std::path::Path) {
    fn walk(dir: &std::path::Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("a") {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name.starts_with("lib") {
                continue;
            }
            let alias = path.with_file_name(format!("lib{}", name));
            if !alias.exists() {
                if let Err(e) = std::fs::copy(&path, &alias) {
                    println!("cargo:warning=could not alias {}: {}", name, e);
                }
            }
        }
    }
    walk(&out.join("build"));
    walk(&out.join("lib"));
}
'''


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    dest = os.path.join(root, "third_party", CRATE)

    matches = glob.glob(os.path.expanduser(
        f"~/.cargo/registry/src/*/{CRATE}"))
    if not matches:
        sys.exit(f"error: {CRATE} not in the cargo registry.\n"
                 f"Run `cargo fetch` in {root} first, then re-run this script.")
    src = matches[0]

    if not os.path.isdir(dest):
        print(f"copying {src}\n     -> {dest}")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copytree(src, dest)
        # registry sources are read-only; we need to rewrite build.rs
        for dirpath, _, files in os.walk(dest):
            os.chmod(dirpath, 0o755)
            for f in files:
                p = os.path.join(dirpath, f)
                os.chmod(p, os.stat(p).st_mode | 0o644)
    else:
        print(f"already vendored: {dest}")

    build_rs = os.path.join(dest, "build.rs")
    with open(build_rs, encoding="utf-8") as fh:
        text = fh.read()
    original = text

    # Patch 1: lib-prefixed aliases so rustc's gnu-target lookup resolves.
    if "vn_add_lib_prefix_aliases" in text:
        print("already patched: MinGW lib-prefix aliases")
    elif ANCHOR not in text:
        sys.exit("error: lib-prefix anchor not found in build.rs; "
                 "whisper-rs-sys has changed and this patch needs updating.")
    else:
        text = text.replace(ANCHOR, ANCHOR + PATCH, 1) + HELPER
        print("patched: MinGW lib-prefix aliases")

    # Patch 2: static libstdc++ so the exe stands alone.
    if "VoiceNotepad: static C++ runtime" in text:
        print("already patched: static C++ runtime")
    elif STDLIB_ANCHOR not in text:
        sys.exit("error: C++ stdlib anchor not found in build.rs; "
                 "whisper-rs-sys has changed and this patch needs updating.")
    else:
        text = text.replace(STDLIB_ANCHOR, STDLIB_PATCH, 1)
        print("patched: static C++ runtime")

    if text != original:
        with open(build_rs, "w", encoding="utf-8") as fh:
            fh.write(text)


if __name__ == "__main__":
    main()
