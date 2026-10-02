# Manual check: span, selected and transport in the three editors.
# Run the cells in order inside one Python session (clausters-editor or
# `python -i`). Each block says what to look at.

# %% Setup: one server, one host
import math

from clausters import Session
from clausters.defs import Buffer
from clausters.gui import edit
from clausters.multitrack import Content, Multitrack, Region, TakeLane, Track
from clausters.segments import Segment
from clausters.seq import EventSequence
from clausters.seq.event import Event

session = Session.live().activate()
session.gui()
server = session.server
rate = server.query_info().nominal_sample_rate

# %% NOTES 1: open a roll and take its transport
notes = EventSequence([(b, Event(midinote=m, dur=0.5, amp=0.2))
                       for b, m in [(0, 72), (1, 76), (2, 79), (3, 84)]])
n = edit(notes, title="notes")
t = n.transport
t.play()                  # sounds the four notes, the roll's line moves

# %% NOTES 2: a span set from the script is drawn on the roll and played
t.stop()
t.span = (1.0, 3.0)       # the band appears over beats 1..3
t.loop()                  # the loop switch turns on in the window
t.play()                  # repeats the 2nd and 3rd notes

# %% NOTES 3: unloop, pause, locate
t.unloop()                # the switch turns off; the pass ends at beat 3
t.pause()
t.locate(0.0)
print(t.span, t.looping)  # (1.0, 3.0) False

# %% NOTES 4: Alt+drag a new band on the roll by hand, then run this
print(t.span)             # the band you swept, in beats

# %% NOTES 5: selected from the script and by hand
n.select(notes.events[:2])    # the first two notes are marked
print(n.selected)
# now click/marquee other notes by hand and run:
print(n.selected)
n.unselect()                  # nothing marked

# %% MULTITRACK 1: two takes in two tracks
frames = int(2.0 * rate)
high = Buffer.from_samples([0.3 * math.sin(2 * math.pi * 660.0 * i / rate)
                            for i in range(frames)], 1, rate, server=server)
higher = Buffer.from_samples([0.3 * math.sin(2 * math.pi * 990.0 * i / rate)
                              for i in range(frames)], 1, rate, server=server)
server.sync()
a = Region(id=20, position=0.0, length=2.0, name="660",
           content=Content.onto({"source": {"source": 1, "lifetime": "session",
                                            "generation": 0},
                                 "start": 0.0, "duration": 2.0}))
b = Region(id=21, position=2.0, length=2.0, name="990",
           content=Content.onto({"source": {"source": 2, "lifetime": "session",
                                            "generation": 0},
                                 "start": 0.0, "duration": 2.0}))
mt = Multitrack(tracks=[Track(id=10, name="one", take_lanes=[TakeLane(id=11, regions=[a])]),
                        Track(id=12, name="two", take_lanes=[TakeLane(id=13, regions=[b])])])
m = edit(mt, sample_rate=rate, server=server, sources={1: high, 2: higher},
         title="multitrack")
mtt = m.transport
mtt.play()                # 660 then 990, the line moves

# %% MULTITRACK 2: span and loop from the script
mtt.stop()
mtt.span = (1.0, 3.0)     # the band appears over 1..3 s
mtt.loop()                # L turns on
mtt.play()                # end of 660, start of 990, repeated

# %% MULTITRACK 3: stop the loop; sweep by hand (Alt+drag) and read it
mtt.unloop()
mtt.stop()
print(mtt.span)           # the band you swept, in seconds

# %% MULTITRACK 4: selected
m.select([b])             # the 990 box is held
print(m.selected)
# click the other box by hand and run:
print(m.selected)
m.unselect()

# %% AUDIO 1: a take, its transport
take = Buffer.from_samples([math.exp(-1.5 * i / frames) * 0.5
                            * math.sin(2 * math.pi * 880.0 * i / rate)
                            for i in range(frames)], 1, rate, server=server)
server.sync()
e = edit(take, title="take")
at = e.transport
at.play()                 # the whole take

# %% AUDIO 2: span from the script = what is selected
at.stop()
at.span = (0.5, 1.0)      # the band appears over 0.5..1.0 s
print(e.selected)         # a Segment from frame 0.5*rate, 0.5 s
at.play()                 # only that half second

# %% AUDIO 3: loop, then select a Segment
at.loop()
at.play()                 # repeats 0.5..1.0
at.stop()
at.unloop()
e.select(Segment(e.buffer, int(1.2 * rate), 0.4))
print(at.span)            # (1.2, 1.6)

# %% AUDIO 4: drag a range by hand, then run this
print(at.span, e.selected)
e.unselect()              # the band goes away
print(at.span)            # None

# %% Close
session.close()
