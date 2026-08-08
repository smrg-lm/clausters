"""The widget prop vocabularies of the three surfaces, checked against a manifest.

A GUI prop is written down in three independent places: the **host** reads it
(``clients/gui/src/host/widget``), the **Python** builder offers it
(``clausters.gui.guidef``) and the **web** builder offers it
(``clients/web/src/gui/guidef.ts``). Nothing makes the three agree, and the
compiler cannot: the wire is untyped JSON by design, so a prop added on one
side and forgotten on the other is silent in every build.

This reads all three and compares them against ``docs/gui-props.md``, which is
the same instrument ``docs/bindings.md`` is for the core's two bindings: it does
not forbid divergence — a client is idiomatic in its own language and a prop may
legitimately reach only one — it forbids **undeclared** divergence. A difference
the manifest does not name fails here, and so does a manifest row that names a
difference no longer there.

The three readers are deliberately different, because the three sources are:

* Python is read by **calling it** — `inspect.signature` over the builders, which
  is exact and cannot drift from what a script can type;
* TypeScript is read **statically**, from the option type of each builder (the
  named interfaces it extends plus its inline literal), which is what a reader
  of the API sees;
* the host is read **statically** from the widget schema's two wire passes,
  ``build`` (construction) and ``apply`` (`/gui_set`), resolving the shared
  prop-reading helpers so a bundle like ``Flow`` or ``EditorProps`` contributes
  its own keys to every widget that embeds it.
"""

import inspect
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
GUIDEF_TS = ROOT / "clients/web/src/gui/guidef.ts"
WIDGET_DIR = ROOT / "clients/gui/src/host/widget"
#: The signal element's model — where the six wire names it presets are listed.
SIGNAL_MOD = ROOT / "clients/gui/src/host/signal/mod.rs"
MANIFEST = ROOT / "docs/gui-props.md"

sys.path.insert(0, str(ROOT / "clients/python"))

# Never a widget prop: the client-side identity keys and the child list.
NOT_A_PROP = {"id", "name", "children"}


def snake(name: str) -> str:
    """``textSize`` -> ``text_size``: the web client's camelCase option name as
    the wire key it becomes."""
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def strip_comments(text: str) -> str:
    return re.sub(r"/\*.*?\*/", "", text, flags=re.S)


# ---------------------------------------------------------------- the Python side

def python_props() -> dict:
    """``{widget kind: {prop}}`` from the builders' own signatures."""
    from clausters.gui import guidef

    out = {}
    for name, fn in vars(guidef).items():
        if not inspect.isfunction(fn) or name.startswith("_"):
            continue
        if fn.__module__ != guidef.__name__:
            continue
        # The builder's kind is the string it passes to `node`.
        kind = re.search(r'node\(\s*"([a-z]+)"', inspect.getsource(fn))
        if not kind:
            continue
        params = {
            p.name
            for p in inspect.signature(fn).parameters.values()
            # `*children`/`*clips` are the positional child list and `**props`
            # is the escape hatch every builder carries, not declared props.
            if p.kind not in (p.VAR_KEYWORD, p.VAR_POSITIONAL)
        }
        out[kind.group(1)] = params - NOT_A_PROP
    return out


# ------------------------------------------------------------------- the web side

def _ts_interfaces(src: str) -> dict:
    out = {}
    for m in re.finditer(r"export interface (\w+)(?: extends (\w+))? \{(.*?)\n\}", src, re.S):
        fields = set(re.findall(r"^\s{4}(\w+)\??:", strip_comments(m.group(3)), re.M))
        out[m.group(1)] = (m.group(2), fields)
    return out


def web_props() -> dict:
    """``{widget kind: {prop}}`` from each builder's option type."""
    src = GUIDEF_TS.read_text()
    ifaces = _ts_interfaces(src)

    def fields_of(name):
        acc = set()
        while name in ifaces:
            parent, fields = ifaces[name]
            acc |= fields
            name = parent
        return acc

    out = {}
    for m in re.finditer(r"export function (\w+)\(", src):
        # The parameter list, balanced from the opening paren.
        start = m.end() - 1
        depth = 0
        for end in range(start, len(src)):
            if src[end] == "(":
                depth += 1
            elif src[end] == ")":
                depth -= 1
                if depth == 0:
                    break
        params = strip_comments(src[start:end + 1])
        body = src[end:]
        kind = re.search(r'node\(\s*"([a-z]+)"', body)
        if not kind:
            continue
        # Every `name?: type` field of the inline option literal, minus the
        # child lists (`children`, a `track`'s `clips`), which are the tree.
        props = {
            m2.group(1)
            for line in params.split("\n")
            if "GuiNode" not in line
            for m2 in re.finditer(r"(\w+)\??\s*:", line)
        }
        for iface in re.findall(r"\b(\w+Options)\b", params):
            props |= fields_of(iface)
        props -= set(ifaces) | {"readonly", "options", "rest", "GuiNode", "Record",
                                "string", "number", "boolean"}
        # A leading positional parameter is a prop too (`label(text)`,
        # `meter(bus)`, `menu(options)`) — but not the option bag itself, and
        # not a child list (`track(clips)`, `panel(…, ...children)`), which is
        # the tree, not a prop.
        for line in params.split("\n"):
            pm = re.match(r"\s{4}(\.\.\.)?(\w+)\s*\??[=:]", line)
            if not pm or pm.group(1):
                continue
            if "Options" in line or "GuiNode" in line:
                continue
            props.add(pm.group(2))
        out[kind.group(1)] = {snake(p) for p in props} - NOT_A_PROP
    return out


# ------------------------------------------------------------------ the host side

def _rust_sources() -> str:
    return "\n".join(p.read_text() for p in sorted(WIDGET_DIR.glob("*.rs")))


def _literal_keys(text: str) -> set:
    """Prop names read out of a props map in this slice of Rust.

    Deliberately literal-by-literal rather than "every string in the body": the
    parse helpers also match on **values** (`"row"`, `"mel"`, `"off"`), and
    counting those as props would quietly grow the host's vocabulary, which is
    the one direction that hides a divergence instead of reporting one.
    """
    keys = set()
    keys |= set(re.findall(r'(?:get|get_mut|contains_key|remove)\(\s*"([a-z_][a-z_0-9]*)"', text))
    # The helpers take the key as an argument: `number_f64(&props, "hop", 0.0)`.
    keys |= set(re.findall(r'props,\s*"([a-z_][a-z_0-9]*)"', text))
    # A local reader closure over the same map: `let f = |k| props.get(k)…`,
    # then `f("margin")`.
    for name in re.findall(r"let (\w+) = \|k(?:ey)?: &str\| props", text):
        keys |= set(re.findall(rf'\b{name}\("([a-z_][a-z_0-9]*)"\)', text))
    return keys


def _helper_keys() -> dict:
    """``{helper name: {prop}}`` for every function that reads a props map.

    Includes the inherent methods (`Flow::parse`, `EditorProps::apply`, …) under
    their `Type::method` name, since that is how the wire passes call them.
    """
    src = _rust_sources()
    lines = src.split("\n")
    bodies = {}
    impl_type = None
    for i, line in enumerate(lines):
        im = re.match(r"impl(?:<[^>]*>)? (\w+)", line)
        if im:
            impl_type = im.group(1)
        if line and not line[0].isspace() and not line.startswith("impl"):
            impl_type = None
        fm = re.match(r"\s*(?:pub(?:\([^)]*\))? )?fn (\w+)", line)
        if not fm:
            continue
        indent = len(line) - len(line.lstrip())
        end = i + 1
        closer = " " * indent + "}"
        while end < len(lines) and lines[end] != closer:
            end += 1
        body = "\n".join(lines[i:end])
        # `Self::other(...)` inside an inherent method is a call to this type's
        # own helper, and the resolver below keys those as `Type::method`.
        if impl_type and indent:
            body = body.replace("Self::", f"{impl_type}::")
        name = fm.group(1)
        # An inherent method is keyed only as `Type::method`: several types have
        # a `parse`, and a bare `parse` would both collide between them and be
        # matched by any `s.parse()` in an unrelated body.
        bodies[f"{impl_type}::{name}" if impl_type and indent else name] = body

    direct = {n: _literal_keys(b) for n, b in bodies.items()}
    # `apply` implementations declare their keys as match arms, not as reads —
    # `WidgetKind::apply` itself and the per-family helpers it delegates to
    # (`apply_signal`).
    for name, body in bodies.items():
        if "apply" in name:
            direct[name] |= _match_arm_keys(body)

    # Resolve one helper calling another, to a fixed point.
    for _ in range(4):
        for name, body in bodies.items():
            for callee in re.findall(r"(?<![.\w])((?:\w+::)?\w+)\(", body):
                if callee in direct and callee != name:
                    direct[name] |= direct[callee]
    return direct


def _match_arm_keys(text: str) -> set:
    """The `"key" | "other" =>` arms of a match on a `/gui_set` key."""
    keys = set()
    for m in re.finditer(r'^\s*("(?:[a-z_][a-z_0-9]*)"(?:\s*\|\s*"[a-z_0-9]*")*)\s*=>', text, re.M):
        keys |= set(re.findall(r'"([a-z_0-9]+)"', m.group(1)))
    return keys


def _arms(text: str, pattern: str) -> list:
    """(`match` arm head, arm body) pairs at one indentation level."""
    out = []
    lines = text.split("\n")
    for i, line in enumerate(lines):
        m = re.match(pattern, line)
        if not m:
            continue
        indent = len(line) - len(line.lstrip())
        end = i + 1
        while end < len(lines):
            nxt = lines[end]
            if nxt.strip() and (len(nxt) - len(nxt.lstrip())) <= indent and re.match(pattern, nxt):
                break
            if nxt.strip() and (len(nxt) - len(nxt.lstrip())) < indent:
                break
            end += 1
        out.append((m, "\n".join(lines[i:end])))
    return out


def generic_props() -> set:
    """The props the host reads for **every** widget, whatever its kind.

    `Widget::build` parses them off the node before the kind is even
    considered — the place props the container's layout applies, plus the two
    style props — so they are not part of any widget's own vocabulary and are
    left out of the comparison below (see `docs/gui-props.md`).
    """
    helpers = _helper_keys()
    mod = (WIDGET_DIR / "mod.rs").read_text()
    keys = set()
    for name in ("Widget::build", "Widget::style_apply"):
        body = helpers.get(name, set())
        keys |= body
    # `Place::parse` is reached through `Widget::build`; assert the pair is
    # actually there rather than silently returning an empty set.
    assert "Place::parse(&node.props)" in mod, "the generic place-prop parse moved"
    return keys - NOT_A_PROP


def signal_presets() -> list:
    """The wire names `host::signal::preset` answers to.

    `waveform`, `plot`, `scope`, `spectrum`, `spectrogram` and `phasescope` are
    six **presets of one element**, so the schema parses them in a single
    guarded arm (`name if signal::preset(name).is_some()`) rather than six named
    ones. The names therefore live in the preset table, not in the wire pass —
    which is exactly the place to read them from, since a seventh preset would
    otherwise reach the wire with nothing checking its props.
    """
    table = SIGNAL_MOD.read_text()
    body = table.split("pub fn preset(", 1)[1]
    names = re.findall(r'^\s{8}"([a-z]+)" =>', body, re.M)
    assert len(names) >= 6, f"the preset table stopped listing its names: {names}"
    return names


#: Widgets whose props are read **outside** their own schema arm, and where.
#:
#: A `clip`'s bodies are *children* — a signal element, a piano-roll, a curve —
#: so the props that describe them are read by `build::clip_bodies` on the way
#: in and by `apply::apply_clip_body` on the way back, not by the `"clip" =>`
#: arm, which holds only the clip's own placement and name. The scanner is told
#: rather than made to guess: the alternative is an arm pretending to read props
#: it does not touch.
OUTBOARD = {"clip": ("clip_bodies", "apply_clip_body")}


def host_props() -> dict:
    """``{widget kind: {prop}}`` from the schema's construction and set passes."""
    helpers = _helper_keys()
    build = (WIDGET_DIR / "build.rs").read_text()
    apply = (WIDGET_DIR / "apply.rs").read_text()

    def keys_in(body):
        keys = _literal_keys(body)
        for callee in re.findall(r"\b((?:\w+::)?\w+)\(", body):
            if callee in helpers:
                keys |= helpers[callee]
        return keys

    out = {}
    # Construction: `"panel" | "box" => WidgetKind::Panel { … }`.
    variant_of = {}
    for m, body in _arms(build, r'\s{8}("[a-z]+"(?:\s*\|\s*"[a-z]+")*)\s*=>'):
        kinds = re.findall(r'"([a-z]+)"', m.group(1))
        variant = re.search(r"WidgetKind::(\w+)", body)
        keys = keys_in(body)
        for kind in kinds:
            out.setdefault(kind, set())
            out[kind] |= keys
            if variant:
                variant_of.setdefault(variant.group(1), []).append(kind)

    # ...and the one **guarded** arm: the signal element's six presets share
    # `build_signal`, so they share its props, and the names come from the
    # preset table itself.
    for m, body in _arms(build, r"\s{8}name if signal::preset"):
        keys = keys_in(body)
        for kind in signal_presets():
            out.setdefault(kind, set())
            out[kind] |= keys
            variant_of.setdefault("Signal", []).append(kind)

    # ...and the props a widget's bodies carry, read outside its own arm.
    for kind, helpers_named in OUTBOARD.items():
        out.setdefault(kind, set())
        for helper in helpers_named:
            assert helper in helpers, f"{helper} moved; OUTBOARD is stale"
            out[kind] |= helpers[helper]

    # Mutation: `WidgetKind::Waveform { … } => match key { … }`.
    for m, body in _arms(apply, r"\s{8}WidgetKind::(\w+)"):
        keys = _match_arm_keys(body) | keys_in(body)
        for variant in re.findall(r"WidgetKind::(\w+)", body.split("=>")[0]):
            for kind in variant_of.get(variant, []):
                out.setdefault(kind, set())
                out[kind] |= keys
    return {k: v - NOT_A_PROP for k, v in out.items()}


# -------------------------------------------------------------------- the manifest

def manifest_rows() -> dict:
    """``{(widget, prop): (sides, verdict)}`` from the divergence table.

    A row that does not parse is an error, not a row to skip: a silently ignored
    line is a declaration nobody makes and nothing enforces.
    """
    rows, table = {}, False
    for line in MANIFEST.read_text().split("\n"):
        if line.startswith("## "):
            table = line.strip() == "## The divergences"
            continue
        if not table or not line.startswith("|"):
            continue
        if set(line) <= set("|- ") or line.startswith("| widget "):  # rule, header
            continue
        m = re.match(r"\|\s*`([a-z]+)`\s*\|\s*`([a-z_0-9]+)`\s*\|([^|]*)\|(.*)\|\s*$", line)
        assert m, f"unreadable row in docs/gui-props.md:\n  {line}"
        widget, prop, sides, verdict = (s.strip() for s in m.groups())
        rows[(widget, prop)] = (sides, verdict)
    return rows


def divergences() -> dict:
    """``{(widget, prop): sides}`` for every prop not offered by all three."""
    host, py, web = host_props(), python_props(), web_props()
    generic = generic_props()
    out = {}
    for widget in sorted(set(py) | set(web)):
        p, w = py.get(widget, set()), web.get(widget, set())
        h = host.get(widget, set()) | generic
        for prop in sorted((p | w) - generic):
            where = []
            if prop in h:
                where.append("host")
            if prop in p:
                where.append("python")
            if prop in w:
                where.append("web")
            if len(where) < 3:
                out[(widget, prop)] = " ".join(where)
    return out


# ------------------------------------------------------------------------- the tests

def test_every_widget_is_read_by_all_three_readers():
    """A reader that silently stops matching would make every check below pass."""
    host, py, web = host_props(), python_props(), web_props()
    assert len(py) >= 28, f"the Python reader found only {len(py)} builders"
    assert set(py) == set(web), (
        "a widget one client builds and the other does not:\n"
        f"  python only: {sorted(set(py) - set(web))}\n"
        f"  web only:    {sorted(set(web) - set(py))}"
    )
    missing = sorted(k for k in py if not host.get(k))
    assert not missing, f"the host reader found no props for {missing}"


def test_divergences_are_the_ones_the_manifest_declares():
    found, declared = divergences(), manifest_rows()
    undeclared = {k: v for k, v in found.items() if k not in declared}
    assert not undeclared, (
        "a prop reaches some surfaces and not others, and docs/gui-props.md does "
        "not say why:\n"
        + "\n".join(f"  {w}.{p}: only {sides}" for (w, p), sides in sorted(undeclared.items()))
    )


def test_the_manifest_has_no_stale_rows():
    found, declared = divergences(), manifest_rows()
    stale = sorted(k for k in declared if k not in found)
    assert not stale, (
        "docs/gui-props.md names a divergence that is no longer there "
        f"(the surfaces now agree): {stale}"
    )


def test_the_manifest_records_where_each_divergence_lives():
    """The `sides` column is data, not prose: it must match what was measured."""
    found, declared = divergences(), manifest_rows()
    wrong = {
        k: (declared[k][0], sides)
        for k, sides in found.items()
        if k in declared and declared[k][0] != sides
    }
    assert not wrong, (
        "docs/gui-props.md records the wrong surfaces for:\n"
        + "\n".join(f"  {w}.{p}: says {said!r}, measured {real!r}"
                    for (w, p), (said, real) in sorted(wrong.items()))
    )


def test_every_divergence_carries_a_verdict():
    declared = manifest_rows()
    assert declared, "docs/gui-props.md declares nothing — is the table still there?"
    blank = sorted(k for k, (_, verdict) in declared.items() if not verdict)
    assert not blank, f"a row with no verdict: {blank}"
