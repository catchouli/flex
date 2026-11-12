# Video effects

Flex can draw a shader over the video while it plays: kaleidoscopes, melting trails,
dreamy swirls, anything you can write as a GLSL fragment shader. Effects run on the GPU,
can react to the sound, and reload as soon as you save the file, so you can write one
while watching it.

Hold OK (or right-click) while a video plays to open the effects panel beside the picture.
Up and Down pick a row, Left and Right change it, OK opens the rows with an arrow, and Back goes
out a level and closes the panel. The cursor starts on the Video effects row, so holding OK and
then pressing OK turns them off, and doing it again brings back the same effect, shuffle and drift.
They're also in Settings → Picture → Video effects.

OK on the Effect row opens the list of effects, and whichever one you rest on shows behind it.
OK keeps it and Back returns to the one you had. Left and Right change section: All, Favourites,
and the effects that use people, faces, depth or the sound. Holding OK (or right-clicking) stars
an effect as a favourite, or with shuffle on ticks it in or out of the shuffle. Left and Right on
the Effect row move to the previous or next effect without opening the list.

Turn up Drift in the panel and every effect's settings wander slowly around where they're set.
The bars move as they go. Things that change at a rate, like Speed, stay put. OK on the row sets
how quickly.

Set Shuffle to Timed in the panel and effects take turns at random, each staying for a random time
between the shortest and longest you set, then fading into the next. Every effect plays once
before any comes round again, and the clock stops while the video's paused or the list of effects
is open. Set it to Scene cuts and the next effect starts on the frame the shot changes instead,
once the shortest turn is up. OK on the row opens the rest: Fade sets how long the change takes,
and Off makes it instant. Fade style picks how: Crossfade dissolves one effect into the next;
Through the picture has each effect ease itself away to the plain picture in its own way (heads
shrink back, colours come back, snow stops) before the next eases in. Random settings moves an effect's settings at random each time it comes
up, without changing the ones you've set. Sound effects says how often a turn comes with a sound
effect (see Changing the sound). Shuffle keeps sound, depth, masks and faces running for
whatever it might pick next, so that nothing stutters at the changes; untick the effects that use
them if the GPU struggles.

Sound effects have their own switch, under Effect, and the Sound row picks which: the Voice
changer, so far. They play on their own or alongside a video effect (see Changing the sound).

Flex comes with these to start from, in [`effects`](effects):

- **Kaleidoscope**: every part of a kaleidoscope on a slider: arms, nested folds, tiling, tunnels,
  which part of the film it sees, colour and trails.
- **Melt**: a turning kaleidoscope with glowing trails that change colour; beats kick it.
- **Synesthesia**: the picture shows what kind of sound it hears: explosions send out shockwaves,
  music lights up neon edges, singing glows gold, laughter throws confetti, rain falls, fire burns,
  thunder flashes, crowds snap cameras, engines rush and silence goes still.
- **Kapow**: comic-book sound effects burst out of the picture as you hear them (BOOM, POW, HA HA,
  SPLASH, VROOM and more), with notes floating up from the music.
- **Mood Ring**: the film's colours follow how its music feels: warm when it's happy, cold when
  it's sad, a pink glow when it's tender, loud, red or ghostly when it's exciting, angry or scary.
- **Studio Audience**: sitcom LAUGH, APPLAUSE, ON AIR and QUIET signs light up as Flex hears
  laughter, clapping, talking and silence.
- **Dream**: a DeepDream-like feedback loop where edges curl into tendrils, bright
  patches blossom and details sprout texture.
- **Anime**: the whole film redrawn as anime by a neural network, in the style of Hayao
  Miyazaki's films, Makoto Shinkai's or Studio Ghibli's, or as an ink sketch, with inked outlines.
- **Painting**: the whole film repainted by a neural network in a painting's style: a
  stained-glass mosaic, pop colours, brushstrokes, flat cubist planes or pointillist dots.
- **Paint Like**: the film painted in the style of any picture, with the film's own fine detail kept: twenty-one famous paintings
  (The Starry Night, The Great Wave, The Scream…), drifting from one to the next, or your own
  pictures dropped in the styles folder.
- **Line Art**: the film as a line drawing by a neural network, in ink on paper or over the
  film's own colours in flat comic-book bands.
- **Living Textures**: the film grown over with a living texture (honeycomb, cobwebs, chrome,
  rainbow scales, eyes…) by neural cellular automata that crawl over the picture in real time,
  carried along with what moves.
- **Drawn World**: real people in an anime, painted or line-drawn world: everything but people
  redrawn.
- **Drawn People**: anime, painted or line-drawn people in the real world: only people redrawn.
- **Depth**: uses a neural network's guess at how far away everything is, for rainbow
  contours rippling through the scene, or fog.
- **Distance**: an effect that grows with distance, or closeness: fog (in any two colours), prisms, heat haze,
  blocks, colour or a rush of speed.
- **Relight**: new coloured lights in the film's world, casting shadows, using a surface
  worked out from depth. Also clay and an oil slick.
- **Diorama**: moves the camera inside the shot: floating, a dolly-zoom, or red and cyan 3D.
- **Rubber Sheet**: stretches the film over a wobbling mesh shaped by depth: waves, ripples,
  facets or wireframe.
- **Undertow**: the background melts and pours away while whoever's up close stays solid.
- **Ego Death**: people leave rainbow ghosts of themselves that spiral away.
- **Sky Eater**: the sky opens into a spiral that endlessly swallows the rest of the film.
- **Breathing Walls**: the room is inside something alive: walls swell and sink with each breath, buckling into wet folds and throbbing veins.
- **Third Eye**: everyone grows a real eye in the middle of their forehead.
- **Wax Museum**: faces soften like candle wax and run down people's necks.
- **Hall of Faces**: each face sprouts a turning ring of copies of itself.
- **Liquid Mercury**: the whole world turns to rippling liquid metal.
- **Time Smear**: time runs slower the further away something is.
- **Mind Bloom**: a kaleidoscope that grows more arms with the bass and blooms into its own
  feedback.
- **Masks**: shows what Flex can tell apart in the picture: people, walls and sky.
- **Faces**: shows what Flex can see of the faces in the picture, for checking and for
  making effects.
- **Face Mesh**: shows what Flex can see of a face up close: its mesh, which way the head points,
  and how open the eyes and mouth are, for checking and for working on an effect.
- **Mouth Portal**: open mouths are holes into a swirling galaxy.
- **War Paint**: glowing tribal markings painted on every face, flaring on the beat.
- **Cyborg**: half of every face is torn away to show steel plates, running circuits and a red eye.
- **Porcelain Dolls**: faces are glazed china with painted lips, and they crack as a shot goes on.
- **X-Ray**: a scanner sweeps across the picture, turning faces into skulls that talk when they do.
- **Drive-In**: everyone's skin is a screen showing the film, on and on forever.
- **Follow Your Nose**: a beam of light comes out of everyone's nose, wherever their head is
  pointing.
- **Spotlight**: faces stay as they are while everything around them melts and dims.
- **Laser Eyes**: a beam from each eye, going the way the face is pointing and flaring on the
  beat.
- **Big Heads**: everyone's head swells up on top of a normal-sized body and pulses with the
  sound.
- **Upside-Down Face**: eyes and mouth turned over inside each face, or the whole face.
- **Mouth Eyes**: eyes and mouth swap places.
- **Cyclops**: everyone's eyes merge into one big one.
- **Pinocchio**: noses grow for as long as a shot lasts and snap back at the cut.
- **Silhouette Cosmos**: people become person-shaped windows into a drifting galaxy, or stand
  in front of one.
- **Invisible Man**: people vanish and the room behind them is filled in from what's been seen.
- **Clone Army**: rows of the same people stretch away behind them.
- **Wallpaper Crawl**: walls writhe, and eyes open in them and watch the people.
- **Aquarium**: the room really floods: a water surface lying level with the floor or table, that wraps around people and furniture,
  with reflections, wakes, fish, and bubbles from people's mouths.
- **Snow Globe**: snow falls behind whatever's near and settles on things; a beat shakes it off.
- **Thermal Vision**: heat colours, with people, faces and animals hottest, and cold hands, feet
  and clothes.
- **Night of the Living**: grey-green people with hollow eyes and grimy rags, under a flickering
  light.
- **Black Hole**: a real black hole hanging in the room: light bends round it into a ring, people pass in front of it, and its disc lights the walls.
- **Melting Clocks**: everything but people sags and drips over its edges.
- **Fractal Zoom**: an endless fall through copies of the live picture, centred on a face.
- **Oil on Water**: rainbow bands like petrol on a puddle, curling around people.
- **Reaction-Diffusion Skin**: animal markings that grow by themselves over every surface, wrapping round shapes and lining up along the depth of the room.
- **Datamosh**: the wrong frames get dragged around by the right movement; the beat cleans it.
- **Pixel Sort Waterfall**: bright pixels pour down the screen and pool at the bottom.
- **Colour Time Split**: red, green and blue run at different moments.
- **Inkblot**: the picture folds into a mirrored Rorschach blot that keeps changing.
- **Strobe Sculpture**: people leave rising, glowing statues of themselves on the beat.
- **Aura**: a coloured energy field streams off each person (or animal), pulsing with the bass.
- **Living Static**: people are made of TV noise, with their faces flickering through.
- **Melting People**: people sag and drip, running off in streams that pool at the bottom.
- **Smoke People**: people come apart at the edges and blow away as smoke.
- **Pixel People**: people turn to chunky pixels, which change size on the beat.
- **Ink Drawing People**: people become a living pen sketch in a full-colour world.
- **Comic Book People**: people are printed in halftone dots with thick outlines, and get speech bubbles while they talk.
- **Fire People**: people burn from the feet up, with flames that follow them.
- **Tiny People**: people shrink where they are, leaving the room huge.
- **Time-Lagged Body**: the tops of people run behind their bottoms, so they ripple as they move.
- **Stop Motion People**: people move like stop-motion puppets, a few poses a second, while
  everything around them plays on smoothly.
- **Colour Thief**: the world is grey and only people have colour, or the other way round, or only
  animals, vehicles, plants or the sky.
- **Glitch Bodies**: people tear into sliding strips on each beat.
- **Swarm**: people dissolve into buzzing specks that scatter with the bass.
- **Tiny Heads**: everyone's head shrinks to a pea on a full-sized body.
- **Googly Eyes**: plastic eyes whose pupils really roll and bounce as heads move.
- **Face Swap Roulette**: everyone's faces swap places, and swap again every few seconds.
- **Everyone Is Them**: the biggest face in the shot gets pasted onto everybody else.
- **Long Face**: faces stretch like toffee, chins sagging with the bass.
- **Clown Car**: greasepaint, a big red smile and a red nose that swells on the beat.
- **Deal With It**: pixel sunglasses drop out of the sky onto every face.
- **Blush Meter**: faces go redder the louder it gets, until steam comes out of their ears.
- **Witness Protection**: a black bar over the eyes, faces pixelated or blurred, or no features at all.
- **Moustache Everyone**: a curly handlebar moustache under every nose.
- **Love Beams**: hearts stream out of people's eyes the way they're facing.
- **Kaleidoface**: a kaleidoscope of someone's face, centred on their nose as they move.
- **Anime Face**: an anime face for everyone: a smaller nose and mouth, a narrower jaw and a smaller chin (with a mesh), skin evened into flat cel shading, thin arched brows that rise and frown with the real ones, and big drawn eyes in several styles (drawn from the side in profile, where the face is reshaped too), inked and shining, that blink when the real ones do, look where they look and sparkle on the beat.
- **Hypno Eyes**: eyes turn into spinning spirals.
- **Perfect Symmetry**: each face is one half mirrored, which is more unsettling than you'd think.
- **Spinning Features**: eyes and mouths turn round and round where they are.
- **Puppet Jaw**: jaws drop open and flap with the sound or with their own mouths, like a cut-out puppet.
- **Giraffe Necks**: heads rise on long stretchy necks that sway to the music.
- **Focus Pull**: a shallow film lens drifting between near and far, or held on a face.
- **Tiny World**: a thin slice in focus and toy-bright colours make everything a model railway.
- **Lidar Sweep**: a dark scene revealed by sweeps of laser light travelling away from the camera.
- **Point Cloud**: the scene as millions of floating dots, with the camera drifting around them.
- **Topographic Map**: the scene as a contour map, with hills where things are close.
- **Torchlight**: a dark room with a torch beam sweeping around it.
- **Magic Eye**: the film as a working stereogram: relax your eyes and it stands out in 3D.
- **Paper Theatre**: flat cardboard layers sway on strings and cast shadows on each other.
- **Thunderstorm**: rain falls behind whatever's close, lit by lightning.
- **Warp Speed**: the distance streaks into hyperspace while whoever's close stays still.
- **Beyond the Veil**: past a shimmering line in the distance, the world turns into something else.
- **Voxel World**: the scene rebuilt from blocks, standing taller the closer they are.
- **Neon Grid**: the world goes dark and everything's edges light up in neon.
- **God Rays**: shafts of light through the air, from the brightest thing in the shot and in its colour, with people cutting dark shadows through them and dust that swirls as things move.
- **Film Mountains**: the picture as a landscape: close things are peaks, far things are valleys.
- **Signals**: shows what Flex can work out from the picture, for checking and making effects.
- **Shot and Scene**: shows what Flex makes of the shot and the scene, for checking and making
  effects.
- **World Grid**: a lattice fixed to the world, to show how well Flex can tell where the camera
  is, for checking and making effects.
- **Motion Paint**: only what's moving shows, painted in a colour for each direction.
- **Echoes**: everything leaves a trail of coloured copies of where it was a moment ago.
- **Everyone's Colour**: each person is washed in their own colour, which follows them about.
- **Halos**: a glowing halo, devil's horns or a gold crown on every head.
- **Big Hands**: hands (or feet) swell up into huge cartoon mitts, throbbing with the bass.
- **Marionettes**: everyone hangs from strings tied to their hands and head, from a rocking
  wooden bar.
- **Stage Lights**: the house lights go down and everyone gets their own follow spot.
- **Mannequins**: people become wooden artists' figures, plastic toys or chrome statues, with
  seams at their joints.
- **Light Suits**: people in dark suits of neon, glowing along their outlines and joints, with
  light rippling down them on the beat.
- **Magic Hands**: balls of crackling energy in everyone's hands, with lightning between them.
- **Night City**: night falls: stars, buildings with their windows lit one by one, streetlight on
  cars and moonlit people.
- **Surveillance**: a security camera's view, tracking everyone in numbered brackets.
- **Mouth Cam**: the screen is one mouth, close up, cutting to another whenever someone else
  starts talking.
- **Film Palette**: the film redrawn in just its own four main colours.
- **Slow Burn**: the picture gets stranger the longer a shot goes on, and every cut snaps it back.
- **Soundstage**: sounds light up what's closest and roll back through the scene.
- **Fog Layer**: a sheet of low mist across the room at one height, churning when wind is heard,
  with people standing up out of it.
- **Ghosts**: pale see-through figures drift through the room and stay put as the camera moves,
  coming out in silence and with sad or scary music.
- **Graffiti**: neon tags, stencils and splashes sprayed on the walls and buildings, staying on
  them as the camera moves.
- **Paper Lanterns**: glowing paper lanterns hang in the air where they are as the camera moves,
  with people passing in front of them.
- **Laser Grid**: security laser beams strung across the room, glowing where anyone breaks them.
- **Fish Shoal**: a shoal of glowing fish swims loops through the room, behind and in front of
  people, scattering when something bangs or someone shouts.

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
don't declare them yourself, or functions of your own with the same names:

| Uniform | What |
|---|---|
| `sampler2D u_video` | the video frame as mpv drew it (with subtitles, and Anime4K or SVP if they're on) |
| `sampler2D u_prev` | the effect's own previous output (black unless `feedback = true`) |
| `float u_amount` | how much of the effect there is, from 0 (the picture as it is) to 1 (all of it), eased in and out when shuffle fades through the plain picture; 1 the rest of the time. Use it on the effect's own levers (a head's size, how thick the snow falls, how far a grade goes) so that at 0 the effect draws the plain picture and grows into itself; an effect that doesn't use it is mixed with the plain picture instead |
| `vec2 u_resolution` | the size being drawn, in pixels |
| `float u_time` | seconds since the effect started; keeps going while paused |
| `float u_video_time` | the playback position in seconds: stops when paused, jumps when you seek |
| `int u_frame` | frames drawn since the effect started |
| `vec4 u_video_rect` | where the picture is: `x0, y0, x1, y1`. Now that effects only see the picture, this is always 0, 0, 1, 1 |
| `float s_<key>` | each `number`, `bool` (0 or 1) or `choice` (the option's index) setting |
| `float u_level`, `u_bass`, `u_mid`, `u_treble` | how loud the sound is overall, below 150 Hz, 150 Hz–2 kHz and above 2 kHz (0–1, with `audio = true`) |
| `float u_beat` | 1 on a beat (a jump in the bass), fading to 0 in about half a second |
| `sampler2D u_spectrum` | the sound's spectrum: x is frequency (30 Hz to 11 kHz, log-spaced, 64 bands), y is age (0 is now, 1 is about two seconds ago); values 0–1 in `.r` |
| `float u_sound_speech`, `u_sound_music`, … | what kind of sound it is, 0–1, with `audio = true` (see [Sound](#sound)) |
| `float depth_at(vec2 uv)` | a function: how near that point of the screen is, from 0 (far) to 1 (near) (with `depth = true`; 0 until the first depth map arrives) |
| `vec4 u_ground` | with `depth = true`: the floor, a table or whatever else is flat and level, found from depth. At a point of the picture (0 to 1, y up) its depth is `dot(vec3(x, y, 1.0), u_ground.xyz)`, and `u_ground.w` is how sure that is, from 0 to 1 |
| `sampler2D u_depth`, `float u_has_depth` | the depth map itself, covering the picture rather than the screen, and 1 once there is one |
| `vec3 normal_at(vec2 uv)` | a function: which way the surface at that point of the screen faces, x to the right, y up and z towards the viewer, unit length (with `depth = true`). It has the shape of things in it: faces, arms, folds (before the first map, it's worked out from depth's slopes, so shows outlines more than shape). Use it for lighting rather than slopes of `depth_at` |
| `vec2 depth_slope(vec2 uv, float reach)` | a function: how much `depth_at` changes from `reach` pixels to one side of the point to the same distance the other, across in x and up in y (with `depth = true`) |
| `sampler2D u_normals`, `float u_has_normals` | the normals themselves (x, y and z from 0 to 1 for -1 to 1), covering the picture, and 1 when the depth network gives them |
| `float person_at(vec2 uv)`, `wall_at`, `sky_at` | functions: how much of that point of the screen is a person, a wall or sky, from 0 to 1 (with `segment = true`; 0 until the first masks arrive). `vec4 masks_at(vec2 uv)` gives all three in r, g and b |
| `vec4 more_masks_at(vec2 uv, int layer)`, `float u_has_more_masks` | a function: more masks (with `segment = true`), all 0 until the first arrive, four to a layer of `sampler2DArray u_more_masks`: layer 0 is ground, ceiling, water and plants in r, g, b and a; 1 window, body skin, hair and clothes; 2 vehicle, animal, building and furniture; 3 head, torso, arm and hand; 4 leg and foot (and, in b and a, which person and how near their middle, which `person_there` and `person_middle_at` read). `u_has_more_masks` is 1 when they're there. The functions below read them |
| `float ground_at(vec2 uv)`, `ceiling_at`, `water_at`, `plant_at`, `window_at`, `vehicle_at`, `animal_at`, `building_at`, `furniture_at` | functions: how much of that point is ground (floors, roads, grass, sand, snow: anything underfoot), ceiling, water, plants and trees, a window, a vehicle (cars, bikes, boats, planes, trains), an animal, a building seen from outside (walls inside are `wall_at`), or furniture, from 0 to 1 (with `segment = true`, from MoGe-2's head; 0 without it, and `u_has_more_masks` says which) |
| `float body_skin_at(vec2 uv)`, `clothes_at` | functions: how much of that point is a person's bare skin, or their clothes (with `segment = true`, from MoGe-2's head). With `hair_at` they add up to `person_at`. `skin_at` is the face's skin, from the face mesh |
| `float head_at(vec2 uv)`, `torso_at`, `arm_at`, `hand_at`, `leg_at`, `foot_at` | functions: which part of a person's body that point is, whatever it's wearing (shoes are feet, hair is head), from 0 to 1 (with `segment = true`, from MoGe-2's head). They add up to `person_at` |
| `int u_person_count`, `float u_person_seen[MAX_PEOPLE]`, `vec4 u_person_box[MAX_PEOPLE]`, `vec2 u_person_centre[MAX_PEOPLE]` | who's who (with `segment = true`, from MoGe-2's head): up to `MAX_PEOPLE` (8) people, the biggest, each keeping their number while they're in shot, so they can be given their own colour or anything else. How far through the arrays to look; 1 while a person's found (fading for a moment once they're lost, 0 for a free number); their box on screen (x0, y0, x1, y1) and middle |
| `vec4 u_person_head[MAX_PEOPLE]`, `vec4 u_person_hands[MAX_PEOPLE * 2]`, `vec4 u_person_feet[MAX_PEOPLE * 2]` | each person's head, hands and feet (hands and feet picture-left first: `u_person_hands[person * 2 + n]`): the middle on screen in xy, the radius in z (as a share of the screen's height), and in w how sure it is, from 0 to 1 (0 when it can't be seen; two hands together are one, the other's w is 0) |
| `int person_there(vec2 uv)`, `float person_middle_at(vec2 uv)` | functions: which person a point on screen belongs to, as an index into the `u_person_` arrays, or -1; and how near a person's middle it is, 1 at the middle (with `segment = true`) |
| `int u_face_count`, `vec4 u_faces[MAX_FACES]` | how far through the arrays to look, and each face's box on screen: x0, y0, x1, y1 (with `faces = true`) |
| `float u_face_seen[MAX_FACES]` | 1 while a face is found, blending in over a moment when it's first found, and 0 as soon as it's lost (a face's slot is kept for a moment in case it comes back, so don't draw it there). Multiply by it. It's 0 for empty slots |
| `vec2 face_point(int face, int point)` | where on the screen a face's point is: 0 right eye, 1 left eye, 2 nose, 3 right corner of the mouth, 4 left (the person's own right and left) |
| `Face face_frame(int face, vec2 aspect)`, `float oval(vec2 p, vec2 centre, Face f, vec2 half_size)` | functions: a face's points multiplied by `aspect` (to take the screen's shape out), as a `Face`: `right`, `left` and the `eyes` between them, `nose`, `mouth` (its middle), `up` and `across` (unit length, the face's own directions), `spread` (between the eyes) and `height` (from eyes to mouth); and how much of a soft oval turned with that face, `half_size` across and up it, `p` is in, from 0 to 1 (with `faces = true`) |
| `float face_at(vec2 uv)` | a function: how much of any face is at that point of the screen, from 0 to 1, soft at the edges |
| `vec3 mesh_point(int face, int n)`, `float u_face_mesh[MAX_FACES]` | a function: the mesh's point `n` (0 to 477, MediaPipe's numbering): x, y on screen and z, smaller nearer, in picture widths (with `mesh = true`). `u_face_mesh` says how much of a mesh that face has, from 0 for faces side-on |
| `vec3 u_face_open[MAX_FACES]` | how open the person's right eye, left eye and mouth are, from 0 to 1 (with `mesh = true`, and only where `u_face_mesh` is above 0) |
| `vec4 u_face_turn[MAX_FACES]` | which way a head points: x to the right of the picture, y up, z towards the viewer, and w 1 if that's known (with `mesh = true`). Side-on faces have this even with no mesh |
| `float on_face(vec2 uv)`, `vec2 face_uv(vec2 uv)`, `int face_there(vec2 uv)` | functions: how much of a face a point on screen is, from 0 to 1; where it is on the face (x 0 to 1 across it, y from the chin up, the same spot on every face whichever way it's turned); and which face it is, or -1 (with `mesh = true`). `face_there` can be -1 at the very edge of a face where `on_face` is still above 0, so clamp it before indexing an array |
| `float skin_at(vec2 uv)`, `lips_at`, `nose_at`, `brows_at`, `eye_at`, `iris_at`, `mouth_at` | functions: how much of a point on screen is that part of a face, from 0 to 1 (with `mesh = true`). `mouth_at` is the gap between the lips, `eye_at` the whole opening of the eye and `iris_at` the coloured part of it |
| `vec4 face_texture(int face, vec2 at)` | a function: a face's pixels at a position on it (as `face_uv` gives), for drawing it somewhere else; alpha is 0 off the face (with `mesh = true`) |
| `float hair_at(vec2 uv)` | a function: where hair is (with `segment = true`), from MoGe-2's head when it's running; otherwise, with `mesh = true` too, roughly, as the person above and around each face |
| `bool meshed(int face)`, `vec2 iris_of(int face, int eye)` | functions: whether a face has a mesh worth using, and where on screen the middle of its right (0) or left (1) iris is, from the mesh (with `mesh = true`) |
| `float face_roll(int face)` | a function: how far a head tilts to the side, anticlockwise, in radians (with `faces = true`) |
| `int who_at(vec2 uv)` | a function: which face a point of the picture belongs with, as an index into `u_faces`, or -1 (with `faces = true`): with `segment = true` too, the face on the same person (see `person_there`), otherwise the nearest. Use it with `person_at` to treat people separately |
| `float u_dt` | seconds since the last frame was drawn |
| `float u_cut`, `u_since_cut` | 1 on a scene cut, fading to 0 in about half a second, and seconds since the last one, for the frame on screen (with `cuts = true`). `u_since_cut` is huge (a million) until the first cut |
| `float u_shot_start` | when this shot began, in seconds into the video: the same all through a shot, and 0 until the first cut (with `cuts = true`), so something random can be chosen once for a whole shot |
| `vec3 history_at(vec2 uv, float seconds)` | a function: the picture that many seconds ago, up to about one, at half size (with `history = true`) |
| `vec2 flow_at(vec2 uv)` | a function: how a point is moving, in screen widths and heights a second (with `flow = true`). Comes from a small network once it has started, and from block matching before that or if it can't run |
| `vec3 plate_at(vec2 uv)`, `float plate_known(vec2 uv)` | functions: the room with people taken out, and how much of that point has really been seen since the last cut rather than filled in from around it (with `plate = true`). `float u_has_plate` is 1 once there's a room to give, and `plate_known` is 0 until then |
| `float u_brightness`, `u_colourfulness`, `u_warmth`, `vec3 u_palette[4]`, `float u_palette_share[4]` | the look of the picture: 0–1, 0–1, -1 (blue) to 1 (orange), its four main colours, and how much of the picture each covers (with `stats = true`). Colours keep their slots as they change |
| `vec3 u_light` | with `stats = true`: where the brightest part of the picture is, x and y from 0 to 1 across the picture, and in z how much it stands out as one place |
| `float hash(vec2 p)`, `float hash(vec3 p)` | functions: a random number from 0 to 1 for a point, the same each time |
| `float noise(vec2 p)`, `float fbm(vec2 p)` | functions: smooth noise from 0 to 1, changing about once a unit, and five octaves of it, from 0 to about 1 |
| `vec3 hue(float t)`, `vec3 hue_rotate(vec3 c, float a)` | functions: a rainbow colour, going round once as `t` goes from 0 to 1; and a colour with its hue turned by `a` radians |
| `vec2 rotate(vec2 p, float a)` | a function: a point turned anticlockwise by `a` radians about the origin |

`uv` and `u_video_rect` run from 0 to 1 over the picture, starting at the bottom left.

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
segment = true                 # where people, walls and sky are; default false
faces = true                   # where faces are; default false
mesh = true                    # a closer look at faces, as well as where they are; default false
scale = 1.0                    # work at this fraction of the screen's size (0.25–1), then
                               # scale up: lower is lighter on the GPU; default 1
history = true                 # older frames; default false
flow = true                    # how things are moving; default false
plate = true                   # the room behind people (runs the masks); default false
cuts = true                    # scene cuts; default false
stats = true                   # the picture's palette and brightness; default false
scene = true                   # what sort of shot and scene this is; default false
camera = true                  # where the camera is, for world_at (runs depth and masks); default false
restyle = "style"              # draw on the picture redrawn by a network, picked by this choice
                               # setting (see Restyled pictures); default none
shuffle = false                # leave it out of shuffle unless someone adds it; default true
suits = ["close", "dialogue"]  # with shuffle at scene cuts, come up more in these: close, wide,
                               # action, dialogue, quiet, if Suit the shot is on; default none

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

Settings show under the effect in the panel and in Settings → Picture → Video effects, and
are kept per effect. A `number` setting can say how it drifts with `drift = "smooth"`,
`"steps"` or `"off"`: use `"off"` for anything multiplied by the time, like a speed, or the
picture will jump as it changes. Left out, whole-numbered counts go in steps and the rest
smoothly. A `choice` setting takes `options = [{ id = "soft", label = "Soft" }, ...]`, and
the shader gets the chosen option's position (0, 1, ...).

### Sound

With `audio = true`, effects hear whatever the computer is playing, recorded back from the
default output: the video, and also music from anywhere else, so they keep reacting with the
video muted. That works on Windows, on macOS 14.6 and later, and on Linux with a PulseAudio or
PipeWire sound server. It can take a couple of seconds to start. Where it can't be recorded,
effects hear the video's own sound from mpv instead: through Flex's patched libmpv, or else a
filter inside mpv that reports only 5 to 10 times a second, so very short sounds can be missed.

Flex measures how loud each of 32 frequency bands is; `u_spectrum` is stretched to its 64 bands
from those. Levels are against the loudest sound of the last few seconds, so quiet scenes still
move things and explosions don't max everything out for long.

The `u_sound_*` values say what kind of sound it is. YAMNet (Google's, trained on AudioSet)
listens to the last second, every half second, on the CPU, and each value is the likeliest of a
group of AudioSet's 521 kinds of sound, eased so it doesn't jump. Several can be high at once
(singing is also music), and film soundtracks mix things, so treat them as hints: fade things
in with them rather than switching on a threshold. They need the recording or the patched
libmpv, and stay 0 with only mpv's filter.

Synesthesia, Kapow, Mood Ring and Studio Audience are built around them. Many others use them
too: the storm pours with heard rain and flashes with thunder, snow, fog, lanterns and fish
answer wind and bangs, ghosts and wall eyes come out in silence and scary music, puppet jaws
follow voices rather than music, anime faces cry when someone does, and so on.

| Value | What counts |
|---|---|
| `u_sound_speech` | talking, narration, a speech synthesizer |
| `u_sound_shouting` | shouts, yells, screams |
| `u_sound_laughter` | laughing, giggling |
| `u_sound_crying` | crying, sobbing, wailing |
| `u_sound_singing` | singing, choirs, rapping, humming |
| `u_sound_music` | any music |
| `u_sound_crowd` | cheering, applause, a crowd, chatter |
| `u_sound_animals` | any animal, birds and insects included |
| `u_sound_vehicles` | cars, trains, boats, aircraft, engines |
| `u_sound_explosions` | explosions, gunfire, fireworks, booms |
| `u_sound_thunder` | thunder and thunderstorms |
| `u_sound_water` | water, rain, streams, the sea |
| `u_sound_wind` | wind, rustling leaves |
| `u_sound_fire` | fire, crackling |
| `u_sound_alarms` | alarms, sirens, phones, whistles |
| `u_sound_impacts` | thuds, bangs, smashes, things breaking |
| `u_sound_silence` | silence |
| `u_sound_happy_music`, `u_sound_sad_music`, `u_sound_tender_music`, `u_sound_exciting_music`, `u_sound_angry_music`, `u_sound_scary_music` | how the music feels, as shares of `u_sound_music`: together they come to about as much as it does, split by which mood it sounds most like. They're rougher guesses than the rest, so lean on them gently |

### Changing the sound

Sound effects change the film's sound. They're switched on with the panel's Sound effects row,
separately from the video effects, and picked on the Sound row; they play on their own or
alongside whichever video effect is showing, and their settings are rows at the bottom of the
panel. There are:

- **Voice changer**: chipmunk, deep, robot, telephone, monster, alien, whisper, megaphone, a
  giant's, a child's, or high and low voices swapped.
- **Instrument**: the voice played on a flute, violin, trumpet, saxophone, clarinet, trombone,
  tuba, bassoon, melodica, sitar or sung vowels.
- **Auto-tune**: the voice snapped to the notes of a key and scale, hard or gently, with
  harmonies (a third, a triad, an octave below, or a choir).
- **Synth voice**: the voice played by a chiptune, a theremin or a vocoder.
- **Reverb**: a room, bathroom, concert hall, cathedral or cave, or far away; everything or
  only the voices.
- **Underwater**: muffled and wobbling, with bubbles.
- **Echo**: everything or only the voices, coming back darker each time.
- **Old sound**: an old radio, a gramophone, a walkie-talkie (only open while someone's
  talking), worn tape, or 8-bit.
- **Stutter**: short slices played again, sometimes backwards; on the voices, people stutter.
- **8D**: the sound going round your head, for headphones.
- **Voices**: the background dropping while people talk, the voices taken out, or only the
  voices.

The ones that change voices do so only while someone's talking. With video effects and shuffle on, Shuffle → Sound effects says how often a turn
comes with a sound effect picked at random (Off keeps the one picked on the Sound row). A paired sound effect
changes and fades at the same moment as the picture.

They're written in Rust (`crates/flex-sound`), each with a `sounds/<id>.toml` giving its name,
description and settings in the same form as an effect's. The sound mpv is about to play goes
through a filter Flex adds to mpv's own (from Flex's patched libmpv), so nothing is decoded
twice and the sound stays in time with the picture. Each 10 ms block is split into the voice
and everything else, changed, and put back together; with no change, what comes out is exactly
what went in, 0.09 s later (made up by holding the picture back as long).

The voice is found in two steps. First by where films put dialogue: the centre channel when
there is one, otherwise the middle of the stereo picture. Then
[DeepFilterNet3](https://github.com/Rikorose/DeepFilterNet) (MIT or Apache-2.0), a small
network made to take noise out of speech, takes what isn't voice out of that, and it goes back
into the background, so music and effects are left alone. What it gives back is clean, dry
speech, so the voice's reverb is split off as well, as its echo: wherever the voice was most of
a band of the spectrum, lately, that much of what's left there. Effects that replace the voice
take the echo out with it (so the voice as it was isn't heard behind them); others leave it. It runs on the CPU, about a twentieth
of one core, on sound turned to 48 kHz. [Silero VAD](https://github.com/snakers4/silero-vad)
(MIT) says, every 32 ms, how likely it is someone's talking. The first time a sound effect is
on, Flex downloads both (8 and 1.3 MB) into its data folder; until they're ready the voice is
split by channel alone. Singing mostly counts as background.

Instrument plays the voice with [DDSP-VST](https://github.com/magenta/ddsp-vst)'s networks
(Apache-2.0): every 20 ms a small pitch network (CREPE micro) hears the voice's pitch and
loudness, and the instrument's network (trained on recordings of that instrument) says how loud
each of 60 harmonics and 65 bands of noise should be, which an oscillator bank and a noise filter
play. Each speaker's voice is moved by whole octaves to the middle of the instrument's range,
and brought to the loudness it was trained on. The networks are run from the weights in their
TFLite files (by Flex's own code, checked against TensorFlow's); the first time an instrument's
picked, Flex downloads Google's release of all eleven (77 MB) and the pitch network (0.7 MB).

Auto-tune, Synth voice and the giant, child and swapped voices use the same pitch network. The
voice's pitch is moved by a pitch-synchronous shifter (TD-PSOLA): grains two of the voice's
periods long are cut where the voice is and laid down closer together or further apart, so the
voice keeps its own sound (its formants); squeezing each grain moves the formants too, which is
what makes a voice sound like a bigger or smaller person's. The other effects are plain signal
processing: filters, delay lines, and a reverb of eight delay lines fed back into each other.

A sound effect can change the voice, the background or both, and is told how likely it is
someone's talking and the kinds of sound heard. With a libmpv that isn't Flex's, the sound stays
as it is.

### Depth

With `depth = true`, Flex works out how far away everything in the picture is, using
[MoGe-2](https://github.com/microsoft/MoGe) ViT-S (MIT), which also says which way every
surface faces. It runs on the GPU as compute shaders, built on the kernels from Maisy's
[neurofield](https://github.com/bombpersons/neurofield). The first time an effect asks for
it Flex downloads the model (141 MB) into its data folder.

Real people are thin next to the room they're in, so in depth from 0 to 1 across the whole
picture a person is nearly flat: their shape is a few hundredths of the range, and the step
to whatever's behind them is most of it. For anything lit or shaded, `normal_at` shows that
shape where `depth_at` can't.

Depth is relative rather than in real distances, and is worked out up to 30 times a second
at 518 × 294, dropping to as little as 294 × 168 to keep each one under about 35 ms. It's
softer than the picture, and smoothed over time so it doesn't flicker, except where
something moved. The model takes a few frames to answer, so while an effect uses depth Flex
holds the picture and the sound back by that much, usually under a tenth of a second, and
each frame is drawn with the depth map made from it. It uses a fair bit of the GPU, so leave
it off unless the effect uses it; the model is unloaded a little while after the last effect
that wanted it. F2 and F3 show how long it's taking and at what size.

### Restyled pictures

With `restyle = "<key>"`, the effect draws on the picture redrawn by a neural network, in a
style picked by its choice setting `<key>`, whose options' ids name the networks:

- [AnimeGANv3](https://github.com/TachibanaYoshino/AnimeGANv3) by Asher Chan, the styles of
  it that can be downloaded freely: `hayao`, `hayao-warm` and `hayao-vivid` (versions of Hayao
  Miyazaki's films' style), `shinkai` and `shinkai-bold` (Makoto Shinkai's), `ghibli` (Studio
  Ghibli's, from a bigger network that takes about half as long again) and `sketch` (black ink
  on white). Free for non-commercial use only: its weights aren't shipped with Flex, but
  downloaded from the author's own releases (4 MB each, 7 MB for `ghibli`) the first time a
  style is wanted.
- Fast neural style (Johnson, Alahi and Fei-Fei's network, as trained by
  [PyTorch's examples](https://github.com/pytorch/examples/tree/main/fast_neural_style)), the
  [ONNX Model Zoo](https://huggingface.co/onnxmodelzoo)'s five paintings' styles: `mosaic`
  (stained glass), `candy` (pop colours), `rain-princess` (thick brushstrokes), `udnie` (flat
  planes) and `pointilism` (dots). BSD licence; downloaded from the Model Zoo's copies (7 MB
  each) the first time a style is wanted. About as fast as AnimeGANv3's styles.
- [Informative Drawings](https://github.com/carolineec/informative-drawings) (Chan, Durand and
  Isola) for line drawings: `lines` (contours) and `lines-anime` (looser, anime-style lines),
  dark lines on white, the same in all three channels. MIT licence; downloaded from the copy
  ControlNet's annotators use (17 MB each) the first time a style is wanted. It's about
  2.4 times the work a pixel, so it's drawn smaller.
- Painting in the style of any picture, by Magenta's
  [arbitrary image stylization](https://www.kaggle.com/models/google/arbitrary-image-stylization-v1)
  (Ghiasi, Lee, Kudlur, Dumoulin and Shlens; Apache-2.0 licence): `art:<painting>` for one of
  the twenty-one public-domain paintings Flex knows (`art:starry-night`, `art:great-wave`,
  `art:scream` and the rest in `effects/paint-like.toml`), `art:morph` to drift from one to
  the next along the film (ten seconds each, five blending), and `art:morph-mine` through the
  pictures in the styles folder (`styles` in the data folder) only. Every choice setting that
  offers an `art:` style also offers each picture in the styles folder, as
  `art:mine:<file name>`, and the effect panel shows a row that opens the folder: Flex turns
  each new picture into a style once (a third of a second on the CPU) and remembers it. The
  model (86 MB) is downloaded from Kaggle Models the first time an `art:` style is wanted. It's
  about 3.1 times the work a pixel of AnimeGANv3's, so it's drawn smaller.
- Living textures, `tex-honeycomb`, `tex-cobwebs`, `tex-bubbles`, `tex-swirls`, `tex-marble`,
  `tex-cracked`, `tex-chrome`, `tex-fingerprints`, `tex-candy`, `tex-zebra`, `tex-foam`,
  `tex-mondrian`, `tex-leopard`, `tex-feathers`, `tex-scales`, `tex-eyes`, `tex-staring`,
  `tex-maze`, `tex-coils` and `tex-cells`: not a network redrawing each frame but the neural
  cellular automata of [Self-Organising Textures](https://distill.pub/selforg/2021/textures)
  (Niklasson, Mordvintsev, Randazzo and Levin; CC BY 4.0), grown on the GPU over the picture a
  few steps every frame, fed a little of the film's colours and carried along with its movement.
  Their weights (all of them, 0.4 MB) are downloaded from the article the first time one is
  wanted.

```toml
restyle = "style"

[[settings]]
key = "style"
label = "Style"
type = "choice"
default = "hayao"
options = [{ id = "hayao", label = "Hayao" }, { id = "shinkai", label = "Shinkai" }]
```

`u_video` is then the redrawn picture, once the first one has come (`u_has_restyled` is 1;
until then it's the real one), and `u_original` the real picture, so an effect can show the
drawing as it is, mix it with the film, or draw on top of it. The network runs on the GPU as
compute shaders, up to once for every frame of the film, at about 180,000 pixels in the
picture's own shape, smaller if it can't keep up. In between, the nearest drawing is moved
along with what's moving in the picture, and the picture and sound are held back the few
frames it takes, as for depth. Only one style runs at a time; F2 and F3 show which, how long
it takes and at what size.

```glsl
uniform sampler2D u_original;  // the real picture (u_video is the drawing)
uniform float u_has_restyled;  // 1 once there's a drawing
uniform vec2 u_restyled_size;  // the pixels the network drew it at (it's scaled up), or 0
```

### People, walls and sky

With `segment = true`, a small network that runs on top of the depth model (Flex's own head on
MoGe-2's features) tells people, walls, sky and the rest apart, so an effect can use them
alongside `depth_at` without paying for depth twice (it adds about 2 ms to a depth map). It was
trained on COCO, ADE20K, ATR, DensePose, CIHP and anime pictures. People and sky are solid,
including in animation and crowds; walls are the weakest, so anything flat and dark behind
someone can be called a wall, and their edges are worth treating as soft. It also tells which
person each person's pixel belongs to (from where it says each person's middle is), so
`person_there` and the `u_person_` arrays can keep people apart even when they overlap, and
find their heads, hands and feet; small or far-off people can be missed or run together. Black bars are left
out before it looks, as they'd otherwise pass for sky.

### Faces

With `faces = true`, Flex finds up to eight faces with
[YuNet](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet) (MIT), a
small network that runs on the GPU like depth does and comes with Flex. It's cheap, and looks
at up to about 30 frames a second. Each frame is drawn with the faces found in it: as with
depth, the picture and sound are held back a moment, usually under a fifth of a second, which
also lets Flex steady each face using the frames either side. A face keeps its slot in the arrays while it's on screen. It works best on
live action; very small faces, faces in profile or turned away, and cartoon characters are
missed some of the time, so effects need to look fine with no faces at all.

With `mesh = true`, Flex also looks closer at each frame's faces, on that same frame: up to
three of the largest get a 478-point mesh from MediaPipe's
[Face Mesh V2](https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker) (Apache
2.0), which gives how open the eyes and mouth are too, and a
[head pose network](https://github.com/yakhyo/head-pose-estimation) (MIT) says which way heads
point, taking turns if there are several. Mesh costs more GPU time than `faces` alone, so only
ask for it if you'll use it. The mesh gives up on faces turned far to the side, so check
`u_face_mesh`; the direction carries on working there, and is left out for the backs of heads.

The mesh is also drawn into maps of the picture for every frame, so a shader can ask about any
pixel: `on_face`, the parts of the face (`skin_at`, `lips_at` and so on), and `face_uv`, which is
the same place on every face however it's turned. Painting in `face_uv` keeps things stuck to the
skin. `face_texture` gives a face's own pixels at any such position, for moving a face to
somewhere else. They only cover the face itself, from hairline to chin, and only faces with a
mesh; `meshed(face)` says whether a slot has one, so an effect can fall back on `u_faces`.

### Shot and scene

With `scene = true`, an effect is told what sort of shot it's in and what's been going on across
the last few, worked out from depth, masks, faces, movement and the sound, so all of those run.
`u_shot_size` goes from 0 (very wide) to 1 (extreme close-up). `u_camera` is pan, tilt, zoom and
roll a second: turning right, turning up, the picture growing and turning clockwise. `u_handheld`
goes from 0 to 1, `u_pace` is cuts a minute, and `u_scene` is action, dialogue, quiet and montage,
each from 0 to 1. Scene scores only look backward, so they take a while to catch up with a new
scene, and two people talking in one long take reads as quiet. The Shot and Scene effect shows
them all, and F3 prints them.

### Pinning things to the world

With `camera = true`, Flex follows points through each shot and works out where the camera is, so
an effect can keep something in one place as the camera moves. `world_at(uv)` says where a point on
screen is in the world, and `world_to_screen(world)` where a world point is on screen, its `z` being
how far away it is. Each shot has a world of its own, with a typical distance of about 1. The first
second of a shot goes on working out its lens, so `u_world_ready` stays 0 until then, fades up to 1,
and drops back to 0 for a moment if the tracking is lost: multiply what you draw by it.
`world_pin(uv, distance)` is the simplest way in: give it a screen position and how far away the
thing is, and it says where that thing is on screen now. `shot_uv(uv)` says where a point on screen
was when the shot began, so a pattern drawn there sticks to what it's on. `world_pin_nearby` is
`world_pin` for one thing that shouldn't be lost: it comes back into view when the camera leaves it. `world_distance(near)`
turns a depth value into a distance. `screen_size(size, away)` is how tall something `size` big in the world
looks `away` from the camera, as a share of the screen's height. `world_ray(uv, origin, direction)` gives the camera's place
and the line of sight through a point, for testing what it passes in front of, and
`world_point(uv, near)` is `world_at` for a depth of your own, such as one from `u_ground`. The
world's y points down. None of them mean anything until `u_world_ready` is above 0, but
`world_pin`, `world_pin_nearby` and `shot_uv` just give back `uv` then, so an effect built on them
still works without the camera. Underneath are `mat3 u_world_rotation` and `vec3 u_world_offset`
(world to camera: `u_world_rotation * world + u_world_offset`), `vec3 u_world_lens` (the focal
length in x, and the size of the picture the camera was tracked in, in y and z, all in pixels) and
`vec2 u_world_depth` (a `depth_at` value `near` is `1 / (x * near + y)` away). It needs
something in the picture to hold on to, so plain walls and fast action lose it. The World Grid
effect shows how well it's doing.

### Black bars

Effects only see the picture. Black bars are left out, whether they're there to fit the window
or are part of the file (Flex finds those from the frames, so they take a moment), and `uv` runs
from 0 to 1 over the picture alone. Subtitles can still sit in the bars.

### Subtitles

With Flex's patched libmpv (`packaging/mpv`), subtitles are kept out of `u_video` and laid over
the finished effect, so effects never distort them. With an ordinary libmpv, subtitles go through
the effect as part of the picture.

## Testing

- While a video plays, Flex checks the shader twice a second and reloads it when it
  changes. If it doesn't compile, the video plays without the effect, Settings → Picture
  → Video effects shows the first error, and the whole compiler output goes to Flex's
  log (Settings → "Open logs folder").
- F3 while a video plays opens a window for choosing the effect and changing its
  settings with sliders while you watch (plus Anime4K and SVP), with meters for the
  sound levels and what kind of sound it hears. Changes are kept, the same as in Settings.
- F2 shows which effect is on, the time it's given, and the sound levels.
- `flex --windowed some-video.mkv` starts Flex straight into a video.
