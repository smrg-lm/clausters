"""What a window answers from a script, whoever holds what it edits: the verbs
of the one verb table, a hand played to it, and what it costs.

`clausters.gui.editing.Editor` and the handlers of an application the GUI host
holds (`clausters.gui.editing.HostedEditor`) both reach a window so; each only
says which window (``_window``) on which host (``_host``).
"""


class WindowVerbs:
    """**A window's verbs, as members**: each one call to the host about the
    window ``_window`` names, ``False`` or a `RuntimeError` with none open."""

    def _has_window(self) -> bool:
        """Whether there is a window to ask."""
        return self._host is not None and self._window is not None

    def verb(self, name: str) -> bool:
        """**Perform the window's verb ``name``**, as its key, its menu entry
        or its tool would -- the door every verb of the one verb table is
        reached by from a script (``docs/verbs.md``). The host performs it on
        what the window's focus or its ``main`` view holds, and a verb that is
        this editor's comes back to it as the window's event would; what it
        edits arrives as a gesture's edit does, through the event loop.

        Answers whether it was asked: ``False`` with no window open, since a
        window's verb acts on what the window holds.
        """
        if not self._has_window():
            return False
        self._host.verb(self._window, str(name))
        return True

    def pointer(self, kind: str, x: float, y: float, mods: str = "") -> bool:
        """**A hand played to the window**: a ``"press"``, ``"move"`` or
        ``"release"`` at ``(x, y)`` in its device pixels, with ``mods`` held --
        `clausters.gui.host.GuiHost.pointer` on this editor's window. What it
        edits arrives as a gesture's edit does, through the event loop.

        Answers whether it was sent: ``False`` with no window open.
        """
        if not self._has_window():
            return False
        self._host.pointer(self._window, kind, x, y, mods)
        return True

    def click(self, x: float, y: float, mods: str = "") -> bool:
        """A press and a release at ``(x, y)``, with ``mods`` held (`pointer`)."""
        return (self.pointer("press", x, y, mods)
                and self.pointer("release", x, y, mods))

    def drag(self, start, end, mods: str = "", steps: int = 8) -> bool:
        """**A drag from ``start`` to ``end``**, each an ``(x, y)`` in the
        window's device pixels: a press, ``steps`` moves along the straight
        line between them, and a release at ``end``, with ``mods`` held
        throughout (`pointer`). It is the drag a hand makes -- what a snap or
        a constraint does to one, it does to this.
        """
        (x0, y0), (x1, y1) = start, end
        if not self.pointer("press", x0, y0, mods):
            return False
        n = max(int(steps), 1)
        for i in range(1, n + 1):
            t = i / n
            self.pointer("move", x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, mods)
        return self.pointer("release", x1, y1, mods)

    def capture(self, path, timeout: float = 5.0) -> str:
        """**Write the window's picture to ``path``** as a PNG, and answer the
        path -- `clausters.gui.host.GuiHost.capture` on this editor's window,
        which waits for the turns before it: what a verb or a gesture edited is
        in the picture.

        Raises `RuntimeError` with no window open, since there is nothing to
        draw.
        """
        if not self._has_window():
            raise RuntimeError("capture() needs the editor's window open")
        return self._host.capture(self._window, path, timeout)

    def activity(self, timeout: float = 1.0) -> dict:
        """**What the window costs, and why** --
        `clausters.gui.host.GuiHost.activity` on this editor's window: its
        frames, ticks, queries and downloads since it opened, and the reasons
        it is ticking now (``demand``, empty for a settled window).

        Raises `RuntimeError` with no window open.
        """
        if not self._has_window():
            raise RuntimeError("activity() needs the editor's window open")
        return self._host.activity(self._window, timeout)

    def profile(self, seconds: float = 5.0, timeout: float = 1.0) -> dict:
        """**What the window costs over the next ``seconds``** --
        `clausters.gui.host.GuiHost.profile` on this editor's window: what
        moved in its frames, their cost by phase, its ticks, queries and
        downloads. ``print(format_window_profile(...))`` reads it out.

        Raises `RuntimeError` with no window open.
        """
        if not self._has_window():
            raise RuntimeError("profile() needs the editor's window open")
        return self._host.profile(self._window, seconds, timeout)

    def view_all(self) -> bool:
        """The whole of what the window's view holds, in view -- the window's ``view_all`` (`verb`)."""
        return self.verb("view_all")

    def cut(self) -> bool:
        """Cut what is selected -- the window's ``cut`` (`verb`)."""
        return self.verb("cut")

    def copy(self) -> bool:
        """Copy what is selected -- the window's ``copy`` (`verb`)."""
        return self.verb("copy")

    def paste(self) -> bool:
        """Paste at the cursor -- the window's ``paste`` (`verb`)."""
        return self.verb("paste")

    def delete(self) -> bool:
        """Delete what is selected -- the window's ``delete`` (`verb`)."""
        return self.verb("delete")

    def select_all(self) -> bool:
        """Select everything the view holds -- the window's ``select_all`` (`verb`)."""
        return self.verb("select_all")

    def to_start(self) -> bool:
        """The position cursor to the start -- the window's ``to_start`` (`verb`)."""
        return self.verb("to_start")

    def to_end(self) -> bool:
        """The position cursor to the end -- the window's ``to_end`` (`verb`)."""
        return self.verb("to_end")

    def loop(self) -> bool:
        """The loop switch, on or off -- the window's ``loop`` (`verb`)."""
        return self.verb("loop")
