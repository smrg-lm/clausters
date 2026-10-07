"""Editing a symbolic score on its engraved page: the score editor.

What it opens is a `clausters.gui.notation.Score` -- notation held as MEI, its
model a sheet -- and it edits that score **in place**: the editor in the shared
crate (the ``openScore`` member of `clausters._native.EditingCore`) holds the
very score the script's handle names, so every edit is read back through the
handle (`Score.sheet`, `Score.mei`) and there is nothing to write back.

**The editor is the crate's**: the window, what each gesture on the page does
to the score, the verbs over what is selected, the entry each one leaves and
the corrections it answers with. What is here is what a language owns -- the
socket, and handing the crate the window it is open in. Each verb below is one
call into the crate, named as it names it.
"""

from __future__ import annotations

from ... import _native
from ...seq.playback import NotesPlayback
from ...seq.sequence import EventSequence
from .domain import Domain
from .editor import Editor
from .samples import _plain
from .view import View


class ScoreDomain(Domain):
    """A score's vocabulary, the crate's ``score``: a step is the page it
    names, which the crate puts back on the score it shares, so there is
    nothing here to carry out."""

    name = "score"
    ingested = True


class ScoreView(View):
    """The toolbar, the palettes beside the page in the scroll it sits in,
    the status line under them and the dialogs a menu entry opens, composed
    by the crate."""

    def build(self, editor) -> dict:
        page = self.widget(editor, "page", editor.structure)
        scroll = self.widget(editor, "scroll", editor.structure)
        status = self.widget(editor, "status", editor.structure)
        # the crate names the toolbar's tools and this numbers them
        tools = {str(name): self.widget(editor, "tool", editor.structure, str(name))
                 for name in editor._call("tools").get("tools") or ()}
        # and the widgets of its dialogs, the same way
        dialogs = {str(name): self.widget(editor, "dialog", editor.structure, str(name))
                   for name in editor._call("dialogs").get("dialogs") or ()}
        # and the entries of its palettes
        palettes = {str(name): self.widget(editor, "palette", editor.structure, str(name))
                    for name in editor._call("palettes").get("palettes") or ()}
        editor._sync_core()
        tree = editor._call("window", widget=page, scroll=scroll, status=status,
                            tools=tools, dialogs=dialogs, palettes=palettes)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class ScoreEditor(Editor):
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
    the toolbar's transport and the Play menu play the score on the server,
    from where the selection starts, with the page's cursor following
    (`play`, `stop`, `transport`). An entry
    that needs more than a pick -- the page's text, its margins, a
    transformation's parameter -- opens a dialog over the window. Ctrl+click
    adds a note to the selection or takes it out, and
    Shift+click extends the selection to it, in time and across the staves
    between. The verbs act on what is selected (`selected`, `select`); each is
    one entry of the editing context's history, so Ctrl+Z over the window walks
    them back.

    Args:
        score: the `clausters.gui.notation.Score` to edit. It is the edited
            one: read it after any gesture.
        title: the window's title.
        value: the written value a note entered on the page takes, as
            ``(numerator, denominator)`` of a whole note; a quarter by default.
        server: the `clausters.defs.Server` it plays on; ``None`` resolves the
            ambient one when it first plays.
        chrome: ``False`` opens the page alone, in its scroll, over the
            status line: no menu bar, no toolbar, no palettes and no dialogs,
            none of them composed -- the window a script wants when it edits
            through this handle and reads the score back. The keys stay (N
            and note entry, the arrows, Delete, the space bar, Ctrl+Z; F1
            shows them all), and so does every verb below. A verb refused
            answers ``False`` and says why on the status line. What a form
            asked -- the path of a first Ctrl+S -- is asked of the handle
            instead (`save`), and the window's close mark closes it at once,
            since the score it edits is the one this handle holds.
    """

    def __init__(self, score, *, title: str = "Score", value=None,
                 width: int = 960, height: int = 640, server=None,
                 over: "EventSequence | None" = None, how: "dict | None" = None,
                 interp: "dict | None" = None, **options):
        options.pop("sample_rate", None)
        self._server = server
        self._score = score
        #: The sequence this page is the reading of, in an editor opened over
        #: one (`over`); ``None`` for an editor over a score of its own.
        self.sequence: "EventSequence | None" = over
        #: The score as the sequence it plays as, rendered when it first plays
        #: and again after every edit. Over a sequence, that sequence.
        self._rendered: "EventSequence | None" = over
        if over is None:
            domain, structure = ScoreDomain(), score
        else:
            from .events import NotesDomain

            domain, structure = NotesDomain(), over
        super().__init__(structure, sample_rate=48_000.0, domain=domain,
                         view=ScoreView(), title=title, width=width,
                         height=height, **options)
        request = {"title": self.title, "w": int(self.size[0]), "h": int(self.size[1])}
        if value is not None:
            request["value"] = [int(value[0]), int(value[1])]
        if not self.chrome:
            request["chrome"] = False
        if over is None:
            self._member, self._structure_id = self._editing.open_score(
                f"score:{id(score)}", score, request, self.domain)
        else:
            if how:
                request["how"] = dict(how)
            if interp is not None:
                request["interp"] = interp
            self._member, self._structure_id = self._editing.open_score_over(
                f"sequence:{id(over)}", score, over, request, self.domain)

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
        over a sequence -- the reading of it, which follows the sequence."""
        return self._score

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _sync_core(self) -> None:
        """Hand the crate the window it is open in and the chrome."""
        self._call("sync", window=self._window, title=self.title,
                   w=int(self.size[0]), h=int(self.size[1]),
                   path=getattr(self._score, "path", None))

    # ---- what is selected, and the value in hand ----

    @property
    def selected(self) -> list:
        """The selected items, as the model names them (`Score.sheet`'s item
        ids), each once."""
        return [int(i) for i in self._call("selected").get("items") or ()]

    def select(self, elements) -> None:
        """Select the page's elements ``elements`` (their ``xml:id``\\s, as the
        page reports them), or nothing with an empty list."""
        self._call("select", elements=[str(e) for e in elements or ()])
        self.adopt()

    @property
    def value(self) -> tuple:
        """The written value a note entered on the page takes, as
        ``(numerator, denominator)`` of a whole note: ``(1, 4)`` is a quarter.
        Set it to write another: ``editor.value = (1, 8)``."""
        numerator, denominator = self._call("value").get("value") or (1, 4)
        return int(numerator), int(denominator)

    @value.setter
    def value(self, value) -> None:
        self._call("sync", value=[int(value[0]), int(value[1])])

    @property
    def entry(self) -> bool:
        """Whether the window is in note entry, where the keys and a press on a
        staff write at the edit cursor. Off by default, and off once a pass
        plays; outside it a press on a staff selects the measure it fell in.
        Set it to switch: ``editor.entry = True``."""
        return bool(self._call("entry").get("entry", False))

    @entry.setter
    def entry(self, on: bool) -> None:
        self._call("sync", entry=bool(on))
        self.adopt()

    @property
    def dotted(self) -> bool:
        """Whether the value a note is entered with is dotted: half as long
        again. Set it to switch: ``editor.dotted = True``."""
        return bool(self._call("input").get("dotted", False))

    @dotted.setter
    def dotted(self, on: bool) -> None:
        self._call("sync", dotted=bool(on))
        self.adopt()

    @property
    def rest(self) -> bool:
        """Whether a press on empty staff writes a rest of `value` rather than
        a note. Set it to switch: ``editor.rest = True``."""
        return bool(self._call("input").get("rest", False))

    @rest.setter
    def rest(self, on: bool) -> None:
        self._call("sync", rest=bool(on))
        self.adopt()

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
        self._call("sync", accidental=None if alter is None else int(alter))
        self.adopt()

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
        self._call("sync", layout=str(layout))
        self.adopt()

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

    def spanner(self, kind: str) -> bool:
        """A ``slur``, a ``crescendo`` or a ``diminuendo`` from the first
        selected item to the last, in time."""
        return self._act({"action": "spanner", "kind": str(kind)})

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

    def open(self, host=None, id: "int | None" = None):
        """Open the window, with its play cursor drawn from the transport.

        The page draws its cursor over the engraver's timemap, anchored at 0,
        and the counter that makes that the score's own time is the position
        of the transport the score plays on -- stopped or rolling, the line is
        where the sound is. With no server to play on there is no position,
        and the page opens to be edited; nor with a server that has no
        transport left, which is logged."""
        window = super().open(host, id)
        if self._host is not None and window is not None:
            try:
                server = self._resolve_server()
            except RuntimeError:
                return window
            try:
                transport = NotesPlayback.of(server, self._sequence()).transport_id
            except (RuntimeError, ValueError) as refused:
                from ...log import log

                log.warning("the score has no play cursor and cannot be played: %s", refused)
                return window
            self._host.head_clock(window, "transport", transport)
        return window

    def _plays_on(self):
        try:
            return self._resolve_server()
        except RuntimeError:
            return None

    def _resolve_server(self):
        if self._server is None:
            from ...base.main import main

            self._server = main.resolve_server()
        return self._server

    def _sequence(self) -> EventSequence:
        """The score as the sequence it plays as: the crate's render, on the
        engraver's time, made on first ask and kept -- so its playback is one,
        and an edit is the same sequence holding the next render."""
        if self._rendered is None:
            self._rendered = EventSequence.from_data(self._render())
        return self._rendered

    def _render(self) -> dict:
        answer = self._call("render")
        if "sequence" not in answer:
            raise ValueError(answer.get("error", "the score could not be rendered"))
        return answer["sequence"]

    def adopt(self) -> None:
        """Another view of this structure edited it: over a sequence, the page
        is read again from it -- window or none -- and then every widget is
        corrected."""
        if self.sequence is not None:
            self._call("follow")
        super().adopt()
        if self.sequence is not None:
            # and what plays is the sequence, so the change is heard
            self._update()

    def _update(self) -> None:
        """The score changed: the sequence it plays as holds the next render,
        and the lane takes it, so the server plays the edit on from where the
        position is."""
        if self._rendered is None:
            return
        # over a sequence, what plays is the sequence, which the edit is
        # already in
        if self.sequence is None:
            try:
                self._rendered._seq = _native.SequenceHandle(self._render())
            except ValueError:
                return
        playback = self._held
        if playback is not None:
            playback.update()

    @property
    def _playback(self) -> NotesPlayback:
        """The score's playback on the server, made when it has none -- which
        raises when the server has no transport left."""
        return NotesPlayback.of(self._resolve_server(), self._sequence())

    @property
    def _held(self) -> "NotesPlayback | None":
        """The score's playback when it has one."""
        if self._rendered is None:
            return None
        return NotesPlayback.held(self._server, self._rendered)

    @property
    def transport(self):
        """**The transport the score plays on**, as the object a script plays:
        a `clausters.defs.Transport` whose verbs (``play``, ``pause``,
        ``stop``, ``locate``, ``loop``, ``wait``) are about this score, in its
        beats -- a quarter to the beat. Once a script has asked for it, it is
        the script's to free (``free()``); otherwise it goes back to the
        server when the editor closes."""
        playback = self._playback
        playback.kept = True
        return playback.transport

    def play(self, beat: "float | None" = None, *, range=None,
             looping: bool = False) -> "ScoreEditor":
        """**Play the score** from ``beat`` -- a quarter to the beat; from the
        start when left out -- on a transport of its own, with the page's
        cursor following. Returns ``self``.

        ``range`` -- ``(start, end)`` in beats -- plays that stretch, going
        back to ``beat``; ``looping`` loops it, or with none the whole score.
        The space bar, the toolbar and the Play menu ask the same, from where
        the selection starts. An edit made while it plays is heard on from
        where the position is."""
        self._playback.load(float(beat if beat is not None else 0.0),
                            range=range, looping=looping, end="contents")
        return self

    def pause(self) -> "ScoreEditor":
        """Pause where it stands: a `resume` carries the notes on."""
        self._playback.call("pause")
        return self

    def resume(self) -> "ScoreEditor":
        """Roll again from where it paused."""
        self._playback.call("resume")
        return self

    def stop(self) -> "ScoreEditor":
        """Stop, free what sounds, and go back to where the pass started."""
        playback = self._held
        if playback is not None:
            playback.call("stop", back=float(playback.cursor))
        return self

    @property
    def playing(self) -> bool:
        """Whether the score is sounding, as the engine answers -- a pass that
        reached its end stopped without anybody here saying so."""
        playback = self._held
        return playback is not None and playback.playing()

    def reflect_step(self) -> None:
        """A history step landed: the window is corrected, and the lane takes
        the score again, so the undo is heard."""
        super().reflect_step()
        self._update()

    def _closed(self) -> bool:
        closed = super()._closed()
        self._release()
        return closed

    def close(self):
        """Close this editor's window. **The score's transport goes back to
        the server with it**, and what was sounding is released -- unless a
        script holds the transport (`transport`), whose it then is to free."""
        closed = super().close()
        self._release()
        return closed

    def _release(self) -> None:
        playback = self._held
        if playback is not None and not playback.kept:
            playback.free()

    def _transport_turn(self, outcome: dict) -> None:
        """What a turn asked of the playback: a play or a stop, the loop
        switch, the cursor placed."""
        if outcome.get("locate") is not None:
            playback = self._held
            if playback is not None:
                playback.cue(float(outcome["locate"]))
        if outcome.get("play") is not None:
            # A play is play or stop: a stop goes back to where the pass began.
            if self.playing:
                self.stop()
            else:
                pass_ = outcome["play"]
                self._playback.cursor = float(pass_.get("from") or 0.0)
                self.play(self._playback.cursor, range=pass_.get("range"),
                          looping=bool(pass_.get("looping")))
        if outcome.get("loop") is not None:
            playback = self._held
            if playback is not None:
                # the loop switch: followed at once by a pass in progress
                pass_ = outcome["loop"]
                playback.set_span(pass_.get("range"), show=False)
                playback.set_looping(bool(pass_.get("looping")))

    # ---- the score's file ----

    def save(self, path=None) -> str:
        """Write the score to its file -- ``path``, which is then the score's,
        or the one it was read from or last saved to -- and answer the path.
        The File menu's Save, as a method (`clausters.gui.notation.Score.write`).
        What is written is then not a change the File menu's Close asks about."""
        written = self.score.write(path)
        self._call("saved")
        return written

    @property
    def unsaved(self) -> bool:
        """Whether the score has changes its file does not hold -- what the
        File menu's Close asks about before it closes the window."""
        return bool(self._call("unsaved").get("unsaved"))

    def load(self, path) -> bool:
        """Open the document in the file at ``path`` in this editor, in place
        of the score, as one entry of the history: the score that was there is
        a step back. The File menu's Open, as a method; the file is then the
        score's."""
        with open(path, encoding="utf-8") as file:
            data = file.read()
        loaded = self._act({"action": "open", "data": data})
        if loaded:
            self.score.path = str(path)
        return loaded

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
        """One verb, through the context: recorded by the crate, and the window
        corrected with what it answers. Whether the score changed; why it did
        not is on the window's status bar."""
        with self._editing.turn(self):
            turned = self._editing.act(self._member, _plain(call))
            outcome = turned.get("outcome") or {}
            changed = bool(outcome.get("changed"))
            if changed:
                self.dirty = True
                self._editing.changed()
                self._update()
            if self._host is not None and self._window is not None:
                self.echo.send(outcome.get("answer"))
            return changed

    # ---- the crate's turns ----

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        turned = self._editing.event(self._member, str(addr), _plain(list(args)))
        outcome = turned.get("outcome") or {}
        if outcome.get("turn") == "closed":
            return self._closed()
        if outcome.get("turn") == "step":
            stepped = self.app.stepped(turned.get("stepped") or {}, self)
            self.echo.send(outcome.get("answer"))
            return stepped
        return self._take(outcome)

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off."""
        self._sync_core()
        wid, tag, values = args[0], args[1], list(args[2:])
        turned = self._editing.event(self._member, "/gui_event",
                                     _plain([wid, 0, 0, tag, *values]))
        return self._take(turned.get("outcome") or {})

    def _take(self, outcome: dict) -> bool:
        """Answer the host with what a turn came to; whether the score
        changed."""
        if outcome.get("turn") in (None, "nothing"):
            return False
        changed = bool(outcome.get("changed"))
        if changed:
            self.dirty = True
            self._editing.changed()
            self._update()
        self._transport_turn(outcome)
        self.echo.send(outcome.get("answer"))
        # A file is this client's to write and to read: the turn said which.
        # (the File menu's Close waits for the file it saves to: a write that
        # fails raises, and the window stays)
        if outcome.get("save"):
            self.save(outcome["save"])
        if self._closing(outcome):
            return changed
        if outcome.get("export"):
            self.export(outcome["export"]["path"], outcome["export"]["format"])
        if outcome.get("open"):
            with open(outcome["open"], encoding="utf-8") as file:
                opened = self._editing.act(
                    self._member, {"action": "open", "data": file.read()})
            if self._take(opened.get("outcome") or {}):
                self.score.path = str(outcome["open"])
                changed = True
        return changed


def is_score(structure) -> bool:
    """Whether `edit` opens this in the score editor: a symbolic score."""
    from ..notation.engraver import Score

    return isinstance(structure, Score)


__all__ = ["ScoreDomain", "ScoreEditor", "ScoreView", "is_score"]
