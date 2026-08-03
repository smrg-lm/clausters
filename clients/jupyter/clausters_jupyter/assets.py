"""Where the browser half comes from, and how it reaches the page.

The package carries a copy of the web client's built `dist/` — the wasm GUI
host, the wasm engine, the shared core and the ES modules around them. It
builds none of it: `clients/web/build.sh` is the one builder, and
`scripts/refresh-web.sh` stages its output here. Resolution follows the same
precedence the rest of the project uses for staged artifacts, and for the same
reason — in a source checkout the vendored copy is the stale one:

    ``CLAUSTERS_WEB_DIST`` → the vendored ``_web/`` → the workspace ``dist/``

**Why the assets travel over the comm rather than over HTTP.** anywidget serves
one module, the widget's own `_esm`; sibling files are not served, and there is
no static route to add one to in Colab or in a remote VS Code kernel. Since the
carrier already reaches the page, the assets ride it: the JS modules become
blob URLs the front end imports, and each `.wasm` arrives as an ArrayBuffer,
which is what wasm-bindgen's ``init`` takes anyway (``__wbg_init(bytes)``), so
nothing in the page ever fetches a URL.

That costs one transfer of a few megabytes per kernel, which is why `bundle`
sends only what the chosen backend needs: the GUI host always, the engine only
for the in-page one.
"""

import hashlib
import os
import pathlib

__all__ = ["dist_dir", "bundle", "digest", "GUI_ASSETS", "ENGINE_ASSETS"]

#: The vendored copy, staged by ``scripts/refresh-web.sh``.
_VENDORED = pathlib.Path(__file__).parent / "_web"

#: Relative to `dist_dir`: what any backend needs — the host's glue and wasm,
#: the shared core, and the page-side driver the widget's module imports.
GUI_ASSETS = (
    "gui-host/clausters_gui.js",
    "gui-host/clausters_gui_bg.wasm",
    "core/clausters_core_web.js",
    "core/clausters_core_web_bg.wasm",
    # The canvas measuring the widget shares with a served page. It rides the
    # comm like everything else rather than being imported: anywidget serves
    # the widget's own module and nothing beside it, so a static import of a
    # sibling fails to resolve in the page.
    "gui/canvasbox.js",
)

#: Extra for ``backend="page"``: the server compiled to wasm, the worklet it
#: runs in and the loader that puts the two together. The worklet is loaded
#: into the audio thread's own scope by URL, so it is a blob like the rest —
#: and it imports the glue and the shim, which is why they ride too.
ENGINE_ASSETS = (
    "engine/clausters_web.js",
    "engine/clausters_web_bg.wasm",
    "engine/worklet.js",
    "engine/worklet-shim.js",
    "engine/loader.js",
)


def dist_dir() -> pathlib.Path:
    """The `dist/` this process serves, by the precedence above.

    Raises `FileNotFoundError` naming the fix when none of the three exists —
    in a fresh checkout the vendored copy is absent until the web package has
    been built once.
    """
    override = os.environ.get("CLAUSTERS_WEB_DIST")
    if override:
        path = pathlib.Path(override).expanduser()
        if not path.is_dir():
            raise FileNotFoundError(
                f"CLAUSTERS_WEB_DIST points at {path}, which is not a directory")
        return path
    if (_VENDORED / "index.js").is_file():
        return _VENDORED
    workspace = _workspace_dist()
    if workspace is not None:
        return workspace
    raise FileNotFoundError(
        "no built web client found. Run scripts/refresh-web.sh (which runs "
        "clients/web/build.sh and stages the result into the package), or "
        "point CLAUSTERS_WEB_DIST at a built clients/web/dist."
    )


def bundle(*, engine: bool) -> dict:
    """The assets to hand the front end: relative path -> bytes.

    ``engine`` includes the in-page server; a native backend does not need it
    and does not pay for it.
    """
    root = dist_dir()
    names = GUI_ASSETS + (ENGINE_ASSETS if engine else ())
    out = {}
    for name in names:
        path = root / name
        if not path.is_file():
            raise FileNotFoundError(
                f"{name} missing from {root} - the build is incomplete; "
                "re-run scripts/refresh-web.sh")
        out[name] = path.read_bytes()
    return out


def digest(payload: dict) -> str:
    """A short content hash of a `bundle`, naming that exact build.

    What it is for: JupyterLab is one page, so a second notebook can boot its
    own wasm host on the blob URLs the first one staged — but only if they are
    the same bytes. A version string would answer that question wrong in the
    one place it matters, a source checkout where `dist/` is rebuilt under a
    fixed version, so the bytes answer it themselves.
    """
    h = hashlib.blake2b(digest_size=16)
    for name in sorted(payload):
        h.update(name.encode())
        h.update(payload[name])
    return h.hexdigest()


def _workspace_dist():
    """`clients/web/dist` when this package is imported from a source
    checkout, else ``None``."""
    here = pathlib.Path(__file__).resolve()
    for parent in here.parents:
        candidate = parent / "clients" / "web" / "dist"
        if (candidate / "index.js").is_file():
            return candidate
    return None
