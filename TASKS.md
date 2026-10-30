# Voice Notepad - tasks

## Done
- Cross-build to Windows from Linux, proven before anything else was written.
  Found and patched two blockers in `whisper-rs-sys` (ggml's static libraries
  lose their `lib` prefix on WIN32; the C++ runtime was linked dynamically)
  and switched to the `-posix` mingw variant, which whisper.cpp's use of
  `std::thread` requires.
- Text model: bullets, Tab/Shift+Tab nesting, the new-bullet hotkey,
  Enter-on-empty-bullet, character-safe cursor maths.
- Markdown conversion both ways, used by Copy, Cut, Copy All and Save.
- Settings with persistence, model download with a progress bar, resampling,
  voice activity detection, transcript scrubbing.
- Speech engine: one thread owning the mic and whisper.cpp, streaming partials
  that firm up in place.
- Notepad-style window: menus, status bar, light/dark theme, Find, Settings.
- System-wide hotkeys, rebindable.
- App icon and version info embedded in the exe.
- NSIS installer, per-user, with Start menu entry and a working uninstaller.
- 102 tests. README, HANDOFF.md, and a study guide in ~/deliverables.

## Done in 2.0.0 - the speech engine was replaced
- Whisper is gone, and with it the fragmentation. Whisper only understands
  complete 30-second clips, so the engine had to detect silences, cut speech
  into phrases and transcribe each one alone - which is exactly why dictation
  came out in disconnected pieces. NVIDIA Parakeet is a streaming model:
  audio goes in a chunk at a time and text comes back continuously.
- Measured on the same clip: Whisper gave "And so, my fellow Americans," /
  "Ask not." / "What your country can do for you..." as three separate
  fragments. Parakeet gives one continuous line, and at 3.55x real time.
- Two models, switchable in Settings: Nemotron streaming (2.4 GB, punctuated)
  and Realtime EOU (483 MB, much faster, no punctuation or capitals).
- Deleted outright: the voice activity detector, the phrase assembler, the
  partial/final replace-in-place machinery, the whisper-rs-sys vendoring and
  its two cross-compile patches. All of it existed to work around Whisper.
- Windows loads onnxruntime.dll at runtime (Microsoft ships no MinGW build to
  link against); the installer puts it beside the exe.
- Help > About, with version, build target, model and settings paths, and a
  Copy details button for bug reports. Also `--version`.

## Done in 1.1.0 and 1.2.0
- Transcription is three to four times faster. Whisper pads every clip to
  thirty seconds and was encoding all of it; it is now told how much of the
  window is real. small.en went from 2.0s to 0.5s per phrase, tiny.en from
  0.3s to 0.1s, and tiny.en got more accurate in the process.
- One reused recogniser state instead of one per phrase (~200 MB of
  allocation saved per inference; not the bottleneck, but worth not doing).
- Partials are paced from when the previous pass finished, so a slow model no
  longer runs them back to back and starves the audio queue.
- Native Linux build, installed here with scripts/install-linux.sh, with a
  desktop entry and icons.
- A path on the command line is opened at startup, like Notepad.
- The app detects a Wayland session and explains that the hotkeys cannot be
  delivered there, rather than failing silently.

## Done in this round
- Fixed switching models mid-dictation. The microphone stayed open with no
  recogniser behind it, so the button stayed red, audio piled up unconsumed
  and no text ever arrived. Four tests cover it.
- The new-bullet hotkey default is now `Ctrl+\``. Note this only applies to
  new settings files; an existing one keeps what it had until Reset to
  defaults.
- Windows version info is generated from the crate version. It had drifted to
  1.2.0 on a 2.0.0 build.
- `cargo test --no-default-features` builds again; the example needed
  `required-features`.
- `scripts/package-release.sh` builds the installer, the portable zip and a
  Linux tarball, with SHA256SUMS, in one command.
- Repo prepared to be public: personal email removed, study guide generator
  untracked (it holds a LAN address), .gitignore hardened, README rewritten
  with a real screenshot, a bug report template added. History squashed to
  five commits authored as lostmachinist.

## Next
- Measure whether the Nemotron model keeps up on the laptop. If not, the EOU
  model in Settings is the answer, at the cost of punctuation.
- Put a number on idle CPU on Windows. It cannot be measured under Wine
  (HANDOFF.md explains how to tell a real regression from the environment's
  noise).
- Find Previous, Replace, and case-insensitive search beyond ASCII.
- A way to toggle listening from outside the app on Wayland - a small IPC
  socket and a `--toggle` flag would let a GNOME custom shortcut do it, which
  is the only route to a working global hotkey under Wayland.
- Consider whisper.cpp's own VAD model if the level gate proves twitchy in a
  noisy room.

## Blocked
- Creating the public GitHub repo. The sandbox refused the publish step, so
  the repo has to be created by hand; everything else is ready and the
  release artifacts are built in dist/.
