# Building Flex

Most people don't need to build Flex themselves: the
[releases page](https://github.com/catchouli/flex/releases/latest) has an installer and a
zip for Windows, and a `.tar.gz` for Linux. This is for everything else: building on a
Mac, or running your own changes.

You'll need [Rust](https://rustup.rs) and a copy of libmpv, the library Flex plays
videos with.

```sh
cargo build --release
```

That gives you `target/release/flex` (`flex.exe` on Windows). `cargo build --release
--workspace` builds the bundled plugins too.

## libmpv

**Windows:** grab `libmpv-2.dll` from the
[mpv releases](https://github.com/mpv-player/mpv/releases) page and drop it next to
`flex.exe`. Those two files are all you need, so you can copy them to whichever PC is
plugged into your TV.

**Mac:** Flex picks up the libmpv that comes with [IINA](https://iina.io) if you have it
installed, or you can point `FLEX_LIBMPV` at a `libmpv.2.dylib`.

**Linux:** install libmpv from your distro (`apt install libmpv2` on Debian and Ubuntu).
Flex works under Wayland (including kiosk setups like cage) or X11. The remote's media
buttons are read from `/dev/input`, so add yourself to the `input` group for those.
