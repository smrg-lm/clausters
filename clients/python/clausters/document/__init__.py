# pyright: basic
"""The document: the structures every application edits, and the one place an
edit is applied.

The model lives in a Rust crate (`crates/clausters-document`) and every client
binds that one -- this client, the web client, and a ``standalone`` GUI host with
no language attached. **The tree stays there**: a client opens a `Document`, or
a `Multitrack`, applies intents to it, and asks for the JSON when it actually
wants the JSON.

The discipline is the point: the crate is the **only** thing that applies an
intent, so no client can apply an edit and then report what it did -- which is
what would let three clients mean three different things by the same gesture.

The edit
--------

- `Document` -- the tree, held by the crate. `Document.apply` hands over
  an intent and takes back what happened; `Document.snapshot` is how the JSON
  leaves, asked for rather than paid per edit.
- `History` and `Log` -- the edit pile. A `Log` is a history over one document,
  which is what an ordinary editing client wants; a `History` spans several
  structures, which is what an application showing a roll and a curve together
  needs so an undo crosses both in one order.
- `apply_intent` and `resolve_selection` -- the one-shot forms, for a caller
  that holds no document.
- `domain_edit` and `domain_coalesce_key` -- the same two questions for a
  structure that is **not** a document: a curve, a span of samples, a timeline
  of events.
- `domain_edit_all` -- the edits one gesture came to, applied as one: all of
  them or none.
- `TREE`, `MULTITRACK`, `POINTS`, `SAMPLES`, `EVENTS` -- the domain names those
  two answer for.

The multitrack
--------------

The structure the multitrack editor edits, in the field's own vocabulary:

- A **source** is samples. It lives outside the multitrack -- the session
  file's table says where -- and is never overwritten.
- A `Region` is **one placed thing**: a span of the timeline (where it starts,
  how long, its fades, which of the overlapping ones is on top) plus a
  `Content` saying what fills it. Six regions over one source are six
  identities and one source, referenced rather than copied. That is the whole
  of non-destructive editing.
- A `TakeLane` is one of a track's several contents, an ordered list of
  regions.
- A `Track` holds several take lanes and **plays one**, which is what comping
  is: record six passes into six take lanes, then take from each.
- An `Automation` is a curve over one parameter, in the multitrack's time.
- A `Multitrack` is the tracks plus what the multitrack has one of: the tempo
  map, the meter map, the markers, the loop and punch spans. They are on the
  multitrack and not on a track precisely so that no two tracks can disagree
  about them.

`Region` is the model's word; **clip** is the picture's. A clip, a track row,
a waveform are what the host draws; a region is what an edit names. Keeping
them apart is deliberate -- the multitrack's defects came from the thing drawn
and the thing addressed being one object.

The timeline
------------

`Tempo`, `Meter` and `Span` are the timeline's entries: a tempo change, a time
signature from a beat on, a half-open stretch of seconds. They are values and
hold nothing, so any structure with a timeline states them the same way.

Everything a multitrack places is placed in **seconds**: a region's position,
length and fades, every automation point, the markers, the loop and the punch.
No tempo change moves anything in it. What fills a region is measured in its
own source's units -- seconds of a recording, beats of a node -- and the two
are not the same axis. The crate makes that a type; here it is a rule the field
names say (`position` and `length` are the region's, `start` and `duration`
are its window's).

The **tempo map and the meter map** are structures the multitrack holds, not
its axis: their entries are stated at beats, in beats per second, and what
reads them is a ruler drawing beats and bars over the seconds and a snap to
them. `Multitrack.tempo_map` is that map as a `clausters.base.TempoMap`, so a
script that wants a region on bar five asks it where bar five is.

The session file
----------------

A `SessionFile` is a multitrack saved, with its source table (`Source`, or a
`FrozenSource` for samples this process does not hold) and how each window was
looking at it (`MultitrackView`, with a `TrackView` and a `TakeLaneView` per
row). The views are beside the multitrack and never inside it: nothing in them
changes what plays, and a multitrack round trips the same whether a view of it
exists or not. `FIRST_VERSION`, `SESSION_FORMAT` and `SESSION_APPLICATION` are
the numbers and the name the format itself carries.

Usage::

    from clausters.document import Content, Multitrack, Tempo

    multitrack = Multitrack()
    multitrack.set_tempo(Tempo(at=0.0, tempo=1.6))     # 96 beats a minute
    bar = multitrack.tempo_map().secs_at(4.0)          # where the second bar begins
    drums = multitrack.tracks.add("drums")
    drums.active_take_lane.regions.add(bar, 2.5, Content.onto(take))
"""

from .._native import (MULTITRACK, EVENTS, POINTS, SAMPLES, TREE, Document,
                       History, Log, domain_coalesce_key, domain_edit,
                       domain_edit_all)
from .._native import document_apply as apply_intent
from .._native import document_resolve as resolve_selection
from ._format import FIRST_VERSION, SESSION_APPLICATION, SESSION_FORMAT
from ._multitrack import (Automation, Content, Curves, Fade, Marker, Markers,
                          Multitrack, Region, Regions, TakeLane, TakeLanes, Track,
                          Tracks)
from ._session_file import FrozenSource, SessionFile, Source
from ._timeline import Meter, Span, Tempo
from ._view import MultitrackView, TakeLaneView, TrackView

__all__ = [
    "MULTITRACK",
    "EVENTS",
    "FIRST_VERSION",
    "SESSION_FORMAT",
    "SESSION_APPLICATION",
    "POINTS",
    "SAMPLES",
    "TREE",
    "Document",
    "History",
    "Log",
    "apply_intent",
    "domain_coalesce_key",
    "domain_edit",
    "domain_edit_all",
    "resolve_selection",
    "Automation",
    "Content",
    "Curves",
    "Fade",
    "FrozenSource",
    "Marker",
    "Markers",
    "Meter",
    "Multitrack",
    "MultitrackView",
    "Region",
    "Regions",
    "SessionFile",
    "Source",
    "Span",
    "TakeLane",
    "TakeLanes",
    "TakeLaneView",
    "Tempo",
    "Track",
    "Tracks",
    "TrackView",
]
