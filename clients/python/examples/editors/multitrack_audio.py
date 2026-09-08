#!/usr/bin/env python3
"""A multitrack of audio, played by the server's transport.

The smallest window that is still a multitrack editor: a ruler in seconds, three
lanes of audio clips, play/pause, stop and a counter. It is here to check the
audio half by ear and by eye, so everything that is not audio is out of the way.

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
from clausters.gui import (Transport, button, clip, label, layout, scroll,
                           timeruler, track, view)
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

# %%
#: clip -> its lane, where it starts and how long it lasts (seconds), its take.
CLIPS = {"a": {"lane": "noise", "at": 0.0, "secs": 2.0, "take": "white"},
         "b": {"lane": "noise", "at": 6.0, "secs": 2.0, "take": "pink"},
         "c": {"lane": "tone", "at": 2.0, "secs": 2.0, "take": "glide"},
         "d": {"lane": "bass", "at": 4.0, "secs": 2.0, "take": "saw"}}

LANES = ("noise", "tone", "bass")

#: lane -> its mixer strip, written by the lane header's own gestures.
LANE_STATE = {name: {"mute": False, "solo": False, "level": 0.8} for name in LANES}


def lane_gain(lane: str) -> float:
    """Nothing when the lane is muted, nothing when another is soloed, its
    fader otherwise."""
    st = LANE_STATE[lane]
    soloing = any(s["solo"] for s in LANE_STATE.values())
    if st["mute"] or (soloing and not st["solo"]):
        return 0.0
    return st["level"]


#: clip -> the node playing it.
nodes = {}


def place(name: str):
    """Put a clip where the arrangement says it is. The first call starts the
    node, every later one is a ``/node_set`` on the node already running."""
    spec = CLIPS[name]
    args = {"at": spec["at"] * SR, "span": spec["secs"] * SR,
            "amp": 0.5 * lane_gain(spec["lane"])}
    node = nodes.get(name)
    if node is None:
        nodes[name] = Synth("reader", {"buf": BUFS[spec["take"]].bufnum, **args},
                            target=multitrack_group, server=server)
    else:
        node.set(args)


def place_all():
    for name in CLIPS:
        place(name)


place_all()
server.sync()


def extent() -> float:
    """Where the last clip ends, in seconds — a clip dragged past the end
    lengthens the arrangement."""
    return max(c["at"] + c["secs"] for c in CLIPS.values())


# %% [markdown]
# ## The window
# A ruler, the lanes, two buttons and a read-out. The ruler and the lanes share
# one navigation group, so a zoom or a pan on any of them moves all of them.

# %%
NAV_GROUP = 7
LANE_HEIGHT = 120.0

gui = session.gui()

#: No `tempo` and no `quant`: an audio arrangement has no beats to count, so the
#: ruler measures seconds.
shared_axis = dict(link=NAV_GROUP, sample_rate=SR)
lane_chrome = dict(snap=0.0, mute=False, solo=False, level=0.8, **shared_axis)


def lane_view(lane: str):
    """One lane and the clips the arrangement puts on it.

    A clip names its take by ``buffer`` — the very buffer the `reader` node
    reads. The host maps the server's segment to draw it, so the samples cross
    no wire and there is one array, in one place, for the picture and the sound.
    """
    return track(
        *[
            clip(
                name=name, offset=spec["at"] * SR,
                dur=len(TAKES[spec["take"]]),
                buffer=BUFS[spec["take"]].bufnum,
                label=spec["take"]
            ) for name, spec in CLIPS.items() if spec["lane"] == lane
        ],
        name=lane, label=lane, h=LANE_HEIGHT, **lane_chrome
    )


editor = view(
    timeruler(name="ruler", ruler="time", h=22.0, **shared_axis),

    scroll(
        *[lane_view(lane) for lane in LANES],
        name="lanes", axis="y", zoom=False, flow="col", gap=4.0,
        content_h=len(LANES) * LANE_HEIGHT + 8.0, weight=1.0
    ),

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

# %% [markdown]
# ## The transport
# ``head_clock="piece"`` says it once: the buttons become `/transport_play`,
# `/transport_stop` and `/transport_locateSample`, and the host draws the line
# from the engine's own position instead of an anchor kept in step here.

# %%
transport = Transport(gui, lambda: [win[lane].id for lane in LANES],
                      head_clock="piece", tempo=1.0, sample_rate=SR,
                      extent=extent, governed=True)
transport.server = server
transport.locate(0.0)

# %% [markdown]
# ## The edits
# A clip's move or resize, a lane's mixer strip, and a click that locates. Each
# one is a message to a node that is already running.

# %%
#: The host names widgets by id, so a report has to be looked up. A clip keeps
#: its id when it crosses lanes -- the host moves the widget, it does not build a
#: new one -- so these are made once.
CLIP_BY_ID = {win[name].id: name for name in CLIPS}
LANE_BY_ID = {win[lane].id: lane for lane in LANES}


def move(name: str, offset: float, dur: float):
    """Write a placement the host reported, in samples, and hear it."""
    CLIPS[name]["at"] = offset / SR
    CLIPS[name]["secs"] = max(dur / SR, 0.0)
    place(name)


def on_clip(name: str):
    """A clip's own two reports.

    ``"clip" offset dur start`` is a move or a resize **inside its lane**.
    ``"lane" lane offset dur start`` is the clip having **crossed the stack**,
    and it is the only report that says where a clip now *is* rather than only
    where it sits -- so it is also what keeps `lane_gain` reading the right
    strip. Handling the first and not the second leaves the node playing at the
    old place while the picture shows the new one.
    """
    def handler(tag, *vals):
        if tag == "clip" and len(vals) >= 2:
            move(name, float(vals[0]), float(vals[1]))
        elif tag == "lane" and len(vals) >= 3:
            CLIPS[name]["lane"] = LANE_BY_ID[int(vals[0])]
            move(name, float(vals[1]), float(vals[2]))
    return handler


def on_lane(name: str):
    """A lane's reports: a block edit, a click that locates, and its strip.

    ``"clips" id offset dur start …`` is **one gesture over a selection**,
    addressed to the lane the hand was on and naming every clip it held by
    widget id, wherever in the stack they sat -- one message so the owner undoes
    it in one step. From the first selection onwards this is what a drag sends
    *instead of* ``"clip"``, which is why a script that reads only the singular
    goes quietly out of step the moment a hand starts working in blocks.
    """
    def handler(tag, *vals):
        if not vals:
            return
        if tag == "clips":
            for wid, offset, dur, _start in zip(vals[0::4], vals[1::4],
                                                vals[2::4], vals[3::4]):
                move(CLIP_BY_ID[int(wid)], float(offset), float(dur))
        elif tag == "locate":
            # One cursor, and it is the transport's: a click names a time
            # wherever it lands, on a clip as much as beside it.
            transport.locate(float(vals[0]) / SR)
        elif tag in ("mute", "solo"):
            LANE_STATE[name][tag] = bool(int(vals[0]))
            win[name].set(**{tag: 1 if LANE_STATE[name][tag] else 0})
            place_all()          # a solo changes what every lane contributes
        elif tag == "level":
            LANE_STATE[name]["level"] = float(vals[0])
            win[name].set(level=LANE_STATE[name]["level"])
            place_all()
    return handler


def on_ruler(tag, *payload):
    if tag == "locate" and payload:
        transport.locate(float(payload[0]) / SR)


for _name in CLIPS:
    win[_name].on_event(on_clip(_name))
for _lane in LANES:
    win[_lane].on_event(on_lane(_lane))
win["ruler"].on_event(on_ruler)

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
    text = f"{transport.position:8.3f} s   of {extent():.3f} s"
    if text != _shown:
        win["counter"].set(text=text)
        _shown = text
    return 0.05


win["counter"].set(text=f"{0.0:8.3f} s   of {extent():.3f} s")
gui.clock.sched(0.05, tick_counter)

# %%
win.wait()
session.close()
