# Making a Flex theme

A theme is one TOML file of colours and styles, plus any pictures or font it uses. The
built-in ones are in [`crates/flex-app/themes`](crates/flex-app/themes) and are written
in exactly the same format, so the easiest way to start is to copy the one closest to
what you want and change it.

## The folder

Each custom theme is a folder with a `theme.toml` in it, inside the `themes` folder in
Flex's data folder (`%APPDATA%\Flex\data\themes` on Windows,
`~/Library/Application Support/Flex/themes` on a Mac; Settings → Theme → "Open themes
folder" takes you there):

```
themes/
  my-theme/
    theme.toml
    background.png     # optional
    pointer.png        # optional
    font.ttf           # optional
```

The folder's name is the theme's id. A folder with the same name as a built-in theme
(`dark`, `ff7`, `osrs`, `brick`, `pokemon`, `win95`, `winxp`, `aqua`, `terminal`, `templeos`,
`farmyard`) replaces it. After adding one, use "Look for new themes" in Settings →
Theme. If a theme has a mistake in it, Flex leaves it out of the list and says why in
its log.

While you're working on one, `FLEX_THEME=my-theme flex` starts Flex in it without
saving it as your choice.

## theme.toml

Colours are `"#rrggbb"`, or `"#rrggbbaa"` to make them see-through. Only `name` and the
first block of colours are required; everything else has a sensible default.

```toml
name = "My theme"                  # shown in Settings
description = "What it looks like"
dark = true                        # false gives text boxes light styling

[colours]
background = "#0e0f12"             # behind everything
text = "#ebebeb"                   # titles and captions on the background
dim = "#9696a0"                    # quieter text on the background
panel = "#1c1e24"                  # rows, buttons and cards
panel_text = "#ebebeb"             # text on panels (defaults to `text`)
panel_dim = "#9696a0"              # quieter text on panels (defaults to `dim`)
accent = "#ebaa28"                 # focus
on_accent = "#000000"              # text on an accent-filled panel
highlight = "#ebaa28"              # a focused poster and its title (defaults to `accent`)
progress = "#ebaa28"               # progress bars (defaults to `accent`)
overlay = "#000000be"              # behind messages and the player's controls
overlay_text = "#ebebeb"           # text on those (defaults to `text`)
```

### Panels

How rows, buttons and cards are drawn. They're drawn in this order: the fill, the
gloss, the bevel, then the borders.

```toml
[panel]
radius = 8                         # corner rounding; 0 for square
gradient = "#0a1450"               # fade from `panel` at the top to this at the bottom
border = { width = 3, colour = "#d0d0dc" }
inner_border = { width = 1, colour = "#50506a" }
inner_gap = 0                      # space between the two borders
bevel = { light = "#ffffff", dark = "#000000", width = 2 }   # raised 3D edges
gloss = "#ffffffb0"                # a shine over the top half
```

### Focus

How the focused item stands out.

```toml
[focus]
style = "fill"                     # "fill", "outline" or "pointer"
gradient = "#1764d8"               # for fill: fade from `accent` to this
ring = { width = 3, colour = "#ffffff" }   # a ring inside the focused panel
pointer = "hand"                   # "hand", "triangle", "hen", or a picture file
pointer_colour = "#f4f4f4"         # for "hand" and "triangle"
```

- `fill` fills the focused panel with the accent colour, and its text turns `on_accent`.
- `outline` keeps the panel as it is, with an accent border and accent text.
- `pointer` keeps it as it is and points at it from the left.

A pointer works with `fill` and `outline` too: set `pointer` and both are drawn. A
pointer picture is drawn 40 points tall with its right edge at the item, so point it to
the right.

### Titles

```toml
[header]
style = "plain"                    # or "bar", for a title bar behind each screen's title
colour = "#3a8cf6"                 # the bar's colour (defaults to `accent`)
gradient = "#0646cf"               # fade the bar to this at the bottom
radius = 8                         # the bar's corner rounding
text = "#ffffff"                   # the title's colour
prefix = "> "                      # put before every title
suffix = "_"                       # and after it
```

### Background

```toml
[background]
gradient = "#080818"               # fade from `background` at the top to this
pattern = "stone"                  # see below
image = "background.png"           # a picture covering the screen, cropped to fit
hill = "#74c043"                   # colours for the "hills" pattern
hill_bottom = "#2c7a1a"
```

The patterns are drawn in code, using the background colour:

| Pattern | Looks like |
|---|---|
| `stone` | uneven stone blocks (Brick) |
| `osrs` | a worn brown panel, framed by stone blocks along the top and bottom and a pillar down each side, in the screen's edges (Old School RuneScape) |
| `pinstripes` | fine horizontal stripes (Aqua) |
| `rain` | columns of falling green code (Terminal) |
| `hills` | rolling hills under soft clouds, in `hill` and `hill_bottom` (Windows XP) |
| `farmyard` | sky, blocky clouds, climbing plants, and a strip of ground with hens and ducks along the bottom (Maisy's Farm) |

A picture is drawn over the gradient and pattern, so a see-through PNG works as a frame
or overlay.

### Messages and the player's controls

Normally a rounded box in the `overlay` colour. An `[overlay]` section draws them like
panels instead, with the same keys as `[panel]`, filled with `overlay`:

```toml
[overlay]
radius = 0
gradient = "#bfae86"
border = { width = 3, colour = "#2b2318" }
inner_border = { width = 2, colour = "#86745a" }
```

### Text

```toml
[text]
shadow = "#000000"                 # a shadow behind text
shadow_offset = 2                  # how far down and right it falls
panel_shadow = true                # whether text on panels gets it too
font = "font.ttf"                  # a font file in the theme's folder, or "monospace"
```

Flex falls back to its usual fonts for characters a theme's font doesn't have (Japanese
titles, for instance).

## Tips

- Check your theme on a few screens: Home, a library's posters, a list such as
  Settings, and a video with its controls showing. Text on the background (`text`,
  `dim`, `highlight`) has to read against the background's busiest part.
- The edges of the screen are kept clear in case the TV crops them, and a pointer sits
  in that margin, so it should be at most about 70 points wide.
- Keep pictures modest in size: a 1920×1080 background is plenty.
