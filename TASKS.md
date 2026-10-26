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

## Done since 1.0.0
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

## Next
- Run it on real Windows. Microphone enumeration, permissions, and whether
  `RegisterHotKey` collides with anything already installed are the parts a
  Wine test cannot really answer.
- Measure idle CPU there too; it cannot be measured under Wine (HANDOFF.md
  explains how to tell a real regression from the environment's noise).
- Find Previous, Replace, and case-insensitive search beyond ASCII.
- A way to toggle listening from outside the app on Wayland - a small IPC
  socket and a `--toggle` flag would let a GNOME custom shortcut do it, which
  is the only route to a working global hotkey under Wayland.
- Consider whisper.cpp's own VAD model if the level gate proves twitchy in a
  noisy room.

## Blocked
- Native Linux build needs `sudo apt install libasound2-dev` (ALSA headers for
  cpal). Nothing else is in the way; Windows is unaffected.
