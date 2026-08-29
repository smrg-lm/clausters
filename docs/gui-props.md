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

`w`, `h`, `x`, `y`, `weight`, `color`, `theme`, `opacity`, `radius` and
`gestures` belong to no widget: the host parses them off **every** node in
`Widget::build`, before the kind is even considered. (`gestures` is a *container's* table — a leaf carries
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
| `signal` | `at` | python web | **reader** — the host reads both, and not in a place this test can see them: `at`/`dur` are a **node**'s placement on its container's time axis (`Widget::span`, parsed in `Widget::build` for any node), not an element's prop, because any body kind can be a segment of a clip. The reader here is per widget — it walks each element's own file — so a prop every kind takes through the node pass reads as absent from all of them. Both clients offer it on the builders that can be a body |
| `signal` | `dur` | python web | **reader** — as above |
| `window` | `layout` | python web | **idiom** — the arrangement is `flow` on the wire: the model spends the word `layout` on the container type itself. Both builders still take `layout=` as its old name and emit `flow`, so a script written before the rename keeps building — a client-side courtesy the host knows nothing about |
| `layout` | `layout` | python web | **idiom** — as above |
| `plane` | `layout` | python web | **idiom** — as above |
| `field` | `sel_start` | host web | **idiom** — a `field` is one container in three uses, and two of them act on less than the whole chrome: a lane has no selection and no vertical window (`EditorProps::parse_lane` says so, and the frame pass draws neither), and a bare ruler draws an axis and nothing else. Both parse the lot only because they share the host's `EditorProps` bundle. The web client declares that bundle once (`TimelineOptions`) and so offers the inert members too; the Python `track` and `timeruler` name props case by case and name only the ones each acts on |
| `field` | `sel_len` | host web | **idiom** — as above |
| `field` | `sel_min` | host web | **idiom** — as above |
| `field` | `sel_max` | host web | **idiom** — as above |
| `field` | `y_start` | host web | **idiom** — as above |
| `field` | `y_len` | host web | **idiom** — as above |

## The divergences between the two builders

The table above compares by **wire type**, and a type is built by several
builders: `waveform`, `plot` and `scope` are all a `signal`, `panel` and
`stack` are both a `layout`, a lane and a ruler are both a `field`. So the
vocabularies it compares are unions, and a prop *one* builder of a type is
missing reads as present because a sibling has it. That is the right reading
for "does this prop reach all three surfaces" and the wrong one for "do the two
clients offer the same thing" — which is the question somebody reading
`guidef.py` against `guidef.ts` is actually asking, and the one the
non-divergence rule is about.

So there is a second reading, keyed by **builder name**, over the two clients
only: the host has no builders, and a client naming a prop the host parses on
that type is what the first table is for. The generic props stay out for the
reason they are out above — neither client names them per builder. Five rows
below are the same `EditorProps` story the first table tells about `field`,
now said about the two builders it is actually about.

| builder | prop | surfaces | verdict |
|---|---|---|---|
| `layout` | `layout` | web | **idiom** — `flow`'s old name, kept so a def written before the rename keeps building. The web client declares it once in the `ContainerOptions` every container extends, so every container has it; Python names it per builder and named it on the two whose own parameter is not already called that (`panel`, `scroll`). Neither client can reach the third state — Python's `layout()` taking a `layout=` would shadow the builder's own name in its own signature |
| `plane` | `layout` | web | **idiom** — `flow`'s old name, kept so a def written before the rename keeps building. The web client declares it once in the `ContainerOptions` every container extends, so every container has it; Python names it per builder and named it on the two whose own parameter is not already called that (`panel`, `scroll`). Neither client can reach the third state — Python's `layout()` taking a `layout=` would shadow the builder's own name in its own signature |
| `track` | `sel_start` | web | **idiom** — the lane draws neither a selection nor a vertical window (`EditorProps::parse_lane`), and the free-standing ruler draws its ticks and nothing else (`draw_time_ruler`, which returns before anything but `time_ticks`). Both parse the whole bundle only because they share the host's `EditorProps`; the web client declares that bundle once as `TimelineOptions` and so offers its inert members too, and Python names props case by case and names only the ones each builder acts on |
| `track` | `sel_len` | web | **idiom** — as above |
| `track` | `sel_min` | web | **idiom** — as above |
| `track` | `sel_max` | web | **idiom** — as above |
| `track` | `y_start` | web | **idiom** — as above |
| `track` | `y_len` | web | **idiom** — as above |
| `timeruler` | `sel_start` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `sel_len` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `sel_min` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `sel_max` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `y_start` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `y_len` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `playhead` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `playhead_at` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `playhead_loop_start` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |
| `timeruler` | `playhead_loop_len` | web | **idiom** — as above; the ruler is missing the playhead family too, which the lane does draw |

**What the two readings cost, and what a third one covers.** Both are static:
they compare the names a surface declares, not what a builder does with one.
A prop that lands under the wrong wire key, or as `true` where the other client
writes `1`, has the same name on both sides and passes here. That is
`clients/web/tests/gui-parity.test.ts`'s sweep — every builder crossed with
every option, built in both clients and compared as documents — which is the
third reading and the only executed one. The three catch different things and
none of them subsumes another.

## What the table says, read as a whole

The table used to be twenty-eight rows, twenty-six of them pointing one way:
the Python client is the reference for the API and was behind on the timeline
chrome — the host implemented the playhead, its loop region, the selection and
the vertical window for every timeline widget, and the Python builders, which
name props case by case rather than sharing an interface, had named only some
of them. That is closed.

What is left is one kind of row, and it is not work:

- **Four `idiom` rows on `field`** — props the host parses there only because
  the lane and the ruler embed the shared `EditorProps` bundle, and then draws
  nothing with. The web client declares the bundle once and inherits them;
  Python names them case by case and does not. Naming an inert prop is worse
  than not naming it, so this asymmetry is the correct one.

  (The twelve rows this replaced said the same thing about `track` and
  `timeruler` separately. The two are one container now — a `field` told apart
  by what is placed on it — so they are one set of rows.)
