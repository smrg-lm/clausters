"""Editing a symbolic score on its engraved page: the score editor.

What it opens is a `clausters.gui.notation.Score` -- notation held as MEI, its
model a sheet. **The score editor is an application of the GUI host's**, as the
audio editor and the multitrack are: the window, what each gesture does to the
score, the verbs over what is selected, the entry each one leaves, the engraver
and the playback all run in the host's process, and this is its handler
(`clausters.gui.editing.HostedEditor`). Each verb below is one call to the
application, named as it names it.

**The script's score is the window's while one is open**: `Score.mei` and
`Score.sheet` read what the window holds, and a change through it is an edit of
the window's. When the window goes, the score holds what it left it as.
"""

from __future__ import annotations

from ...seq.sequence import EventSequence
from .events import SequenceHandler, _held_of
from .hosted import HostedTransport, _Held
from .samples import _plain


class ScoreEditor(SequenceHandler):
    """A symbolic score on its page, edited by hand, in place.

    A press on a note selects it, a drag moves it along its staff, and a press
    on a staff selects its measure. **Note entry** is a mode (`entry`, the
    toolbar's pencil, or N; Escape leaves it): an edit cursor stands on a
    staff, in a voice, and what is entered is written there over what was
    there, nothing after it moving -- a letter `a` to `g` writes that pitch of
    `value` and the cursor goes on, Shift and a letter adds it to the chord, a
    press on a staff writes at the time it fell at (on a note, into its
    chord), the arrows move the cursor and Ctrl+Alt+1 to 4 change its voice.
    Playing leaves the mode. The menu bar holds every action, and the toolbar what a hand
    reaches for while it writes: the value, its dot, a rest, an accidental,
    the articulations, a tie, a triplet, the voice and the layout. Beside the
    page stand the palettes: what can be written, a kind of element to a
    folding group, each entry a verb over what is selected. The space bar,
    the toolbar's transport and the Play menu play the score in the
    application, from where the selection starts, with the page's cursor following
    (`play`, `stop`, `transport`). An entry
    that needs more than a pick -- the page's text, its margins, a
    transformation's parameter -- opens a dialog over the window. Ctrl+click
    adds a note to the selection or takes it out, and
    Shift+click extends the selection to it, in time and across the staves
    between. The verbs act on what is selected (`selected`, `select`); each is
    one entry of the history, so Ctrl+Z over the window walks them back.

    Args:
        score: the `clausters.gui.notation.Score` to edit. It is the edited
            one: read it after any gesture. Left out, the window opens with no
            document, its File menu making or opening one (`new_file`, `open_file`).
        title: the window's title.
        value: the written value a note entered on the page takes, as
            ``(numerator, denominator)`` of a whole note; a quarter by default.
        chrome: ``False`` opens the page alone, in its scroll, over the
            status line: no menu bar, no toolbar, no palettes and no dialogs,
            none of them composed -- the window a script wants when it edits
            through this handle and reads the score back. The keys stay (N
            and note entry, the arrows, Delete, the space bar, Ctrl+Z; F1
            shows them all), and so does every verb below. A verb refused
            answers ``False`` and says why on the status line and in
            `refused`. What a form
            asked -- the path of a first Ctrl+S -- is asked of the handle
            instead (`save`), and the window's close mark closes it at once,
            since the score it edits is the one this handle holds.
    """

    _app = "score"

    def __init__(self, score=None, *, title: str = "Score", value=None,
                 width: "int | None" = None, height: "int | None" = None,
                 chrome: bool = True, over: "EventSequence | None" = None,
                 how: "dict | None" = None, interp: "dict | None" = None):
        super().__init__(title=title, width=width, height=height, chrome=chrome)
        self._score = score
        self._refused: "str | None" = None
        self._value = value
        self._how = how
        self._interp = interp
        #: The sequence this page is the reading of, in an editor opened over
        #: one (`over`); ``None`` for an editor over a score of its own.
        self.sequence: "EventSequence | None" = over
        self._shown = over
        #: What the editor edits: the sequence it is over, or the score.
        self.structure = score if over is None else over

    def _held(self) -> _Held:
        if self.sequence is not None:
            return _held_of(self.sequence)
        if self._score is None:
            return self.__dict__.setdefault("_own_windows", _Held())
        held = self._score.__dict__.get("_windows")
        if held is None:
            held = self._score.__dict__["_windows"] = _Held()
        return held

    def _options(self, beside) -> dict:
        if self._score is None:
            return {}
        options: dict = {"mei": self._score.mei()}
        path = getattr(self._score, "path", None)
        if path is not None and self.sequence is None:
            options["path"] = str(path)
        if self._value is not None:
            options["value"] = [int(self._value[0]), int(self._value[1])]
        if self.sequence is not None:
            if self._how:
                options["how"] = dict(self._how)
            if self._interp is not None:
                options["interp"] = self._interp
            if beside is None:
                options["sequence"] = self.sequence.data()
        return options

    def open(self, host=None):
        """Open the window, its play cursor drawn from the transport of its
        own the application gives it. A pass ends where the music does."""
        opened = self.closed
        window = super().open(host)
        # the page reads and writes the window's from now on
        if opened:
            if self._score is not None:
                self._score.__dict__["_page"] = self
            self._do("end", end="contents")
        return window

    def _take(self) -> None:
        if self.sequence is not None:
            super()._take()

    def _settle(self, left: "dict | None", last: bool) -> None:
        if self._score is not None and self._score.__dict__.get("_page") is self:
            self._score.__dict__.pop("_page", None)
            self._score._settle(left)
        if last and self.sequence is not None:
            self._let_go(left)

    @classmethod
    def over(cls, sequence, *, interp: "dict | None" = None, **options) -> "ScoreEditor":
        """**The score editor over a sequence**, on the page it is read into
        (`clausters.gui.notation.Score.from_events`): what
        ``edit(sequence, view="score")`` opens.

        The sequence is the structure and the page a reading of it. Opening
        changes nothing: a take keeps the times it was played with. **An edit
        on the page changes in the sequence only what it changed on the
        page** -- the note moved, marked or written, in its notation keys and
        in what it sounds -- and every other event stays as it was, with its
        time, its level and its curves. The entry is the sequence's, so a
        roll open over the same sequence and this page are one undo order,
        and each follows what the other does.

        The transcription's keys among ``options`` (``meter``, ``key``,
        ``clef``, ``beat_unit``, ``division``, ``tuplets``, ``voices``,
        ``dynamics``) say how the sequence is read where its events do not,
        and ``interp`` is the reading; the rest are the editor's own. The
        editor's `structure` is the sequence, and `score` the page."""
        from ..notation import TRANSCRIPTION_KEYS, Score

        how = {key: options.pop(key) for key in TRANSCRIPTION_KEYS if key in options}
        how = {key: value for key, value in how.items() if value is not None}
        page = Score.from_events(sequence, interp=interp, **how)
        return cls(page, over=sequence, how=how, interp=interp, **options)

    @property
    def score(self):
        """The score on the page: the one the editor was opened over, or --
        over a sequence -- the reading of it, which follows the sequence;
        ``None`` for an editor opened over no score."""
        return self._score

    @property
    def refused(self) -> "str | None":
        """**Why the last verb was refused**, or ``None`` when it was not: the
        reason the status line shows, here for a script, which can tell a
        refusal it can do something about (nothing selected) from one it
        cannot. Every verb that answers ``False`` sets it, and every verb that
        goes through clears it."""
        return self._refused

    # ---- what is selected, and the value in hand ----

    @property
    def selected(self) -> list:
        """The selected items, as the model names them (`Score.sheet`'s item
        ids), each once."""
        return [int(i) for i in self._call("selected").get("items") or ()]

    def select(self, elements) -> None:
        """Select the page's elements ``elements`` (their ``xml:id``\\s, as the
        page reports them), or nothing with an empty list."""
        self._do("set", call={"verb": "select", "elements": [str(e) for e in elements or ()]})

    @property
    def value(self) -> tuple:
        """The written value a note entered on the page takes, as
        ``(numerator, denominator)`` of a whole note: ``(1, 4)`` is a quarter.
        Set it to write another: ``editor.value = (1, 8)``."""
        numerator, denominator = self._call("value").get("value") or (1, 4)
        return int(numerator), int(denominator)

    @value.setter
    def value(self, value) -> None:
        self._set(value=[int(value[0]), int(value[1])])

    @property
    def entry(self) -> bool:
        """Whether the window is in note entry, where the keys and a press on a
        staff write at the edit cursor. Off by default, and off once a pass
        plays; outside it a press on a staff selects the measure it fell in.
        Set it to switch: ``editor.entry = True``."""
        return bool(self._call("entry").get("entry", False))

    @entry.setter
    def entry(self, on: bool) -> None:
        self._set(entry=bool(on))

    @property
    def dotted(self) -> bool:
        """Whether the value a note is entered with is dotted: half as long
        again. Set it to switch: ``editor.dotted = True``."""
        return bool(self._call("input").get("dotted", False))

    @dotted.setter
    def dotted(self, on: bool) -> None:
        self._set(dotted=bool(on))

    @property
    def rest(self) -> bool:
        """Whether a press on empty staff writes a rest of `value` rather than
        a note. Set it to switch: ``editor.rest = True``."""
        return bool(self._call("input").get("rest", False))

    @rest.setter
    def rest(self, on: bool) -> None:
        self._set(rest=bool(on))

    @property
    def next_accidental(self) -> "int | None":
        """The accidental the next note entered takes, in semitones from its
        letter (``1`` a sharp, ``-1`` a flat, ``0`` a natural), or ``None``.
        It is for that one note: writing it lets the accidental go. Set it to
        arm one: ``editor.next_accidental = 1``. (`accidental` is the verb
        over what is selected.)"""
        armed = self._call("input").get("accidental")
        return None if armed is None else int(armed)

    @next_accidental.setter
    def next_accidental(self, alter: "int | None") -> None:
        self._set(accidental=None if alter is None else int(alter))

    # ---- the layout, which is the window's, and the page, the document's ----

    @property
    def layout(self) -> str:
        """How the window looks at the score: ``"page"``, every page of the
        paper one under another, fixed whatever the window's size; or
        ``"continuous"``, one system as long as the music, with no page. Set it
        to switch: ``editor.layout = "continuous"``. It is the window's, not
        the score's, and enters no history."""
        return str(self._call("layout").get("layout", "page"))

    @layout.setter
    def layout(self, layout: str) -> None:
        self._set(layout=str(layout))

    @property
    def page(self) -> dict:
        """The page the score is laid out on: ``{"page", "paper", "landscape",
        "papers"}`` -- the setup itself (``width``, ``height`` and ``margins``
        in tenths of a millimetre, ``staff`` in hundredths), the name of its
        paper when it is a known one, which way up it is, and the names of the
        papers there are. Change it with `set_page`."""
        return self._call("page")

    def set_page(self, paper: "str | None" = None, *, landscape: "bool | None" = None,
                 width: "int | None" = None, height: "int | None" = None,
                 margins=None, staff: "int | None" = None) -> bool:
        """Lay the score out on another page, as one entry of the history: a
        ``paper`` by name (``"A4"``, ``"Letter"``, ``"Octavo"`` ... -- see
        `page`), turned with ``landscape``, or a ``width`` and ``height`` of
        its own; the ``margins`` (top, right, bottom, left) and the ``staff``
        height. Lengths are in tenths of a millimetre and the staff in
        hundredths (``720`` is 7.2 mm). What is left out stays as it is. The
        setup is the score's, and travels in its MEI."""
        call = {"action": "page"}
        for key, value in (("paper", paper), ("landscape", landscape),
                           ("width", width), ("height", height), ("staff", staff)):
            if value is not None:
                call[key] = value
        if margins is not None:
            call["margins"] = [int(m) for m in margins]
        return self._act(call)

    def set_text(self, field: str, text: "str | None" = None, *,
                 index: "int | None" = None, region: "str | None" = None,
                 halign: "str | None" = None, valign: "str | None" = None,
                 pages: "str | None" = None) -> bool:
        """Write a text of the page, or move it, as one entry of the history.

        ``field`` is ``"title"``, ``"subtitle"``, ``"composer"``,
        ``"arranger"``, ``"lyricist"``, ``"translator"``, ``"copyright"`` or
        ``"note"`` -- a footnote: ``index`` says which, from zero, and none adds
        one. ``text`` writes it, and an empty one takes it away. ``region``
        (``"head"``, ``"foot"``), ``halign`` (``"left"``, ``"center"``,
        ``"right"``), ``valign`` (``"top"``, ``"middle"``, ``"bottom"``) and
        ``pages`` (``"first"``, ``"all"``) put it in a cell of the page's head
        or foot; what is left out stays as it is, and a field nobody moved
        sits where the printed page puts it. A press on a text names its field
        on the status line."""
        call = {"action": "text", "field": str(field)}
        for key, value in (("text", text), ("index", index), ("region", region),
                           ("halign", halign), ("valign", valign), ("pages", pages)):
            if value is not None:
                call[key] = value
        return self._act(call)

    # ---- the verbs, over what is selected ----

    def move(self, steps: int) -> bool:
        """Move the selected notes ``steps`` diatonic steps along their staves,
        up when positive -- each takes the key signature's alteration for the
        letter it lands on."""
        return self._act({"action": "move", "steps": int(steps)})

    def scale(self, numerator: int, denominator: int) -> bool:
        """Scale the selected items' written values by ``numerator /
        denominator`` (``scale(2, 1)`` is twice as long), against the barlines
        already there."""
        return self._act({"action": "scale", "factor": [int(numerator), int(denominator)]})

    def articulation(self, name: str) -> bool:
        """Give the selected notes an articulation (by its MEI name: ``stacc``,
        ``acc``, ``ten``, ``marc``...), or take it away when all of them have
        it."""
        return self._act({"action": "articulation", "name": str(name)})

    def dynamic(self, name: "str | None" = None) -> bool:
        """Put a dynamic (``pp`` ... ``ff``) under the first selected note, or
        take it away with none."""
        return self._act({"action": "dynamic", "name": name})

    def ornament(self, name: "str | None" = None) -> bool:
        """Give the selected notes an ornament (``trill``, ``mordent``,
        ``turn``, ``fermata``), or take it away with none."""
        return self._act({"action": "ornament", "name": name})

    def clear_marks(self) -> bool:
        """Take every mark off the selected notes."""
        return self._act({"action": "clear_marks"})

    def tie(self) -> bool:
        """Tie the selected notes to the next, or untie them when the first is
        tied already."""
        return self._act({"action": "tie"})

    def silence(self) -> bool:
        """Turn the selected notes into rests of the same length."""
        return self._act({"action": "silence"})

    def select_left(self) -> bool:
        """Select the item before -- the window's ``select_left`` (`verb`)."""
        return self.verb("select_left")

    def select_right(self) -> bool:
        """Select the item after -- the window's ``select_right`` (`verb`)."""
        return self.verb("select_right")

    def cursor_left(self) -> bool:
        """In note entry: the cursor back -- the window's ``cursor_left`` (`verb`)."""
        return self.verb("cursor_left")

    def cursor_right(self) -> bool:
        """In note entry: the cursor forward -- the window's ``cursor_right`` (`verb`)."""
        return self.verb("cursor_right")

    def bar_left(self) -> bool:
        """In note entry: the cursor to the bar before -- the window's ``bar_left`` (`verb`)."""
        return self.verb("bar_left")

    def bar_right(self) -> bool:
        """In note entry: the cursor to the bar after -- the window's ``bar_right`` (`verb`)."""
        return self.verb("bar_right")

    def staff_up(self) -> bool:
        """In note entry: the cursor to the staff above -- the window's ``staff_up`` (`verb`)."""
        return self.verb("staff_up")

    def staff_down(self) -> bool:
        """In note entry: the cursor to the staff below -- the window's ``staff_down`` (`verb`)."""
        return self.verb("staff_down")

    def enter_rest(self) -> bool:
        """In note entry: write a rest at the cursor -- the window's ``enter_rest`` (`verb`)."""
        return self.verb("enter_rest")

    def write(self, pitch: str) -> bool:
        """In note entry: **write the note ``pitch``** (``"A"`` to ``"G"``) at
        the cursor, nearest the note before -- the window's ``pitch_a`` ...
        ``pitch_g`` (`verb`)."""
        return self.verb(f"pitch_{_letter(pitch)}")

    def chord(self, pitch: str) -> bool:
        """In note entry: **add ``pitch``** (``"A"`` to ``"G"``) to the chord
        just written -- the window's ``chord_a`` ... ``chord_g`` (`verb`)."""
        return self.verb(f"chord_{_letter(pitch)}")

    def entry_voice(self, voice: int) -> bool:
        """In note entry: **write in voice ``voice``** (1 to 4) from here on --
        the window's ``voice_1`` ... ``voice_4`` (`verb`)."""
        if int(voice) not in (1, 2, 3, 4):
            raise ValueError(f"a voice is 1 to 4, not {voice!r}")
        return self.verb(f"voice_{int(voice)}")

    def delete(self) -> bool:
        """Remove the selected items; what follows them moves earlier."""
        return self._act({"action": "delete"})

    def voice(self, to: "int | None" = None) -> bool:
        """Move the selected items into the other voice of their staff, or into
        voice ``to`` (from zero) when one is named, leaving rests where they
        were."""
        call: dict = {"action": "voice"}
        if to is not None:
            call["to"] = int(to)
        return self._act(call)

    def grace(self, kind: "str | None" = None) -> bool:
        """Make the selected notes grace notes -- ``"acc"``, an appoggiatura,
        or ``"unacc"``, an acciaccatura -- or notes of the bar again with
        none."""
        return self._act({"action": "grace", "kind": kind})

    def accidental(self, alter: int) -> bool:
        """Give the selected notes an accidental: ``alter`` semitones from the
        letter (``1`` a sharp, ``-1`` a flat, ``0`` a natural, ``2`` and ``-2``
        the doubles), printed whatever the key says."""
        return self._act({"action": "accidental", "alter": int(alter)})

    def insert_measures(self, count: int = 1, *, after: bool = False) -> bool:
        """Open ``count`` empty measures before the first selected measure, or
        after the last with ``after``; the music past them moves along."""
        return self._act({"action": "measures",
                          "edit": "insert_after" if after else "insert_before",
                          "count": int(count)})

    def remove_measures(self) -> bool:
        """Take out the measures the selection covers, with what is written in
        them."""
        return self._act({"action": "measures", "edit": "remove"})

    def set_barline(self, kind: str) -> bool:
        """Give the last selected measure a right barline: ``single``,
        ``dbl``, ``end``, ``rptstart``, ``rptend``, ``rptboth`` or
        ``invis``."""
        return self._act({"action": "barline", "kind": str(kind)})

    def set_break(self, kind: str) -> bool:
        """Break the line or the page before the first selected measure
        (``system``, ``page``), or take the break back (``none``)."""
        return self._act({"action": "break", "kind": str(kind)})

    def set_meter(self, count: int, unit: int) -> bool:
        """Change the meter from the first selected measure on: ``count`` beats
        of ``unit`` (``set_meter(3, 4)`` is three quarters)."""
        return self._act({"action": "meter", "count": int(count), "unit": int(unit)})

    def spanner(self, kind: str, join: "bool | None" = None) -> bool:
        """A ``slur``, a ``crescendo`` or a ``diminuendo`` from the first
        selected item to the last, in time. A slur says whether it joins its
        notes into one line with no new attack (``join``): attacked unless it
        says so, and drawing it again with the other answer changes it."""
        call = {"action": "spanner", "kind": str(kind)}
        if join is not None:
            call["join"] = bool(join)
        return self._act(call)

    def mark(self, name: str, value=True) -> bool:
        """A mark on the selected notes, by its name: ``tremolo`` (strokes, 1
        to 3), ``arpeggio`` (``up``, ``down``), ``breath`` (``breath``,
        ``caesura``), ``ring`` (``True``), ``fingering`` and ``harmony``
        (text). A value every one of them has already takes it away, and so
        does ``None``."""
        return self._act({"action": "mark", "mark": str(name), "value": value})

    def lyric(self, text: str, verse: int = 1) -> bool:
        """A syllable of the lyrics under the first selected note, in
        ``verse``; one that ends in ``-`` runs on into the next, and an empty
        one takes it away."""
        return self._act({"action": "lyric", "text": str(text), "verse": int(verse)})

    def beat_repeat(self) -> bool:
        """Draw the selected notes as repeats of the beat before each, which
        they then hold -- or as themselves again."""
        return self._act({"action": "beat_repeat"})

    def control(self, kind: str, text: str = "", bpm: float | None = None) -> bool:
        """Write at the first selected item a ``tempo`` (with its speed
        ``bpm``, in quarter notes a minute), a ``dir`` or a ``reh``; with no
        text and no speed, take it back."""
        return self._act({"action": "control", "kind": str(kind), "text": str(text),
                          "bpm": None if bpm is None else float(bpm)})

    def set_key(self, key: str) -> bool:
        """Change the key from the first selected measure on (``"D"``,
        ``"Bb"``), or take a change back with ``"none"``."""
        return self._act({"action": "key", "key": str(key)})

    def set_clef(self, clef: str) -> bool:
        """Change the clef where the first selected item starts, on its staff
        (``"G2"``, ``"F4"``, ``"C3"``), or take it back with ``"none"``."""
        return self._act({"action": "clef", "clef": str(clef)})

    def set_ending(self, label: str = "") -> bool:
        """Mark the selected measures as an ending played in the passes
        ``label`` names (``"1"``, ``"2"``); empty takes it back."""
        return self._act({"action": "ending", "label": str(label)})

    def navigation(self, kind: str) -> bool:
        """A navigation mark: ``segno`` and ``coda`` on the first selected
        measure, ``fine``, ``dacapo``, ``dalsegno`` and ``tocoda`` on the last;
        ``none`` takes them off both."""
        return self._act({"action": "navigation", "kind": str(kind)})

    def measure_repeat(self) -> bool:
        """Write each selected measure as a repeat of the one before, or as
        itself again when every one already is."""
        return self._act({"action": "measure_repeat"})

    def multirests(self) -> bool:
        """Draw runs of empty measures as one numbered rest, or each as
        itself."""
        return self._act({"action": "multirests"})

    def set_staff(self, *, lines: int | None = None, label: str | None = None,
                  abbr: str | None = None, transpose: int | None = None) -> bool:
        """Say what the selected staves are -- the first, with nothing
        selected: their ``lines``, their name (``label``, ``abbr``), how many
        semitones they sound from what they write. What is left out stays."""
        call: dict = {"action": "staff"}
        for key, value in (("lines", lines), ("label", label), ("abbr", abbr),
                           ("transpose", transpose)):
            if value is not None:
                call[key] = value
        return self._act(call)

    def group(self, symbol: str) -> bool:
        """Group the staves the selection covers under a ``brace``, a
        ``bracket`` or a ``line``; ``none`` takes away the groups over
        them."""
        return self._act({"action": "group", "symbol": str(symbol)})

    def transform(self, name: str, **params) -> bool:
        """A transformation over the measures the selection covers -- or over
        everything, with nothing selected: ``"transpose"`` (``semitones``, or
        ``steps`` for a diatonic one), ``"invert"`` (``axis``),
        ``"retrograde"``, ``"stretch"`` (``factor``, as ``[n, d]``) or
        ``"repeat"`` (``count``)."""
        return self._act({"action": "transform", "name": str(name), **params})

    # ---- playing it ----

    @property
    def transport(self) -> HostedTransport:
        """**The transport the page plays on** (`HostedTransport`), as the
        object a script plays, its verbs about this score and in its beats -- a
        quarter to the beat. It is the window's, and goes with it."""
        return HostedTransport(self)

    def play(self, beat: "float | None" = None, *, range=None,
             looping: bool = False) -> "ScoreEditor":
        """**Play the score** from ``beat`` -- a quarter to the beat; from the
        start when left out -- on the window's transport, with the page's
        cursor following. Returns ``self``.

        ``range`` -- ``(start, end)`` in beats -- plays that stretch, going
        back to ``beat``; ``looping`` loops it, or with none the whole score.
        The space bar, the toolbar and the Play menu ask the same, from where
        the selection starts. An edit made while it plays is heard on from
        where the position is."""
        args = {"beat": float(beat if beat is not None else 0.0), "looping": bool(looping)}
        if range is not None:
            args["range"] = [float(range[0]), float(range[1])]
        self._do("play", **args)
        return self

    def pause(self) -> "ScoreEditor":
        """Pause where it stands: a `resume` carries the notes on."""
        self._do("pause")
        return self

    def resume(self) -> "ScoreEditor":
        """Roll again from where it paused."""
        self._do("resume")
        return self

    def stop(self) -> "ScoreEditor":
        """Stop, free what sounds, and go back to where the pass started."""
        self._do("stop")
        return self

    @property
    def playing(self) -> bool:
        """Whether the score is sounding, as the engine answers -- a pass that
        reached its end stopped without anybody here saying so."""
        if self.closed:
            return False
        return bool(self._do("playing"))

    def show_span(self, span) -> None:
        """A page has no time range of its own: what plays is a `play`'s
        ``range``."""
        raise ValueError("a page has no time range: play(range=...) plays a stretch")

    # ---- the score's file ----

    def save(self, path=None) -> str:
        """Write the score to its file -- ``path``, which is then the score's,
        or the one it was read from or last saved to -- and answer the path.
        The File menu's Save, as a method. The application writes it, and what
        is written is then not a change the File menu's Close asks about."""
        args = {} if path is None else {"path": str(path)}
        written = (self._do("save", **args) or {}).get("destination")
        if written is not None and self.score is not None:
            self.score.path = str(written)
        return written

    @property
    def unsaved(self) -> bool:
        """Whether the score has changes its file does not hold -- what the
        File menu's Close asks about before it closes the window."""
        return bool(self._call("unsaved").get("unsaved"))

    def new_file(self) -> None:
        """**A new score in the window**, one staff and four empty bars with no
        file, as its File menu's New puts one there. The document is another
        one, not an edit: the window lets go of the score it was opened over,
        which holds what the window left it as, as a close leaves it; nothing
        walks back across it, and the window goes on as the host's own
        (`closed`). A window opened over no score has nothing to let go of, and
        this handler stays."""
        self._do("new_file")

    def open_file(self, path) -> None:
        """**The document in the file at ``path`` in the window**, as its File
        menu's Open opens one, letting go of the score it was opened over as
        `new_file` does. A file that does not read raises and leaves everything
        as it was, and one another window holds brings that window forward and
        raises."""
        self._do("open_file", path=str(path))

    def close_file(self) -> None:
        """**No document in the window**, as its File menu's Close file leaves
        it, letting go of the score it was opened over as `new_file` does."""
        self._do("close_file")

    @property
    def has_document(self) -> bool:
        """Whether the window has a document: with none, its File verbs are all
        it answers."""
        return not self._query("file").get("empty", True)

    def export(self, path, format: "str | None" = None) -> str:
        """Write the score **rendered** to the file at ``path`` and answer the
        path: a Standard MIDI File (``format="smf"``) or a MIDI 2.0 Clip File
        (``"clip"``), by ``path``'s extension when left out -- ``.midi2`` is a
        clip, anything else a MIDI file. The File menu's two Exports, as a
        method.

        It is the sequence `clausters.gui.notation.Score.render_events`
        answers, at the score's own tempo, written as a sequence writes either
        (`clausters.seq.EventSequence.to_smf`, ``to_clip``): its notes, a
        channel to a voice, the dynamics as each channel's expression."""
        import os

        target = os.fspath(path)
        if format is None:
            format = "clip" if target.lower().endswith(".midi2") else "smf"
        if format not in ("smf", "clip"):
            raise ValueError(f"an export is 'smf' or 'clip', not {format!r}")
        rendered = EventSequence.from_data(self._render())
        data = rendered.to_clip() if format == "clip" else rendered.to_smf()
        with open(target, "wb") as file:
            file.write(data)
        return target

    def operate(self, op: dict) -> bool:
        """A model operation, whole (`clausters.gui.notation.sheet`'s
        vocabulary) -- for what has no verb here -- as one entry of the
        history. (``apply`` is every editor's door for the host's messages.)"""
        return self._act({"action": "op", "op": op})

    def _act(self, call: dict) -> bool:
        """One verb, in the application: recorded there, and the window
        corrected with what it answers. Whether the score changed; why it did
        not is on the window's status bar and in `refused`."""
        answer = self._do("act", call=_plain(call)) or {}
        self._refused = answer.get("refused") if isinstance(answer, dict) else None
        return bool(answer.get("changed")) if isinstance(answer, dict) else False

    def _render(self) -> dict:
        answer = self._call("render")
        if "sequence" not in answer:
            raise ValueError(answer.get("error", "the score could not be rendered"))
        return answer["sequence"]

    def _page(self, call: dict):
        """A script's call on the document the window has."""
        return self._do("page", call=call)


def is_score(structure) -> bool:
    """Whether `edit` opens this in the score editor: a symbolic score."""
    from ..notation.engraver import Score

    return isinstance(structure, Score)


__all__ = ["ScoreEditor", "is_score"]


def _letter(pitch: str) -> str:
    """A pitch's letter, as the note-entry verbs spell it: ``"C"`` is ``c``."""
    letter = str(pitch).strip().lower()
    if letter not in ("a", "b", "c", "d", "e", "f", "g"):
        raise ValueError(f"a pitch is a letter A to G, not {pitch!r}")
    return letter
