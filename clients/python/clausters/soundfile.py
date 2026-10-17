"""**A sound file on the client's side**: its path and its shape, read from the
header, and the one place a client reads or writes audio files.

A `clausters.defs.Buffer` is memory on the **server**; a `SoundFile` is a file,
with a path or -- made with `SoundFile.empty` -- not written anywhere yet. The
two meet through reads and writes: `SoundFile.to_buffer` (or
`clausters.defs.Buffer.read`) loads one into a buffer, and
`clausters.defs.Buffer.write` answers with the `SoundFile` it wrote.

Every byte goes through the server's own codecs, through the same library a
render runs in, so the client needs no audio library of its own: the header
and the samples come from the decoder ``/buffer_allocRead`` uses, and
`SoundFile.write` writes the bytes the web client writes for the same samples.
The path is the client's filesystem; `to_buffer` hands it to the server, which
reads its own -- the same disk when both run on one machine.
"""

from __future__ import annotations

from array import array
from dataclasses import dataclass

__all__ = ["SoundFile"]


@dataclass(frozen=True)
class SoundFile:
    """A sound file: where it is and what shape it has.

    Made by `SoundFile.open` (a file on disk, its header read), by
    `SoundFile.empty` (a shape with no file yet) or by `SoundFile.write` and
    `clausters.defs.Buffer.write` (the file just written). Nothing here holds
    samples: `read` decodes them when they are asked for.
    """

    #: Where the file is, or ``None`` for one not written yet.
    path: "str | None"
    #: Frames per channel.
    frames: int
    channels: int
    #: The file's own rate; nothing resamples.
    sample_rate: float
    #: The container's short name (``"wav"``, ``"flac"``, ``"mp3"``, ...);
    #: ``None`` for a file not written yet.
    codec: "str | None" = None
    #: A WAV's sample format (``"int16"``, ``"int24"``, ``"int32"``,
    #: ``"float"``); ``None`` for other formats and for a file not written yet.
    sample_format: "str | None" = None

    @classmethod
    def open(cls, path) -> "SoundFile":
        """The file at `path`, its shape read from the header -- no sample is
        decoded. WAV, FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and the rest:
        whatever the server's decoder reads. Raises `clausters.ipc.RenderError`
        when the file is missing or not audio."""
        from . import ipc

        frames, channels, rate, codec, sample_format = ipc.soundfile_info(path)
        return cls(str(path), frames, channels, rate, codec, sample_format)

    @classmethod
    def empty(cls, channels: int, frames: int, sample_rate: float) -> "SoundFile":
        """A file of this shape that is not written anywhere yet: silence to
        open in an editor, which asks for a path when it is saved."""
        if channels < 1 or frames < 0 or not sample_rate > 0:
            raise ValueError(
                f"an empty sound file needs channels >= 1, frames >= 0 and a "
                f"positive rate, got {channels}, {frames}, {sample_rate}")
        return cls(None, int(frames), int(channels), float(sample_rate))

    @classmethod
    def write(cls, path, samples, channels: int, sample_rate: float, *,
              sample_format: str = "int16") -> "SoundFile":
        """Write interleaved `samples` (any sequence of floats, an
        ``array('f')`` without a copy) to a WAV file at `path`, and answer the
        file. ``sample_format`` is ``"int16"``, ``"int24"`` or ``"float"``.
        What a render returns is saved this way::

            stats = render(...)
            SoundFile.write("take.wav", stats.samples, stats.channels, stats.sample_rate)
        """
        from . import ipc

        ipc.write_soundfile(path, samples, channels, sample_rate, sample_format)
        frames = len(samples) // channels if channels else 0
        return cls(str(path), frames, int(channels), float(sample_rate), "wav",
                   "float" if sample_format == "float32" else sample_format)

    @property
    def duration(self) -> float:
        """Length in seconds."""
        return self.frames / self.sample_rate if self.sample_rate else 0.0

    @property
    def source(self):
        """The `clausters.multitrack.Source` this file is in a document: a file
        source, or a volatile one for a file not written yet, with its shape."""
        from .multitrack import Source

        source = Source.file(self.path) if self.path else Source.volatile()
        return source.shaped(self.channels, self.frames, self.sample_rate)

    def read(self, start: int = 0, frames: int = -1) -> array:
        """Decode `frames` frames from `start` (``-1``, the default, to the
        end) into interleaved float32 samples in ``[-1, 1]``, at the file's own
        rate. A file not written yet reads as the silence it holds."""
        if self.path is None:
            start = min(max(0, start), self.frames)
            count = self.frames - start if frames < 0 else min(frames, self.frames - start)
            return array("f", bytes(4 * count * self.channels))
        from . import ipc

        samples, _, _, _ = ipc.read_soundfile(self.path, start, frames)
        return samples

    def to_buffer(self, *, wait: bool = True, timeout: "float | None" = None,
                  server=None):
        """Load the file into a new server buffer (`clausters.defs.Buffer.read`);
        a file not written yet becomes a zeroed buffer of its shape
        (`clausters.defs.Buffer.alloc`)."""
        from .defs import Buffer

        if self.path is None:
            return Buffer.alloc(self.frames, self.channels, sample_rate=self.sample_rate,
                                wait=wait, timeout=timeout, server=server)
        return Buffer.read(self, wait=wait, timeout=timeout, server=server)
