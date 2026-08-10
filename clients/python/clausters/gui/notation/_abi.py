"""The ABI plumbing the package's two native callers share.

Both halves of the layer reach the same C ABI — the engraver on one side, the
MEI encoder on the other — and both need the same two shapes to get there: a
size-then-fill call whose payload is text, and a UTF-8 byte view of a Python
string. They live here so neither half has to import the other for them.
"""

from __future__ import annotations

from ... import _native

_MISSING = (
    "no engraver in libclausters_ffi: build libverovio with "
    "third_party/build-verovio.sh, build the ABI with `cargo build -p "
    "clausters-ffi --features verovio`, and stage both with build_native.py"
)


def _engraver():
    """The loaded ABI, once it is known to carry the engraver.

    Raises ``RuntimeError`` — never an ``AttributeError`` out of ctypes — when
    the library was built without the ``verovio`` feature, which is the case a
    source checkout hits before staging.
    """
    if not _native.has_engraver():
        raise RuntimeError(_MISSING)
    return _native.lib()


def _text(fn, *args) -> str:
    """A size-then-fill call whose payload is text."""
    return _native.size_then_fill(fn, *args).decode("utf-8")


def _u8(s: str):
    return _native.as_u8(s.encode("utf-8"))


