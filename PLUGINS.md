# Writing a Flex plugin

Plugins started with an LED strip. We wanted the one behind our TV to glow with
whatever's on screen, driven by [lotus-lamp-rs](https://github.com/bombpersons/lotus-lamp-rs),
and that kind of thing is far too specific to build into Flex itself. So Flex has
plugins instead: small add-ons that Flex starts, sends events to and gets messages
back from. That strip's plugin is in [`plugins/lotus-lamp`](plugins/lotus-lamp) and
makes a good example.

A plugin is just a program. Flex starts it, writes messages to its stdin and reads
messages from its stdout, one JSON object per line. Anything it prints to stderr ends
up in Flex's log. That means you can write one in any language, and if it crashes, Flex
carries on (and restarts it after a short wait).

## The folder

Each plugin is a folder with a `plugin.toml` and its program:

```
plugins/
  my-lights/
    plugin.toml
    my-lights.exe
```

Flex looks in two places: `plugins` next to `flex.exe` (where releases put the plugins
they ship with), and `plugins` in Flex's data folder (`%APPDATA%\Flex\data\plugins` on
Windows, `~/Library/Application Support/Flex/plugins` on a Mac; Settings → Plugins →
"Open plugins folder" takes you there). If both have a folder with the same name, the
data folder's wins. After adding one, use "Look for new plugins" or restart Flex.

Plugins start off. Nothing runs until someone switches it on in Settings → Plugins.

## plugin.toml

```toml
name = "My lights"                 # shown in Settings
description = "Lights behind the TV"
version = "1.0.0"
command = "my-lights"              # the program, in this folder (".exe" is added on Windows)
args = []                          # optional
env = { RUST_LOG = "info" }        # optional extra environment variables
subscribe = ["light"]              # which messages you want (see below)

[light]
zones = 1                          # how many colours you want per update

[[settings]]
key = "brightness"
label = "Brightness"
type = "number"
min = 10
max = 100
step = 10
unit = "%"
default = 100
```

### Subscriptions

`subscribe` says what the plugin is for, so Flex knows what to send it and what work to
do.

| Subscription | You get | Notes |
|---|---|---|
| `light` | `light` messages, about 25 a second during videos | Flex only samples the screen while a running plugin subscribes to this |
| `playback` | `playback` messages when something starts, pauses, resumes or stops | Videos and music |

Every plugin gets `hello`, `settings`, `action` and `shutdown`, whatever it subscribes to.

### Settings

Each `[[settings]]` entry becomes a row on the plugin's page in Settings → Plugins. Flex
stores the values and sends them to you, so a plugin doesn't need to save anything
itself.

| `type` | Shows as | Value |
|---|---|---|
| `bool` | on/off | `true` / `false` |
| `choice` | cycles through `options` | the chosen option's `id` (a string) |
| `number` | cycles from `min` to `max` in `step`s | a number |
| `text` | opens a text box | a string |
| `button` | a button | none: pressing it sends an `action` |

Every type also takes `detail` (a line of help under the setting). A `text` setting with
`secret = true` (a password or API key) shows only as "set" and is typed in hidden. `default` is the
value before anyone changes it. Choice options look like
`options = [{ id = "", label = "The first one found" }]`, and a plugin can replace them
while it runs (with `choices`, below), which is how "Find strips" fills in the list.

## Messages from Flex

Every message has a `type`. Ignore types and fields you don't recognise: new ones may
be added, and that's how older plugins keep working with newer versions of Flex.

**`hello`** is always the first message. It carries the protocol version (currently 1),
Flex's version and the current settings:

```json
{"type":"hello","api":1,"flex_version":"0.5.1","settings":{"brightness":100}}
```

**`settings`** is sent when the user changes something. It includes every setting, not
only the one that changed.

```json
{"type":"settings","settings":{"brightness":80}}
```

**`light`** carries colours for the lights (with `subscribe = ["light"]`):

```json
{"type":"light","zones":[[255,140,60]],"brightness":0.42}
```

- `zones` has as many colours as `[light] zones` asked for. Each colour is at full
  brightness (its brightest channel is 255, unless it's black). With several zones they
  go clockwise around the edge of the picture, starting at the bottom left: up the left
  side, across the top, down the right side and back along the bottom.
- Colours and brightness are in **linear light**: the amount of light per channel,
  which is what LEDs dimmed by PWM want (half the value gives half the light). Don't
  send them to a device that expects screen colours as they are, or everything looks
  washed out; convert each channel to sRGB first (roughly `c^(1/2.2)`).
- `brightness` runs from 0.0 (off) to 1.0. The colour the user should see is each
  channel multiplied by it. It's sent separately because many lights dim more smoothly
  with their own brightness control than with dark colour values.
- Smoothing, the user's ambient light settings and black bars are all handled before
  you get it, so a plugin should just pass it on.
- Flex sends one roughly every 40 ms while a video plays, and a repeat every couple of
  seconds when nothing changes. If you fall behind, Flex skips ahead to the newest
  colours instead of building up a queue.

**`playback`** (with `subscribe = ["playback"]`), sent when something starts, pauses,
resumes or stops (not continuously):

```json
{"type":"playback","state":"playing","title":"Frieren · S1E3 …","kind":"video",
 "position":61.2,"duration":1420.5,
 "media":{"kind":"episode","title":"Frieren","year":2023,"tmdb":null,"anilist":154587,"season":1,"episode":3}}
```

- `state` is `playing`, `paused` or `stopped`. `kind` is `video` or `music`.
- `position` and `duration` are in seconds. A `stopped` message says what was playing
  and where it stopped. When one thing follows another (the next episode), you get
  `stopped` for the first before `playing` for the next.
- `media` says what it is, for films and episodes Flex has matched online: `kind` is
  `movie` or `episode`, with the film or show's `title` and `year`, its `tmdb` id (films
  and TV) or `anilist` id (anime), for episodes the `season` and `episode` number, and
  `anime: true` when it's in an Anime library.
  It's missing for unmatched files, music and YouTube. The Trakt plugin in
  [`plugins/trakt`](plugins/trakt) uses it.

**`action`** means a `button` setting was pressed. `id` is its `key`.

```json
{"type":"action","id":"find"}
```

**`shutdown`** is sent when Flex is quitting or the plugin has been switched off. Tidy
up (turn the lights off, say) and exit. Flex waits up to two seconds, then kills the
process. If Flex goes away without saying goodbye, stdin closes; treat that the same
way.

## Messages to Flex

Write these to stdout, one per line, and flush after each.

| Message | Does |
|---|---|
| `{"type":"status","ok":true,"text":"Connected"}` | Sets the status line under your plugin's name. `ok: false` shows it as a problem |
| `{"type":"choices","key":"device","options":[{"id":"a1","label":"Strip A"}]}` | Replaces a `choice` setting's options |
| `{"type":"set","settings":{"device":"a1"}}` | Changes and saves settings, e.g. to remember the device you connected to |
| `{"type":"toast","text":"Found 2 strips"}` | Shows a short message on screen for a few seconds. Add `"seconds":600` to keep it up longer and a `"key"` to replace it later; a toast with the same key and empty `text` removes it (the Trakt plugin keeps its sign-in code up this way until it's used) |
| `{"type":"log","level":"info","text":"…"}` | Writes to Flex's log (`error`, `warn`, `info` or `debug`) |

Lines that aren't JSON are written to the log as they are, so stray prints won't break
anything.

## In Rust

The [`flex-plugin`](crates/flex-plugin) crate has all the message types and a loop that
reads them for you. It merges light updates that arrive while you're busy, so a slow
device only ever gets the newest colour.

```rust
use flex_plugin::{Host, Light, Plugin, Settings};

struct MyLights;

impl Plugin for MyLights {
    fn hello(&mut self, host: &Host, _settings: &Settings) {
        host.status(true, "Ready");
    }

    fn light(&mut self, _host: &Host, light: &Light) {
        let [r, g, b] = light.zones[0].map(|c| (f32::from(c) * light.brightness) as u8);
        // ...send r, g, b to your lights
    }

    fn shutdown(&mut self, _host: &Host) {
        // ...turn them off
    }
}

fn main() {
    flex_plugin::run(&mut MyLights);
}
```

Every method has a default that does nothing, so implement only the ones you need.
`Host` is cheap to clone and works from any thread, which is handy for async code; see
the Lotus Lamp plugin for that.

A plugin in this repo goes in `plugins/<name>/` with its `plugin.toml` next to its
`Cargo.toml`. The workspace picks it up automatically, CI builds it, and releases copy
it into `plugins/<name>/` next to `flex.exe`. Plain `cargo build` skips plugins so
everyday builds stay quick; use `cargo build --workspace`.

## In anything else

Here's a whole (if not very useful) plugin in Python, which prints each colour to
Flex's log:

```python
import json, sys

def send(msg):
    print(json.dumps(msg), flush=True)

for line in sys.stdin:
    msg = json.loads(line)
    if msg["type"] == "hello":
        send({"type": "status", "ok": True, "text": "Watching the colours"})
    elif msg["type"] == "light":
        r, g, b = msg["zones"][0]
        print(f"#{r:02x}{g:02x}{b:02x} at {msg['brightness']:.0%}", file=sys.stderr)
    elif msg["type"] == "shutdown":
        break
```

with a `plugin.toml` like:

```toml
name = "Colour logger"
command = "python3"
args = ["logger.py"]
subscribe = ["light"]
```

`command` may be a program on the `PATH` like this, as well as one in the plugin's
folder. Plugins run with the plugin's folder as their working directory.

## Testing

- Put your plugin in the data folder, switch it on, and watch Flex's log (Settings →
  "Open logs folder"): starts, exits, stderr and `log` messages all go there.
- Press F2 while a video plays to see the colour Flex is sending.
- To test against a separate copy of Flex's data, run Flex with
  `FLEX_DATA_DIR=/some/folder`. Plugins are then read from `/some/folder/plugins`.
