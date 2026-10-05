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
    """The toolbar, the page in the scroll it sits in and the status line
    under it, composed by the crate."""

    def build(self, editor) -> dict:
        page = self.widget(editor, "page", editor.structure)
        scroll = self.widget(editor, "scroll", editor.structure)
        status = self.widget(editor, "status", editor.structure)
        # the crate names the toolbar's tools and this numbers them
        tools = {str(name): self.widget(editor, "tool", editor.structure, str(name))
                 for name in editor._call("tools").get("tools") or ()}
        editor._sync_core()
        tree = editor._call("window", widget=page, scroll=scroll, status=status,
                            tools=tools)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class ScoreEditor(Editor):
    """A symbolic score on its page, edited by hand, in place.

    A press on a note selects it, a drag moves it along its staff, and a press
    on empty staff writes a note of `value` there (`entry`; off, it selects the
    measure). The menu bar holds every action, and the toolbar what a hand
    reaches for while it writes: the value, its dot, a rest, an accidental,
    the articulations, a tie, a triplet, the voice and the layout. Ctrl+click
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
    """

    def __init__(self, score, *, title: str = "Score", value=None,
                 width: int = 960, height: int = 640, **options):
        options.pop("sample_rate", None)
        super().__init__(score, sample_rate=48_000.0, domain=ScoreDomain(),
                         view=ScoreView(), title=title, width=width,
                         height=height, **options)
        request = {"title": self.title, "w": int(self.size[0]), "h": int(self.size[1])}
        if value is not None:
            request["value"] = [int(value[0]), int(value[1])]
        self._member, self._structure_id = self._editing.open_score(
            f"score:{id(score)}", score, request, self.domain)

    @property
    def score(self):
        """The score the page edits -- the one the editor was opened over."""
        return self.structure

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _sync_core(self) -> None:
        """Hand the crate the window it is open in and the chrome."""
        self._call("sync", window=self._window, title=self.title,
                   w=int(self.size[0]), h=int(self.size[1]))

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
        """Whether a press on empty staff writes a note. On by default; off,
        the same press on a staff selects the measure it fell in. Set it to
        switch: ``editor.entry = False``."""
        return bool(self._call("entry").get("entry", True))

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

    def transform(self, name: str, **params) -> bool:
        """A transformation over the measures the selection covers -- or over
        everything, with nothing selected: ``"transpose"`` (``semitones``, or
        ``steps`` for a diatonic one), ``"invert"`` (``axis``),
        ``"retrograde"``, ``"stretch"`` (``factor``, as ``[n, d]``) or
        ``"repeat"`` (``count``)."""
        return self._act({"action": "transform", "name": str(name), **params})

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
        self.echo.send(outcome.get("answer"))
        return changed


def is_score(structure) -> bool:
    """Whether `edit` opens this in the score editor: a symbolic score."""
    from ..notation.engraver import Score

    return isinstance(structure, Score)


__all__ = ["ScoreDomain", "ScoreEditor", "ScoreView", "is_score"]
