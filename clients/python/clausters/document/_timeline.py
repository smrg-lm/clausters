# pyright: basic
"""The timeline's entries: a tempo, a meter, a span.

Private: the names are `clausters.document`'s.
"""

from dataclasses import dataclass, field

from ._format import _unknown_fields


@dataclass
class Tempo:
    """One entry of the tempo map: from here on, this tempo.

    `at` is a beat and `tempo` is beats per **second**, the unit every tempo in
    clausters is in; beats per minute is only how a ruler or a text field may
    show it. `ramp` says the tempo runs from here to the next entry rather than
    stepping. A ritardando is a ramp; a section change is a step.
    """

    at: float
    tempo: float
    ramp: bool = False
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"at": self.at, "tempo": self.tempo}
        if self.ramp:
            out["ramp"] = True
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Tempo":
        return cls(at=float(written.get("at", 0.0)), tempo=float(written["tempo"]),
                   ramp=bool(written.get("ramp", False)),
                   extra=_unknown_fields(written, "at", "tempo", "ramp"))


@dataclass
class Meter:
    """One entry of the meter map: from here on, this time signature.

    It says how beats make **bars**, which is what a ruler draws and what a
    snap-to-bar means. It never moves a beat: a meter change does not shift what
    is placed, it re-bars it.
    """

    at: float
    beats: int
    unit: int
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"at": self.at, "beats": self.beats, "unit": self.unit}
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Meter":
        return cls(at=float(written.get("at", 0.0)),
                   beats=int(written["beats"]), unit=int(written["unit"]),
                   extra=_unknown_fields(written, "at", "beats", "unit"))


@dataclass
class Span:
    """A span of the timeline: the loop, the punch, a named region of the multitrack.

    Half-open, so two spans that meet cover no instant twice. In seconds.
    """

    start: float
    end: float

    @property
    def length(self) -> float:
        return max(0.0, self.end - self.start)

    def write(self) -> dict:
        return {"start": self.start, "end": self.end}

    @classmethod
    def read(cls, written: dict) -> "Span":
        return cls(start=float(written.get("start", 0.0)),
                   end=float(written.get("end", 0.0)))

