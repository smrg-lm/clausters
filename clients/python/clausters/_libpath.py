"""Where the native cdylibs live, preferring the copies bundled in the wheel.

The package is pure Python at runtime but reaches the Rust core through three
cdylibs (``clausters-ffi`` for `clausters._native`, the ``clausters`` crate
built with ``embed,realtime`` for `clausters.ipc`, and ``clausters-midi`` built
with ``live`` for `clausters._midi`). A built
**wheel** ships those cdylibs *inside* the package, under ``clausters/_libs/``,
so an installed package is self-contained -- ``pip install`` then ``import
clausters`` just works, no ``cargo`` and no ``target/`` directory needed.

In a plain source checkout that ``_libs/`` directory is absent and the loaders
fall back to the workspace's ``target/{release,debug}/`` (the developer build).
All three loaders share the same precedence, defined here:

1. an explicit override env var (``CLAUSTERS_FFI_LIB`` / ``CLAUSTERS_LIB`` /
   ``CLAUSTERS_MIDI_LIB``),
2. the bundled ``clausters/_libs/`` (a wheel / editable install),
3. the workspace ``target/{release,debug}/`` (a source checkout with cargo).
"""

import os

_PKG_DIR = os.path.dirname(os.path.abspath(__file__))
#: The directory a built wheel stages the cdylibs into (may not exist).
LIBS_DIR = os.path.join(_PKG_DIR, "_libs")
#: The directory a built wheel stages the standalone server binary into.
BIN_DIR = os.path.join(_PKG_DIR, "_bin")


def bundled_candidates(names) -> list[str]:
    """Absolute paths for ``names`` inside the bundled ``_libs/`` directory."""
    return [os.path.join(LIBS_DIR, n) for n in names]


def bundled_bin_candidates(names) -> list[str]:
    """Absolute paths for ``names`` inside the bundled ``_bin/`` directory."""
    return [os.path.join(BIN_DIR, n) for n in names]


def workspace_candidates(names) -> list[str]:
    """Absolute paths for ``names`` under the workspace ``target/{release,debug}/``.

    ``clients/python/clausters/`` -> the repo root is three levels up."""
    root = os.path.dirname(os.path.dirname(os.path.dirname(_PKG_DIR)))
    out = []
    for profile in ("release", "debug"):
        out += [os.path.join(root, "target", profile, n) for n in names]
    return out


def gui_workspace_candidates(names) -> list[str]:
    """Absolute paths for ``names`` under ``clients/gui/target/{release,debug}/``.

    The ``clausters-gui`` crate is an **independent** cargo workspace with its
    own ``target/`` (not the repo-root one), so its binary is located here in a
    source checkout. ``clients/python/clausters/`` -> the repo root is three
    levels up, then ``clients/gui/target/``."""
    root = os.path.dirname(os.path.dirname(os.path.dirname(_PKG_DIR)))
    gui_target = os.path.join(root, "clients", "gui", "target")
    out = []
    for profile in ("release", "debug"):
        out += [os.path.join(gui_target, profile, n) for n in names]
    return out
