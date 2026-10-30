# Voice Notepad

A single-window text editor that looks and behaves like Windows Notepad, with
local speech-to-text behind one mic button. You narrate while you work, and
your words appear in the document as you speak, at the cursor.

The recogniser is NVIDIA Parakeet, a *streaming* model: audio goes in a chunk
at a time and text comes back as it is recognised, continuously, so dictation
reads as flowing prose rather than disconnected phrases.

Nothing leaves the machine. The speech model is downloaded once on first run
and everything after that is offline.

![The window](assets/icon-preview.png)

## Installing on Windows

Run `VoiceNotepad-2.0.0-Setup.exe` and click through it. It installs for your
user only, so Windows will not ask for an administrator password. When it
finishes, Voice Notepad is in the Start menu.

The first time you open it, it downloads the speech model and shows a
progress bar. The recommended model is 2.4 GB, so give it a few minutes; you
can type while it downloads, and there is a much smaller one in Settings if
you would rather. After that it never uses the network again.

There is also `VoiceNotepad-2.0.0-portable.zip` if you would rather not
install anything: unzip it and run the exe. Keep `onnxruntime.dll` in the same
folder - the program will not start without it.

## Installing on Linux

```sh
./scripts/install-linux.sh      # everything goes under ~/.local, no root needed
./scripts/uninstall-linux.sh    # puts it back
```

Then run `voice-notepad [file]`, or pick Voice Notepad out of the application
menu.

One thing to know: **the system-wide hotkeys need an Xorg session.** Wayland
deliberately does not let an application claim keys system-wide, so under
Wayland the hotkeys work while the Voice Notepad window has focus but not
while another program does. The app detects this and says so in the status bar
and in Settings. Log in to an Xorg session if you need them everywhere, or use
the mic button.

## Using it

### Talking

| What | How |
| --- | --- |
| Start or stop listening | `Ctrl+Alt+Space`, or click the mic button |
| New bullet | `Ctrl+Alt+B` |

Both hotkeys work system-wide, so they still work while another program has
focus. That is the point: you can be clicking around the app you are testing
and still start, stop and punctuate your notes without switching windows.

The mic button is grey when paused and red when listening, and a small red dot
shows at the right of the status bar while it is listening.

Text streams in as you speak. The phrase in progress keeps being rewritten as
the transcription firms up, so you can read along; when you pause, it settles
and stops moving.

### Bullets

Bullets only ever appear when you press `Ctrl+Alt+B`. A pause in speech never
creates one.

- `Ctrl+Alt+B` starts a bullet at the cursor.
- `Tab` on a bullet line pushes it one level deeper; `Shift+Tab` pulls it back.
- `Ctrl+Alt+B` on a sub-bullet starts the next bullet at that same level.
- `Enter` on an empty bullet removes the marker.
- Levels cycle `•`, `◦`, `▪` as you nest, and nest as deep as you like.

If you just start talking with no bullet, the words go in as ordinary text.

### Getting the text out

Copy, Cut, Copy All and Save all convert what is on screen to markdown: a dash
for each bullet, four spaces of indentation per level, plain text otherwise.
No timestamps, no speaker labels, no segment markers, anywhere. It pastes
straight into an LLM.

`Edit > Copy All as Markdown` (`Ctrl+Shift+C`) takes the whole note in one go.

So this on screen:

```
Testing the login screen.
• Password field accepts empty input
    ◦ no error message shown
```

copies as this:

```
Testing the login screen.
- Password field accepts empty input
    - no error message shown
```

### Speed and the two models

Transcription runs on the CPU, a chunk of audio at a time. What matters is
whether a chunk is recognised faster than it takes to say, and on a modern
desktop the recommended model manages it about four times over.

| Model | Size | Notes |
| --- | --- | --- |
| Nemotron streaming (English) | 2.4 GB | The default. Punctuation and capitals. |
| Realtime EOU | 483 MB | Far smaller and faster, but writes **no punctuation and no capital letters** - just lowercase words. |

Switch in Settings. If dictation lags behind your speech on a slower machine,
that is the lever to pull.

### Settings

Right-click the mic button, or `View > Settings`. You can change the theme,
the speech model, the microphone, and both hotkeys. Settings are kept in
`%APPDATA%\VoiceNotepad\config\config.json`.

Models, smallest and fastest first: `tiny.en` (75 MB), `base.en` (142 MB),
`small.en` (466 MB, the default), `medium.en` (1.5 GB). A bigger model is more
accurate and needs more CPU. Changing it downloads the new one.

### The rest of the menus

File has New, Open, Save, Save As and Exit. Edit has Undo, Redo, Cut, Copy,
Paste, Delete, Copy All as Markdown, Select All and Find. View has the zoom
controls, Word Wrap and Settings. The shortcuts are Notepad's.

The theme follows Windows' light or dark setting; the sun/moon button next to
the mic flips it.

## Building it

The build runs on Linux and cross-compiles to Windows. See
[HANDOFF.md](HANDOFF.md) for what has to be installed and why.

```sh
./scripts/build-installer.sh      # exe + installer, into dist/
./scripts/build-windows.sh        # just the exe
cargo test --no-default-features  # the logic tests, no audio hardware needed
```

## Sharing it with other people

Send them `VoiceNotepad-2.0.0-Setup.exe` and nothing else. It is a single
file - the support DLL is packed inside it and only appears on their disk when
they install.

Expect Windows to warn about it. An unsigned program from an unknown publisher
gets a blue "Windows protected your PC" screen; the way past it is **More
info** then **Run anyway**, which is worth telling people in advance. The only
real fix is a code-signing certificate - see HANDOFF.md, and
`scripts/sign-windows.sh`, which signs the build once a certificate exists.

## Licence

MIT. See [LICENSE](LICENSE).
