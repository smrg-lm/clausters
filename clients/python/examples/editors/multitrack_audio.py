#!/usr/bin/env python3
"""A multitrack of audio, edited and played by the server's transport.

The multitrack editor over a **piece** — `clausters.multitrack.Multitrack`, the
document the three applications share — opened with `clausters.gui.edit` like
any other structure. It is here to check the whole editor by ear and by eye, so
everything that is not the piece or its sound is out of the way.

What to do in the window:

- **Move a box**, drag its edge to trim it, drag it onto another lane, sweep a
  block and move it as one. Whatever the hand does, the readers follow: the
  widget reports the **piece as it now stands** and one handler puts them where
  it says. There is no widget id in this file and no handler per box.
- **Drag a break-point.** The row under the first lane is a **track
  automation** — it runs the length of the timeline, because a track's gain does
  not begin and end with a box — and the line drawn inside the first box is a
  **clip envelope**, which lasts exactly as long as that box does. They are the
  same curve in the two places one lives, and both are heard: the track's scales
  its whole lane, the box's shapes that box alone.
- **Double click a box** to enter it. What a box holds is a structure like any
  other, so entering one opens the take in the sample editor — and it is opened
  on the **piece's** own undo order, so `Ctrl`+`Z` walks the notes drawn inside
  a box and the boxes dragged on the stack as one history.
- **`Ctrl`+`Z`** anywhere: the piece has the history every other editor has,
  because it is one of the fundamental structures rather than a special case.

**The time of this piece is the server's.** `clausters.gui.Transport` is built
with ``head_clock="piece"``, which settles everything under it: play, pause and
stop are ``/transport_play``, ``/transport_stop`` and
``/transport_locateSample``; the lanes are one **governed** group, so a pause
freezes them with every node's state intact and playing again continues rather
than starting over; and the host draws the line from the engine's own position,
which holds while stopped and jumps where a locate puts it, with nothing sent per
frame.

**A box is a reader.** Each one is a single resident node reading its buffer at
the transport's position -- no queue, nothing scheduled, nothing re-cued. Moving
a box while it plays is one ``/node_set``, on a node that is already running, so
it is heard where it was dropped with nothing that is sounding cut.

**No tempo.** An audio piece has no beats to declare, so it states none and a
beat is a second — the reader's default, and the reason every number here can be
read as seconds.

Run it as a script (``python multitrack_audio.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter.
"""

# %%
from clausters import Buffer, Group, Session, Synth
from clausters.gui import Transport, button, edit, label, layout
from clausters.defs import SynthDef, out
from clausters.defs.ugens import (buf_rd, control, line, pink_noise, saw, sine,
                                  transport_pos, white_noise)
from clausters.multitrack import (Automation, Content, Lane, Multitrack, Region,
                                  Track)

SR = 48_000.0
TAKE_DUR = 2.0

# %% [markdown]
# ## The takes, rendered offline
# Four signals, so that one lane is told from the next by ear.

# %%
def gentake(name: str, expr, secs: float = TAKE_DUR) -> list:
    """Render ``expr`` for ``secs`` seconds offline and hand back its samples."""
    session = Session.nrt(tempo=1.0)          # beats == seconds
    server = session.server
    SynthDef(name, out(0.0, expr)).send(server)
    node = Synth(name, server=server)
    server.send_bundle_after(secs, ("/node_free", node.id))
    stats = session.render(sample_rate=SR, channels=1)
    return list(stats.samples)


white = gentake("t_white", white_noise() * 0.5)
pink = gentake("t_pink", pink_noise() * 0.5)
glide = gentake("t_glide", sine(line(220.0, 440.0, TAKE_DUR)) * 0.5)
saw110 = gentake("t_saw", saw(110.0) * 0.5)

print(f"rendered {len(white)}, {len(pink)}, {len(glide)}, {len(saw110)} frames")

# %% [markdown]
# ## The box: a buffer read at the transport's position
# `transport_pos` is the transport's position minus where the box starts, so the
# reader is at frame 0 when the transport reaches it. The gate is the box's
# length: `buf_rd` clamps past the end instead of going quiet.

# %%
def reader(name: str = "reader") -> SynthDef:
    """A box, sounding. No position of its own: seeking, looping and pausing
    are the transport's, and moving it is one ``/node_set`` of ``at``."""
    buf = control("buf", 0.0, "ir")
    at = control("at", 0.0)          # where it starts, in frames on the transport's axis
    span = control("span", 0.0)      # how long it lasts, in frames
    amp = control("amp", 0.2, lag=0.02)
    pos = transport_pos(at)
    live = (pos >= 0.0) * (pos < span)
    sig = buf_rd(buf, 0.0, pos) * live * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The session, the takes on the server, and the group that plays them

# %%
session = Session.live(tempo=1.0, latency=0.1)
server = session.server
reader().send(server)

TAKES = {"white": white, "pink": pink, "glide": glide, "saw": saw110}
BUFS = {name: Buffer.from_samples(samples, server=server) for name, samples in TAKES.items()}
server.sync()

#: `transport_group` **binds the group the transport governs**, and it is the
#: call this whole example rests on: from here the engine freezes that subtree on
#: `transport_stop` and thaws it on `transport_play`, with every node's state
#: intact. A group of its own and never the root, which would freeze every sound
#: the session has.
multitrack_group = Group(server=server)
server.transport_group(multitrack_group)

# %% [markdown]
# ## The piece
# Three tracks, one lane each, four boxes. A **source id** is what the document
# names — never a path and never a buffer number — because a piece must open in
# a program that has no Python in it. Which buffer each source was read into is
# the one thing about a piece that is not in the piece, and it travels beside it
# in `clausters.gui.editing.Sources`.

# %%
#: source id -> the take it names, so the two tables below are written once.
SOURCES = {1: "white", 2: "glide", 3: "saw", 4: "pink"}

#: The **fader** is the client's key in a track's own table: the document holds
#: no mixer, because a level is not a fact about the piece.
LEVEL = "level"


def window(source: int) -> Content:
    """A box over the whole of one source, from its start."""
    return Content.onto({"source": {"source": source, "lifetime": "session",
                                    "generation": 0},
                         "start": 0.0, "duration": TAKE_DUR})


def box(id: int, at: float, source: int, name: str) -> Region:
    return Region(id=id, position=at, length=TAKE_DUR, name=name,
                  content=window(source))


#: **A track automation**: a row of its own under the lane, as long as the
#: timeline. `target` is the client's own word for what it drives, and the range
#: is read out of it — the document says what a curve automates and never reads
#: it, so which range that parameter has is stated where the parameter is.
noise_gain = Automation(id=100, name="gain", visible=True,
                        target={"ctl": LEVEL, "min": 0.0, "max": 1.0},
                        points=[{"at": 0.0, "value": 1.0},
                                {"at": 8.0, "value": 0.2}])

#: **A clip envelope**: a layer drawn inside its box, lasting exactly as long as
#: the box does. Its time is the box's own, from zero.
white_env = Automation(id=101, name="fade", visible=True,
                       target={"ctl": "amp", "min": 0.0, "max": 1.0},
                       points=[{"at": 0.0, "value": 0.0},
                               {"at": TAKE_DUR, "value": 1.0}])

first = box(20, 0.0, 1, "white")
first.automation.append(white_env)

piece = Multitrack(tracks=[
    Track(id=10, name="noise", automation=[noise_gain],
          lanes=[Lane(id=11, regions=[first, box(21, 6.0, 4, "pink")])]),
    Track(id=12, name="tone",
          lanes=[Lane(id=13, regions=[box(22, 2.0, 2, "glide")])]),
    Track(id=14, name="bass",
          lanes=[Lane(id=15, regions=[box(23, 4.0, 3, "saw")])]),
])

# %% [markdown]
# ## Making what is drawn be what sounds
# One handler. A hand moved a box, crossed a lane, trimmed one, drew a curve,
# moved a fader or soloed a track — it does not matter which, because what the
# editor hands back is the **piece**, and this puts the readers where the piece
# now says they are.

# %%
#: region id -> the node playing it.
nodes = {}


def curve_at(curve: Automation, beat: float) -> float:
    """What a break-point curve says at ``beat`` — straight lines between its
    points, held at the ends. The shape a point carries is the drawing's; this
    example only needs the level."""
    points = sorted(curve.points, key=lambda p: float(p["at"]))
    if not points:
        return 1.0
    if beat <= float(points[0]["at"]):
        return float(points[0]["value"])
    for before, after in zip(points, points[1:]):
        a, b = float(before["at"]), float(after["at"])
        if beat <= b:
            span = b - a
            over = 0.0 if span <= 0.0 else (beat - a) / span
            return float(before["value"]) + over * (
                float(after["value"]) - float(before["value"]))
    return float(points[-1]["value"])


def track_gain(track: Track, at: float) -> float:
    """What a track contributes at ``at``: nothing when it is muted, nothing
    when another is soloed, its fader times its automation otherwise — the
    mixer's rules, and they are the script's, because the host carries the flags
    and never reads them."""
    soloing = any(t.soloed for t in piece.tracks)
    if track.muted or (soloing and not track.soloed):
        return 0.0
    level = float((track.config or {}).get(LEVEL, 1.0))
    for curve in track.automation:
        if curve.enabled and (curve.target or {}).get("ctl") == LEVEL:
            level *= curve_at(curve, at)
    return level


def region_gain(region: Region) -> float:
    """What a box's own curves say at its start — its envelope, in the one place
    a curve that acts on this placement alone lives. A box that has curves is a
    small track acting on itself."""
    if region.muted:
        return 0.0
    level = 1.0
    for curve in region.automation:
        if curve.enabled:
            level *= curve_at(curve, 0.0)
    return level


def sound_the_piece():
    """Make what is drawn be what sounds.

    The whole of the driver, and it runs on every edit whoever made it — this
    window's gesture, a second window over the same piece, or a step of the
    history. A box that went away takes its node with it.
    """
    seen = set()
    for track in piece.tracks:
        lane = track.active_lane
        for region in (lane.regions if lane is not None else ()):
            seen.add(region.id)
            source = ((region.content.window or {}).get("source") or {}).get("source")
            take = SOURCES.get(int(source)) if source is not None else None
            if take is None:
                continue
            args = {"at": editor.bridge.frame_at(region.position),
                    "span": editor.bridge.frames_over(region.position, region.length),
                    "amp": 0.5 * track_gain(track, region.position)
                    * region_gain(region)}
            node = nodes.get(region.id)
            if node is None:
                nodes[region.id] = Synth(
                    "reader", {"buf": BUFS[take].bufnum, **args},
                    target=multitrack_group, server=server)
            else:
                node.set(args)
    for id in [id for id in nodes if id not in seen]:
        nodes.pop(id).free()


# %% [markdown]
# ## The window
# `edit` opens the piece. The multitrack is **one widget**: it draws its own
# ruler, its own lane headers, its own automation rows, its own stack and its own
# boxes, so there is no `track` to build and no `scroll` to wrap them in. The
# transport row rides in ``extra``, which is where a window's own chrome goes.

# %%
gui = session.gui()

editor = edit(
    piece, sample_rate=SR, host=gui,
    #: Which buffer each source was read into — and, given as the **objects**,
    #: what a box opens as when it is entered: the same table answers both,
    #: because a piece names a source and only whoever loaded it holds the take.
    sources={id: BUFS[take] for id, take in SOURCES.items()},
    extra=[
        # No rewind: stop already rewinds, which is what tells it from pause.
        layout(
            button(label="play/pause", name="b_play", w=110.0),
            button(label="stop", name="b_stop", w=110.0),
            label("", name="counter", text_size=2.0, weight=1.0),
            flow="row", h=40.0, gap=6.0
        ),
    ],
    title="Clausters multitrack (audio)", width=1000, height=560)

win = editor.window

#: **One subscription, for the whole piece**, and it is not a subscription: the
#: editor tells the script the data changed, whoever changed it.
editor.on_change = sound_the_piece

#: **The piece is sounded once here and by nothing else afterwards.** The same
#: call an edit makes: a piece is a statement, so putting the readers where it
#: says is one verb whether it is the first time or the hundredth.
sound_the_piece()
server.sync()
print(f"{len(nodes)} readers in group {multitrack_group.id}, "
      f"{piece.end:.1f} s of piece")

# %% [markdown]
# ## The transport
# ``head_clock="piece"`` says it once: the buttons become `/transport_play`,
# `/transport_stop` and `/transport_locateSample`, and the host draws the line
# from the engine's own position instead of an anchor kept in step here.

# %%
#: The widget the editor drew for the piece — the one thing a transport needs
#: from it, since the head is a property of the view the line is drawn on.
piece_widget = next(iter(editor.view.widgets))

transport = Transport(gui, lambda: [piece_widget], head_clock="piece",
                      tempo=1.0, sample_rate=SR,
                      extent=lambda: piece.end, governed=True)
transport.server = server
transport.locate(0.0)

# %% [markdown]
# ## The buttons and the read-out

# %%
def play_pause():
    """Pause freezes the governed group where it stands, so playing again
    continues rather than starting over."""
    if transport.playing:
        transport.pause()
    else:
        transport.play(server)


win["b_play"].on_click(play_pause)
win["b_stop"].on_click(transport.stop)

_shown = None


def tick_counter():
    """`refresh` is the one round trip: the position is the engine's. The line
    costs nothing — the host draws it from the segment every frame."""
    global _shown
    transport.refresh()
    text = f"{transport.position:8.3f} s   of {piece.end:.3f} s"
    if text != _shown:
        win["counter"].set(text=text)
        _shown = text
    return 0.05


win["counter"].set(text=f"{0.0:8.3f} s   of {piece.end:.3f} s")
gui.clock.sched(0.05, tick_counter)

# %%
editor.wait()
session.close()
