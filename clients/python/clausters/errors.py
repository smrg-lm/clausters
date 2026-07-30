"""Library-specific exception types.

A small hierarchy so callers can catch *what* went wrong concretely instead of
guessing from a stray ``AttributeError`` or a generic ``OSError``. Everything
derives from `ClaustersError`, and each leaf *also* derives from the
builtin it used to be raised as (``OSError``, ``RuntimeError``,
``BufferError``, ``TimeoutError``, ``ValueError``) — so existing
``except OSError:`` / ``except RuntimeError:`` code (and the test-suite skips)
keep working unchanged while new code can be precise:

    try:
        stats = clausters.render(score)
    except clausters.LibraryFeatureError as e:
        print(e.symbol, "needs", e.feature)   # build it with the right feature
    except clausters.ClaustersError:
        ...                                    # any other library failure
"""


class ClaustersError(Exception):
    """Base class for every error this library raises on purpose."""


# ---- the embed cdylib (loading / ABI) ----


class LibraryError(ClaustersError):
    """A problem with the native ``libclausters`` shared library itself."""


class LibraryNotFoundError(LibraryError, OSError):
    """The ``libclausters`` cdylib could not be located on disk."""


class LibraryFeatureError(LibraryError, OSError):
    """The cdylib loaded but a required FFI symbol is missing — it was built
    without the Cargo feature that exports it.

    The concrete cause behind the otherwise cryptic ``undefined symbol`` /
    ``AttributeError``: a plain ``cargo build`` produces a ``libclausters.so``
    with the FFI surface compiled out. `symbol` and `feature` say
    exactly what is missing and what to rebuild with.
    """

    def __init__(self, message: str, *, symbol: str, feature: str):
        super().__init__(message)
        #: the missing C symbol (e.g. ``"clausters_abi_version"``)
        self.symbol = symbol
        #: the Cargo feature(s) that export it (e.g. ``"embed,realtime"``)
        self.feature = feature


class AbiMismatchError(LibraryError, OSError):
    """The cdylib's ABI version does not match this binding's."""

    def __init__(self, message: str, *, got: int, expected: int):
        super().__init__(message)
        #: ABI version reported by the loaded cdylib
        self.got = got
        #: ABI version this Python binding speaks
        self.expected = expected


# ---- runtime failures ----


class RenderError(ClaustersError, RuntimeError):
    """The offline ``render`` call failed (the core reported an error)."""


class ServerError(ClaustersError, RuntimeError):
    """The embedded server could not be opened (no audio device, etc.)."""


class CommandError(ClaustersError, RuntimeError):
    """A server command was answered with a ``/fail`` reply."""


class SegmentError(ClaustersError, ValueError):
    """The shared-memory segment is not a valid clausters segment."""


class CommandRingFull(ClaustersError, BufferError):
    """The command ring is full; the command was not sent (retry later)."""


class ReplyTimeout(ClaustersError, TimeoutError):
    """No reply arrived within the timeout."""
