"""The shared configuration file, read by the client.

Clausters reads one TOML schema across the server and every client (the schema
and its precedence are documented once, on the server side). The client only
**reads** it -- the user edits the files; nothing here writes them. Two layers
combine, the project file overriding the user file field by field, and a value
passed explicitly in code (or a server launched with its own flags) still wins
over both:

1. **user** -- ``$CLAUSTERS_CONFIG``, else ``$XDG_CONFIG_HOME/clausters/config.toml``,
   else (Windows) ``%APPDATA%\\clausters\\config.toml``, else
   ``~/.config/clausters/config.toml``.
2. **project** -- the nearest ``clausters.toml`` found walking up from the
   current working directory (like Cargo finding ``Cargo.toml``).

The client reads the ``[client]`` section (connection defaults) and the
``[server]`` section (the `ServerOptions` defaults). It mirrors the resolution
the Rust server and GUI use, so all three agree on the same files. Parsing uses
the standard-library ``tomllib``, which is why the package requires Python 3.11.
"""

import os
import sys
import tomllib
from pathlib import Path

#: Env var pointing straight at the user config file (highest priority).
_CONFIG_ENV = "CLAUSTERS_CONFIG"
#: The file name searched for the project layer, walking up from the CWD.
_PROJECT_FILE = "clausters.toml"

_cache: "dict | None" = None


def user_config_path() -> "Path | None":
    """The user config file path, by the same precedence the Rust side uses:
    ``$CLAUSTERS_CONFIG``, then ``$XDG_CONFIG_HOME``, then (Windows)
    ``%APPDATA%``, then ``~/.config``. ``None`` if no home can be found."""
    env = os.environ.get(_CONFIG_ENV)
    if env:
        return Path(env)
    xdg = os.environ.get("XDG_CONFIG_HOME")
    if xdg:
        return Path(xdg) / "clausters" / "config.toml"
    if os.name == "nt":
        appdata = os.environ.get("APPDATA")
        if appdata:
            return Path(appdata) / "clausters" / "config.toml"
    home = os.environ.get("HOME")
    if home:
        return Path(home) / ".config" / "clausters" / "config.toml"
    return None


def find_project_config(start: "Path | None" = None) -> "Path | None":
    """The nearest ``clausters.toml`` at or above ``start`` (the current working
    directory by default), or ``None`` if none is found up to the filesystem
    root."""
    base = (start or Path.cwd()).resolve()
    for cur in (base, *base.parents):
        candidate = cur / _PROJECT_FILE
        if candidate.is_file():
            return candidate
    return None


def _read(path: "Path | None") -> dict:
    """Parses one TOML file. A missing file yields ``{}``; a malformed one yields
    ``{}`` and a warning on stderr (so a typo is noticed, not silently ignored)."""
    if path is None or not path.is_file():
        return {}
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        print(f"clausters: ignoring malformed config {path}: {e}", file=sys.stderr)
        return {}


def _merge(low: dict, high: dict) -> dict:
    """Merges ``high`` over ``low`` recursively, so a project section overrides a
    user section key by key (the same field-by-field rule as the Rust merge)."""
    out = dict(low)
    for key, value in high.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = _merge(out[key], value)
        else:
            out[key] = value
    return out


def load_config(*, refresh: bool = False) -> dict:
    """The merged configuration (project over user) as a nested dict.

    The result is cached after the first call, since the files do not change
    during a run and several objects read it at construction time; pass
    ``refresh=True`` to re-read from disk.

    Args:
        refresh: re-read the files instead of returning the cached result.

    Returns:
        A nested ``dict`` with ``server`` / ``client`` / ``gui`` / ``standalone``
        sections (absent sections simply missing). Empty when no config exists.
    """
    global _cache
    if _cache is None or refresh:
        _cache = load_config_from(Path.cwd())
    return _cache


def load_config_from(cwd: "Path | None") -> dict:
    """Like `load_config` but searches for the project file from ``cwd`` and does
    not cache -- the testable core. ``None`` skips the project layer."""
    user = _read(user_config_path())
    project = _read(find_project_config(cwd)) if cwd is not None else {}
    return _merge(user, project)


def client_config() -> dict:
    """The ``[client]`` section (connection defaults), or ``{}``."""
    return load_config().get("client", {})


def server_config() -> dict:
    """The ``[server]`` section (the `ServerOptions` defaults), or ``{}``."""
    return load_config().get("server", {})
