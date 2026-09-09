#!/usr/bin/env python3
"""A multitrack of audio, played by the server's transport.

The smallest window that is still a multitrack editor: a ruler in seconds, three
lanes of audio clips, play/pause, stop and a counter. It is here to check the
audio half by ear and by eye, so everything that is not audio is out of the way.

**The lanes and the clips are one widget.** `clausters.gui.Multitrack` holds the
piece and `multitrack` draws it, so this script describes what the piece *is* and
never composes a tree of lanes. One subscription covers the whole of it: whatever
a hand does — move a clip, cross a lane, trim one, sweep a block, `q`, Delete, a
fader — the widget reports **the piece as it now stands**, and the only handler
here puts the readers where it says. There is no widget id in this file and no
handler per clip.

**The time of this arrangement is the server's.** `clausters.gui.Transport` is
built with ``head_clock="piece"``, which settles everything under it: play, pause
and stop are ``/transport_play``, ``/transport_stop`` and
``/transport_locateSample``; the lanes are one **governed** group, so a pause
freezes them with every node's state intact and playing again continues rather
than starting over; and the host draws the line from the engine's own position,
which holds while stopped and jumps where a locate puts it, with nothing sent per
frame.

**A clip is a reader.** Each one is a single resident node reading its buffer at
the transport's position -- no queue, nothing scheduled, nothing re-cued. Moving
a clip while it plays is one ``/node_set``, on a node that is already running, so
it is heard where it was dropped with nothing that is sounding cut.

**No tempo.** An audio arrangement has no beats to declare, so every number here
is in seconds.

Run it as a script (``python multitrack_audio.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter.
"""

# %%
from clausters import Buffer, Group, Session, Synth
from clausters.gui import (Multitrack, Transport, button, label, layout,
                           timeruler, view)
from clausters.defs import SynthDef, out
from clausters.defs.ugens import (buf_rd, control, line, pink_noise, saw, sine,
                                  transport_pos, white_noise)

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
# ## The clip: a buffer read at the transport's position
# `transport_pos` is the transport's position minus where the clip starts, so the
# reader is at frame 0 when the transport reaches it. The gate is the clip's
# length: `buf_rd` clamps past the end instead of going quiet.

# %%
def reader(name: str = "reader") -> SynthDef:
    """A clip, sounding. No position of its own: seeking, looping and pausing
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
# ## The arrangement
# One `clausters.gui.Multitrack`: the lanes, the clips, and the widget that
# draws them. A clip is named by **your own word**, and that is the word that
# comes back when a hand moves it — there is no widget id here, and no handler
# per clip.

# %%
#: clip name -> the node playing it.
nodes = {}

#: clip name -> the take it reads. It is the **same buffer** the widget draws
#: and the `reader` node sounds: one array, on the server, for the picture and
#: for the sound.
TAKE_OF = {"white": "white", "glide": "glide", "saw": "saw", "pink": "pink"}

piece = Multitrack(
    lanes=[("noise",), ("tone",), ("bass",)],
    clips=[(name, lane, at * SR, TAKE_DUR * SR, 0.0, name,
            BUFS[TAKE_OF[name]].bufnum)
           for name, lane, at in [("white", "noise", 0.0),
                                  ("glide", "tone", 2.0),
                                  ("saw", "bass", 4.0),
                                  ("pink", "noise", 6.0)]],
)


def lane_gain(lane: str) -> float:
    """What a lane contributes: nothing when it is muted, nothing when another
    is soloed, its fader otherwise — the mixer's three rules, and they are the
    script's, because the host carries the flags and never reads them."""
    found = piece.lane(lane)
    if found is None:
        return 0.0
    soloing = any(l.solo for l in piece.lanes)
    if found.mute or (soloing and not found.solo):
        return 0.0
    return found.gain


def place(name: str):
    """Put a clip's reader where the piece says it is. The first call starts the
    node, every later one is a ``/node_set`` on the node already running."""
    clip = piece.clip(name)
    if clip is None:
        return
    args = {"at": clip.at, "span": clip.dur, "amp": 0.5 * lane_gain(clip.lane)}
    node = nodes.get(name)
    if node is None:
        nodes[name] = Synth("reader", {"buf": BUFS[TAKE_OF[name]].bufnum, **args},
                            target=multitrack_group, server=server)
    else:
        node.set(args)


def sound_the_piece(_what=None):
    """Make what is drawn be what sounds.

    The whole of the driver. A hand moved a clip, crossed a lane, trimmed one,
    moved a fader or soloed a track — it does not matter which, because what the
    widget reports is the **piece**, and this puts the readers where the piece
    now says they are. A clip that went away takes its node with it.
    """
    for name in list(nodes):
        if piece.clip(name) is None:
            nodes.pop(name).free()
    for clip in piece.clips:
        place(clip.name)


#: **The piece is sounded once here and by nothing else afterwards.** The same
#: call an edit makes: the arrangement is a statement, so putting the readers
#: where it says is one verb whether it is the first time or the hundredth.
sound_the_piece()
server.sync()
print(f"{len(nodes)} readers in group {multitrack_group.id}, "
      f"{piece.extent / SR:.1f} s of piece")


# %% [markdown]
# ## The window
# A ruler, the piece, two buttons and a read-out. The multitrack is **one
# widget**: it draws its own lane headers, its own stack and its own boxes, so
# there is no `track` to build and no `scroll` to wrap them in. It shares the
# ruler's navigation group, so a zoom or a pan on either moves both.

# %%
NAV_GROUP = 7

gui = session.gui()

#: No `tempo` and no `quant`: an audio arrangement has no beats to count, so the
#: ruler measures seconds.
shared_axis = dict(link=NAV_GROUP, sample_rate=SR)

editor = view(
    timeruler(name="ruler", ruler="time", h=22.0, **shared_axis),

    piece.view(name="piece", weight=1.0, **shared_axis),

    # No rewind: stop already rewinds, which is what tells it from pause.
    layout(
        button(label="play/pause", name="b_play", w=110.0),
        button(label="stop", name="b_stop", w=110.0),
        label("", name="counter", text_size=2.0, weight=1.0),
        flow="row", h=40.0, gap=6.0
    ),

    title="Clausters multitrack (audio)", w=1000, h=520, flow="col"
)

#: The tree is a value and opening it is a step: `view` describes the window,
#: `open` gives back the handle its widgets are reached through.
win = editor.open()

#: **One subscription, for the whole piece.** Whatever a hand does — move a
#: clip, cross a lane, trim one, sweep a block, `q`, Delete, a fader — the
#: widget reports the piece and `piece` is already it by the time this runs.
piece.attach(win)
piece.on_change = sound_the_piece

# %% [markdown]
# ## The transport
# ``head_clock="piece"`` says it once: the buttons become `/transport_play`,
# `/transport_stop` and `/transport_locateSample`, and the host draws the line
# from the engine's own position instead of an anchor kept in step here.

# %%
transport = Transport(gui, lambda: [win["piece"].id], head_clock="piece",
                      tempo=1.0, sample_rate=SR,
                      extent=lambda: piece.extent / SR, governed=True)
transport.server = server
transport.locate(0.0)

# %% [markdown]
# ## The edits
# There are none to write. **The piece reports the piece**, so the one handler
# above is the whole driver: a move, a lane crossing, a trim, a block, `q`,
# Delete, a fader — all of them arrive as what the piece now is, and
# `sound_the_piece` puts the readers where it says.
#
# The only thing left is the click that is *not* an edit: one cursor, and it is
# the transport's.

# %%
piece.on_locate = lambda at: transport.locate(at / SR)
win["ruler"].on_event(
    lambda tag, *payload: transport.locate(float(payload[0]) / SR)
    if tag == "locate" and payload else None)

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
    text = f"{transport.position:8.3f} s   of {piece.extent / SR:.3f} s"
    if text != _shown:
        win["counter"].set(text=text)
        _shown = text
    return 0.05


win["counter"].set(text=f"{0.0:8.3f} s   of {piece.extent / SR:.3f} s")
gui.clock.sched(0.05, tick_counter)

# %%
win.wait()
session.close()
