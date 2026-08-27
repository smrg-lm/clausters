#!/usr/bin/env python3
"""Build the cargo artifacts and stage them inside the package for wheels.

The Clausters Python package is pure Python at runtime, but reaches the Rust
core through artifacts built by cargo, not pip:

- ``clausters-ffi`` -> ``libclausters_ffi`` (the numeric core: :mod:`clausters._native`)
- ``clausters-midi`` built with ``live`` -> ``libclausters_midi`` (the MIDI file
  writers *and* the virtual ports behind `clausters.responders.MidiFunc`:
  :mod:`clausters._midi`). The feature is off in the crate's defaults and on
  here without a knob: a client that cannot open a port cannot play or record
  MIDI at all.
- the ``clausters`` crate built with ``embed,realtime`` -> ``libclausters``
  (the embedded server / offline render: :mod:`clausters.ipc`)
- the ``clausters`` binary (default features) -> the **standalone server**
  shipped as the wheel's ``clausters`` command (a separate, networked or
  shared-memory server process; the embedded one above runs in-process).
- the ``clausters-gui`` binary (from the **independent** ``clients/gui`` cargo
  workspace) -> the **visual server** the launcher runs (`clausters.launch` /
  `clausters.Session.gui`). Bundled here too, stripped, so the one package is
  self-contained — server *and* GUI, no separate install.
- ``libfaust``, copied out of the build machine's prefix (it is not ours to
  build) -> a `FaustDef` compiles on a machine without it installed. The `faust`
  feature is on by default, so this is what keeps the two def families *equally*
  usable from an installed wheel. It is also the single heaviest thing in the
  wheel, because the Faust compiler is an LLVM JIT and links it in
  (``third_party/build-faust.sh`` explains what that link does and does not
  take).
- ``libverovio`` and its SMuFL resource data, copied out of the prefix
  ``third_party/build-verovio.sh`` installed into (they are ours to build, but
  not ours to link) -> the `score` widget's notation engraves and edits from an
  installed wheel with nothing else present, and the client keeps
  ``dependencies = []``. Bound with ctypes at runtime, so nothing links it.

This module builds them and copies the cdylibs into ``clausters/_libs/`` and the
binaries into ``clausters/_bin/`` so they ship with the wheel (and are picked up
by an editable install). It is imported by ``setup.py`` and is also runnable on
its own to stage the artifacts ahead of a plain ``pip install``::

    python clients/python/build_native.py            # release (default)
    python clients/python/build_native.py --debug     # debug profile

Environment knobs (also honoured by ``setup.py``):

- ``CLAUSTERS_WORKSPACE``        path to the cargo workspace root (auto-detected
                                 by searching upward otherwise).
- ``CLAUSTERS_SKIP_NATIVE_BUILD`` if set, never run cargo; package whatever is
                                 already staged in ``clausters/_libs/`` and
                                 ``clausters/_bin/``.
- ``CLAUSTERS_SKIP_GUI_BUILD``   if set, do not build/stage the heavy
                                 ``clausters-gui`` binary (a light, server-only
                                 wheel); a source checkout's ``clients/gui/target``
                                 binary is still used at runtime if present.
- ``CLAUSTERS_GUI_FEATURES``     extra cargo features for the GUI binary (e.g.
                                 ``standalone``); default none.
- ``CLAUSTERS_CARGO_FEATURES``   features for the embed library
                                 (default ``embed,realtime``).
- ``CLAUSTERS_CARGO_PROFILE``    ``release`` (default) or ``debug``.
- ``FAUST_PREFIX``               where ``third_party/build-faust.sh`` installed
                                 libfaust (default: ``~/.local``, then
                                 ``/usr/local``).
- ``VEROVIO_PREFIX``             where ``third_party/build-verovio.sh`` installed
                                 libverovio (same defaults).
- ``CLAUSTERS_SKIP_FAUST``       build without the `faust` def family instead of
                                 stopping when libfaust is missing (and without
                                 it even when installed). A SynthDef-only server:
                                 every ``/def_send faust`` fails.
- ``CLAUSTERS_SKIP_SYNTH``       the peer knob, for a deliberately Faust-only
                                 build: no ``/def_send synth``, no UGen graphs. It has no
                                 library to miss, so there is nothing to probe —
                                 it is a preference, not a fallback.
- ``CLAUSTERS_SKIP_VEROVIO``     build without the notation layer; the `score`
                                 widget will not engrave.
- ``CLAUSTERS_REQUIRE_COMPLETE`` refuse every ``CLAUSTERS_SKIP_*`` above: this
                                 build ships whole or not at all. Set by CI and
                                 by the release.

Both vendored libraries behave the same way, which is deliberate: they are built
the same way (a pinned source, a script under ``third_party/``, a prefix), they
are missing for the same reason, and so they fail the same way — one line naming
the recipe, before anything is compiled, with an explicit opt-out for the
developer who is working on something else today.
"""

import os
import platform
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PKG_DIR = os.path.join(HERE, "clausters")
LIBS_DIR = os.path.join(PKG_DIR, "_libs")
BIN_DIR = os.path.join(PKG_DIR, "_bin")

# The cdylib stems cargo builds and the crate that produces each.
_CRATES = {
    # `verovio` pulls `notation` with it: the whole notation layer, which is
    # native and shared, so the wheel's ABI carries it and the Python client is
    # a shell over it rather than a second implementation. Dropped when the
    # machine has no libverovio to link -- see `_ffi_features`.
    "clausters_ffi": ("clausters-ffi", "verovio"),
    "clausters": ("clausters", "embed,realtime"),  # overridable below
    # `live` is not this crate's default -- the file writers (SMF, MIDI 2.0
    # clip) need no system MIDI -- but the wheel always ships it, because
    # `MidiFunc`/`MidiReceiver` open a virtual port and without the feature a
    # client can only write MIDI files, never play or record one.
    "clausters_midi": ("clausters-midi", "live"),
}

# The server crate's default features, mirroring the `default` list in the root
# Cargo.toml — so dropping one means re-adding the rest by hand, which is what
# `--no-default-features` costs and why this list has to move when that one does.
_DEFAULT_FEATURES = ["synth", "faust", "realtime", "midi", "pipewire", "rtprio"]


def bin_name() -> str:
    """Platform file name of the standalone server binary."""
    return "clausters.exe" if platform.system() == "Windows" else "clausters"


def gui_bin_name() -> str:
    """Platform file name of the ``clausters-gui`` visual-server binary."""
    return "clausters-gui.exe" if platform.system() == "Windows" else "clausters-gui"


def _dylib_names(stem: str) -> list[str]:
    """Platform shared-library file name(s) for a cargo crate ``stem``."""
    system = platform.system()
    if system == "Darwin":
        return [f"lib{stem}.dylib"]
    if system == "Windows":
        return [f"{stem}.dll"]
    return [f"lib{stem}.so"]


def _is_workspace(path: str) -> bool:
    cargo = os.path.join(path, "Cargo.toml")
    if not os.path.isfile(cargo):
        return False
    try:
        with open(cargo, encoding="utf-8") as f:
            return "[workspace]" in f.read()
    except OSError:
        return False


def find_workspace() -> str | None:
    """The cargo workspace root: ``CLAUSTERS_WORKSPACE`` or the nearest ancestor
    with a ``[workspace]`` ``Cargo.toml``. ``None`` if not reachable (e.g. an
    isolated build that copied only this directory)."""
    env = os.environ.get("CLAUSTERS_WORKSPACE")
    if env:
        return os.path.abspath(env) if _is_workspace(env) else None
    path = HERE
    while True:
        if _is_workspace(path):
            return path
        parent = os.path.dirname(path)
        if parent == path:
            return None
        path = parent


def staged_libs() -> list[str]:
    """The cdylibs already present in ``clausters/_libs/`` for this platform."""
    if not os.path.isdir(LIBS_DIR):
        return []
    wanted = {n for stem in _CRATES for n in _dylib_names(stem)}
    return [os.path.join(LIBS_DIR, n) for n in wanted
            if os.path.exists(os.path.join(LIBS_DIR, n))]


def staged_bin() -> str | None:
    """The standalone binary staged in ``clausters/_bin/``, or ``None``."""
    path = os.path.join(BIN_DIR, bin_name())
    return path if os.path.exists(path) else None


def staged_gui_bin() -> str | None:
    """The GUI binary staged in ``clausters/_bin/``, or ``None``."""
    path = os.path.join(BIN_DIR, gui_bin_name())
    return path if os.path.exists(path) else None


def _cargo_build(workspace: str, crate: str, features: str | None, profile: str,
                 no_default: bool = False):
    cmd = ["cargo", "build", "-p", crate]
    if profile == "release":
        cmd.append("--release")
    if no_default:
        cmd.append("--no-default-features")
    if features:
        cmd += ["--features", features]
    print("clausters: " + " ".join(cmd))
    subprocess.run(cmd, cwd=workspace, check=True)


def _cargo_build_bin(workspace: str, profile: str, features: str = "",
                     no_default: bool = False):
    """Build the standalone server binary — default features unless a def family
    was left out, in which case the survivors are named explicitly."""
    cmd = ["cargo", "build", "--bin", "clausters"]
    if profile == "release":
        cmd.append("--release")
    if no_default:
        cmd.append("--no-default-features")
    if features:
        cmd += ["--features", features]
    print("clausters: " + " ".join(cmd))
    subprocess.run(cmd, cwd=workspace, check=True)


def stage(workspace: str, profile: str) -> list[str]:
    """Copy every freshly built cdylib into ``clausters/_libs/``."""
    target = os.path.join(workspace, "target", profile)
    os.makedirs(LIBS_DIR, exist_ok=True)
    copied = []
    for stem in _CRATES:
        for name in _dylib_names(stem):
            src = os.path.join(target, name)
            if os.path.exists(src):
                shutil.copy2(src, os.path.join(LIBS_DIR, name))
                copied.append(name)
    return copied


def stage_binary(workspace: str, profile: str) -> str | None:
    """Copy the freshly built standalone binary into ``clausters/_bin/``,
    preserving its executable bit (``copy2``). Returns its name or ``None``."""
    src = os.path.join(workspace, "target", profile, bin_name())
    if not os.path.exists(src):
        return None
    os.makedirs(BIN_DIR, exist_ok=True)
    shutil.copy2(src, os.path.join(BIN_DIR, bin_name()))
    return bin_name()


def _gui_workspace(workspace: str) -> str:
    """The independent ``clausters-gui`` crate directory under the repo root."""
    return os.path.join(workspace, "clients", "gui")


def _cargo_build_gui(workspace: str, profile: str):
    """Build the ``clausters-gui`` binary in its own workspace (``clients/gui``)."""
    cmd = ["cargo", "build", "--bin", "clausters-gui"]
    if profile == "release":
        cmd.append("--release")
    features = os.environ.get("CLAUSTERS_GUI_FEATURES")
    if features:
        cmd += ["--features", features]
    print("clausters: " + " ".join(cmd) + " (in clients/gui)")
    subprocess.run(cmd, cwd=_gui_workspace(workspace), check=True)


def stage_gui_binary(workspace: str, profile: str) -> str | None:
    """Copy the freshly built ``clausters-gui`` binary into ``clausters/_bin/``
    and strip it. Returns its name or ``None``.

    The binary is heavy chiefly because of debug symbols; a release build is a
    fraction of a debug one, and stripping the staged copy trims it further
    (system libraries are dynamic, not embedded), so the wheel stays small
    without touching the developer's cargo profiles."""
    src = os.path.join(_gui_workspace(workspace), "target", profile, gui_bin_name())
    if not os.path.exists(src):
        return None
    os.makedirs(BIN_DIR, exist_ok=True)
    dst = os.path.join(BIN_DIR, gui_bin_name())
    shutil.copy2(src, dst)
    _strip(dst)
    return gui_bin_name()


def _needed_libs(path: str) -> dict[str, str]:
    """The shared libraries ``path`` links against, as ``{soname: resolved path}``.

    Linux only (``ldd``); other platforms return nothing, which simply means no
    Faust libraries are staged there.
    """
    if platform.system() != "Linux" or shutil.which("ldd") is None:
        return {}
    try:
        out = subprocess.run(["ldd", path], check=True, capture_output=True,
                             text=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return {}
    needed = {}
    for line in out.splitlines():
        # "  libfaust.so.2 => /home/u/.local/lib/libfaust.so.2 (0x00007f...)"
        if "=>" not in line:
            continue
        soname, _, rest = line.strip().partition("=>")
        resolved = rest.strip().split(" (")[0].strip()
        if soname.strip() and os.path.isfile(resolved):
            needed[soname.strip()] = resolved
    return needed


# Baseline shared libraries every glibc target is assumed to provide. These are
# never vendored: bundling the C/C++ runtime or the loader would pin a copy older
# than (or ABI-incompatible with) the host's and break everything that links them
# system-wide. Matches the spirit of auditwheel's policy whitelist, scoped to what
# libfaust can pull in. Everything else in its transitive closure *is* vendored
# (see ``stage_faust_libs``).
_SYSTEM_SONAME_PREFIXES = (
    "libc.so", "libm.so", "libdl.so", "librt.so", "libpthread.so",
    "libutil.so", "libresolv.so", "libnsl.so", "libgcc_s.so", "libstdc++.so",
    "ld-linux",
)


def stage_faust_libs(profile: str) -> list[str]:
    """Copy libfaust — and its transitive deps — beside the cdylibs in ``_libs/``.

    The `faust` feature is on by default, so the built artifacts link libfaust
    dynamically. Bundling it is what makes an installed wheel able to compile a
    FaustDef on a machine without it installed — the same self-contained
    packaging the ``clausters-gui`` binary gets. ``build.rs`` writes a `DT_RPATH`
    of ``$ORIGIN``/``$ORIGIN/../_libs``, inherited by transitive dependencies, so
    the loader finds these copies before (or without) any system ones.

    libfaust does not stop at itself: the LLVM it links statically still reaches
    libz and libzstd, neither of which is ours nor guaranteed on the target.
    Worse, their sonames drift between distro generations — a wheel built where
    LLVM linked ``libxml2.so.2`` fails to load on a host that only ships
    ``libxml2.so.16`` (exactly the "cannot open shared object file" the
    standalone server dies with). So we vendor the **whole transitive closure**
    of libfaust, minus the baseline system libraries in
    ``_SYSTEM_SONAME_PREFIXES``. ``ldd`` already flattens the tree, so one pass
    over the root's resolved path captures deps-of-deps.

    The libraries are read off the *staged* artifacts, keyed by the exact soname
    the loader asks for. A server built without the feature needs no libfaust, so
    nothing is found and nothing is staged.
    """
    artifacts = [p for p in (staged_bin(), *staged_libs()) if p]
    # The one library we deliberately vendor (the Faust JIT compiler), keyed by
    # the exact soname the loader asks for and its resolved build-host path. We
    # scope the closure to *this* root, not to the whole binary, so we never drag
    # in the GUI's graphics/audio system stack.
    roots: dict[str, str] = {}
    for art in artifacts:
        for soname, resolved in _needed_libs(art).items():
            if soname.startswith("libfaust."):
                roots.setdefault(soname, resolved)
    wanted: dict[str, str] = dict(roots)
    for resolved in list(roots.values()):
        for soname, dep in _needed_libs(resolved).items():
            if soname.startswith(_SYSTEM_SONAME_PREFIXES):
                continue
            wanted.setdefault(soname, dep)
    if wanted and platform.system() == "Linux" and shutil.which("patchelf") is None:
        raise SystemExit(
            "clausters: patchelf is required to bundle libfaust into a "
            "relocatable wheel (it rewrites their run path to $ORIGIN). Install it "
            "with `pip install patchelf` (a self-contained wheel) or your package "
            "manager, then rebuild."
        )
    copied = []
    for soname, resolved in sorted(wanted.items()):
        dst = os.path.join(LIBS_DIR, soname)
        src = os.path.realpath(resolved)
        # Re-running staging resolves a root to its already-staged copy (the
        # artifacts' rpath includes ``_libs``); don't copy a file onto itself.
        if os.path.realpath(dst) != src:
            shutil.copy2(src, dst)
            _strip(dst)
        if platform.system() == "Linux":
            _set_origin_rpath(dst)
        copied.append(soname)
    return copied


# Set by CI and the release: this build must leave nothing out. Requiring every
# piece is the default, so the only job left for this is to refuse a
# CLAUSTERS_SKIP_* — and that job is the same for all three pieces, which is why
# it is one variable and not one per piece.
_REQUIRE_COMPLETE = "CLAUSTERS_REQUIRE_COMPLETE"


def _skipping(skip: str, without: str) -> bool:
    """Whether ``skip`` asks for a piece to be left out — refused outright when
    the build must be complete.

    One rule for the three pieces, because they fail the same way: a package
    missing one raises at *the user's* run time, and nothing downstream reports
    it — the notation tests skip themselves, a FaustDef only fails when someone
    sends one. So a build that must be complete refuses the request rather than
    honouring it quietly.
    """
    if not os.environ.get(skip):
        return False
    if os.environ.get(_REQUIRE_COMPLETE):
        raise SystemExit(
            f"clausters: {skip} is set and so is {_REQUIRE_COMPLETE}, which "
            f"means this build must leave nothing out -- and {skip} means "
            f"{without}.")
    print(f"clausters: {skip} set -- {without}")
    return True


def _links(lib: str, prefix: str | None, env: str, recipe: str, skip: str,
           without: str) -> bool:
    """Whether this build links ``lib``, having probed for it and not found it.

    One answer for both vendored libraries, which is the point. libfaust and
    libverovio are built the same way — a pinned source, a script under
    ``third_party/``, a prefix that defaults to ``~/.local`` — and they are
    missing for the same reason: a checkout that has not run the script yet. So
    they behave the same way here. Present, we link them. Absent, the build stops
    on one line naming the recipe, rather than in the linker (``unable to find
    library -lverovio``, under a page of `cc` arguments, is where this used to
    end up). Absent *and* deliberately skipped, we build without, and say what
    that costs.

    The default is to require them because that is what a def family and an
    engraver are: parts of the product, not options. The opt-out exists for the
    developer who wants to work on something else today and can live `without`
    — building a 13 MB C++ library to touch the sequencer is a bad trade.
    """
    # The skip is read before the probe on purpose: it means "build without
    # this", not "I could not find it", so it holds whether or not the library
    # happens to be installed.
    if _skipping(skip, without):
        return False
    if prefix is not None:
        return True
    raise SystemExit(
        f"clausters: no {lib} found (looked in {env}, ~/.local, /usr/local); "
        f"build it with {recipe} -- or set {skip}=1 to build without it "
        f"({without})")


def _links_faust() -> bool:
    """Whether the server artifacts are built with the `faust` def family."""
    return _links("libfaust", _faust_prefix(), "FAUST_PREFIX",
                  "third_party/build-faust.sh", "CLAUSTERS_SKIP_FAUST",
                  "a SynthDef-only server: every /def_send faust fails")


def _dropped_families(with_faust: bool) -> set[str]:
    """Which def families this build leaves out.

    They are peers, so either can go and the crate still builds: `faust` when
    there is no libfaust to link (or nobody wants to wait for one), `synth` when
    the build is deliberately Faust-only.
    """
    dropped = set()
    if not with_faust:
        dropped.add("faust")
    if _skipping("CLAUSTERS_SKIP_SYNTH", "no /def_send synth, no UGen graphs"):
        dropped.add("synth")
    return dropped


def _server_features(extra: str, dropped: set[str]) -> tuple[str, bool]:
    """The ``--features`` list and whether to pass ``--no-default-features``, for
    a server artifact built on top of ``extra``.

    Dropping a default feature means turning the defaults off and naming the
    survivors, because cargo features only ever add — which is exactly the knob
    this file did not have, and why a Faust-only or SynthDef-only package could
    not be built through it at all.

    Dropping nothing returns the command line unchanged rather than an
    equivalent-but-different one: the ordinary build, the one CI and the release
    run, should not acquire flags because an opt-out exists that nobody used.
    """
    if not dropped:
        return extra, False
    keep = [f for f in _DEFAULT_FEATURES if f not in dropped]
    keep += [f for f in extra.split(",") if f and f not in keep]
    return ",".join(keep), True


def _links_verovio() -> bool:
    """Whether ``clausters-ffi`` is built with the `verovio` notation layer."""
    return _links("libverovio", _verovio_prefix(), "VEROVIO_PREFIX",
                  "third_party/build-verovio.sh", "CLAUSTERS_SKIP_VEROVIO",
                  "the `score` widget will not engrave")


def _prefix(env: str, names: list[str]) -> str | None:
    """The prefix the *linker* will look in, or ``None`` if the library is not
    there — the question this whole file needs answered before it runs cargo.

    Mirrors the resolution in ``build.rs`` (both of them), including the part
    that is easy to get wrong: an explicitly set ``*_PREFIX`` **wins outright**,
    with no fallback to the defaults. Walking the defaults anyway would let this
    report "found it in ~/.local" about a build that is going to link somewhere
    else entirely and fail there — the two have to agree, or the check is worse
    than none.
    """
    prefix = os.environ.get(env)
    if not prefix:
        local = os.path.expanduser("~/.local")
        prefix = local if _has_lib(local, names) else "/usr/local"
    return prefix if _has_lib(prefix, names) else None


def _has_lib(prefix: str, names: list[str]) -> bool:
    lib = os.path.join(prefix, "lib")
    return any(os.path.exists(os.path.join(lib, name)) for name in names)


def _faust_prefix() -> str | None:
    """Where ``build-faust.sh`` installed libfaust. Either library form counts —
    build.rs accepts the shared object or the archive."""
    return _prefix("FAUST_PREFIX", _faust_names())


def _faust_names() -> list[str]:
    system = platform.system()
    if system == "Darwin":
        return ["libfaust.dylib", "libfaust.a"]
    if system == "Windows":
        return ["faust.dll", "faust.lib"]
    return ["libfaust.so", "libfaust.a"]


def _verovio_prefix() -> str | None:
    """Where ``build-verovio.sh`` installed libverovio."""
    return _prefix("VEROVIO_PREFIX", [_verovio_name()])


def _verovio_name() -> str:
    system = platform.system()
    if system == "Darwin":
        return "libverovio.dylib"
    if system == "Windows":
        return "verovio.dll"
    return "libverovio.so"


def stage_verovio() -> list[str]:
    """Copy libverovio and its SMuFL resource data into ``_libs/``.

    Same arrangement as libfaust beside it — a third-party library we build from
    a pinned source (``third_party/build-verovio.sh``) into a prefix, then bundle
    so an installed wheel needs nothing else on the machine. The client binds it
    with ctypes at runtime, so unlike libfaust nothing links it at build time;
    that is also why there is no transitive closure to vendor (its only shared
    dependencies are the C++/C runtime) and no run path to rewrite.

    The resource data comes along because verovio bakes its resource path in at
    *configure* time, pointing at the prefix it was built for. Staged beside the
    library as ``_libs/verovio/``, it is found by `clausters.gui.notation`, which
    passes it to each toolkit explicitly — a toolkit that cannot find its SMuFL
    data engraves nothing.

    Missing, there is nothing to decide here: `_links_verovio` already stopped
    the build, before anything was compiled, unless the engraver was skipped on
    purpose. Which is the case this reaches — and it is worth being loud about,
    because a wheel without the engraver raises at *the user's* run time and the
    notation tests skip themselves rather than failing, so nothing downstream
    would report it.
    """
    prefix = _verovio_prefix()
    if prefix is None:
        print("clausters: no libverovio staged (CLAUSTERS_SKIP_VEROVIO) -- the "
              "`score` widget will not engrave")
        return []
    name = _verovio_name()
    os.makedirs(LIBS_DIR, exist_ok=True)
    dst = os.path.join(LIBS_DIR, name)
    shutil.copy2(os.path.join(prefix, "lib", name), dst)
    _strip(dst)
    data_src = os.path.join(prefix, "share", "verovio")
    staged = [name]
    if os.path.isdir(data_src):
        data_dst = os.path.join(LIBS_DIR, "verovio")
        shutil.rmtree(data_dst, ignore_errors=True)
        shutil.copytree(data_src, data_dst)
        staged.append("verovio/ (SMuFL resources)")
    return staged


def _strip(path: str):
    """Best-effort strip of debug symbols from a staged binary (POSIX). A missing
    ``strip`` tool or a Windows build leaves it as-is."""
    if os.name == "nt" or shutil.which("strip") is None:
        return
    try:
        subprocess.run(["strip", path], check=True)
    except (OSError, subprocess.CalledProcessError):
        pass


def _set_origin_rpath(path: str):
    """Rewrite a vendored library's run path to ``$ORIGIN`` so it finds its
    siblings in ``_libs/`` (Linux only).

    libfaust/libLLVM and their transitive deps come from the build host, not our
    build, so they carry the host's run paths — libLLVM's is ``$ORIGIN/../lib``,
    a directory that does not exist in the wheel. Worse, libLLVM uses ``DT_RUNPATH``,
    which (unlike the ``DT_RPATH`` ``build.rs`` gives *our* artifacts) is **not**
    inherited down the dependency chain: the standalone binary's ``$ORIGIN/../_libs``
    is therefore not consulted for libLLVM's own deps (libxml2, libzstd, …), and
    the loader falls through to the system, whose soname may differ (the
    ``libxml2.so.2`` vs ``libxml2.so.16`` failure). Pointing every vendored lib at
    ``$ORIGIN`` — the same directory they all live in — makes each one resolve its
    direct deps locally, which covers the whole graph. This is what auditwheel
    does; here it must run in ``build_native`` because the release builds a plain
    wheel with no auditwheel/repair step."""
    subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", path], check=True)


def build_and_stage(profile: str = "release", *, allow_skip: bool = False) -> list[str]:
    """Build the cdylibs (unless skipped) and stage them; return staged names.

    With ``allow_skip`` (used from ``setup.py``), a missing workspace or a
    ``CLAUSTERS_SKIP_NATIVE_BUILD`` request falls back to the libs already
    staged instead of failing, so an isolated build of a pre-staged tree still
    produces a valid wheel."""
    skip = bool(os.environ.get("CLAUSTERS_SKIP_NATIVE_BUILD"))
    workspace = find_workspace()

    if skip or workspace is None:
        already = [os.path.basename(p) for p in staged_libs()]
        if staged_bin():
            already.append(bin_name())
        if staged_gui_bin():
            already.append(gui_bin_name())
        reason = ("CLAUSTERS_SKIP_NATIVE_BUILD set" if skip
                  else "cargo workspace not found (set CLAUSTERS_WORKSPACE)")
        if already and (allow_skip or skip):
            print(f"clausters: {reason}; using staged artifacts: {', '.join(already)}")
            return already
        raise SystemExit(
            f"clausters: {reason} and nothing staged in clausters/_libs/. "
            "Run this from inside the repo (it finds the workspace), set "
            "CLAUSTERS_WORKSPACE, or pre-stage the artifacts."
        )

    # Both vendored libraries are decided before anything is compiled, so a
    # missing one costs a message rather than a linker failure ten minutes in.
    with_faust = _links_faust()
    with_verovio = _links_verovio()

    dropped = _dropped_families(with_faust)
    if {"synth", "faust"} <= dropped:
        print("clausters: both def families skipped -- the server keeps its "
              "engine core (groups, buses, buffers) but every /synth_new fails")

    features = os.environ.get("CLAUSTERS_CARGO_FEATURES")
    for stem, (crate, default_feat) in _CRATES.items():
        no_default = False
        if stem == "clausters_ffi":
            feat = "verovio" if with_verovio else ""
        elif stem == "clausters_midi":
            feat = default_feat     # always `live`; the server's knobs are not its
        elif features:
            feat = features  # an explicit list is yours to get right
        else:
            feat, no_default = _server_features(default_feat, dropped)
        _cargo_build(workspace, crate, feat, profile, no_default)
    copied = stage(workspace, profile)
    if not copied:
        raise SystemExit("clausters: cargo produced no cdylibs to stage")
    # The standalone server binary (default features), bundled so the wheel's
    # `clausters` command can run a separate (networked / shared-memory) server.
    _cargo_build_bin(workspace, profile, *_server_features("", dropped))
    binname = stage_binary(workspace, profile)
    if binname:
        copied.append(binname)
    # The visual server (clausters-gui), from its own workspace, bundled so the
    # one package is self-contained. Skippable for a light, server-only wheel.
    if not os.environ.get("CLAUSTERS_SKIP_GUI_BUILD"):
        _cargo_build_gui(workspace, profile)
        guiname = stage_gui_binary(workspace, profile)
        if guiname:
            copied.append(guiname)
    elif staged_gui_bin():
        copied.append(gui_bin_name())
    # libfaust + its libLLVM, read off the staged artifacts: the `faust` feature
    # is on by default, and bundling them is what lets an installed wheel
    # JIT-compile a FaustDef with nothing else on the machine.
    copied += stage_faust_libs(profile)
    # verovio, the engraver behind the `score` widget: bundled for the same
    # reason as libLLVM above, and because the published one cannot edit.
    copied += stage_verovio()
    print("clausters: staged " + ", ".join(copied)
          + f" into {LIBS_DIR} and {BIN_DIR}")
    return copied


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    profile = os.environ.get("CLAUSTERS_CARGO_PROFILE", "release")
    if "--debug" in argv:
        profile = "debug"
    if "--release" in argv:
        profile = "release"
    build_and_stage(profile)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
