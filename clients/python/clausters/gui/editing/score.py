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
    """The page, the scroll it sits in and the status line under it, composed
    by the crate."""

    def build(self, editor) -> dict:
        page = self.widget(editor, "page", editor.structure)
        scroll = self.widget(editor, "scroll", editor.structure)
        status = self.widget(editor, "status", editor.structure)
        editor._sync_core()
        tree = editor._call("window", widget=page, scroll=scroll, status=status)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class ScoreEditor(Editor):
    """A symbolic score on its page, edited by hand, in place.

    A press on a note selects it, a drag moves it along its staff, and a press
    on empty staff writes a note of `value` there. The verbs act on what is
    selected (`selected`, `select`); each is one entry of the editing context's
    history, so Ctrl+Z over the window walks them back.

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

    def voice(self) -> bool:
        """Move the selected items into the other voice of their staff, leaving
        rests where they were."""
        return self._act({"action": "voice"})

    def spanner(self, kind: str) -> bool:
        """A ``slur``, a ``crescendo`` or a ``diminuendo`` from the first
        selected item to the last, in time."""
        return self._act({"action": "spanner", "kind": str(kind)})

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
