# Flex

A simple TV-friendly media player for a folder-based library. Rust + egui, with video
via libmpv rendered inside the app. Everything is driven by arrows, OK and Back.

## Status: Phases 1–3

- Libraries: add/remove in Settings (folder picker with drive letters on Windows), each typed
  Movies / TV / Anime. Scans run in the background; the index lives in SQLite.
- Folder view per library, fuzzy search across everything, playback with an "Up next"
  screen at the end of an episode.
- Next: resume position and watched state, incremental rescans, series matching.

Data is stored in `%APPDATA%\Flex\data\flex.db` (Windows) or
`~/Library/Application Support/Flex/flex.db` (macOS).

## Controls

Everyday use only needs the remote; typing (search) needs a keyboard.

| Action | Keys |
|---|---|
| Move | Arrow keys (Left/Right page through lists) |
| OK | Enter |
| Play/Pause | Space, remote play/pause (Windows). Acts as OK outside the player |
| Back | Esc, Backspace, Browser Back (remote), mouse back button |
| Stop | Remote stop button (Windows) |
| Next / previous episode | Remote next/previous buttons (Windows), Page Down / Page Up |
| Search | Remote search button (Windows), Ctrl/Cmd+F, or just start typing |
| Quit | Hold Back for 1.5 seconds |
| In the search box | Down moves into results, Enter opens the best match, Esc clears |
| Input log | F1, which shows exactly what each key or remote button sends (on Windows also raw key codes and HID reports) |
| Fullscreen | Remote zoom-in toggles, zoom-out leaves (handled as HID zoom, Ctrl/Cmd + `=`/`-` or Ctrl+wheel); also the top-bar button, double-clicking the video, or F11 (Windows; macOS reserves F11). Remembered between runs; `--windowed` ignores it |

In the player: **OK/Play** pause · **Left/Right** seek 10s (hold for 30s steps) ·
**Up** cycle subtitles · **Down** cycle audio · **Back** hide the OSD, then stop · **Stop** stop.
At the end of an episode, **Up next** offers the next file in the same folder: OK/Play plays
it, Back returns to where you started. Movies and the last episode in a folder go straight back.

## Running

```
flex [--windowed] [VIDEO]
```

Passing a video plays it straight away; normally you'd start with no arguments.

libmpv is loaded at runtime, so building needs nothing extra.

### Windows

1. `cargo build --release`
2. Download an `mpv-dev-x86_64-*.7z` build from
   <https://github.com/shinchiro/mpv-winbuild-cmake/releases> and extract `libmpv-2.dll`.
3. Put `libmpv-2.dll` next to `target\release\flex.exe`. Those two files are all you need
   to copy to another PC.

### macOS

Needs `libmpv.2.dylib` from somewhere: Homebrew (`brew install mpv`), or set
`FLEX_LIBMPV=/path/to/libmpv.2.dylib`.

### Cross-compiling Windows from macOS

```
rustup target add x86_64-pc-windows-gnu   # plus: brew install mingw-w64
cargo build --release --target x86_64-pc-windows-gnu
```

## Troubleshooting

- `FLEX_MPV_OPTS="hwdec=no"` passes extra mpv options (comma separated), e.g. to rule
  out hardware decoding problems. `hwdec=auto-copy` is a good middle ground.
  `start=-10` starts every file 10s before its end, handy for testing "Up next".
- `RUST_LOG=debug` for more logging (debug builds show a console on Windows).
