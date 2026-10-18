# Flex

<a href="screenshots/home.jpg"><img src="screenshots/home-thumb.jpg" width="32%"></a>
<a href="screenshots/anime.jpg"><img src="screenshots/anime-thumb.jpg" width="32%"></a>
<a href="screenshots/show.jpg"><img src="screenshots/show-thumb.jpg" width="32%"></a>

A media player and organiser that scans your local libraries (local folders or mounted
drives) and sorts everything into Movies, Shows, Anime and Music collections. It runs
entirely locally, so there's no server to set up, and it can import an existing Plex
database to bring your watch history across.

Music libraries are sorted into albums and artists using your files' tags, with album
covers, artist photos and discographies pulled in from MusicBrainz.

It plays YouTube too, in the same player as everything else: your subscriptions and
search, with no ads, and sponsor segments skipped by
[SponsorBlock](https://sponsor.ajay.app). Between that and the libraries, pretty much
everything we watch goes through Flex now.

It's also really fast. I got fed up with how sluggish Plex is on my TV PC, and this
opens instantly and stays snappy even with tens of thousands of videos.

It's made to be used from the couch with a TV remote (arrows, OK and back are all you
need), but a keyboard and mouse work fine too.

And it has plugins, for bolting on extras that don't belong in a media player. We
wanted the LED strip behind our TV to glow with whatever's on screen, so the first
plugin does just that, talking to the strip over Bluetooth with
[lotus-lamp-rs](https://github.com/bombpersons/lotus-lamp-rs).

## Getting it running

You'll need Rust and a copy of libmpv.

On Windows, run `cargo build --release`, then grab `libmpv-2.dll` from the
[mpv releases](https://github.com/mpv-player/mpv/releases) page
and drop it next to `flex.exe`. Those two files are all you need, so you can copy them
to whichever PC is plugged into your TV.

On a Mac it'll pick up the libmpv that comes with [IINA](https://iina.io) if you have it,
or you can point `FLEX_LIBMPV` at a `libmpv.2.dylib`.

On Linux, install libmpv from your distro (`apt install libmpv2` on Debian and Ubuntu)
and run `flex`. Each release has a `linux-x64.tar.gz` too. It works under Wayland
(including kiosk setups like cage) or X11. The remote's media buttons are read from
`/dev/input`, so add yourself to the `input` group for those.

Then:

1. Open Settings and add your libraries, picking whether each is Movies, TV, Anime or
   Music.
2. If you want posters, descriptions and episode names, get a free API read access token
   from [TMDB](https://www.themoviedb.org/settings/api) and paste it into Settings.
   Music is matched on [MusicBrainz](https://musicbrainz.org), which doesn't need a key.
3. If you're coming from Plex, use "Import from Plex…" and point it at your
   `com.plexapp.plugins.library.db`.

If something gets matched to the wrong show or album, open it and pick "Match online…"
(or "Folders & seasons…" for shows) to fix it.

## YouTube

<a href="screenshots/youtube.jpg"><img src="screenshots/youtube-thumb.jpg" width="49%"></a>

Switch it on in Settings → YouTube and a YouTube card appears on Home. Flex fetches
[yt-dlp](https://github.com/yt-dlp/yt-dlp) (and [Deno](https://deno.com), which yt-dlp
needs for some of YouTube's checks) into its data folder by itself, and keeps them up to
date, since YouTube changes things a lot and a newer yt-dlp is usually the fix.

Add channels under Channels by pasting a link or an @handle, or import your existing
subscriptions from Google Takeout (`subscriptions.csv`), FreeTube, NewPipe or an OPML
file. New videos are picked up from each channel's feed every half an hour, so there's no
API key to set up. Search works too.

Videos take a few seconds to start, since yt-dlp has to ask YouTube for them first, but
Flex starts on whichever video you have highlighted, so it's usually ready by the time you
press OK. Hold OK on a video to mark it watched or jump to its channel. Settings →
YouTube also has the quality limit (1080p unless you change it), whether to show Shorts
(hidden by default) and which SponsorBlock categories to skip (sponsors, self-promotion
and "like and subscribe" reminders by default).

## Plugins

Plugins live in a `plugins` folder, either next to `flex.exe` (releases come with the
LED strip one) or in Flex's data folder (Settings → Plugins → "Open plugins folder").
Every plugin is off until you switch it on in Settings → Plugins, and one that's off
never runs, so they cost nothing if you don't use them. `cargo build --release
--workspace` builds the plugins in this repo along with Flex.

For the LED strip, switch on "LED strip (Lotus Lamp)" and press play. It connects to
the first strip it finds and remembers it; "Find strips" lets you pick another.
Settings → Ambient light chooses between the whole picture and just its edges, how
smoothly it changes, how vivid and bright it is, and what it shows when you're not
watching anything (a dim warm white unless you change it). Press F2 during a video to
see the colour being sent.

There's also a [Trakt](https://trakt.tv) plugin, which keeps your Trakt history up to
date with what you watch: switch it on, choose "Connect to Trakt" and enter the code it
shows at the address it gives (auth.trakt.tv/activate) on your phone.

Flex works out the colour itself and only samples the screen while a light plugin is
running, so a plugin for a different kind of light only has to pass it on. A plugin is
just a program that reads and writes lines of JSON, so you can write one in whatever
you like. See [PLUGINS.md](PLUGINS.md).

## Remote controls

Arrows to move, OK to select, Back to go back. Hold OK for more options, and hold Back
to quit. Play/pause, stop and next/previous work if your remote has them, and on my
remote zoom+ toggles fullscreen and zoom- jumps to search.
