"""The **audio editor**: a sound file edited as a list of parts over immutable
files, by the application the GUI host holds.

The editor is one program, and it is not this one. The GUI host opens it on a
file (`clausters.gui.host.GuiHost.open_app`), in a window of the host's, on an
engine of the application's own: what a gesture does to the take, the renders
that make new samples, the history, the working folder and the playback all
run in the host's process. This object is its **handler**: every member below
is one call to the application (`clausters.gui.host.GuiHost.call_app`), and
what it answers is read from there each time rather than kept here.

What it opens is **a sound file**, read where it is; nothing it was handed is
written until it is saved. A cut takes a span out of the list, a paste puts
the copied spans in, and an edit that makes new samples (a pencil stroke, a
mix, a gain curve applied, a reverse) renders them into a file of the
application's working folder, which the list then names. Nothing a history
entry names is written again, so an undo is the list before.
"""

#: The containers a part of a list reads in place; a file of any other codec
#: is not opened yet.
_STREAMED = ("wav", "w64", "rf64")

#: Why a verb that acts on a file is refused by an editor with none open.
from .hosted import HostedEditor, _Held

_NO_FILE = "no file is open"


class AudioTransport:
    """**The audio editor's transport**, as a script plays it: the verbs and
    the span a server's `clausters.defs.Transport` has, in the take's
    seconds, each one a call to the application. The span is the band a drag
    sweeps and the loop switch is `L`, so a script and a hand read and write
    one state. It is the editor's own: another window's play never moves it.
    """

    def __init__(self, editor):
        self._editor = editor

    def _state(self) -> dict:
        return self._editor._query("transport")

    @property
    def playing(self) -> bool:
        """Whether the take is playing."""
        return bool(self._editor._do("playing"))

    def play(self, at: "float | None" = None) -> "AudioTransport":
        """Roll, from ``at`` seconds when given, else from the cursor."""
        self._editor._do("play", **({} if at is None else {"at": float(at)}))
        return self

    def pause(self) -> "AudioTransport":
        """Hold where it is; a `play` goes on from there."""
        self._editor._do("pause")
        return self

    def stop(self) -> "AudioTransport":
        """Stop, and bring the file shown into focus at its cursor."""
        self._editor._do("stop")
        return self

    def locate(self, at: float) -> "AudioTransport":
        """Move the cursor to ``at`` seconds."""
        self._editor._do("locate", at=float(at))
        return self

    @property
    def span(self):
        """The time range ``(start, end)`` in seconds, or ``None``."""
        span = self._state()["span"]
        return None if span is None else (float(span[0]), float(span[1]))

    @span.setter
    def span(self, span) -> None:
        self._editor._do("span", show=True, span=None if span is None
                         else [float(span[0]), float(span[1])])

    @property
    def looping(self) -> bool:
        """Whether the loop switch is on: over the span, or the whole take."""
        return bool(self._state()["looping"])

    def loop(self, start: "float | None" = None, end: "float | None" = None) -> "AudioTransport":
        """**Loop**: with ``start`` and ``end``, set the `span` to them first;
        then turn the loop on, over the span or the whole take."""
        if start is not None and end is not None:
            self.span = (start, end)
        self._editor._do("looping", on=True)
        return self

    def unloop(self) -> "AudioTransport":
        """Turn the loop off; the `span` stays."""
        self._editor._do("looping", on=False)
        return self

    def wait(self, timeout: "float | None" = None) -> bool:
        """Block until the take stops playing, or ``timeout`` seconds pass.
        Answers whether it stopped."""
        import time

        deadline = None if timeout is None else time.monotonic() + float(timeout)
        while self.playing:
            if deadline is not None and time.monotonic() >= deadline:
                return False
            time.sleep(0.05)
        return True


class AudioEditor(HostedEditor):
    """**The audio editor the GUI host holds, and this script its handler.**

    `clausters.gui.edit` over a `clausters.SoundFile` answers one, open. Each
    member is a call to the application in the host's process, answered by
    it: a member that edits returns once the edit is made, renders and all,
    and one that reads asks the application again each time.

    **What is marked** (`selected`, `select`, `unselect`) is the samples a
    drag over the take covers, as a `clausters.segments.Segment` over `file`
    (its ``start`` a frame and its ``duration`` seconds), or ``None``. What
    is marked and the transport's `span` are one range.

    **An editor with no file open** is the application on its own:
    ``AudioEditor()`` opens one, and closing its last file leaves it so. A
    verb that acts on a file raises `ValueError` until one is added
    (`new_file`, `open_file`).

    Args:
        file: the `clausters.SoundFile` to edit, read where it is; one with
            no path (`clausters.SoundFile.empty`) is silence of its shape,
            not written anywhere yet, whose first save asks where. ``None``
            opens the editor with no file.
        title: the window's title.
        layers: what the picture measures, innermost last; the application's
            own when not given.
    """

    _app = "audio"

    def __init__(self, file=None, *, title: str = "Audio", layers=None):
        super().__init__(title=title)
        self._file = file
        self._layers = layers
        self._holders = _Held()

    # ---- the window ----

    def _held(self) -> _Held:
        # a sound file is read where it is, by this window alone
        return self._holders

    def _options(self, beside) -> dict:
        if self._file is not None and self._file.path is not None:
            return {"path": str(self._file.path)}
        return {}

    def open(self, host=None):
        """**Ask the host to open the editor**, on ``host`` or the ambient
        one, and answer its window. An editor already open answers the window
        it has."""
        opening = self.closed
        window = super().open(host)
        if opening:
            # silence not written anywhere yet: a new file of its shape
            if self._file is not None and self._file.path is None:
                self.new_file(self._file.channels, self._file.frames, self._file.sample_rate)
            if self._layers is not None:
                self.layers = self._layers
        return window

    # ---- the door ----

    def _call(self, verb: str, **args) -> dict:
        """One verb of the editor, answered at once; ``ValueError`` when it
        needs a file and none is open."""
        answer = super()._call(verb, **args)
        if answer.get("error") == _NO_FILE:
            raise ValueError(_NO_FILE)
        return answer

    def _version(self) -> int:
        return int(self._query("history").get("version", 0))

    # ---- the window's verbs ----

    def mix(self) -> bool:
        """Paste onto what is there, mixed with it: the window's ``mix`` (`verb`)."""
        return self.verb("mix")

    def reverse(self) -> bool:
        """Play the selection backwards, or the whole take with nothing
        selected, rendered into a file the list then names: the window's
        ``reverse`` (`verb`)."""
        return self.verb("reverse")

    # ---- the history: the order of the file in focus ----

    def undo(self) -> bool:
        """Step the file in focus back one edit. Answers whether anything was
        undone."""
        return self._step("undo")

    def redo(self) -> bool:
        """Step the file in focus forward again. Answers whether anything was
        redone."""
        return self._step("redo")

    def _step(self, direction: str) -> bool:
        if not getattr(self, f"can_{direction}"):
            return False
        args = [int(self._opened()), 0, self._version(), direction]
        self._do("event", addr="/gui_event", args=args)
        return True

    # ---- the transport, and what is marked ----

    @property
    def transport(self) -> AudioTransport:
        """**The take's transport**: ``play``, ``pause``, ``stop``,
        ``locate``, ``loop``, ``wait`` and ``span``, in the take's seconds."""
        return AudioTransport(self)

    @property
    def sample_rate(self) -> float:
        """The rate of the file in focus, in hertz."""
        return float(self._call("binding").get("rate") or 0.0)

    @property
    def cursor(self) -> "float | None":
        """Where the cursor of the file in focus is, in seconds, or ``None``
        with no file open."""
        binding = self._call("binding")
        if binding.get("cursor") is None or not binding.get("rate"):
            return None
        return float(binding["cursor"]) / float(binding["rate"])

    @property
    def selected(self):
        """What is marked: a `clausters.segments.Segment` over `file`, or
        ``None``."""
        from ...segments import Segment

        span = self.transport.span
        if span is None:
            return None
        return Segment(self.file, round(span[0] * self.sample_rate), span[1] - span[0])

    def select(self, marked) -> None:
        """Mark ``marked``, a `clausters.segments.Segment` (its ``start`` a
        frame, its ``duration`` seconds); ``None`` marks nothing."""
        if marked is None:
            self.unselect()
            return
        start = float(marked.start) / self.sample_rate
        self.transport.span = (start, start + float(marked.duration))

    def unselect(self) -> None:
        """Mark nothing."""
        self.transport.span = None

    # ---- the files ----

    @property
    def file(self):
        """**The `clausters.SoundFile` the take is**, at its length now: the
        file it was opened from or last saved to, one with no path when it has
        not been written yet, and ``None`` with no file open."""
        from ...soundfile import SoundFile

        answer = self._call("file")
        path = answer.get("path")
        frames, channels = int(answer.get("frames", 0)), int(answer.get("channels", 1))
        rate = float(answer.get("rate") or 0.0)
        if path is None:
            return SoundFile(None, frames, channels, rate) if answer.get("untitled") else None
        return SoundFile(path, frames, channels, rate, answer.get("codec"),
                         answer.get("sample_format"))

    @property
    def files(self) -> tuple:
        """The open `clausters.SoundFile`s, in list order."""
        from ...soundfile import SoundFile

        return tuple(
            SoundFile(f["path"], int(f["frames"]), int(f["channels"]), float(f["rate"]),
                      f["codec"], f["sample_format"])
            if f["path"] is not None or f["untitled"] else None
            for row in self._call("files")["files"] for f in [row["file"]])

    @property
    def parts(self) -> list:
        """What the take is made of now: one ``dict`` per span of a file, in
        reading order -- its ``path``, the ``start`` and the ``frames`` of it
        read, and its fades and shape."""
        return list(self._call("segments").get("parts") or ())

    @property
    def dirty(self) -> bool:
        """**Whether the file in focus has edits its file does not hold.** A
        save that has not completed leaves it dirty; an undo back to what was
        written leaves it clean."""
        return bool(self._call("binding").get("dirty"))

    def save(self, path: "str | None" = None, *, sample_format: str = "float") -> str:
        """**Write the take as the edits have left it**: over the file it was
        opened from, or, given a ``path``, there, which a later `save` then
        writes over. Ctrl+S in the window is the same save. ``sample_format``
        is ``"float"``, ``"int24"`` or ``"int16"``. Answers the path written,
        once the file is whole.

        Raises:
            ValueError: the format is not one of those three.
        """
        request = {"format": str(sample_format)}
        if path is not None:
            request["path"] = str(path)
        return str(self._do("save", request=request)["destination"]["path"])

    def new_file(self, channels: int, frames: int, rate: float) -> None:
        """Add and focus an empty file of that shape."""
        self._do("newFile", shape={"channels": int(channels), "frames": int(frames),
                                   "rate": float(rate)})

    def open_file(self, paths) -> None:
        """Open a path or a list of paths in this editor, focusing the last.

        A file a window of the host already holds is not read again: in this
        editor it is focused, and in another window that window comes forward
        with it in focus.
        """
        self._do("openFile", paths=list(paths) if isinstance(paths, (list, tuple))
                 else str(paths))

    def focus_file(self, index: int) -> None:
        """Focus the file at zero-based ``index``, with its cursor and selection."""
        self._do("verb", call={"verb": "focus_file", "index": int(index)})

    def close_file(self, index=None) -> None:
        """Close the file in focus, or the file at zero-based ``index``."""
        self._do("verb", call={"verb": "close_file",
                               **({} if index is None else {"index": int(index)})})

    def show_files(self) -> None:
        """Show the file list to the left of the take."""
        self._do("verb", call={"verb": "show_files"})

    def hide_files(self) -> None:
        """Hide the file list."""
        self._do("verb", call={"verb": "hide_files"})

    # ---- the spectrogram's settings ----

    @property
    def spectrogram(self) -> dict:
        """**How the spectrogram analyzes and draws the take**: ``window``
        (frames: 256, 512, 1024, 2048 or 4096), ``shape`` (the window's:
        ``"hann"``, ``"hamming"``, ``"blackman"``, ``"sine"``, ``"welch"`` or
        ``"rectangular"``), ``overlap`` (0, windows end to end, then 0.5,
        0.75, 0.875 or 0.9375, each halving the hop), ``scale``
        (``"linear"``, ``"log"``, ``"mel"``, ``"bark"`` or ``"note"``),
        ``tuning`` (the A4 of a note axis, in hertz), ``colormap``
        (``"viridis"``, ``"magma"`` or ``"gray"``), ``range`` (the decibels
        the colours span, 20 to 120), ``gain`` (how far their top is below
        full scale, 0 to 60 dB), ``interpolate`` (drawn smooth rather than in
        blocks of measured magnitudes) and ``normalize`` (each frequency band
        at its own scale). The window's, whichever file is in focus; a view
        state, outside the history."""
        return dict(self._call("spectrogram")["spectrogram"])

    def set_spectrogram(self, **settings) -> None:
        """Change the settings named (`spectrogram`), the others kept.
        ``ValueError`` names a value none of them takes, and then nothing
        changes."""
        result = self._view("set_spectrogram", **settings)
        reason = (result or {}).get("reason") if isinstance(result, dict) else None
        if reason:
            raise ValueError(reason)

    def show_spectrogram_settings(self) -> None:
        """Show the spectrogram's settings panel to the right of the take."""
        self._do("verb", call={"verb": "show_spectrogram_settings"})

    def hide_spectrogram_settings(self) -> None:
        """Hide the spectrogram's settings panel."""
        self._do("verb", call={"verb": "hide_spectrogram_settings"})

    def retry_pending(self) -> None:
        """Retry failed repairs of this editor's files without repeating edits.

        Call after resolving the I/O failure. Raises
        `clausters.errors.CommandError` if repair fails again. Does nothing if
        none remain."""
        self._do("retry")

    # ---- what the picture shows ----

    @property
    def layers(self) -> tuple:
        """Visual layers, back to front: names or objects with `draw`,
        `visible`, `solo`, `alpha` and `y`. Two different domains cannot share
        the vertical axis; use `y="box"` for the overlaid amplitude layer."""
        return tuple(self._call("binding").get("layers") or ())

    @layers.setter
    def layers(self, stack) -> None:
        self._view("layers", stack=stack if isinstance(stack, str) else list(stack))

    def _view(self, verb: str, **args):
        return self._do("verb", call={"verb": verb, "version": self._version(), **args})

    @property
    def layer(self) -> str:
        """The active edit body, separate from the take's visual layers."""
        return self._call("layer")["layer"]

    @layer.setter
    def layer(self, name: str) -> None:
        self._view("layer", name=name)

    def show_layer(self, name: str) -> None:
        """Show a visual layer, appending it if absent."""
        self._view("show_layer", name=name)

    def hide_layer(self, name: str) -> None:
        """Hide a visual layer, keeping its place and settings."""
        self._view("hide_layer", name=name)

    def solo_layer(self, name: str) -> None:
        """Toggle a layer's solo; only visible soloed layers are drawn."""
        self._view("solo_layer", name=name)

    def layer_alpha(self, name: str, alpha: float) -> None:
        """Set a layer's opacity between zero and one."""
        self._view("layer_alpha", name=name, alpha=alpha)

    def view_wave(self) -> None:
        """Show the peak and RMS waveform in the take's lane."""
        self._view("view_wave")

    def view_spectrogram(self) -> None:
        """Show the spectrogram in the take's lane."""
        self._view("view_spectrogram")

    def view_both(self) -> None:
        """Show both, stacked: the waveform in the take's lane and the
        spectrogram in a lane under it, on one time axis."""
        self._view("view_both")

    @property
    def ruler(self) -> str:
        """**What the window's time ruler counts in**: ``"beats"`` (bars and
        beats, through the tempo map), ``"time"`` (minutes and seconds) or
        ``"samples"`` -- the View menu's Ruler, the window's own and no edit. A take's ruler is under it, seconds until another
        is chosen; a take has no tempo, so ``"beats"`` is refused with why."""
        return self._ruler()

    @ruler.setter
    def ruler(self, unit: str) -> None:
        self._set_ruler(unit)

    @property
    def folded(self) -> list:
        """The channels drawn folded, in order. A view state, outside the
        history."""
        return list(self._call("folded")["folded"])

    def fold_channel(self, channel: int) -> None:
        """Fold channel ``channel`` (from 0) to a strip, or unfold it."""
        self._channels("fold_channel", channel=int(channel))

    def solo_channel(self, channel: int) -> None:
        """Show channel ``channel`` alone: every other channel folded."""
        self._channels("solo_channel", channel=int(channel))

    def show_all_channels(self) -> None:
        """Unfold every channel."""
        self._channels("show_all_channels")

    def _channels(self, verb: str, **args) -> None:
        result = self._view(verb, **args)
        reason = (result or {}).get("reason") if isinstance(result, dict) else None
        if reason:
            raise ValueError(reason)

    # ---- the gain envelope ----

    @property
    def envelope(self) -> list:
        """Gain envelope as time/value/shape/curve quads, with time in
        seconds. One is unity. Editing the curve is non-destructive;
        `apply_envelope` renders it into a new take."""
        return list(self._call("envelope")["points"])

    def set_envelope(self, points) -> bool:
        """Set gain quads as one undoable edit. Times must increase."""
        return self._envelope("setautomation", points=[float(p) for p in points])

    def show_envelope(self) -> None:
        """Show and select the gain curve over the take."""
        self._view("show_envelope")

    def apply_envelope(self) -> bool:
        """Render gain into a new take and reset the curve to unity, in one
        undo step."""
        return self._envelope("apply_envelope")

    def _envelope(self, verb: str, **args) -> bool:
        result = self._do("act", call={"verb": verb, **args}) or {}
        if result.get("reason"):
            raise ValueError(result["reason"])
        return bool(result.get("changed"))


def reads_in_place(file) -> bool:
    """Whether the audio editor reads ``file``, a `clausters.SoundFile`,
    where it is: a PCM file on disk a part streams from. Any other file on
    disk (an MP3, a FLAC) is decoded into the editor's working folder when
    it opens, and its first save asks for a path."""
    return file.path is not None and file.codec in _STREAMED


__all__ = ["AudioEditor", "AudioTransport", "reads_in_place"]
