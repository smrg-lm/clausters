# The widget props, and where each one is written down

A GUI prop is declared in three independent places, and **no compiler ties any
two of them together**:

| Surface | What it is | How it is kept honest |
|---|---|---|
| the host | `clients/gui/src/host/widget` — `build` reads a prop at construction, `apply` accepts it from `/gui_set` | cargo, but only against itself: the wire is untyped JSON |
| the Python client | `clausters.gui.guidef` — one builder per widget, props as keyword arguments | nothing |
| the web client | `clients/web/src/gui/guidef.ts` — one builder per widget, props as an option type | nothing |

The wire being open is the point: `/gui_def` carries `{id, type, ...props,
children}` and an unknown prop is ignored, so a widget can be added without
touching the protocol. The cost is that a prop added on one side and forgotten
on another is silent in every build — the host does nothing, the client offers
an option that lands nowhere, and both are green.

This file is what `docs/bindings.md` is for the core's two bindings, applied to
the GUI layer: it does **not** require the three to agree — a client is
idiomatic in its own language, and a prop may legitimately reach only one — it
requires that every difference be one somebody decided.
`clients/python/tests/test_gui_props.py` reads all three surfaces and fails when
it finds a difference this table does not name, or when the table names one that
is no longer there.

## How the three are read

The readers are deliberately different, because the sources are:

- **Python is read by calling it** — `inspect.signature` over the builders. This
  is exact: it is literally what a script can type.
- **The web client is read statically**, from each builder's option type: the
  interfaces it extends (`WidgetOptions`, `ContainerOptions`, `TimelineOptions`,
  `SourceOptions`) plus its inline literal. That is what a reader of the API
  sees, and camelCase is folded to the wire's snake_case (`textSize` →
  `text_size`).
- **The host is read statically** from the widget schema's two wire passes,
  resolving the shared prop-reading helpers so a bundle like `Flow` or
  `EditorProps` contributes its keys to every widget that embeds it.

What none of this checks is whether a prop *does* what its name suggests on all
three, or whether the value shapes agree. That needs types the wire does not
have; this catches the coarse drift, which is the one that has actually
happened.

## The generic props

`w`, `h`, `x`, `y`, `weight`, `color` and `theme` belong to no widget: the host
parses them off **every** node in `Widget::build`, before the kind is even
considered. So they are left out of the per-widget comparison, and the two
clients name them differently by necessity — the web client declares them once
in the `WidgetOptions` interface every builder extends, while Python takes them
through the `**props` every builder ends with and documents them in the
`guidef` module docstring. Neither is a gap; TypeScript has no `**kwargs`, and
Python has no interface to extend.

## How to read a row

The verdict column carries one of three, with the same meanings they have in
`docs/bindings.md`:

- **`idiom`** — the same capability, shaped for the language. Nothing to do.
- **`n/a`** — deliberately absent, with the reason. Nothing to do.
- **`gap`** — present on one surface, missing on another, and **nobody has
  decided**. Each is either work waiting or a decision waiting to be written
  down. They are not failures: the test passes with gaps in it, because a
  manifest that forbade them would just be lied to.

The `surfaces` column is data, not prose — the test compares it against what it
measured, so a row that becomes half-true fails rather than rotting.

## The divergences

| widget | prop | surfaces | verdict |
|---|---|---|---|
| `pianoroll` | `playhead_loop_len` | host web | **gap** — the sweep's loop region, documented in `docs/gui-protocol.md` and implemented; the Python builder never named it, so a Python script reaches it only through `**props` |
| `pianoroll` | `playhead_loop_start` | host web | **gap** — as above |
| `plot` | `buffer` | web | **gap** — the web builder's `plot` takes `SourceOptions`, which carries the two bulk sources; the host's `plot` reads neither, so the option lands nowhere. Either the host grows them (the heavy views have them) or the option type stops claiming them |
| `plot` | `cache` | web | **gap** — as above |
| `spectrogram` | `log_freq` | host python | **idiom** — the legacy boolean superseded by `freq_scale`, kept working by the host and still named by the Python builder for the scripts that used it. The web client is newer than the rename and never had it |
| `spectrogram` | `playhead` | host web | **gap** — the *static* playhead (where the cursor stands while nothing sweeps), beside the `playhead_at` the Python builder does name |
| `spectrogram` | `playhead_loop_len` | host web | **gap** — as `pianoroll` |
| `spectrogram` | `playhead_loop_start` | host web | **gap** — as `pianoroll` |
| `spectrum` | `log_freq` | host python | **idiom** — as `spectrogram` |
| `timeruler` | `playhead` | host web | **gap** — the ruler carries the same timeline chrome as the views it sits above; the Python builder declares only the time axis (`ruler`, `tempo`, `sample_rate`, `beat_at`, `quant`, `link`) |
| `timeruler` | `playhead_at` | host web | **gap** — as above |
| `timeruler` | `playhead_loop_len` | host web | **gap** — as above |
| `timeruler` | `playhead_loop_start` | host web | **gap** — as above |
| `timeruler` | `sel_len` | host web | **gap** — as above |
| `timeruler` | `sel_start` | host web | **gap** — as above |
| `timeruler` | `y_len` | host web | **gap** — as above |
| `timeruler` | `y_start` | host web | **gap** — as above |
| `track` | `link` | host web | **gap** — the shared navigation group; a Python-built track cannot join one by name |
| `track` | `playhead` | host web | **gap** — listed for `track` in `docs/gui-protocol.md`, absent from the Python builder |
| `track` | `playhead_loop_len` | host web | **gap** — as above |
| `track` | `playhead_loop_start` | host web | **gap** — as above |
| `track` | `sel_len` | host web | **gap** — as above |
| `track` | `sel_start` | host web | **gap** — as above |
| `track` | `y_len` | host web | **gap** — as above |
| `track` | `y_start` | host web | **gap** — as above |
| `waveform` | `playhead` | host web | **gap** — the static playhead, as `spectrogram` |
| `waveform` | `playhead_loop_len` | host web | **gap** — as `pianoroll` |
| `waveform` | `playhead_loop_start` | host web | **gap** — as `pianoroll` |

## What the table says, read as a whole

Twenty-six of the twenty-eight rows point the same way: **the Python client is
the reference for the API and is behind on the timeline chrome.** The host
implements the playhead, its loop region, the selection and the vertical window
for every timeline widget, `docs/gui-protocol.md` documents them, the web
client's `TimelineOptions` declares them once for all of them — and the Python
builders name them widget by widget, which is how `track` and `timeruler` ended
up naming almost none.

That is a real asymmetry and not this file's to fix: closing it changes the
Python API (new keyword arguments), which is a milestone, not a manifest entry.
What the manifest does is make it visible and keep it from growing — the next
prop added to one surface and not the others fails a test on the way in.

The two rows that point the other way are `plot`'s `buffer` and `cache`: the web
client offers two options the host does not read. Those are the more urgent
kind, because a script that passes them gets no error and no effect.
