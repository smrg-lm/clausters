#!/usr/bin/env python3
"""Theme groups and per-widget accents: the style surface, level by level.

The host draws every chrome color from one **theme** — a table of named roles
(``background``, ``panel``, ``text``, ``accent``, ...) — and the customization
is the same partial table at every level, each overlaying the previous:

1. **The host style file** (``--theme file.toml``, or ``[gui.theme]`` in the
   shared config): one look for the whole host. This example writes a small
   file that warms the accent and boots the host with it.
2. **A theme group** (the ``theme`` prop on a container — ``window``, ``panel``,
   ``scroll``, ``track``): a partial ``{"role": "#rrggbb[aa]"}`` table scoped
   to that subtree, recursive by construction — a nested group overlays the
   *inherited* table, not the default.
3. **The `color` prop** (any widget): the single-color shorthand — it re-seeds
   just the roles that carry the widget's function (a slider's handle and
   fill, a meter's bar, a trace), leaving the rest of its theme alone.

Overlays resolve when a def arrives or a ``set`` changes them — never per
frame — so a styled window costs exactly what a plain one costs.

The window shows four rows: the host look (from the file theme), a cool theme
group, a nested darker group inside it, and a row of per-widget accents. After
a few seconds the script restyles the group and one accent live via ``set``.

The example **launches its own GUI host** and needs no audio server. Needs a
display and a Vulkan/Metal/DX12/GL adapter.

Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python   # bundles the server + GUI binaries

then::

    python clients/python/examples/gui_style.py
"""

import sys
import tempfile
import time
from pathlib import Path

from clausters.gui import GuiHost, knob, label, panel, slider, toggle, window

# Level 1: the host style file — the whole host warms up.
HOST_THEME = """\
accent = "#e08840"
accent_dim = "#8a5428"
hilite = "#f0a060"
"""

# Level 2: a theme group — a cool pane inside the warm host.
COOL = {"accent": "#4090e0", "accent_dim": "#285a8a", "hilite": "#60b0f0",
        "panel": "#10141cbf"}
# Level 3: nested — the inherited cool table, darkened further.
NESTED = {"panel": "#0a0d12", "text": "#8098b0"}


def controls(base_id: int, tag: str) -> dict:
    """One row of ordinary controls; the theme in force colors all of them."""
    return panel(base_id,
                 label(base_id + 1, tag, w=220.0),
                 slider(base_id + 2, label="amp", value=0.6),
                 knob(base_id + 3, label="freq", min=20.0, max=2000.0, value=440.0),
                 toggle(base_id + 4, label="on", value=True),
                 layout="row")


def style_window() -> dict:
    return window(
        controls(10, "host theme (file)"),
        panel(20,
              controls(30, "theme group (cool)"),
              panel(40, controls(50, "nested group (darker)"), layout="col",
                    theme=NESTED),
              layout="col", theme=COOL),
        panel(60,
              label(61, "per-widget accents", w=220.0),
              slider(62, label="a", value=0.3, color="#e04060"),
              slider(63, label="b", value=0.5, color="#40c080"),
              slider(64, label="c", value=0.7, color="#c0b040"),
              layout="row"),
        title="Style", w=980, h=560, layout="col",
    )


def main():
    theme_file = Path(tempfile.mkdtemp(prefix="clausters-style-")) / "warm.toml"
    theme_file.write_text(HOST_THEME)
    with GuiHost.boot(extra_args=("--theme", str(theme_file))) as gui:
        gui.define(1, style_window())
        print("host warm (file theme); one cool theme group, nested darker;")
        print("three per-widget accents. Live restyles follow.")

        changes = [
            (4.0, 20, {"theme": '{"accent": "#b060e0", "accent_dim": "#6a3a8a", '
                       '"hilite": "#d090f0", "panel": "#161020bf"}'},
             "the cool group turns violet (a set of its theme prop)"),
            (8.0, 62, {"color": "#40d0d0"}, "accent 'a' turns cyan (a set of color)"),
            (12.0, 20, {"theme": ""}, "the group clears: back to the host theme"),
        ]
        deadline = time.monotonic() + 30.0
        t0 = time.monotonic()
        pending = list(changes)
        while time.monotonic() < deadline:
            msg = gui.poll(timeout=0.1)
            if msg is not None and msg[0] == "/gui_closed":
                print(f"window {msg[1][0]} closed")
                break
            while pending and time.monotonic() - t0 > pending[0][0]:
                _, target, props, what = pending.pop(0)
                gui.set(target, **props)
                print(what)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
