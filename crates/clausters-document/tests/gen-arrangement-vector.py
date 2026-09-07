"""Write the parity vector the Rust suite reads: an arrangement built with the
Python client's `clausters.arrangement`, as it writes it.

Nothing checks that the two sides agree on the format unless something crosses
between them, and no build ever reaches this client's call sites. So the vector
is generated here, committed, and parsed by `tests/arrangement_parity.rs`: if
either side moves, that test fails instead of a user finding out with a session
that will not open.

What it deliberately covers is everything a whole-value comparison would not
name if it broke one of them: a track comped from three takes playing the
second, two regions overlapping with a crossfade and a layer order saying which
is on top, a composite region placing the general tree, an automation curve
whose point shapes nothing here reads, a tempo map that ramps, a meter change,
markers sharing a beat, a loop and a punch, and a field a newer writer added.

Run from the repo root, and commit whatever moves:

    python3 crates/clausters-document/tests/gen-arrangement-vector.py
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[3] / "clients/python"))

from clausters.arrangement import (Arrangement, Automation, Content, Fade,  # noqa: E402
                                   Lane, Marker, Meter, Region, Session, Span,
                                   Source, Tempo, Track)


def window(source: int, start: float = 0.0, duration: float = 4.0) -> dict:
    """A window onto samples, the way the document names one."""
    return {
        "source": {"source": source, "lifetime": "session", "generation": 0},
        "start": start,
        "duration": duration,
    }


def build() -> Arrangement:
    piece = Arrangement()
    piece.set_tempo(Tempo(at=0.0, bpm=96.0))
    piece.set_tempo(Tempo(at=32.0, bpm=120.0, ramp=True))
    piece.set_meter(Meter(at=0.0, beats=4, unit=4))
    piece.set_meter(Meter(at=32.0, beats=7, unit=8))
    piece.add_marker(Marker(id=1, at=0.0, name="intro"))
    piece.add_marker(Marker(id=2, at=32.0, name="B"))
    piece.loop_span = Span(start=0.0, end=32.0)
    piece.punch = Span(start=8.0, end=16.0)

    # Comped from three takes, playing the second.
    vocals = Track(id=10, name="vocals", active=1, lanes=[
        Lane(id=11, name="take 1"), Lane(id=12, name="take 2"),
        Lane(id=13, name="comp"),
    ])
    for index, lane in enumerate(vocals.lanes):
        lane.place(Region(id=20 + index, position=0.0, length=16.0,
                          name=f"vox {index}",
                          content=Content.onto(window(100 + index))))

    # Two regions overlapping, crossfaded, the layer saying which is on top.
    guitars = Track(id=30, name="guitars", soloed=True, lanes=[Lane(id=31)])
    guitars.lanes[0].place(Region(
        id=32, position=0.0, length=20.0, content=Content.onto(window(200)),
        fade_out=Fade(length=4.0)))
    guitars.lanes[0].place(Region(
        id=33, position=16.0, length=16.0, layer=1, muted=True,
        content=Content.onto(window(201, start=2.0), playrate=1.5,
                             args={"seed": 7}),
        fade_in=Fade(length=4.0, shape={"curve": "exp"})))
    guitars.automation.append(Automation(
        id=34, name="level", target={"ctl": "level"}, visible=True,
        points=[{"at": 0.0, "value": 0.0, "data": {}},
                {"at": 16.0, "value": 1.0, "data": {"shape": "exp"}}]))

    # The general tree, placed: what a composite region is for.
    sections = Track(id=40, name="sections", lanes=[Lane(id=41)])
    sections.lanes[0].place(Region(
        id=42, position=32.0, length=16.0,
        content=Content.composite({
            "id": 43,
            "kind": "aggregate",
            "grouping": "concrete",
            "members": [{"offset": 0.0, "node": {"id": 44, "kind": "clang"}}],
        })))

    # A field a newer writer added, on the region and on the piece.
    sections.lanes[0].regions[0].extra["warp"] = {"mode": "beats"}
    piece.extra["groove"] = {"name": "mpc60"}

    piece.tracks.extend([vocals, guitars, sections])
    return piece


def saved() -> Session:
    """The same piece as a **session**: the arrangement plus where its samples
    are, which is the half the piece deliberately does not carry.

    It covers the three states a table has to be able to say, because a save
    that could not say them would either block or decide for the person: a file
    that is there, samples nobody wrote down, and a working copy whose
    destructive edit is still open.
    """
    session = Session(arrangement=build(), provenance={"script": "make.py"})
    for source in (100, 101, 102, 200):
        session.sources[source] = Source.file(f"takes/{source}.wav").shaped(
            2, 480_000, 48_000.0)
    session.sources[200].provenance = {"def": "sines"}
    session.sources[201] = Source.volatile()
    session.sources[300] = Source.file("scratch/300.wav", lifetime="temporary")
    session.sources[300].editing = {"from": 200, "confirmed": False}
    return session


if __name__ == "__main__":
    here = pathlib.Path(__file__).resolve().parent
    (here / "arrangement_vector.json").write_text(
        json.dumps(build().write(), indent=1) + "\n")
    (here / "arrangement_session_vector.json").write_text(
        json.dumps(saved().write(), indent=1) + "\n")
    print(f"wrote {here / 'arrangement_vector.json'} and its session")
