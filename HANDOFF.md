# Voice Notepad - handoff

What was built, why it is built that way, how to rebuild it, and what is
still rough. Written for whoever picks this up next, including me in six
months.

## What it is

A Rust desktop app: one window, Notepad's layout and menus, plus a mic button
that runs NVIDIA Parakeet locally and streams the words into the document.
Built on Linux, cross-compiled to Windows and installed natively here.

**2.0 replaced the speech engine.** Whisper only understands complete
thirty-second clips, so the old engine detected silences, cut speech into
phrases and transcribed each one on its own -- which is why dictation came out
as disconnected fragments. Parakeet is a streaming model: audio goes in a
chunk at a time, text comes back continuously, and the model keeps its own
state between chunks. The voice detector, the phrase assembler and the
partial/final replace-in-place machinery all existed to work around Whisper
and are gone.

## How it fits together

```
src/
  main.rs        window setup; the binary is a thin shell over the library
  lib.rs         module list, and the `app` feature split (see Testing)
  ui.rs          the window: menus, text area, status bar, dialogs
  document.rs    the text model: bullets, indent, cursor-relative edits
  markdown.rs    display <-> markdown, both directions
  stt.rs         the speech engine thread; owns the mic and the recogniser
  audio.rs       cpal capture, and the microphone self-test
  resample.rs    downmix and rate-convert to 16 kHz mono
  transcript.rs  scrubbing annotations; joining streaming fragments
  diagnostics.rs crash logging and the version string
  model.rs       downloading the model with progress
  config.rs      settings, hotkey specs, where things live on disk
  theme.rs       the Notepad look, light and dark
  platform/      via platform.rs: the one thing that differs per OS
  hotkeys.rs     system-wide hotkey registration
```

Two decisions shape everything else.

**The on-screen string is the document.** There is no parallel tree of bullet
objects. A bullet line is recognised by its prefix - some multiple of four
spaces, a marker character, a space - and parsed back out when needed. Hand
edits and hotkey edits therefore cannot disagree, because there is only one
thing to disagree with. The cost is that bullet detection is a string
operation; `document::parse_line` is the single place it happens.

**One background thread owns the microphone and the recogniser.** cpal's
`Stream` is not `Send` on Windows and a recogniser is expensive to build, so they
live together on one thread that talks to the UI over channels
(`stt::Command` in, `stt::Event` out). The UI thread never touches either, so
it cannot be blocked by a model load or an inference pass. When not listening
that thread blocks on `recv()` and costs nothing.

### How live text works

While a phrase is being spoken, the whole phrase so far is re-transcribed every
700 ms and sent as `Event::Partial`. The UI keeps that text in a known
character range and swaps the range for each better guess, so the words firm
up on screen as you keep talking. When the voice detector says the phrase is
over, one last pass is sent as `Event::Final` and the text stops moving.

The UI records what it wrote into that range. If the text there is not what it
put there - you typed over it, or undid it - it lets go and starts a fresh
span rather than corrupting your edit.

## Why Parakeet, with numbers

Measured on whisper.cpp's `jfk.wav`, on this box:

| | Result |
| --- | --- |
| Whisper small.en, phrase at a time | `And so, my fellow Americans,` / `Ask not.` / `What your country can do for you, ask what you can do for your country.` -- three fragments |
| Parakeet Nemotron streaming | `And so, my fellow Americans ask not what your country can do for you ask what you can do for your country` -- one continuous line |

Speed: 560 ms of audio recognised in 136 ms on average, worst chunk 153 ms.
3.55x faster than real time on CPU, so it keeps up comfortably. Model load is
about 1.2 s; resident memory about 2.55 GB with the Nemotron model.

The two models are in `config::SpeechModel`. Nemotron (2.4 GB) punctuates and
capitalises; EOU (483 MB) is roughly five times smaller and faster but emits
neither. There is no quantised export of the streaming Nemotron model, which
is why the big one is as big as it is.

## Streaming fragments and their spacing

The recogniser emits sub-word pieces, and **the leading space on a piece is
the only thing that distinguishes the start of a new word from the middle of
the current one**: `polit` + `ical` is one word, `that` + ` is` is two.

The first version of this carried the old whole-utterance logic over -- trim
each piece, join with a space -- and dictation came out as `polit ical star`
and `two fun ctions`. `transcript::join_fragment` now preserves the model's
own spacing, dropping a leading space only at the start of a line or straight
after a bullet marker, where it would read as an indent. If dictation ever
starts breaking words apart again, that function is where to look.

## Decisions worth knowing about

**CPU, not GPU.** A CUDA or DirectML build cross-compiled from Linux would
need the matching Windows toolkits on the build box and the matching runtime
on the target machine. CPU keeps up at 3.55x real time, which is what matters.
`parakeet-rs` has `cuda`, `directml` and other execution-provider features if
this ever needs revisiting.

**ONNX Runtime is linked differently per platform, and has to be.** Linux
links it statically, so the binary stands alone. Windows cannot: Microsoft
publishes no MinGW build, so there is nothing to link against when
cross-compiling, and `ort-sys` fails outright with *no prebuilt binaries
available for target x86_64-pc-windows-gnu*. The Windows build therefore uses
`ort`'s `load-dynamic`, which loads `onnxruntime.dll` at runtime instead of
linking it. `stt::prepare_runtime()` points the loader at the DLL beside the
exe, and the installer puts it there. `scripts/fetch-onnxruntime.sh` fetches
it; the version must satisfy the `api-28` feature, so ONNX Runtime 1.28 or
newer (we ship 1.31.0, matching the static build Linux gets).

The cost is that the Windows deliverable is no longer literally a single file.
The alternative would be switching the Windows target to MSVC and
cross-compiling with `cargo-xwin`, which would allow a static ORT; that is the
route to take if one file ever matters more than a simple toolchain.

**egui/eframe rather than native Win32.** Cross-platform for free, which the
spec asked for where it is cheap, and it draws a convincing Notepad: flat
chrome, square corners, Windows' selection blue, a monospace page. A native
Win32 build would look fractionally more authentic and would kill the Linux
goal outright.

**The glow (OpenGL) backend, not the default wgpu.** Smaller binary, faster
cold start, and it runs under Wine, which is the only way this box could test
the Windows build at all.

**Per-user install.** `%LOCALAPPDATA%\Programs`, not Program Files. No UAC
prompt, and the app never runs elevated - which for something that registers
global hotkeys and writes to the user's AppData is the right side to be on.

**Panics unwind, they do not abort.** A panic in the audio callback thread
used to take the whole window down with no message at all. `diagnostics`
installs a panic hook that writes to a log beside the settings, and Help >
About says where that log is.

## Building it

### What has to be installed

Already present on this box:

```sh
# for the Windows cross-build
sudo apt install g++-mingw-w64-x86-64 mingw-w64-x86-64-dev unzip wine64
rustup target add x86_64-pc-windows-gnu

# additionally, to build and run natively on Linux
sudo apt install libasound2-dev
```

Nothing else: dropping whisper.cpp took cmake and libclang with it, since
there is no longer any C++ to build or any bindings to generate.

Not needed: NSIS. `scripts/build-installer.sh` fetches the official Windows
makensis once into `~/.local/share/voicenotepad-build` and runs it under Wine.

### The commands

```sh
./scripts/build-installer.sh     # everything -> dist/VoiceNotepad-2.0.0-Setup.exe
./scripts/build-windows.sh       # just the Windows exe
./scripts/build-windows.sh debug # ... unoptimised, with a console window

./scripts/install-linux.sh       # build and install natively, under ~/.local
./scripts/uninstall-linux.sh     # and remove it again
```

### The mingw toolchain

Nothing special any more. Version 1.x needed the `-posix` mingw variant for
two reasons, both of which belonged to whisper.cpp: libstdc++ only implements
`std::thread` under that threading model, and the win32 variant's libgcc
would not link at all (`gthr-win32.o` wants `__mingwthr_key_dtor`, which rustc
places too late in the library order). With no C++ left in the build, the
default toolchain links cleanly and is what `.cargo/config.toml` now uses --
retested, not assumed.

`-static-libgcc` and `-static-libstdc++` are still passed as cheap insurance
against a dependency reintroducing a runtime dependency. The result imports
only Windows system DLLs, which is worth re-checking after any dependency
change:

```sh
x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release/VoiceNotepad.exe \
  | grep -i 'DLL Name' | sort -u
```

`onnxruntime.dll` deliberately does **not** appear there: it is loaded at
runtime, not linked.

## Sharing it, and Windows SmartScreen

### What to send people

`dist/VoiceNotepad-<version>-Setup.exe` **is a single file.** The ONNX Runtime
DLL is packed inside it and only lands on disk at install time. Nothing else
needs to go with it. The portable zip is the only two-file deliverable, and
that is what the zip is for.

### Signing

`scripts/sign-windows.sh` Authenticode-signs a file with `osslsigncode`, from
Linux. `build-installer.sh` calls it on the exe before packaging and on the
installer afterwards, so both are signed. With no certificate configured it
prints why it skipped and the build carries on, so this costs nothing until
there is a certificate.

```sh
export VOICENOTEPAD_CERT=~/secrets/codesign.pfx   # outside the project
export VOICENOTEPAD_CERT_PASS='...'               # environment only, never a file here
./scripts/build-installer.sh
```

The signature is timestamped (`-ts`), which is what keeps already-released
builds verifying after the certificate expires. Without a timestamp everything
signed stops validating on the expiry date.

`osslsigncode` is not installed on this box; the script finds it under
`~/.local/share/voicenotepad-build/sign/` where it was unpacked without root,
or `sudo apt install osslsigncode` to have it properly.

### The single-exe experiment, and why it was not taken

ONNX Runtime *does* have a prebuilt static library for the MSVC target, so the
exe can be made genuinely self-contained. It was built and it works:
`cargo-xwin` plus clang/lld unpacked into `~/.local/share/voicenotepad-build/llvm`,
a `DirectML.lib` from the Microsoft.AI.DirectML NuGet package, and some
case-variant symlinks in the xwin SDK because the Windows SDK is
case-insensitive and Linux is not. The manifest still carries the
`target_env = "msvc"` section that makes it possible.

It was rejected because the resulting 35 MB exe imports `VCRUNTIME140.dll`
and `MSVCP140.dll` -- the Visual C++ Redistributable. `-C
target-feature=+crt-static` does not fix it: pyke's prebuilt ONNX Runtime is
built against the dynamic CRT, so a static CRT fails to link with a wall of
undefined `__declspec(dllimport)` symbols.

So the choice was between one DLL we ship ourselves and a system prerequisite
we cannot control, whose failure mode is a cryptic *VCRUNTIME140.dll was not
found* on a machine that happens to lack it. The MinGW build wins. Revisit if
ONNX Runtime ever ships a static-CRT build.

## Testing

`cargo test --no-default-features` runs 88 tests of the pure logic - bullets,
markdown, settings, model completeness, resampling, fragment joining - with
no audio hardware and no GPU. That is what the `app` feature split is for: the
modules needing cpal, eframe and the recogniser sit behind it, so the parts where
the spec pins down behaviour stay testable anywhere.

`cargo test --target x86_64-pc-windows-gnu` runs all 101 as actual Windows
binaries under Wine, via the `runner = "wine"` line in `.cargo/config.toml`.
This is worth doing before shipping; it is the only place the hotkey and theme
modules get executed.

`examples/transcribe-file.rs` runs a WAV through the real pipeline -- the same
`Recogniser`, the same chunking, the same fragment joining -- reports the
speed, and asserts the output has no timestamps or markers in it. This is how
the numbers above were measured:

```sh
cargo run --release --example transcribe-file -- \
  ~/.local/share/voicenotepad/models/nemotron-streaming-en clip.wav
```

### What was verified by hand

Under Wine, driving the real window with synthetic X11 input: typing; the
`Ctrl+Alt+B` global hotkey; Tab/Shift+Tab nesting to three levels and back;
Copy All producing exactly the specified markdown (copied, then pasted back
into the document to read it); undo; the theme toggle; Settings via
right-clicking the mic; the mic turning red with the status-bar dot; the model
downloading on first run, with combined progress across its five files; and a
clean install/run/uninstall cycle on a fresh Wine prefix with the ONNX
Runtime DLL alongside.

On Linux, additionally: real speech through a real microphone transcribed
into the document, continuously and correctly spaced.

Note that Wine needs a virtual desktop (`wine explorer /desktop=...`) for
keyboard input to work on a bare Xvfb with no window manager. Without it the
window appears and the mouse works, but keystrokes go nowhere - which looks
exactly like an application bug and is not one.

## Known limitations

**Dictation itself is proven, on Linux.** Real speech through a real
microphone was transcribed into the document continuously and correctly
spaced. That had never been true before 2.0.

**Not run on real Windows.** Everything above was verified under Wine 10 on
Linux. Wine is a good proxy and the binary is a genuine PE that links only
system DLLs, but it is a proxy. The things most worth a second look on real
hardware are microphone enumeration and permissions, and whether
`RegisterHotKey` collides with anything already installed.

**Idle CPU could not be measured here.** Under Wine on Xvfb the app sits at
about half a core when idle - but so does a completely blank eframe window
built the same way, which is how I know it is the environment's software-GL
message pump rather than this app. The one real bug in this area is fixed: the
window title was being sent as a viewport command every frame, which marked
the viewport dirty, which requested another repaint, forever; that had it
pinned at 950% CPU. Worth re-measuring on Windows, and the way to tell whether
any future regression is ours is to build a twenty-line blank eframe window
and compare.

**The model is 2.4 GB and holds about 2.55 GB of memory.** There is no
quantised export of the streaming Nemotron model. The EOU model in Settings
is a fifth of the size, at the cost of all punctuation and capitalisation.

**It transcribes whatever it hears.** There is no voice detector in front of
the recogniser any more -- the model handles silence itself -- so background
speech, a video playing nearby or a noisy room will end up in the note. Press
the hotkey to stop listening rather than leaving it on.

**Find is plain substring search.** No regular expressions, no Find Previous,
no Replace. Notepad's Find has more than this.

**Dictation is append-only.** A streaming recogniser emits text once and does
not revise it, so there is no "in-progress phrase firming up in place" any
more -- the original spec asked for that, but it was a way of making a batch
model feel live, and a streaming model is simply live. Text appears as it is
recognised and then stays put.

**Linux: built and installed, but lightly exercised.** `scripts/install-linux.sh`
puts it under `~/.local` with a desktop entry and icons. Verified on this box
under X11: it renders, picks up the system dark theme, opens a file given on
the command line, and the bullets and hotkeys work. Not verified: running for
any length of time, a real microphone, or any desktop other than GNOME. Note
that on a Wayland desktop winit will use Wayland regardless of `DISPLAY`,
which is worth knowing when testing - unset `WAYLAND_DISPLAY` to force X11.
