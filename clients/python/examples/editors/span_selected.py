# Manual check: span, selected and transport in the three editors.
# Run the cells in order inside one Python session (clausters-editor or
# `python -i`). Each block says what to look at. The multitrack's pass over a
# range is its transport's `play(start, end, looping=...)`, and what a script
# holds of it is the multitrack itself, the window's while it is open.

# %% Setup: one server, one host
import math

import pathlib

from clausters import Session, SoundFile
from clausters.gui import edit
from clausters.multitrack import Content, Multitrack, Source
from clausters.multitrack import Session as SavedSession
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

# %% MULTITRACK 1: two takes in two tracks, saved as a session
out = pathlib.Path(__file__).resolve().parents[1] / "out"
out.mkdir(exist_ok=True)
frames = int(2.0 * rate)
for name, freq in (("span_selected-660.wav", 660.0), ("span_selected-990.wav", 990.0)):
    SoundFile.write(out / name, [0.3 * math.sin(2 * math.pi * freq * i / rate)
                                 for i in range(frames)], 1, rate, sample_format="float")
mt = Multitrack()
a = mt.tracks.add("one").active_take_lane.regions.add(
    0.0, 2.0, Content.onto({"source": {"source": 1, "lifetime": "session",
                                       "generation": 0},
                            "start": 0.0, "duration": 2.0}), name="660")
b = mt.tracks.add("two").active_take_lane.regions.add(
    2.0, 2.0, Content.onto({"source": {"source": 2, "lifetime": "session",
                                       "generation": 0},
                            "start": 0.0, "duration": 2.0}), name="990")
saved = SavedSession(multitrack=mt)
saved.sources[1] = Source.file("span_selected-660.wav").shaped(1, frames, rate)
saved.sources[2] = Source.file("span_selected-990.wav").shaped(1, frames, rate)
saved.save(out / "span_selected.json")
m = edit(saved, title="multitrack")
mtt = m.transport
mtt.play()                # 660 then 990, the line moves

# %% MULTITRACK 2: a pass over a range, looped, from the script
mtt.stop()
mtt.play(1.0, 3.0, looping=True)  # end of 660, start of 990, repeated

# %% MULTITRACK 3: stop the loop; the script's multitrack is the window's
mtt.stop()
b.position = 2.5          # the 990 box moves in the window
print(a.name, b.position)

# %% AUDIO 1: a take, its transport
take = SoundFile.write(out / "span_selected.wav",
                       [math.exp(-1.5 * i / frames) * 0.5
                        * math.sin(2 * math.pi * 880.0 * i / rate)
                        for i in range(frames)], 1, rate, sample_format="float")
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
e.select(Segment(e.file, int(1.2 * rate), 0.4))
print(at.span)            # (1.2, 1.6)

# %% AUDIO 4: drag a range by hand, then run this
print(at.span, e.selected)
e.unselect()              # the band goes away
print(at.span)            # None

# %% Close
session.close()
