"""Engrave a score into the host's ``score`` display list, driving verovio.

This is the client-side rendering step: verovio lays out a digital score (MEI,
MusicXML, ABC, Humdrum, PAE, ...) into SVG, and this module walks that SVG into
the flat, resolution-independent display list the GUI host's ``score`` widget
consumes — a SMuFL glyph-outline table plus placed primitives (glyphs, staff
lines, stems, beams, slurs) in verovio page units, each carrying the MEI
``xml:id`` it was engraved from. The host tessellates it; **verovio lives here,
never in the host**, so any language client can reuse the same host renderer by
sending the same display list.

verovio is an *optional* dependency — install it with ``pip install verovio``.
`engrave` raises a clear error if it is missing. The heavy lifting is verovio's;
this module is only the SVG-to-display-list adapter.
"""

from __future__ import annotations

import json
import re
import xml.etree.ElementTree as ET

_SVG = "{http://www.w3.org/2000/svg}"
_XLINK_HREF = "{http://www.w3.org/1999/xlink}href"
_CODEPOINT = re.compile(r"([0-9A-Fa-f]{4,6})")
_NUM = r"[-\d.eE]+"
_TRANSFORM = re.compile(rf"(translate|scale)\(\s*({_NUM})\s*[, ]?\s*({_NUM})?\s*\)")
# a staff line / stem / ledger line: a single "M x y L x y" segment
_LINE = re.compile(rf"^M\s*({_NUM})\s+({_NUM})\s+L\s*({_NUM})\s+({_NUM})\s*$")


def engrave(data: str, *, page: int = 1, scale: int = 40,
            page_width: int = 2100, options: dict | None = None) -> dict:
    """Engrave ``data`` (a score in any format verovio auto-detects) into a
    ``score`` display list: ``{"vb": [w, h], "glyphs": {...}, "prims": [...]}``.

    Pass the result to `clausters.gui.guidef.score` as its ``display_list``, or
    to `score_view` to get a scrollable page. The score **wraps into systems**
    at ``page_width`` (verovio page units), and the page grows as tall as the
    music needs (all systems on one page), so a long score reads at ``scale``
    instead of being squeezed onto one line. ``scale`` sets the staff size;
    extra verovio ``options`` are merged over the defaults. Raises
    ``RuntimeError`` if verovio is not installed.
    """
    try:
        import verovio
    except ImportError as exc:  # pragma: no cover - exercised only without verovio
        raise RuntimeError(
            "engraving a score needs the optional 'verovio' package "
            "(pip install verovio)"
        ) from exc

    tk = verovio.toolkit()
    opts = {"scale": scale, "adjustPageHeight": True, "svgViewBox": True,
            "breaks": "auto", "pageWidth": page_width}
    if options:
        opts.update(options)
    tk.setOptions(opts)
    if not tk.loadData(data):
        raise ValueError("verovio could not load the score data")
    svg = tk.renderToSVG(page)
    dl = svg_to_display_list(svg)
    tm = tk.renderToTimemap({"includeMeasures": False})
    timemap = json.loads(tm) if isinstance(tm, (str, bytes, bytearray)) else tm
    dl["cursors"] = _cursor_track(dl, timemap)
    return dl


def _cursor_track(display_list: dict, timemap: list) -> list:
    """Fold the timemap (onset ms -> the MEI ids sounding then) together with
    the placed geometry into a playback-cursor track: for each onset, the page-x
    of its leftmost note and the y-span of that note's system. This is the
    bridge from musical time to score geometry — the same id that carries the
    onset carries the glyph position (see the Phase-1 finding). Sorted by time,
    ready for the host's ``playhead``."""
    id_x, id_y = _id_positions(display_list["prims"])
    systems = _staff_systems(display_list["prims"])
    track = []
    for entry in timemap:
        t = entry.get("tstamp")
        ons = [i for i in entry.get("on", []) if i in id_x]
        if t is None or not ons:
            continue
        lead = min(ons, key=lambda i: id_x[i])  # the leftmost onset note
        y0, y1 = _system_bounds(systems, id_y[lead])
        track.append({"t": round(float(t), 1), "x": round(id_x[lead], 1),
                      "y0": round(y0, 1), "y1": round(y1, 1)})
    track.sort(key=lambda c: c["t"])
    return track


def _id_positions(prims):
    """Map each MEI id to a page position, preferring the glyph (notehead)
    placement — its transform origin ``(tx, ty)``."""
    id_x, id_y = {}, {}
    for p in prims:
        pid = p.get("id")
        if not pid or pid in id_x:
            continue
        if p["k"] == "glyph":
            id_x[pid], id_y[pid] = p["xf"][0], p["xf"][1]
        elif p["k"] == "line" and p["pts"]:
            id_x[pid], id_y[pid] = p["pts"][0][0], p["pts"][0][1]
    return id_x, id_y


def _staff_systems(prims):
    """Cluster the horizontal staff lines into systems, each a ``(y_top,
    y_bottom)`` pair. Staff lines are wide horizontal ``line`` prims; a large
    vertical gap between them starts a new system."""
    ys = sorted({round(p["pts"][0][1], 1) for p in prims
                 if p["k"] == "line" and len(p["pts"]) == 2
                 and abs(p["pts"][0][1] - p["pts"][1][1]) < 1.0
                 and abs(p["pts"][0][0] - p["pts"][1][0]) > 500.0})
    if not ys:
        return []
    systems, group = [], [ys[0]]
    for y in ys[1:]:
        if y - group[-1] > 500.0:  # gap between systems >> gap between lines
            systems.append((group[0], group[-1]))
            group = [y]
        else:
            group.append(y)
    systems.append((group[0], group[-1]))
    return systems


def _system_bounds(systems, y):
    """The ``(y0, y1)`` cursor span for a note at page-y ``y``: its system's
    staff extent, padded for stems above and below."""
    if not systems:
        return (y - 400.0, y + 400.0)
    top, bot = min(systems, key=lambda s: min(abs(y - s[0]), abs(y - s[1])))
    pad = (bot - top) * 0.6 + 100.0
    return (top - pad, bot + pad)


def score_view(display_list: dict, *, scroll_id: int, score_id: int,
               width: float = 1000.0, zoom: bool = True) -> dict:
    """Wrap an engraved ``display_list`` in a vertical `scroll` sized to the
    page, ready to drop into a window. The content area is ``width`` wide and as
    tall as the page's aspect needs, so a multi-system score scrolls vertically
    (the wheel scrolls; ``zoom`` enables cursor-anchored zoom to read a dense
    passage). Returns the `scroll` node; give it and the inner `score` distinct
    ids (``scroll_id``/``score_id``)."""
    from .guidef import score, scroll

    vb = display_list.get("vb") or [1.0, 1.0]
    aspect = (vb[1] / vb[0]) if vb[0] else 1.0
    height = round(width * aspect, 1)
    return scroll(
        scroll_id,
        score(score_id, display_list=display_list, x=0.0, y=0.0, w=width, h=height),
        axis="y", zoom=zoom, content_w=width, content_h=height,
    )


def svg_to_display_list(svg: str) -> dict:
    """Walk a verovio SVG string into a ``score`` display list. Split out of
    `engrave` so it is testable on a captured SVG without verovio installed."""
    root = ET.fromstring(svg)
    glyph_defs = _collect_glyph_defs(root)
    # the drawing lives inside the inner <svg class="definition-scale">.
    inner = _find_definition_scale(root)
    target, vb = (inner, _viewbox(inner)) if inner is not None else (root, _viewbox(root))

    glyphs: dict[str, str] = {}
    prims: list[dict] = []
    _walk(target, _IDENTITY, None, glyph_defs, glyphs, prims)
    return {"vb": vb, "glyphs": glyphs, "prims": prims}


# -- the SVG walk -----------------------------------------------------------
# verovio only emits translate()/scale() transforms; an (offset, scale) pair
# composes them exactly, so we carry that instead of a full matrix.
_IDENTITY = (0.0, 0.0, 1.0, 1.0)  # (tx, ty, sx, sy)


def _compose(parent, child):
    ptx, pty, psx, psy = parent
    ctx, cty, csx, csy = child
    return (ptx + psx * ctx, pty + psy * cty, psx * csx, psy * csy)


def _apply(xf, x, y):
    tx, ty, sx, sy = xf
    return (tx + sx * x, ty + sy * y)


def _parse_transform(s):
    if not s:
        return _IDENTITY
    xf = _IDENTITY
    for kind, a, b in _TRANSFORM.findall(s):
        a = float(a)
        b = float(b) if b else (a if kind == "scale" else 0.0)
        local = (a, b, 1.0, 1.0) if kind == "translate" else (0.0, 0.0, a, b)
        xf = _compose(xf, local)
    return xf


def _collect_glyph_defs(root) -> dict[str, str]:
    """Map each glyph id (e.g. ``E0A4-n1sc384i``) to the raw outline path ``d``
    verovio inlines in ``<defs>``, keyed by its bare SMuFL codepoint. The glyph
    paths carry an inner ``scale(1,-1)``; we fold that flip into the instance
    transform at placement time (`_walk`), so the stored ``d`` is verbatim."""
    out: dict[str, str] = {}
    for g in root.iter(f"{_SVG}g"):
        gid = g.get("id") or ""
        m = _CODEPOINT.match(gid)
        path = g.find(f"{_SVG}path")
        if m and path is not None and path.get("d"):
            out[m.group(1).upper()] = path.get("d")
    return out


def _walk(node, xf, mei_id, glyph_defs, glyphs, prims):
    xf = _compose(xf, _parse_transform(node.get("transform")))
    nid = node.get("id") or mei_id
    cls = (node.get("class") or "").split()
    tag = node.tag.replace(_SVG, "")

    if tag == "use":
        href = node.get(_XLINK_HREF) or node.get("href") or ""
        m = _CODEPOINT.search(href.lstrip("#"))
        if m:
            cp = m.group(1).upper()
            if cp in glyph_defs:
                glyphs.setdefault(cp, glyph_defs[cp])
                # the placed transform, with the glyph's inner scale(1,-1) flip
                # folded into a negative sy so the host maps font units -> page.
                tx, ty, sx, sy = xf
                prims.append({"k": "glyph", "cp": cp,
                              "xf": [round(tx, 2), round(ty, 2),
                                     round(sx, 4), round(-sy, 4)],
                              "id": nid})
        return
    if tag == "path" and node.get("d"):
        d = node.get("d").strip()
        line = _LINE.match(d)
        if line:
            x1, y1, x2, y2 = (float(v) for v in line.groups())
            p1, p2 = _apply(xf, x1, y1), _apply(xf, x2, y2)
            prims.append({"k": "line",
                          "pts": [[round(p1[0], 1), round(p1[1], 1)],
                                  [round(p2[0], 1), round(p2[1], 1)]],
                          "w": _stroke_width(node, xf), "id": nid})
        else:
            # a filled outline (slur, tie): keep `d` verbatim in its own units
            # and let the host apply the transform, so comma/space coordinate
            # separators are never rewritten.
            prims.append({"k": "fill", "d": d, "xf": _xf_list(xf), "id": nid})
        return
    if tag == "polygon" and node.get("points"):
        prims.append({"k": "fill", "d": _points_to_path(node.get("points")),
                      "xf": _xf_list(xf), "id": nid})
        return
    if tag == "polyline" and node.get("points"):
        # a stroked open path (hairpin, some brackets): a thick polyline, not a
        # fill — filling its endpoints would paint a solid wedge.
        pts = _points(node.get("points"))
        if len(pts) >= 2:
            prims.append({"k": "line",
                          "pts": [[round(a, 1), round(b, 1)]
                                  for a, b in (_apply(xf, x, y) for x, y in pts)],
                          "w": _stroke_width(node, xf), "id": nid})
        return
    if tag == "rect":
        prims.append({"k": "fill", "d": _rect_to_path(node), "xf": _xf_list(xf),
                      "id": nid})
        return
    if tag == "ellipse":
        prims.append({"k": "fill", "d": _ellipse_to_path(node), "xf": _xf_list(xf),
                      "id": nid})
        return
    if tag == "text":
        prim = _text_prim(node, xf, nid)
        if prim:
            prims.append(prim)
        return  # its tspans are consumed here, not walked as elements

    for child in node:
        _walk(child, xf, nid, glyph_defs, glyphs, prims)


def _stroke_width(node, xf):
    w = node.get("stroke-width")
    _, _, sx, _ = xf
    return round(float(w) * sx, 1) if w else 1.0


def _xf_list(xf):
    tx, ty, sx, sy = xf
    return [round(tx, 2), round(ty, 2), round(sx, 4), round(sy, 4)]


def _points(points):
    """Parse an SVG ``points`` list into ``[(x, y), ...]`` (local coordinates)."""
    coords = [float(v) for v in re.split(r"[ ,]+", points.strip()) if v]
    return [(coords[i], coords[i + 1]) for i in range(0, len(coords) - 1, 2)]


def _points_to_path(points):
    parts = [f"{'M' if i == 0 else 'L'}{x:.1f} {y:.1f}"
             for i, (x, y) in enumerate(_points(points))]
    return " ".join(parts) + " Z"


def _text_prim(node, xf, nid):
    """A verovio ``<text>`` node: the string lives in nested ``<tspan>``s (with
    the pixel ``font-size`` on the innermost one), the baseline ``x, y`` on the
    outer ``<text>``. Emit one ``text`` primitive with the page-mapped baseline
    and em size — the host draws it in its own font (verbatim text, not SMuFL:
    volta numbers, tempo, lyrics, titles)."""
    s = "".join(node.itertext()).strip()
    if not s:
        return None
    x = float(node.get("x", 0.0))
    y = float(node.get("y", 0.0))
    px, py = _apply(xf, x, y)
    _, _, sx, _ = xf
    size = _text_font_size(node) * sx
    return {"k": "text", "s": s, "x": round(px, 1), "y": round(py, 1),
            "size": round(size, 1), "id": nid}


def _text_font_size(node):
    """The deepest ``font-size`` in ``px`` under ``node`` (verovio puts a real
    size on the innermost tspan and ``0px`` on the wrapper), defaulting to a
    readable size when none is stated."""
    best = 0.0
    for el in node.iter():
        fs = el.get("font-size", "")
        m = re.match(rf"({_NUM})px", fs)
        if m:
            best = max(best, float(m.group(1)))
    return best or 400.0


def _ellipse_to_path(node):
    """An ``<ellipse>`` (augmentation dots, etc.) as a closed path of four cubic
    beziers — the standard circle/ellipse approximation — in local coordinates,
    so it fills through the same tessellator as every other region (the host
    applies the transform)."""
    cx = float(node.get("cx", 0.0)); cy = float(node.get("cy", 0.0))
    rx = float(node.get("rx", 0.0)); ry = float(node.get("ry", 0.0))
    k = 0.5522847498  # 4/3 * (sqrt(2)-1): control-point offset for a quarter arc
    def pt(x, y):
        return f"{x:.1f} {y:.1f}"
    return (
        f"M{pt(cx + rx, cy)} "
        f"C{pt(cx + rx, cy + ry * k)} {pt(cx + rx * k, cy + ry)} {pt(cx, cy + ry)} "
        f"C{pt(cx - rx * k, cy + ry)} {pt(cx - rx, cy + ry * k)} {pt(cx - rx, cy)} "
        f"C{pt(cx - rx, cy - ry * k)} {pt(cx - rx * k, cy - ry)} {pt(cx, cy - ry)} "
        f"C{pt(cx + rx * k, cy - ry)} {pt(cx + rx, cy - ry * k)} {pt(cx + rx, cy)} Z"
    )


def _rect_to_path(node):
    x = float(node.get("x", 0)); y = float(node.get("y", 0))
    w = float(node.get("width", 0)); h = float(node.get("height", 0))
    pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
    parts = [f"{'M' if i == 0 else 'L'}{px:.1f} {py:.1f}"
             for i, (px, py) in enumerate(pts)]
    return " ".join(parts) + " Z"


def _find_definition_scale(root):
    for svg in root.iter(f"{_SVG}svg"):
        if svg.get("class") == "definition-scale":
            return svg
    return None


def _viewbox(node):
    vb = (node.get("viewBox") or "").split()
    if len(vb) == 4:
        return [float(vb[2]), float(vb[3])]
    return [0.0, 0.0]
