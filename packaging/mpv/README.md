# Flex's patches to mpv

Flex can use any libmpv, but these patches let it do things an ordinary one can't:

- `0001`: draws subtitles into a framebuffer of their own, so video effects leave them alone
  and they can be laid on top.
- `0002`: a `flextap` audio filter and `mpv_flex_set_audio_tap`, which hand the samples being
  played to Flex. Effects now record the computer's own sound instead (`system_sound.rs` in
  flex-app), and only use this where that can't be done.
- `0003`: a `flexsound` audio filter and `mpv_flex_set_sound`, which let Flex change the samples
  in place, for sound effects (`crates/flex-sound`), and say when a seek clears them.

`v0.40.0/` is the Linux package's. `windows/` is the same changes against the commit in
`windows/COMMIT` (`0001` there is the first two together), which needs newer libraries than
Debian has. Flex looks for each function when it loads libmpv, and does without what isn't
there: subtitles go through the effect, sound for effects comes from a filter inside mpv, and
the sound isn't changed.

    git clone -b v0.40.0 https://github.com/mpv-player/mpv
    cd mpv
    git am /path/to/flex/packaging/mpv/v0.40.0/*.patch

## Building

They're built on a Linux machine, once whenever the patches or mpv change, rather than on every
release:

- `.github/scripts/build-libmpv-linux.sh <output dir>`, on Debian 13: libmpv.so.2 and the
  libraries it needs, with their notices.
- `.github/scripts/build-libmpv-windows.sh <output dir> [work dir]`: a single libmpv-2.dll,
  cross-compiled with shinchiro's mpv-winbuild-cmake (its README lists the tools it needs).
  The first build takes hours; given the same work folder, later ones only redo what changed.

macOS can't be built from Linux (it needs Apple's SDK, which comes with Xcode), so that one
takes a Mac.
