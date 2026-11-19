# The GUI host's configuration

Every key the **`clausters-gui`** host reads: the `[gui]` section (its ports and
its two legs), the `[gui.theme]` and `[gui.metrics]` role tables (its whole
look and its whole sizing), the `[gui.keys]` table (which key performs which
verb) and the `[standalone]` section (the self-contained app). The reader is the host binary — including the one `Session.gui()` or
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
| `tcp` | boolean, port or bind | `true` — on at `host_port`, on loopback | `--tcp [[addr:]port]` / `--no-tcp` | The front's TCP leg, which is how a `/gui_def` tree escapes the datagram limit (and the Python client's default carrier) |
| `ws` | boolean, port or bind | off; `true` means `57220`, on loopback | `--ws [[addr:]port]` | The front's WebSocket leg — a page driving a native host |
| `max_frame` | integer (bytes) | `16777216` (16 MiB) | `--max-frame` | Largest OSC frame on the stream legs, TCP and WebSocket alike |
| `server` | string `host:port` | off | `--server` | Also attach the **client leg** to a running audio server: what a `buffer`-fed waveform and a bound widget need |
| `shm` | string (path) | off | `--shm` | Map the audio server's shared-memory segment (its own `--shm` path) for zero-message meters, scopes and playheads |
| `data_dir` | string (path) | the same place the server uses | `--data-dir` | The GuiDef store: where a named GuiDef persists and `/gui_load` reads from |
| `headless` | boolean | `false` | `--headless` | Show no window: draw each one offscreen, to be captured and played to from a script, on any adapter a software one included; with none at all, run the protocol alone (tests, machines with no GPU) |
| `font` | string (path) | off — the host's own face | `--font` | The typeface to draw text with, on a host built with a rasterizer (its optional `font-atlas` feature); with the feature and no path, one of the system's faces. Any other build draws its embedded bitmap face and ignores this |
| `msaa` | integer | `1` — off | `--msaa` | Antialiasing: the MSAA sample count of every window's render pass (`4` is the usual smoothing). One multisampled attachment per window and nothing per widget; a count the GPU does not offer for the surface format falls back to `1` |
| `theme` | table | the built-in dark theme | `--theme <path>` | Color-role overrides — [the table below](#guitheme) |
| `metrics` | table | the generated table | — | Size-role overrides — [the table below](#guimetrics) |

Every leg binds **loopback** until an address says otherwise, the audio
server's own rule: `tcp`, `ws` and the flags that override them take a bind
(`"0.0.0.0:57220"`, `"0.0.0.0"`, `"[::1]:57210"`), and `--udp [addr:]port`
moves the UDP leg by itself. A host reachable from another machine is therefore
something written down — worth knowing before you write it, since two things
degrade over that distance and neither is a bind: a source's `path` carrier
writes a file the host maps, so client and host are assumed to share a
filesystem, and `data_dir` names the *host's* GuiDef store.

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
overlaid on the host's look, unlisted ones keep it. An unknown role or an
unparseable color warns and is skipped — a style file written for a newer host
still loads.

The host has **two looks**, dark (the default) and light, both from the
design system its look is specified from: its neutral set, and the domain
colors (traces, meters, keys, code) chosen to read on each ground. One
reserved key picks one:

| Key | Type | Default | What it does |
| --- | --- | --- | --- |
| `look` | `"dark"` or `"light"` | `"dark"` | Puts the whole theme back to that look before any role the table names applies, whatever order the entries are written in — what `scale` is to the sizes |

```toml
[gui.theme]
look = "light"
accent = "#ff8c40"       # the color that carries a widget's function
```

`--theme <path>` reads the same flat table from a free-standing file (no
`[gui.theme]` header needed), laid over the section, so a look travels as one
file. The same table also cascades **over the wire**: a container's `theme` prop
scopes an overlay to its subtree and any widget's `color` prop re-seeds the
roles that carry its function — see the widget protocol in the
[Clausters server book](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
A host running in a browser tab has no config file, so the page sends the same
table over the wire, `GuiHost.theme` (and its sizing sibling `GuiHost.metrics`)
— see the [web client book](https://clausters-web.readthedocs.io/).

The roles are named by **function** rather than by widget, so one entry
restyles everything that means the same thing by it. The specification's own
names are accepted too and set the role they name: `foreground`,
`card_foreground` and `popover_foreground` are `text`, `popover` is
`popup`, `muted_foreground` is `text_dim`, `border` is `separator`,
`ring` is `focus`, `accent_foreground` is `hover_text`, and
`primary_foreground` and `secondary_foreground` are `primary_text` and
`secondary_text`. The one it cannot take is `accent`: here it is the color
that carries a widget's function (a slider's fill, a meter's bar), so what
the specification calls accent, the fill under the hand, is `hover`.

The values are written here as the hex a file would use; the host holds them
as floats, so a value copied back is the same color to within the 8 bits of
the notation:

| Role | Dark | Light | What it colors |
| --- | --- | --- | --- |
| `background` | `#1e1e1e` | `#ffffff` | The window clear color, the backdrop under everything. |
| `panel` | `#262626` | `#f5f5f5` | A panel's translucent fill over the backdrop. |
| `text` | `#fafafa` | `#0a0a0a` | Primary text: labels, values, readouts. |
| `text_dim` | `#a1a1a1` | `#737373` | De-emphasized text (the node tree's parameter lines). |
| `label_dim` | `#a1a1a1` | `#737373` | De-emphasized in-view labels (a lane's name tag). |
| `status_bar` | `#262626` | `#f5f5f5` | The **status bar**'s band along the window's bottom edge: a shade off the backdrop, so the strip reads as a place the window talks rather than as a panel holding something. |
| `card` | `#171717` | `#ffffff` | A **card**'s ground: a dialog's, a hover card's, a section raised off the window. |
| `primary` | `#e5e5e5` | `#171717` | The **primary** fill: the button that does what a form is for. |
| `primary_text` | `#171717` | `#fafafa` | The text on `primary`. |
| `secondary` | `#262626` | `#f5f5f5` | The **secondary** fill: a bar's ground, a secondary button, a chosen segment. |
| `secondary_text` | `#fafafa` | `#171717` | The text on `secondary`. |
| `muted` | `#262626` | `#f5f5f5` | The **muted** fill: a toggle that is on, a ghost button under the pointer, a block of code in a page. |
| `hover_text` | `#fafafa` | `#171717` | The text on `hover`: an entry of a list under the pointer or the keyboard. |
| `input` | `#ffffff26` | `#e5e5e5` | The edge of a field the hand types in. |
| `shadow` | `#0000001a` | `#0000001a` | What a raised surface casts on the window: the ink of every elevation's shadow, its alpha the strongest layer's. |
| `scrim` | `#0000001a` | `#0000001a` | What a dialog lays over the window behind it, which is out of reach while it is up. |
| `field` | `#171717` | `#ffffff` | A control's body fill (slider, knob, scope field). |
| `track` | `#171717` | `#fafafa` | The inset groove/body under a control's value (a slider's track), and the body of the static info views (node tree, plot). |
| `accent` | `#4ade80` | `#16a34a` | The color that carries a widget's function: a slider's fill, a knob's pointer, a meter's bar, the live views' frame. |
| `accent_dim` | `#404040` | `#e5e5e5` | The accent's quiet form (an unlit toggle, a knob's arc). |
| `hilite` | `#4ade80` | `#16a34a` | The accent's lit form (a pressed control, a window's edge marker). |
| `hover` | `#2e2e2e` | `#ececec` | What the pointer is over: a row of an open list, a flat button, a tab. |
| `text_disabled` | `#737373` | `#a1a1a1` | The text of something that cannot be acted on now: a disabled control, a disabled entry of a menu. |
| `popup` | `#171717` | `#ffffff` | The ground of what opens over the window: a list, a menu, a tip. Opaque, since it covers what it opened over. |
| `separator` | `#ffffff1a` | `#e5e5e5` | A line between groups: a menu's separator, a toolbar's, a group's frame. |
| `trace` | `#66d99e` | `#16a34a` | A drawn signal or curve (scope trace, bpf curve, automation curve). |
| `trace_bright` | `#73e6a8` | `#22c55e` | The brighter trace of the phase scope's beam. |
| `trace_body` | `#d9f0b29e` | `#16a34a59` | The measured body drawn inside a signal's envelope (an RMS layer). Translucent by role: a body is read *against* the peaks it sits in. |
| `trace_signal` | `#fa8c59f2` | `#fa8c59f2` | **The reconstruction** drawn over a signal's samples (the `signal` measure): the band-limited waveform between them. An ink of its own because it is read *against* the samples it is drawn over, the analog waveform in a colour apart from the digital samples, and warm so it does not read as another channel. |
| `trace_loudness` | `#8cc7faf2` | `#8cc7faf2` | **The loudness curve** drawn over a signal (the `momentary` and `short` measures): what a meter would have read at each point of the take. An ink of its own, and cool where the reconstruction is warm, because it is the one layer that does *not* measure the same axis as the picture under it -- it is read against its own LU scale and against the target line, not against the samples it crosses. |
| `trace_guide` | `#7394b88c` | `#7394b88c` | **The loudness target and the edges of its scale**: the line a delivery specification names (EBU R 128's -23 LUFS) and the bounds of the strip the curve is drawn in. Quiet, because a guide is read *behind* the curve it is a reference for. |
| `point` | `#fafafa` | `#0a0a0a` | A curve's grabbable break-point. |
| `frame` | `#ffffff26` | `#e5e5e5` | The neutral frame of the editor views (piano roll, tracks, patch). |
| `view_frame` | `#407361` | `#407361` | The frame of the timeline views (waveform, spectrogram). |
| `frame_info` | `#4c7399` | `#4c7399` | The frame of the live info view (the node tree). |
| `frame_plot` | `#738cb2` | `#738cb2` | The frame of the measuring plot. |
| `view_field` | `#1e1e1e` | `#ffffff` | The dark body of the heavy views (waveform, spectrogram, patch). |
| `lane` | `#1e1e1e` | `#ffffff` | A lane's background (a track lane, the piano roll's grid). |
| `header` | `#262626` | `#f5f5f5` | A track's header strip. |
| `lane_alt` | `#191919` | `#f5f5f5` | The alternate, darker lane (black-key rows, the velocity lane). |
| `channel_divider` | `#ffffff26` | `#e5e5e5` | The divider line between stacked channels. |
| `grid` | `#ffffff26` | `#0000001a` | A view's reference grid (the phase scope's cross and square). |
| `grid_line` | `#2a2a2a` | `#ececec` | A fine grid line (the piano roll's row lines). |
| `baseline` | `#404040` | `#d4d4d4` | The zero baseline of a value axis. |
| `plate` | `#0a0a0a9e` | `#ffffffb3` | The ground a **text plate** lays under a caption drawn over a picture (a clip's name over its take, a roll's cursor read-out): a translucent dark box, so the text reads without hiding what it is written on. Its alpha is the whole role. |
| `focus` | `#737373` | `#a1a1a1` | The ring around the widget holding the keyboard focus. Its own role rather than the accent: what is focused and what is *active* are two different questions, and a window answers both at once. |
| `selection` | `#60a5fa` | `#3b82f6` | The selection color; fills and edges derive by alpha. |
| `playhead` | `#f28c4ce6` | `#f28c4ce6` | The playhead line — the one that **sweeps**, whose position is the engine's. |
| `cursor` | `#e5e5e5` | `#171717` | The **position cursor**: the line a click on the ruler places, where a playback starts and a paste lands. |
| `ruler_text` | `#a1a1a1` | `#737373` | Ruler tick labels. |
| `ruler_line` | `#737373` | `#a1a1a1` | Ruler tick lines. |
| `object_fill` | `#293852` | `#dbeafe` | A placed object's fill (a clip's body, a patch member box). |
| `object_edge` | `#7399d9` | `#3b82f6` | A placed object's edge. |
| `box_fill` | `#f2f5f7` | `#f2f5f7` | A patch box's central band (the def-name band): **white**, so the name reads as black text on white (`box_text`) and the box stands out on the dark canvas. Distinct from `object_fill` (a clip's body) — a patch box is its own thing. |
| `box_text` | `#171a1f` | `#171a1f` | A patch box's central-band text (the def name) — near-black, for the white `box_fill` band. |
| `value_fill` | `#faf0cc` | `#faf0cc` | A patch **value** box's central band (a `const` literal / parameter box, set apart from the white UGen boxes so a data box reads as data): a pale cream, still black-text (`box_text`). |
| `port_strip` | `#262626` | `#e5e5e5` | A patch box's port strips (top inlets and bottom outlets, one color) — dark grey, framing the white middle band (`box_fill`). |
| `port` | `#bfd1eb` | `#737373` | A patch box's wiring port (the cell a cord connects to: its edge and pin). |
| `cord` | `#f25752` | `#f25752` | A patch **audio** (`ar`) cord — the signal path. The rate reads by **colour** (all cords share one weight): the three are contrasting primaries — audio **red**, control **blue**, init **yellow** — legible against each other and the dark field. |
| `cord_control` | `#478ffa` | `#478ffa` | A patch **control** (`kr`) cord (blue). |
| `cord_init` | `#fac733` | `#fac733` | A patch **init** (`ir`) cord — a scalar wire, also drawn dashed (yellow). |
| `live` | `#f2b840` | `#f2b840` | The live/rendered marker (a sounding patch wire). |
| `note_fill` | `#8ccc9e` | `#8ccc9e` | A note's fill in the piano roll. |
| `note_edge` | `#c7f2d1` | `#16a34a` | A note's edge. |
| `selected_fill` | `#cce6fa` | `#cce6fa` | A selected note's fill. |
| `selected_edge` | `#ffffff` | `#171717` | A selected note's edge. |
| `velocity` | `#b28ce6` | `#b28ce6` | A velocity bar. |
| `osc_markers` | `#171717` | `#f5f5f5` | The OSC markers' background. |
| `flag` | `#f2bf73` | `#f2bf73` | A marker flag (an OSC marker, an overview's pressed key). |
| `trigger` | `#d9cc6666` | `#d9cc6666` | The oscilloscope's trigger-level line. |
| `warn` | `#ff6467` | `#e7000b` | The negative/warning readout (the phase scope's anti-correlation). |
| `success` | `#4ade80` | `#16a34a` | **Up and well**: a session ready, a thing that worked. |
| `warning` | `#fbbf24` | `#d97706` | **Under way, or a caution**: a session running code. |
| `destructive` | `#ff6467` | `#e7000b` | **Down, or failed**: a session stopped, an action that destroys. |
| `meter_field` | `#141414` | `#e5e5e5` | The well a meter's column stands in: darker than any panel it sits on, because a column is read against its own empty space. |
| `meter_low` | `#3dc770` | `#3dc770` | A meter's column below the alignment level (-18 dBFS): the level a mix works at. |
| `meter_mid` | `#f2cc40` | `#f2cc40` | Its column from the alignment level to the hot end (-6 dBFS): headroom being used. Reached at -12 dBFS and held, so a column using its headroom reads amber rather than green with a cast on it. |
| `meter_high` | `#eb4738` | `#eb4738` | Its column from the hot end (-6 dBFS) to the top: the last decibels, where a peak that grows any further clips. A band and not a ramp, or the column would read amber at -6 and red only at full scale. |
| `meter_clip` | `#ff382e` | `#ff382e` | The lamp above a meter's column: lit once the signal was flattened and left lit until a hand puts it out. Brighter and flatter than `meter_high`, because it is a latched statement about the past and not another band of the level. |
| `key_white` | `#e5e5e5` | `#ffffff` | A playable white key. |
| `key_white_dim` | `#d4d4d4` | `#fafafa` | The piano roll's dimmer white key. |
| `key_black` | `#1c1c1c` | `#262626` | A black key. |
| `key_pressed` | `#73cc99` | `#73cc99` | A pressed white key. |
| `key_pressed_black` | `#40996b` | `#40996b` | A pressed black key. |
| `key_inactive` | `#6b6e75` | `#d4d4d4` | An inactive (out-of-range) white key. |
| `key_inactive_black` | `#3d4047` | `#a1a1a1` | An inactive black key. |
| `key_gap` | `#0d0d0d` | `#a1a1a1` | The gap/edge line between keys. |
| `key_label` | `#595959` | `#a1a1a1` | The octave label on a playable key. |
| `key_label_dim` | `#4d4d4d` | `#a1a1a1` | The octave label in the piano roll's key gutter. |
| `key_overview` | `#171717` | `#f5f5f5` | The keyboard overview strip's background. |
| `key_overview_active` | `#262626` | `#e5e5e5` | The overview's active-range band. |
| `key_overview_black` | `#0a0a0a` | `#737373` | The overview's black-key marks. |
| `series_1` | `#4cc78c` | `#16a34a` | Channel 1 of the series palette (the classic mono trace). |
| `series_2` | `#f2b840` | `#f2b840` | Channel 2 of the series palette. |
| `series_3` | `#73a6f2` | `#73a6f2` | Channel 3 of the series palette. |
| `series_4` | `#e67399` | `#e67399` | Channel 4 of the series palette. |
| `syntax_keyword` | `#569cd6` | `#0000ff` | A keyword, and a constant the language names (`None`, `true`). |
| `syntax_control` | `#c586c0` | `#af00db` | A keyword that moves control or brings a module in (`if`, `return`, `import`). |
| `syntax_name` | `#9cdcfe` | `#0070c1` | A name: a variable, a property, a parameter. |
| `syntax_function` | `#dcdcaa` | `#795e26` | A function's name, where it is defined and where it is called. |
| `syntax_type` | `#4ec9b0` | `#267f99` | A type, and a name that starts with a capital (a class). |
| `syntax_number` | `#b5cea8` | `#098658` | A number. |
| `syntax_string` | `#ce9178` | `#a31515` | A string. |
| `syntax_regexp` | `#d16969` | `#af00db` | A regular expression written as a literal. |
| `syntax_comment` | `#6a9955` | `#008000` | A comment. |
| `syntax_operator` | `#d4d4d4` | `#383a42` | An operator, punctuation, an escape inside a string. |
| `find_match` | `#00ffff8a` | `#ffff0054` | A match of a code editor's find bar, under the text. |
| `find_current` | `#ff00ff8a` | `#ff6a0054` | The match of the find bar that is selected. |
| `bracket_match` | `#328c8252` | `#328c8252` | The bracket by a code editor's caret and the one it pairs with. |
| `bracket_mismatch` | `#bb555544` | `#bb555544` | A bracket by the caret that nothing closes. |

**Code follows the ground.** A table that turns `background` light (and back)
without a `look` and names none of the code's roles (the `syntax_` ones and
its marks, `find_match` to `bracket_mismatch`) also puts the light look's
set (or the dark one's) in them, so a light theme reads its code without
fourteen more entries. A table that names any of them keeps what it named.

## `[gui.metrics]`

The sizing counterpart, with the same partial-overlay semantics: `role = number`
in **logical** pixels (glyph scales for the `_scale` text roles, pixels to
the em for the type scale's `type_` ones), unlisted roles keep
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

The 114 roles, with the value each takes at `scale = 1.0`:

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
| `status_h` | 18 | The height of **one line of the status bar** along the window's bottom edge. A ruler's height rather than a control's: the bar is a strip of chrome with a line written on it, which is what a ruler is. |
| `ruler_w` | 46 | The width of a ruler strip beside a vertical axis (sized for its widest labels). |
| `header_w` | 72 | The width of a row's header column (a lane's name and controls). |
| `panel_h` | 36 | The height of a **panel's header**: the one strip along a panel's top, which a dock's panel draws its grip, its tabs or its tools and its buttons in, and a window that names a `header` its tools in. |
| `divider_w` | 1 | A hairline: a divider between lanes, a box edge. |
| `focus_ring` | 3 | The weight of the ring around the widget holding the keyboard focus. Heavier than a hairline on purpose: it has to read *over* the edge a control already draws. |
| `trace_w` | 1.5 | The weight of a drawn signal trace. |
| `point_radius` | 4 | The radius of a placed point (a break-point, an automation node). |
| `plate_radius` | 4 | The corner radius of a **text plate** — the ground a caption drawn over a picture sits on. |
| `hit_slop` | 4 | The slack around a small target's geometry, so it stays clickable. |
| `grip_w` | 12 | The width of an **edge grip**: the strip at each end of a placed object that resizes it (a clip's ends), drawn where it can be grabbed. |
| `label_gap` | 14 | The smallest gap between two ruler labels before the ladder steps up. |
| `tick_gap` | 7 | The smallest gap between two ruler ticks before the ladder steps up. |
| `text_scale` | 1.5 | Primary text: control labels, values, readouts. |
| `label_scale` | 1.5 | In-view labels drawn over a surface (a lane's name tag). |
| `caption_scale` | 1.5 | Reduced text: ruler labels, a clip's caption. |
| `micro_scale` | 1 | The densest legible mark (a key's octave label). |
| `type_xs` | 12 | The smallest size: a key beside its entry, help under a field, the status footer, a count. |
| `type_sm` | 14 | The interface's size: menus, tabs, buttons, fields, a dialog's text. |
| `type_base` | 16 | A page's body. |
| `type_lg` | 18 | An alert's title. |
| `line_xs` | 16 | The line `type_xs` is set on. |
| `line_sm` | 20 | The line `type_sm` is set on. |
| `line_base` | 24 | The line `type_base` is set on. |
| `line_lg` | 28 | The line `type_lg` is set on. |
| `radius_sm` | 6 | The small corner: a menu's entry, a menu bar's title, a tooltip. |
| `radius_md` | 8 | The medium corner: a button, a field, a menu's list, a section. |
| `radius_lg` | 10 | The large corner: the specification's base radius. |
| `radius_xl` | 14 | The extra large corner: a dialog's card. |
| `shadow_xs` | 1 | The extra small lift: a field, an outlined button. |
| `shadow_md` | 4 | The medium lift: a menu's list, a hover card. |
| `shadow_lg` | 10 | The large lift: a submenu's list, a dialog. |
| `press_shift` | 1 | How far a pressed button goes down. |
| `bar_h` | 40 | The application bar's height. |
| `bar_inset` | 6 | The application bar's inset at its ends. |
| `footer_h` | 24 | The status footer's height. |
| `footer_inset` | 12 | The status footer's inset at its ends. |
| `panel_grip` | 20 | The width of a panel's grip, at the start of its header. |
| `tab_w` | 224 | The widest a tab grows. |
| `tab_inset` | 12 | A tab's inset before its name. |
| `tab_end` | 4 | A tab's inset after its close mark. |
| `tab_close` | 24 | The side of a tab's close mark, and of the small tools of a header. |
| `menu_title_x` | 8 | A menu bar title's inset across. |
| `menu_title_y` | 4 | A menu bar title's inset down. |
| `menu_inset` | 4 | A menu's list, from its edge to its entries. |
| `menu_item_x` | 8 | An entry's inset across. |
| `menu_item_y` | 6 | An entry's inset down. |
| `menu_w` | 144 | The narrowest a menu's list is. |
| `submenu_w` | 128 | The narrowest a submenu's list is. |
| `menu_mark` | 32 | The indent a check or a radio entry gives its text, for the mark. |
| `menu_max_w` | 576 | The widest a menu's list grows (a list of recent files). |
| `menu_aside` | 16 | The gap between an entry's text and its aside (a recent file's folder). |
| `card_x` | 12 | A hover card's inset across. |
| `card_y` | 8 | A hover card's inset down. |
| `card_w` | 768 | The widest a hover card grows. |
| `card_gap` | 6 | Between a hover card's head and the documentation under it. |
| `card_doc_h` | 240 | The tallest a hover card's documentation stands before it scrolls. |
| `tip_x` | 12 | A tooltip's inset across. |
| `tip_y` | 6 | A tooltip's inset down. |
| `tip_end` | 6 | A tooltip's inset after the key it ends in. |
| `tip_offset` | 10 | How far a tooltip stands from what it is the tip of. |
| `tip_arrow` | 5 | How far a tooltip's arrow reaches out of it, toward what it is the tip of. |
| `tip_key_gap` | 8 | Between a tooltip's text and its key. |
| `kbd_h` | 20 | The height of a key drawn as a key cap (a tooltip's). |
| `kbd_x` | 4 | A key cap's inset across. |
| `dialog_inset` | 24 | A dialog's inset. |
| `dialog_gap` | 24 | Between a dialog's parts: its header, its body, its footer. |
| `dialog_w` | 448 | The width a dialog takes. |
| `dialog_wide_w` | 576 | The width a dialog of settings takes. |
| `alert_w` | 512 | The width an alert dialog takes. |
| `dialog_title_gap` | 8 | Between a dialog's title and its description. |
| `alert_title_gap` | 6 | Between an alert dialog's title and its description. |
| `dialog_close` | 16 | How far a dialog's close button stands from its top right corner. |
| `section_inset` | 16 | A bordered section's inset, inside a dialog. |
| `list_row_h` | 44 | A row of a list inside a dialog. |
| `button_h` | 36 | A button's height. |
| `button_sm_h` | 32 | A small button's height. |
| `button_xs_h` | 24 | An extra small button's height. |
| `button_lg_h` | 40 | A large button's height. |
| `button_x` | 10 | A button's inset across. |
| `button_gap` | 6 | Between a button's icon and its text. |
| `field_h` | 36 | A field's height. |
| `field_x` | 10 | A field's inset across. |
| `field_y` | 4 | A field's inset down. |
| `text_area_h` | 64 | The least a text area is high. |
| `label_to_field` | 8 | Between a label and the control it names. |
| `check_w` | 16 | A checkbox's side. |
| `check_radius` | 4 | A checkbox's corner. |
| `check_gap` | 8 | Between a checkbox or a switch and the words beside it. |
| `switch_w` | 32 | A switch's track, along it. |
| `switch_h` | 18 | A switch's track, across it. |
| `toggle_x` | 8 | A toggle button's inset across. |
| `badge_h` | 20 | A badge's height. |
| `badge_x` | 8 | A badge's inset across. |
| `scrollbar_w` | 10 | A scrollbar's width. |
| `scrollbar_inset` | 2 | How far a scrollbar's thumb keeps off its edges. |
| `split_grab` | 4 | The band a split's line is grabbed in. |
| `zone_inset` | 6 | How far a drop zone stands inside the place a dragged panel would land. |
| `icon` | 16 | An icon's side. |
| `icon_sm` | 12 | A small icon's side. |
| `find_h` | 36 | The find bar's height. |
| `find_field_h` | 28 | The find bar's field's height. |

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

## `[gui.keys]`

**Which key performs which verb.** No widget names a key: a multitrack splits,
a roll quantizes, a window plays — those are **verbs**, and this table says
which chord asks for each. An entry is a verb's name and one chord, or a list
of them; `[]` unbinds the verb. The table is **partial**: unlisted verbs keep
their defaults (Ctrl+Z undoes, Space plays, `q` quantizes, `e` splits, `j`
joins, …), and a chord means one verb, so binding it takes it from whatever had
it. An unreadable chord warns and is skipped.

```toml
[gui.keys]
split = "S"                     # a verb the host performs, on another key
redo = ["Ctrl+Shift+Z", "Ctrl+Y"]
quantize = []                   # no key at all
export = "Ctrl+E"               # the application's own verb
```

A chord is `Ctrl`, `Shift` and `Alt` joined by `+` to a key: a character,
`Space`, `Delete`, `Backspace`, `Enter`, `Home`, `End`, `Up`, `Down`, `Left`,
`Right` or `F1` to `F12`, case-insensitive. A letter's Shift is part of the
chord (`Q` is not `Shift+Q`); a punctuation character already says its own.
Tab and Escape cannot be bound — they walk the focus and dismiss. **`Ctrl` is
the platform's command key**: on a Mac it is Command (a file may write `Cmd`,
and a menu shows it so), so one table reads the way each platform expects;
Control on a Mac, and the logo key elsewhere, is a modifier the host has no
name for, and a key pressed with it reaches nothing.

**A name the host does not perform is the application's**, and that is what
makes a window with no menu at all usable from the keyboard: pressing `export`'s
chord reports `"export"` from the window, which the script answers in the
window's `on_event` like any other event. When the window *has* a menu bar with
an entry for that verb, the key reports exactly that entry's pick
(`"menu", "export"`), so one handler covers both; and a menu shows the chord
beside every entry whose verb is bound. **F1 shows them all** (the verb
`keys`): a sheet over the window listing every key in force in it and what it
does, which its close mark or Escape takes down, in any window, with chrome
or without. The host's own verbs read in words there, an application's in the
words its window declares, and any other as its name, so `select_all` is
*Select all*.

**An application's modes are scopes of this table, and the application brings
them.** The score editor's window declares `score` and `note_entry` -- their
verbs, their default chords and their words -- when it opens, and the host
lays them under this section: a `[gui.keys.note_entry]` table here rebinds a
key of note entry and wins over the editor's default whenever the window
opens.

```toml
[gui.keys.note_entry]
enter_rest = "R"                # a rest on R in note entry, not on 0
```

`--keys <path>` reads the same flat table from a free-standing file, laid over
the section, so a user's keys travel as one file. A script can also bind keys
after launch with `GuiHost.keys`, the same table as a dict — which is how an
application gives its own verbs default keys. The verbs the host performs, their
defaults and the order a key reaches them are in the widget protocol, "The keys
are a table", in the
[Clausters server book](https://clausters.readthedocs.io/en/latest/gui-protocol.html).

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

[gui.keys]
split = "S"

[standalone]
gui = "drone"
```
