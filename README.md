# Flex

A simple TV-friendly media player for a folder-based library. Rust + egui, with video
via libmpv rendered inside the app. Everything is driven by arrows, OK and Back.

## Status: Phase 1 (video + input test)

Currently a d-pad folder browser plus full-screen playback. Libraries, scanning, resume
and series matching come next.

## Controls

| Action | Keys |
|---|---|
| Move | Arrow keys (Left/Right page through lists) |
| OK | Enter, Space |
| Back | Esc, Backspace, Browser Back (remote), mouse back button |
| Input log | F1, which shows exactly what each key or remote button sends |
| Fullscreen | F11 |

In the player: **OK** pause/play · **Left/Right** seek 10s (hold for 30s steps) ·
**Up** cycle subtitles · **Down** cycle audio · **Back** hide the OSD, then stop.

## Running

```
flex [--windowed] [FOLDER_OR_VIDEO]
```

With no argument it opens the drive list on Windows, or `~/media` (else `~`) on macOS.

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
- `RUST_LOG=debug` for more logging (debug builds show a console on Windows).
