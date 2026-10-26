# Voice Notepad - handoff

What was built, why it is built that way, how to rebuild it, and what is
still rough. Written for whoever picks this up next, including me in six
months.

## What it is

A Rust desktop app: one window, Notepad's layout and menus, plus a mic button
that runs whisper.cpp locally and streams the words into the document. Built
on Linux, cross-compiled to a self-contained 12 MB `VoiceNotepad.exe`, wrapped
in a 4 MB NSIS installer.

## How it fits together

```
src/
  main.rs        window setup; the binary is a thin shell over the library
  lib.rs         module list, and the `app` feature split (see Testing)
  ui.rs          the window: menus, text area, status bar, dialogs
  document.rs    the text model: bullets, indent, cursor-relative edits
  markdown.rs    display <-> markdown, both directions
  stt.rs         the speech engine thread; owns the mic and whisper.cpp
  audio.rs       cpal capture
  resample.rs    downmix and rate-convert to 16 kHz mono
  vad.rs         voice activity detection: where phrases start and stop
  transcript.rs  scrubbing Whisper's output; padding short clips
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

**One background thread owns the microphone and whisper.cpp.** cpal's `Stream`
is not `Send` on Windows and a Whisper context is expensive to build, so they
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

## Speed

This was the first thing to fix after 1.0.0 shipped: it was usable but slow.

Whisper pads every clip to thirty seconds and, left to itself, encodes the
whole padded window - so a three second phrase spent most of its time encoding
silence. `audio_ctx_for` in `stt.rs` tells it how much of the window is real.

Measured on whisper.cpp's `jfk.wav` with `small.en`, per phrase:

| Encoder context | Time | Text |
| --- | --- | --- |
| 1500 (full) | 2.0 s | correct |
| 768 | 0.9 s | correct |
| **512** | **0.5 s** | **correct - matches the full-context baseline** |
| 256 | 0.3 s | repeats itself: "Ask not. Ask not." |

512 is the floor (`MIN_AUDIO_CTX`). Whisper's position embeddings are trained
on the full window, so cutting too close to the audio degrades it. `tiny.en`
went from 0.3 s to 0.1 s and became *more* accurate at the same time.

Two smaller things went with it. The recogniser state is built once and
reused, rather than per phrase - measured at only 0.01 s, so this was not the
bottleneck it looked like, but it avoids allocating and freeing close to 200 MB
every inference. And the gap between partials is timed from when the previous
pass *finished*; timing from the start meant a pass that outlasted the
interval left no gap at all, so partials ran back to back and the audio queue
went undrained.

If it still feels slow, the model size is the biggest remaining lever, and it
is a Settings change rather than a code change.

## Decisions worth knowing about

**CPU, not GPU.** The spec allowed this if GPU support complicated the
single-binary goal, and it does: a CUDA or Vulkan build cross-compiled from
Linux would need the Windows CUDA toolkit or the Windows Vulkan SDK on the
build box, and would make the exe depend on the matching runtime on the target
machine. `small.en` on CPU transcribes a phrase in well under the time it took
to say it, which is what matters here. Changing this means building on Windows
with `whisper-rs`'s `cuda` or `vulkan` feature.

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

**No carried-over prompt between phrases.** Whisper accepts an initial prompt,
and feeding it the previous phrase is the usual streaming trick. Measured on
`jfk.wav` it made no difference, and `whisper-rs` 0.16's `set_initial_prompt`
leaks a `CString` on every call, which adds up in an app meant to run all day.
Dropped.

**Pre-roll before the voice detector fires.** This one did matter. Keeping the
200 ms before speech is detected and prepending it to the phrase was worth
`and so am i fell america` -> `And so am I fellow Americans` on the test clip.
Without it the attack of every phrase is clipped and the first word suffers.

## Building it

### What has to be installed

Already present on this box:

```sh
# for the Windows cross-build
sudo apt install g++-mingw-w64-x86-64 mingw-w64-x86-64-dev cmake libclang1-21 unzip wine64
rustup target add x86_64-pc-windows-gnu

# additionally, to build and run natively on Linux
sudo apt install libasound2-dev
```

Not needed: NSIS. `scripts/build-installer.sh` fetches the official Windows
makensis once into `~/.local/share/voicenotepad-build` and runs it under Wine.

### The commands

```sh
./scripts/build-installer.sh     # everything -> dist/VoiceNotepad-1.1.0-Setup.exe
./scripts/build-windows.sh       # just the Windows exe
./scripts/build-windows.sh debug # ... unoptimised, with a console window

./scripts/install-linux.sh       # build and install natively, under ~/.local
./scripts/uninstall-linux.sh     # and remove it again
```

### The whisper-rs-sys patch

`whisper-rs-sys` 0.15 does not cross-compile to `x86_64-pc-windows-gnu` as
published, for two separate reasons. `scripts/vendor-whisper-sys.py` copies it
out of the cargo registry into `third_party/` and patches both; `Cargo.toml`
points `[patch.crates-io]` at that copy. The script is idempotent and the build
scripts run it for you. `third_party/` is gitignored - delete it and it is
rebuilt.

1. **Library naming.** whisper.cpp's `ggml/CMakeLists.txt` strips the `lib`
   prefix from static libraries on WIN32 ("remove the lib prefix on win32
   mingw"), producing `ggml.a`. That matches what rustc looks for on MSVC
   (`ggml.lib`), but on the gnu target rustc resolves `-l static=ggml` to
   `libggml.a`, so the link fails with *could not find native static library
   `ggml`*. The patch drops `libX.a` copies next to each `X.a` after the cmake
   build.

2. **The C++ runtime.** The crate emits `rustc-link-lib=dylib=stdc++`, which
   would make the exe need `libstdc++-6.dll` beside it. The patch links it
   statically instead, and asks the compiler (`-print-file-name=libstdc++.a`)
   where the archive is, because rustc has no idea where mingw keeps it.

If a future `whisper-rs-sys` moves the anchor lines, the script fails loudly
with which patch no longer applies rather than silently producing a broken
build.

### The mingw threading model

`.cargo/config.toml` uses `x86_64-w64-mingw32-gcc-posix`, not the default. Two
reasons, and both are hard failures rather than preferences:

- whisper.cpp uses `std::thread`, which libstdc++ only implements under the
  posix threading model.
- The win32 variant's libgcc does not link here at all: `gthr-win32.o` wants
  `__mingwthr_key_dtor`, which lives in `libmingw32.a`, which rustc places
  earlier in the library order than `-lgcc`.

`-static-libgcc` and `-static-libstdc++` are passed as link args so nothing
from the toolchain is needed at runtime. The result imports only Windows
system DLLs - check with:

```sh
x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release/VoiceNotepad.exe | grep -i 'DLL Name'
```

## Testing

`cargo test --no-default-features` runs 89 tests of the pure logic - bullets,
markdown, settings, resampling, voice detection, transcript scrubbing - with
no audio hardware and no GPU. That is what the `app` feature split is for: the
modules needing cpal, eframe and whisper.cpp sit behind it, so the parts where
the spec pins down behaviour stay testable anywhere.

`cargo test --target x86_64-pc-windows-gnu` runs all 102 as actual Windows
binaries under Wine, via the `runner = "wine"` line in `.cargo/config.toml`.
This is worth doing before shipping; it is the only place the hotkey and theme
modules get executed.

`examples/transcribe-file.rs` runs a WAV through the real pipeline - the same
voice detector, the same Whisper parameters, the same cleanup - and asserts
the output has no timestamps or markers in it:

```sh
cargo run --release --target x86_64-pc-windows-gnu --example transcribe-file -- \
  model.bin audio.wav          # phrase at a time, as live dictation does
cargo run ... -- model.bin audio.wav --whole   # one clip, as a baseline
```

### What was verified by hand

Under Wine, driving the real window with synthetic X11 input: typing; the
`Ctrl+Alt+B` global hotkey; Tab/Shift+Tab nesting to three levels and back;
Copy All producing exactly the specified markdown (copied, then pasted back
into the document to read it); undo; the theme toggle; Settings via
right-clicking the mic; the mic turning red with the status-bar dot; the model
downloading on first run; whisper.cpp transcribing; and a clean
install/run/uninstall cycle on a fresh Wine prefix.

Note that Wine needs a virtual desktop (`wine explorer /desktop=...`) for
keyboard input to work on a bare Xvfb with no window manager. Without it the
window appears and the mouse works, but keystrokes go nowhere - which looks
exactly like an application bug and is not one.

## Known limitations

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

**Accuracy depends on how you pause.** A phrase is committed after 900 ms of
silence, and each phrase is transcribed on its own. Speaking in complete
thoughts gives noticeably better text than trailing off mid-sentence, because
a fragment has less context to work with. Measured on `jfk.wav` with
`tiny.en`: the whole clip transcribes perfectly, while phrase-at-a-time turns
"my fellow Americans" into "am I fellow Americans". Bigger models are more
forgiving. Lengthening `HANGOVER_FRAMES` in `vad.rs` trades latency for
accuracy, and costs less than it sounds like, because partials keep streaming
regardless - only the final correction is delayed.

**Long unbroken phrases get expensive.** Every partial re-transcribes the whole
phrase so far, so a 15-second sentence makes each partial cost 15 seconds of
audio to decode. There is a hard commit at 15 s to bound this. A sliding
window would be the fix, at the cost of complicating the replace-in-place
logic that makes the text firm up smoothly.

**Find is plain substring search.** No regular expressions, no Find Previous,
no Replace. Notepad's Find has more than this.

**The voice detector is a level gate.** An adaptive noise floor with an onset
count and a hangover. It handles a steady hiss well and is cheap, but a noisy
room with sudden sounds - a door, a cough - can open a phrase. whisper.cpp
ships a proper VAD model (`whisper-rs` exposes `set_vad_model_path`) which
would be the upgrade if it becomes annoying.

**Global hotkeys do not work under Wayland.** `global-hotkey` has only an X11
backend, and a Wayland compositor will not deliver keystrokes to an X11 grab -
the registration succeeds and then nothing ever fires. The app detects a
Wayland session (`platform::hotkey_caveat`) and says so in the status bar and
in Settings rather than leaving someone to work it out. An Xorg session is the
fix. The other route, if it ever matters, is a small IPC socket plus a
`--toggle` flag so a GNOME custom shortcut can drive it; that is noted in
TASKS.md and not built.

**Linux: built and installed, but lightly exercised.** `scripts/install-linux.sh`
puts it under `~/.local` with a desktop entry and icons. Verified on this box
under X11: it renders, picks up the system dark theme, opens a file given on
the command line, and the bullets and hotkeys work. Not verified: running for
any length of time, a real microphone, or any desktop other than GNOME. Note
that on a Wayland desktop winit will use Wayland regardless of `DISPLAY`,
which is worth knowing when testing - unset `WAYLAND_DISPLAY` to force X11.
