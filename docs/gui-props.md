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

`w`, `h`, `x`, `y`, `weight`, `color`, `theme` and `gestures` belong to no
widget: the host parses them off **every** node in `Widget::build`, before the
kind is even considered. (`gestures` is a *container's* table — a leaf carries
one harmlessly, since nothing ever asks a leaf what a drag on it does.) So they are left out of the per-widget comparison, and the two
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
| `timeruler` | `playhead` | host web | **idiom** — a ruler draws an axis and nothing else: it parses the whole chrome only because it shares the host's `EditorProps` bundle, and the frame pass gives it a ruler strip with no playhead, selection or vertical window. The web client declares that bundle once (`TimelineOptions`) and so offers the inert members too; the Python builder names props widget by widget and names only the ones the ruler acts on (`ruler`, `tempo`, `beat_at`, `quant`, `sample_rate`, `link`) |
| `timeruler` | `playhead_at` | host web | **idiom** — as above |
| `timeruler` | `playhead_loop_len` | host web | **idiom** — as above |
| `timeruler` | `playhead_loop_start` | host web | **idiom** — as above |
| `timeruler` | `sel_len` | host web | **idiom** — as above |
| `timeruler` | `sel_start` | host web | **idiom** — as above |
| `timeruler` | `y_len` | host web | **idiom** — as above |
| `timeruler` | `y_start` | host web | **idiom** — as above |
| `track` | `sel_len` | host web | **idiom** — a lane has no selection and no vertical window (`EditorProps::parse_lane` says so, and the frame pass draws neither), so these four are inert on it for the same reason the ruler's are. The lane *does* act on the playhead and its loop region, and the Python builder names those |
| `track` | `sel_start` | host web | **idiom** — as above |
| `track` | `y_len` | host web | **idiom** — as above |
| `track` | `y_start` | host web | **idiom** — as above |

## What the table says, read as a whole

The table used to be twenty-eight rows, twenty-six of them pointing one way:
the Python client is the reference for the API and was behind on the timeline
chrome — the host implemented the playhead, its loop region, the selection and
the vertical window for every timeline widget, and the Python builders, which
name props widget by widget rather than sharing an interface, had named only
some of them. That is closed: `waveform`, `spectrogram`, `pianoroll`, `track`
and `score` name the whole chrome they act on, and the web client gained the
`log_freq` alias and `score`'s loop region in the same pass.

What is left is one kind of row, and it is not work:

- **Twelve `idiom` rows on `timeruler` and `track`** — props the host parses
  for those two only because both embed the shared `EditorProps` bundle, and
  then draws nothing with. The web client declares the bundle once and inherits
  them; Python names them per widget and does not. Naming an inert prop is
  worse than not naming it, so this asymmetry is the correct one.
