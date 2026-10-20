"""Read and write audio files from the Python client.

`SoundFile` stores a file's path and audio metadata: frame count, channel
count, sample rate, codec and sample format. Use `open` to inspect an existing
file, `read` to decode its samples, and `write` to save samples as WAV.
`empty` describes silence without creating a file.

`to_buffer` loads the samples into a `clausters.defs.Buffer` on a server.
Local file operations use the codecs provided by the Clausters native
library and do not require a running server. Loading a file into a buffer
requires its path to be accessible to the selected server.
"""

from __future__ import annotations

from array import array
from dataclasses import dataclass

__all__ = ["SoundFile"]


@dataclass(frozen=True)
class SoundFile:
    """Audio file metadata and an optional filesystem path.

    `open` reads metadata from an existing file. `write` creates a WAV file and
    returns its metadata. `empty` creates a descriptor for silence with
    `path=None`. Samples are loaded only when `read` or `to_buffer` is called.
    """

    #: File path, or `None` for a descriptor created by `empty`.
    path: "str | None"
    #: Frames per channel.
    frames: int
    #: Number of audio channels.
    channels: int
    #: Sample rate in Hz.
    sample_rate: float
    #: The container's short name (`"wav"`, `"flac"`, `"mp3"`, ...);
    #: `None` for a file not written yet.
    codec: "str | None" = None
    #: A WAV's sample format (`"int16"`, `"int24"`, `"int32"`,
    #: `"float"`); `None` for other formats and for a file not written yet.
    sample_format: "str | None" = None

    @classmethod
    def open(cls, path) -> "SoundFile":
        """Read an audio file's metadata and return a `SoundFile`.

        Reads the frame count, channel count, sample rate, codec and sample format
        without loading the samples. Supported formats include WAV, FLAC,
        OGG/Vorbis, MP3, MP4/AAC, ALAC and AIFF.

        Args:
            path: Path to an existing audio file on the client's filesystem.

        Raises:
            clausters.ipc.RenderError: The file cannot be opened or its format
                is unsupported.
        """
        from . import ipc

        frames, channels, rate, codec, sample_format = ipc.soundfile_info(path)
        return cls(str(path), frames, channels, rate, codec, sample_format)

    @classmethod
    def empty(cls, channels: int, frames: int, sample_rate: float) -> "SoundFile":
        """Describe silence with the given channels, frame count and sample rate.

        Returns a `SoundFile` with `path=None`. This does not create a file or
        allocate samples. `read` returns zeros and `to_buffer` allocates a zeroed
        server buffer. An audio editor asks for a destination on its first save.

        Raises:
            ValueError: Channels or sample rate are not positive, or frames
                is negative.
        """
        if channels < 1 or frames < 0 or not sample_rate > 0:
            raise ValueError(
                f"an empty sound file needs channels >= 1, frames >= 0 and a "
                f"positive rate, got {channels}, {frames}, {sample_rate}")
        return cls(None, int(frames), int(channels), float(sample_rate))

    @classmethod
    def write(cls, path, samples, channels: int, sample_rate: float, *,
              sample_format: str = "int16") -> "SoundFile":
        """Write interleaved samples to a WAV file and return its `SoundFile`.

        Args:
            path: Destination path on the client's filesystem.
            samples: Sequence of floating-point samples in frame-major order
                (for stereo: left, right, left, right, ...).
            channels: Number of channels. Each frame must contain this many
                samples.
            sample_rate: Sample rate in Hz.
            sample_format: WAV encoding: `"int16"` (default), `"int24"` or
                `"float"`.

        Raises:
            clausters.ipc.RenderError: The samples, format or metadata are invalid,
                or the file cannot be written.
        """
        from . import ipc

        ipc.write_soundfile(path, samples, channels, sample_rate, sample_format)
        frames = len(samples) // channels if channels else 0
        return cls(str(path), frames, int(channels), float(sample_rate), "wav",
                   "float" if sample_format == "float32" else sample_format)

    @property
    def duration(self) -> float:
        """Duration in seconds, calculated as frames divided by sample rate."""
        return self.frames / self.sample_rate if self.sample_rate else 0.0

    @property
    def source(self):
        """Return a document `clausters.multitrack.Source` for this file.

        The source includes the channel count, frame count and sample rate. It
        references the file path when available; otherwise it is a volatile source.
        """
        from .multitrack import Source

        source = Source.file(self.path) if self.path else Source.volatile()
        return source.shaped(self.channels, self.frames, self.sample_rate)

    def read(self, start: int = 0, frames: int = -1) -> array:
        """Decode a frame range into an interleaved `array('f')` of samples.

        `start` is a zero-based frame offset. `frames=-1` reads to the end of the
        file. Each frame contains one float32 sample per channel, in channel order.
        Integer audio is scaled to `[-1, 1]`; floating-point audio can exceed that
        range. The sample rate is unchanged. If `path` is `None`, returns zeros
        for the requested range.
        """
        if self.path is None:
            start = min(max(0, start), self.frames)
            count = self.frames - start if frames < 0 else min(frames, self.frames - start)
            return array("f", bytes(4 * count * self.channels))
        from . import ipc

        samples, _, _, _ = ipc.read_soundfile(self.path, start, frames)
        return samples

    def to_buffer(self, *, wait: bool = True, timeout: "float | None" = None,
                  server=None):
        """Load this file into a new server `clausters.defs.Buffer`.

        Uses `Buffer.read` when the file has a path. The selected server must be
        able to read that path. For `path=None`, uses `Buffer.alloc` to create a
        zeroed buffer with the specified frame count, channel count and sample rate.

        Args:
            wait: Wait for the server to finish loading or allocating the buffer.
            timeout: Maximum time to wait, using the buffer method's default if None.
            server: Server to use, or the default server if omitted.

        Raises:
            ValueError: A file without a path has zero frames.
        """
        from .defs import Buffer

        if self.path is None:
            if self.frames < 1:
                raise ValueError("a sound file of no frames has nothing to allocate: "
                                 "a buffer holds at least one")
            return Buffer.alloc(self.frames, self.channels, sample_rate=self.sample_rate,
                                wait=wait, timeout=timeout, server=server)
        return Buffer.read(self, wait=wait, timeout=timeout, server=server)
