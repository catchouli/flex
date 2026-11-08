# Video effects

Flex can draw a shader over the video while it plays: kaleidoscopes, melting trails,
dreamy swirls, anything you can write as a GLSL fragment shader. Effects run on the GPU,
can react to the sound, and reload as soon as you save the file, so you can write one
while watching it.

Choose one in Settings → Picture → Video effects, or hold OK while a video plays to
switch to the next one (and back to off). Its settings show under it in Settings.

Flex comes with two to start from, in [`effects`](effects):

- **Melt**: a turning kaleidoscope with glowing trails that change colour; beats kick it.
- **Dream**: a DeepDream-like feedback loop where edges curl into tendrils, bright
  patches blossom and details sprout texture.
- **Depth**: uses a neural network's guess at how far away everything is, for rainbow
  contours rippling through the scene, or fog.

## Making one

An effect is a `.frag` file in an `effects` folder, plus an optional `.toml` file with
the same name:

```
effects/
  wobble.frag
  wobble.toml     (optional)
```

Flex looks in `effects` next to `flex.exe` (the ones it ships with), then in `effects`
in Flex's data folder (`%APPDATA%\Flex\data\effects` on Windows,
`~/Library/Application Support/Flex/effects` on a Mac; "Open effects folder" in Settings
takes you there). A file in the data folder wins over one with the same name next to
Flex, so copy `melt.frag` and `melt.toml` there to change them. After adding one, use
"Look for new effects".

Development builds (`cargo run`) also look in the source folder's `effects`, so edits
there show up straight away.

### The shader

Write one function, which gives the colour at `uv`:

```glsl
vec4 effect(vec2 uv) {
    vec3 c = texture(u_video, uv).rgb;
    return vec4(c.gbr, 1.0);
}
```

It's GLSL 1.40 / 3.30 (or ES 3.00 where that's all there is). Flex adds these, so
don't declare them yourself:

| Uniform | What |
|---|---|
| `sampler2D u_video` | the video frame as mpv drew it (with subtitles, and Anime4K or SVP if they're on) |
| `sampler2D u_prev` | the effect's own previous output (black unless `feedback = true`) |
| `vec2 u_resolution` | the size being drawn, in pixels |
| `float u_time` | seconds since the effect started; keeps going while paused |
| `float u_video_time` | the playback position in seconds: stops when paused, jumps when you seek |
| `int u_frame` | frames drawn since the effect started |
| `vec4 u_video_rect` | where the picture is without its black bars: `x0, y0, x1, y1` |
| `float s_<key>` | each `number`, `bool` (0 or 1) or `choice` (the option's index) setting |
| `float u_level`, `u_bass`, `u_mid`, `u_treble` | how loud the sound is overall, below 150 Hz, 150 Hz–2 kHz and above 2 kHz (0–1, with `audio = true`) |
| `float u_beat` | 1 on a beat (a jump in the bass), fading to 0 in about half a second |
| `sampler2D u_spectrum` | the sound's spectrum: x is frequency (30 Hz to 11 kHz, log-spaced, 64 bands), y is age (0 is now, 1 is about two seconds ago); values 0–1 in `.r` |
| `float depth_at(vec2 uv)` | a function: how near that point of the screen is, from 0 (far) to 1 (near) (with `depth = true`; 0 until the first depth map arrives) |
| `sampler2D u_depth`, `float u_has_depth` | the depth map itself, covering the picture rather than the screen, and 1 once there is one |

`uv` and `u_video_rect` run from 0 to 1, starting at the bottom left. Effects cover the
whole screen, black bars included, so they can draw into them.

Use `u_video_time` for anything that should freeze when the video's paused, and `u_time`
for things that should keep drifting. Go easy on big sudden changes driven by `u_beat`:
whole-screen flashes are hard on the eyes.

### The .toml

```toml
name = "Wobble"                # shown in Settings; defaults to the file's name
description = "Makes it wobble"
feedback = true                # give the shader its own last frame (u_prev); default false
audio = true                   # the sound uniforms; default false
depth = true                   # how far away things are; default false
scale = 1.0                    # work at this fraction of the screen's size (0.25–1), then
                               # scale up: lower is lighter on the GPU; default 1

[[settings]]
key = "speed"                  # becomes `uniform float s_speed;`
label = "Speed"
type = "number"                # number, bool or choice
detail = "How fast it wobbles" # help under the setting
min = 0
max = 2
step = 0.25
unit = "×"
default = 1
```

Settings show under the effect in Settings → Picture → Video effects and are kept per
effect. A `choice` setting takes `options = [{ id = "soft", label = "Soft" }, ...]`, and
the shader gets the chosen option's position (0, 1, ...).

### Sound

With `audio = true`, Flex reads the playing file's sound a second time with ffmpeg
(libmpv can't share what it plays), just ahead of the playback position, so the levels
follow the picture exactly, stop when paused and catch up after a seek. Levels are
against the loudest sound of the last few seconds, so quiet scenes still move things and
explosions don't max everything out for long. It needs ffmpeg (next to Flex, or on the
`PATH`), and only works for files: YouTube and discs get silence (all zeros). Over a
network share it doubles what's read from the file, so leave it off unless the effect
uses it.

### Depth

With `depth = true`, Flex works out how far away everything in the picture is, using
[Depth Anything V2 Small](https://github.com/DepthAnything/Depth-Anything-V2) (Apache-2.0)
running on the GPU as compute shaders, built on the kernels from Maisy's
[neurofield](https://github.com/bombpersons/neurofield). The first time an effect asks for
it Flex downloads the model (99 MB) into its data folder.

Depth is relative rather than in real distances, and is worked out about 20 times a
second at 518 × 294, so it arrives a frame or two after the picture and is softer than it.
It's smoothed over time so it doesn't flicker. It uses a fair bit of the GPU, so leave it
off unless the effect uses it; the model is unloaded a little while after the last
effect that wanted it. F2 and F3 show how long it's taking.

## Testing

- While a video plays, Flex checks the shader twice a second and reloads it when it
  changes. If it doesn't compile, the video plays without the effect, Settings → Picture
  → Video effects shows the first error, and the whole compiler output goes to Flex's
  log (Settings → "Open logs folder").
- F3 while a video plays opens a window for choosing the effect and changing its
  settings with sliders while you watch (plus Anime4K and SVP), with meters for the
  sound levels. Changes are kept, the same as in Settings.
- F2 shows which effect is on, the time it's given, and the sound levels.
- `flex --windowed some-video.mkv` starts Flex straight into a video.
