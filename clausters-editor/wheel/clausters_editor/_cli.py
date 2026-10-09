"""The ``clausters-editor`` console script: run the bundled application.

It locates the binary, checks on Linux that the system has the web view the
binary links against (WebKitGTK 4.1), and runs it with the interpreter this
package is installed in as the session's default: until an environment is
chosen in the editor, the session imports what this environment has.

Lookup precedence: the ``CLAUSTERS_EDITOR_BIN`` override, the binary bundled in
the wheel (``clausters_editor/bin/``), then a source checkout's
``src-tauri/target/release/``.
"""

import ctypes
import ctypes.util
import os
import stat
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_NAME = "clausters-editor.exe" if os.name == "nt" else "clausters-editor"

#: The library the Linux binary links against, by its soname.
_WEBKIT = "libwebkit2gtk-4.1.so.0"

#: How each distribution family installs it, keyed by an `ID`/`ID_LIKE` word
#: of /etc/os-release.
_INSTALL = {
    "debian": "sudo apt install libwebkit2gtk-4.1-0",
    "ubuntu": "sudo apt install libwebkit2gtk-4.1-0",
    "fedora": "sudo dnf install webkit2gtk4.1",
    "rhel": "sudo dnf install webkit2gtk4.1   (from EPEL)",
    "arch": "sudo pacman -S webkit2gtk-4.1",
    "suse": "sudo zypper install libwebkit2gtk-4_1-0",
    "opensuse": "sudo zypper install libwebkit2gtk-4_1-0",
    "alpine": "sudo apk add webkit2gtk-4.1",
    "void": "sudo xbps-install libwebkit2gtk41",
}


def binary_path() -> str:
    """Absolute path to the application binary, or raise `SystemExit`."""
    candidates = [
        os.environ.get("CLAUSTERS_EDITOR_BIN"),
        os.path.join(_HERE, "bin", _NAME),
        os.path.join(_HERE, "..", "..", "src-tauri", "target", "release", _NAME),
    ]
    for c in candidates:
        if c and os.path.exists(c):
            return os.path.abspath(c)
    raise SystemExit(
        "clausters-editor: the application binary was not found. A wheel "
        "bundles it; in a source checkout build it with `npm run tauri build "
        "-- --no-bundle` from clausters-editor/, or point CLAUSTERS_EDITOR_BIN "
        "at it."
    )


def _os_release() -> "list[str]":
    """The `ID` and `ID_LIKE` words of /etc/os-release, most specific first."""
    words = []
    try:
        with open("/etc/os-release", encoding="utf-8") as f:
            for line in f:
                key, _, value = line.strip().partition("=")
                if key in ("ID", "ID_LIKE"):
                    words += value.strip("\"'").split()
    except OSError:
        pass
    return words


def install_hint() -> str:
    """The command that installs WebKitGTK 4.1 on this distribution."""
    for word in _os_release():
        if word in _INSTALL:
            return _INSTALL[word]
    return "install WebKitGTK 4.1 (libwebkit2gtk-4.1) with your package manager"


def has_webkit() -> bool:
    """Whether the system has WebKitGTK 4.1. The loader cache is asked first;
    loading the library is the fallback, for systems without one."""
    if ctypes.util.find_library("webkit2gtk-4.1"):
        return True
    try:
        ctypes.CDLL(_WEBKIT)
    except OSError:
        return False
    return True


def _ensure_executable(path: str):
    """Best-effort: add the executable bit if missing (a wheel may unpack the
    bundled binary without it). Ignored if the location is read-only."""
    try:
        mode = os.stat(path).st_mode
        if not mode & stat.S_IXUSR:
            os.chmod(path, (mode | 0o555) & 0o7777)
    except OSError:
        pass


def main(argv=None) -> int:
    """Run the application, forwarding ``argv``. On POSIX this *replaces* the
    Python process (``os.execv``); on Windows it spawns and returns the exit
    code."""
    argv = sys.argv[1:] if argv is None else list(argv)
    path = binary_path()
    if sys.platform.startswith("linux") and not has_webkit():
        print(
            "clausters-editor needs WebKitGTK 4.1, the system's web view, "
            f"which is not installed ({_WEBKIT} was not found).\n"
            f"  {install_hint()}",
            file=sys.stderr,
        )
        return 1
    _ensure_executable(path)
    os.environ.setdefault("CLAUSTERS_EDITOR_DEFAULT_PYTHON", sys.executable)
    if os.name == "nt":
        import subprocess

        return subprocess.call([path, *argv])
    os.execv(path, [path, *argv])  # never returns on success
    return 0  # pragma: no cover
