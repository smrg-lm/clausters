# The GUI host's configuration

Every key the **`clausters-gui`** host reads: the `[gui]` section (its ports and
its two legs), the `[gui.theme]` and `[gui.metrics]` role tables (its whole
look and its whole sizing) and the `[standalone]` section (the self-contained
app). The reader is the host binary — including the one `Session.gui()` or
`GuiHost().boot()` launches — so a look you set here is the look your script's
windows open with, with nothing to say in the script.

Where the file lives and how the user and project layers merge are on the
[Configuration](../configuration.md) page; the precedence is the same
everywhere:

```
command-line flag  >  project clausters.toml  >  user config.toml  >  built-in default
```

Every key is optional, and unknown keys are ignored.

## `[gui]`

| Key | Type | Default | Flag | What it sets |
| --- | --- | --- | --- | --- |
| `host_port` | integer | `57210` | `--port` | The port of the host's script-facing front (UDP and TCP alike) |
| `tcp` | boolean or port | `true` — on at `host_port` | `--tcp [port]` / `--no-tcp` | The front's TCP leg, which is how a `/gui_def` tree escapes the datagram limit (and the Python client's default carrier) |
| `ws` | boolean or port | off; `true` means `57220` | `--ws [port]` | The front's WebSocket leg — a page driving a native host |
| `max_frame` | integer (bytes) | `16777216` (16 MiB) | `--max-frame` | Largest OSC frame on the stream legs, TCP and WebSocket alike |
| `server` | string `host:port` | off | `--server` | Also attach the **client leg** to a running audio server: what a `buffer`-fed waveform and a bound widget need |
| `shm` | string (path) | off | `--shm` | Map the audio server's shared-memory segment (its own `--shm` path) for zero-message meters, scopes and playheads |
| `data_dir` | string (path) | the same place the server uses | `--data-dir` | The GuiDef store: where a named GuiDef persists and `/gui_load` reads from |
| `headless` | boolean | `false` | `--headless` | Run the protocol with no display (tests, machines with no GPU) |
| `theme` | table | the built-in dark theme | `--theme <path>` | Color-role overrides — [the table below](#guitheme) |
| `metrics` | table | the generated table | — | Size-role overrides — [the table below](#guimetrics) |

Two flags have no configuration key: `--standalone [name]` (which reads
`[standalone]`, below) and `--config <path>`, which replaces the whole
user+project chain with one file. Verbosity is the server's: `-v`/`-vv`/`-vvv`,
`-q`, `RUST_LOG`.

From Python you rarely write `server` or `shm`: `Session.gui()` points the
client leg at the session's own server and maps the segment that server was
launched with, and it passes `--port` itself. What the file is for is everything a script should not have to know — the
look, the sizing, the data directory.

## `[gui.theme]`

A **partial** table of `role = "#rrggbb"` or `"#rrggbbaa"`: listed roles are
overlaid on the built-in dark theme, unlisted ones keep it. An unknown role or
an unparseable color warns and is skipped — a style file written for a newer
host still loads.

```toml
[gui.theme]
accent = "#ff8c40"       # the color that carries a widget's function
background = "#101014"
text = "#e8e8ec"
```

`--theme <path>` reads the same flat table from a free-standing file (no
`[gui.theme]` header needed), laid over the section, so a look travels as one
file. The same table also cascades **over the wire**: a container's `theme` prop
scopes an overlay to its subtree and any widget's `color` prop re-seeds the
roles that carry its function — see the widget protocol in the
[Clausters server book](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
A host running in a browser tab has no config file, so the page passes the same
table as JSON to `GuiBridge.theme` (and its sizing sibling `GuiBridge.metrics`)
— see the [web client book](https://clausters-web.readthedocs.io/).

The 66 roles, named by **function** rather than by widget, so one entry restyles
everything that means the same thing by it. The defaults are written here as the
hex a file would use; the host holds them as floats, so a value copied back is
the same color to within the 8 bits of the notation:

| Role | Default | What it colors |
| --- | --- | --- |
| `background` | `#0d0d12` | The window clear color, the backdrop under everything. |
| `panel` | `#1a1c248c` | A panel's translucent fill over the backdrop. |
| `text` | `#d9dee6` | Primary text: labels, values, readouts. |
| `text_dim` | `#8c99a8` | De-emphasized text (the node tree's parameter lines). |
| `label_dim` | `#99a1b2` | De-emphasized in-view labels (a lane's name tag). |
| `field` | `#242630` | A control's body fill (slider, knob, scope field). |
| `track` | `#1a1c24` | The inset groove/body under a control's value (a slider's track), and the body of the static info views (node tree, plot). |
| `accent` | `#4cc78c` | The color that carries a widget's function: a slider's fill, a knob's pointer, a meter's bar, the live views' frame. |
| `accent_dim` | `#388066` | The accent's quiet form (an unlit toggle, a knob's arc). |
| `hilite` | `#66d99e` | The accent's lit form (a pressed control, a window's edge marker). |
| `trace` | `#66d99e` | A drawn signal or curve (scope trace, bpf curve, automation curve). |
| `trace_bright` | `#73e6a8` | The brighter trace of the phase scope's beam. |
| `point` | `#e6edf2` | A curve's grabbable break-point. |
| `frame` | `#4c576b` | The neutral frame of the editor views (piano roll, tracks, patch). |
| `view_frame` | `#407361` | The frame of the timeline views (waveform, spectrogram). |
| `frame_info` | `#4c7399` | The frame of the live info view (the node tree). |
| `frame_plot` | `#738cb2` | The frame of the measuring plot. |
| `view_field` | `#14171c` | The dark body of the heavy views (waveform, spectrogram, patch). |
| `lane` | `#171a21` | A lane's background (a track lane, the piano roll's grid). |
| `header` | `#242933` | A track's header strip. |
| `lane_alt` | `#12141a` | The alternate, darker lane (black-key rows, the velocity lane). |
| `lane_divider` | `#4c5461cc` | The divider line between stacked lanes. |
| `grid` | `#4c576699` | A view's reference grid (the phase scope's cross and square). |
| `grid_line` | `#292e38` | A fine grid line (the piano roll's row lines). |
| `baseline` | `#475261` | The zero baseline of a value axis. |
| `focus` | `#73b2ffe6` | The ring around the widget holding the keyboard focus. Its own role rather than the accent: what is focused and what is *active* are two different questions, and a window answers both at once. |
| `selection` | `#8cbff2` | The selection color; fills and edges derive by alpha. |
| `playhead` | `#f28c4ce6` | The playhead line. |
| `ruler_text` | `#a6adb8` | Ruler tick labels. |
| `ruler_line` | `#737a85` | Ruler tick lines. |
| `object_fill` | `#293852` | A placed object's fill (a clip's body, a patch member box). |
| `object_edge` | `#7399d9` | A placed object's edge. |
| `box_fill` | `#f2f5f7` | A patch box's central band (the def-name band): **white**, so the name reads as black text on white (`box_text`) and the box stands out on the dark canvas. Distinct from `object_fill` (a clip's body) — a patch box is its own thing. |
| `box_text` | `#171a1f` | A patch box's central-band text (the def name) — near-black, for the white `box_fill` band. |
| `value_fill` | `#faf0cc` | A patch **value** box's central band (a `const` literal / parameter box, set apart from the white UGen boxes so a data box reads as data): a pale cream, still black-text (`box_text`). |
| `port_strip` | `#26292e` | A patch box's port strips (top inlets and bottom outlets, one color) — dark grey, framing the white middle band (`box_fill`). |
| `port` | `#bfd1eb` | A patch box's wiring port (the cell a cord connects to: its edge and pin). |
| `cord` | `#f25752` | A patch **audio** (`ar`) cord — the signal path. The rate reads by **colour** (all cords share one weight): the three are contrasting primaries — audio **red**, control **blue**, init **yellow** — legible against each other and the dark field. |
| `cord_control` | `#478ffa` | A patch **control** (`kr`) cord (blue). |
| `cord_init` | `#fac733` | A patch **init** (`ir`) cord — a scalar wire, also drawn dashed (yellow). |
| `live` | `#f2b840` | The live/rendered marker (a sounding patch wire). |
| `note_fill` | `#8ccc9e` | A note's fill in the piano roll. |
| `note_edge` | `#c7f2d1` | A note's edge. |
| `selected_fill` | `#cce6fa` | A selected note's fill. |
| `selected_edge` | `#ffffff` | A selected note's edge. |
| `velocity` | `#b28ce6` | A velocity bar. |
| `event_lane` | `#1a1721` | The OSC event lane's background. |
| `flag` | `#f2bf73` | An event marker flag (an OSC marker, an overview's pressed key). |
| `trigger` | `#d9cc6666` | The oscilloscope's trigger-level line. |
| `warn` | `#d96b6b` | The negative/warning readout (the phase scope's anti-correlation). |
| `key_white` | `#dbdee6` | A playable white key. |
| `key_white_dim` | `#d1d6e0` | The piano roll's dimmer white key. |
| `key_black` | `#1a1c24` | A black key. |
| `key_pressed` | `#73cc99` | A pressed white key. |
| `key_pressed_black` | `#40996b` | A pressed black key. |
| `key_inactive` | `#6b6e75` | An inactive (out-of-range) white key. |
| `key_inactive_black` | `#3d4047` | An inactive black key. |
| `key_gap` | `#0d0f14` | The gap/edge line between keys. |
| `key_label` | `#595e6b` | The octave label on a playable key. |
| `key_label_dim` | `#4c5261` | The octave label in the piano roll's key gutter. |
| `key_overview` | `#12141a` | The keyboard overview strip's background. |
| `key_overview_active` | `#292e38` | The overview's active-range band. |
| `key_overview_black` | `#0a0d0f` | The overview's black-key marks. |
| `series_1` | `#4cc78c` | Channel 1 of the series palette (the classic mono trace). |
| `series_2` | `#f2b840` | Channel 2 of the series palette. |
| `series_3` | `#73a6f2` | Channel 3 of the series palette. |
| `series_4` | `#e67399` | Channel 4 of the series palette. |

## `[gui.metrics]`

The sizing counterpart, with the same partial-overlay semantics: `role = number`
in **logical** pixels (glyph scales for the text roles), unlisted roles keep
their generated default, and an unknown role or an unusable number warns and is
skipped.

One reserved key comes first:

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `scale` | number | `1.0` | The **density**: it regenerates the whole table at that multiplier (below 1 compact, above 1 comfortable) before any explicit role applies, whatever order the entries are written in |

`scale` is deliberately the *whole* density surface — a host has one density the
way it has one look, so there is nothing to set per widget and nothing on the
wire. The defaults are not invented per role either: they are generated from one
quantized modular scale over the font cell (14 px), which is what makes a
button, a number field and a menu line up in a row unaided.

```toml
[gui.metrics]
scale = 0.9              # a compact host, every role regenerated
control_h = 26           # ...except this one, set outright
```

The 24 roles, with the value each takes at `scale = 1.0`:

| Role | Default | What it sizes |
| --- | --- | --- |
| `pad` | 4 | Inside a widget, between its frame and its content. |
| `gap` | 6 | Between two siblings of a container. |
| `margin` | 6 | Inside a container, before its children. |
| `indent` | 14 | One nesting level of an indented list (the node tree's children). |
| `control_h` | 22 | The height of one line of control: a cell of text and its padding — what makes a button, a number field and a menu line up in a row. |
| `row_h` | 28 | The pitch of one row in a list of controls (a control plus a gap). |
| `track_thick` | 4 | The thickness of a control's groove across its axis (a slider's track). |
| `handle_thick` | 8 | The thickness of the value riding that groove (a slider's handle). |
| `handle_grip` | 18 | The length of a handle across the control's axis (its grip). |
| `box_side` | 24 | The side of a square marker (a toggle's box). |
| `knob_d` | 48 | The diameter of a round control (a knob): **two lines of control**, not a box-sized marker. A dial is read by its angle, so it needs the sweep to be legible — and a disc reads smaller than a box of the same bounding rect, which is why it is its own role rather than `control_h` twice over. |
| `ruler_h` | 18 | The height of a ruler strip along a horizontal axis. |
| `ruler_w` | 46 | The width of a ruler strip beside a vertical axis (sized for its widest labels). |
| `header_w` | 96 | The width of a row's header column (a lane's name and controls). |
| `divider_w` | 1 | A hairline: a divider between lanes, a box edge. |
| `focus_ring` | 2 | The weight of the ring around the widget holding the keyboard focus. Heavier than a hairline on purpose: it has to read *over* the edge a control already draws. |
| `trace_w` | 1.5 | The weight of a drawn signal trace. |
| `point_radius` | 4 | The radius of a placed point (a break-point, an automation node). |
| `hit_slop` | 4 | The slack around a small target's geometry, so it stays clickable. |
| `label_gap` | 14 | The smallest gap between two ruler labels before the ladder steps up. |
| `tick_gap` | 7 | The smallest gap between two ruler ticks before the ladder steps up. |
| `text_scale` | 2 | Primary text: control labels, values, readouts. |
| `label_scale` | 2 | In-view labels drawn over a surface (a lane's name tag). |
| `caption_scale` | 1.5 | Reduced text: ruler labels, a clip's caption. |
| `micro_scale` | 1 | The densest legible mark (a key's octave label). |

Sizes here are the host's **shared vocabulary**, not every number it draws: a
widget's own interlocking geometry (the patcher's box and port series, the piano
roll's key gutter, the score's staff step) stays inside that widget, and so do
sub-pad optical nudges. A role earns its place when more than one widget means
the same thing by it.

The numbers are **logical**, the same units a GuiDef's own `w`/`h`/`x`/`y`/
`margin`/`gap`/`text_size` carry: each window resolves the table to its display's
physical pixels — every role scaled and re-quantized so hairlines stay hairlines
— once when its scale changes, never per frame. Density and display scale are
therefore two different knobs on one table: `scale` is the look you chose,
the window's scale is the screen you are on.

Two roles are worth knowing because they decide how a window falls out rather
than how it looks: `control_h` and the text scales feed every widget's **natural
size** — how tall a control asks to be when a `row`/`col` gives it no explicit
`h` — so a density change re-flows the layout, it does not just recolor it.

## `[standalone]`

The self-contained app: a saved GuiDef plus its defs, booted against an embedded
audio server with no language interpreter in the picture.

| Key | Type | Default | Flag | What it sets |
| --- | --- | --- | --- | --- |
| `gui` | string | — | `--standalone <name>` | The saved GuiDef to open when `--standalone` is given no name |
| `boot` | boolean | `true`; only `false` means anything | — | Run the GuiDef's own `boot` messages and the data directory's `boot.json` preset |
| `data_dir` | string (path) | `[gui].data_dir`, else the server's data directory | `--data-dir` | Where the bundle lives. Read **only** under `--standalone`, so a bundle can live apart from the GuiDef store the same host uses otherwise |

With `gui` and `data_dir` set, the launch is just `clausters-gui --standalone`.
Two things to know before it runs: the mode links the audio server into the host,
so it needs a binary built with the `standalone` feature (the one bundled in the
Python wheel is not, unless you rebuilt it asking for that feature), and the
bundle itself — the GuiDef with its SynthDefs, GraphDefs, FaustDefs and its
optional `boot.json` — is written from Python. See [Bundles](../bundles.md).

## A worked file

```toml
[gui]
host_port = 57210
ws = true                      # a browser can drive this host too
shm = "/dev/shm/clausters"     # meters and scopes with no messages
data_dir = "/srv/clausters"

[gui.theme]
accent = "#ff8c40"
selection = "#ffb37a"
playhead = "#4cc7f2"

[gui.metrics]
scale = 1.15                   # a comfortable host on a large screen
knob_d = 64                    # ...with bigger dials than the density gives

[standalone]
gui = "drone"
```
