#!/usr/bin/env python3
"""Phase-1 spike: verovio MEI -> SVG -> flat GPU display list.

This does NOT touch the GUI or the wgpu painter yet. It only proves that the
geometry verovio emits (glyph placements, staff lines, stems, beams, slurs)
can be walked out of the rendered SVG into a flat, resolution-independent list
of typed primitives in page coordinates, each carrying the MEI xml:id of the
element it belongs to (for later hit-testing / edit-back).

Run:  clients/python/.venv/bin/python spikes/notation/svg_to_displaylist.py

Everything here uses only the `verovio` pip package + the stdlib. No compiled
verovio, no clone under third_party/.
"""

from __future__ import annotations

import json
import re
import sys
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path

import verovio

HERE = Path(__file__).resolve().parent
SVG_NS = "{http://www.w3.org/2000/svg}"
XLINK_HREF = "{http://www.w3.org/1999/xlink}href"


# --------------------------------------------------------------------------
# Transform stack: verovio only ever emits translate(x,y) and scale(sx,sy),
# so a full affine matrix is overkill -- an (offset, scale) pair composes them
# exactly. We keep it that simple on purpose; if a real matrix(...) ever shows
# up we assert so it is not silently mishandled.
# --------------------------------------------------------------------------
@dataclass(frozen=True)
class Xform:
    tx: float = 0.0
    ty: float = 0.0
    sx: float = 1.0
    sy: float = 1.0

    def then(self, other: "Xform") -> "Xform":
        # self is the parent transform, other is the child's local transform.
        return Xform(
            tx=self.tx + self.sx * other.tx,
            ty=self.ty + self.sy * other.ty,
            sx=self.sx * other.sx,
            sy=self.sy * other.sy,
        )

    def apply(self, x: float, y: float) -> tuple[float, float]:
        return (self.tx + self.sx * x, self.ty + self.sy * y)


_XF_RE = re.compile(r"(translate|scale)\(\s*([-\d.eE]+)\s*[, ]\s*([-\d.eE]+)?\s*\)")


def parse_transform(s: str | None) -> Xform:
    if not s:
        return Xform()
    xf = Xform()
    for kind, a, b in _XF_RE.findall(s):
        a = float(a)
        b = float(b) if b else (a if kind == "scale" else 0.0)
        local = Xform(a, b, 1.0, 1.0) if kind == "translate" else Xform(0, 0, a, b)
        xf = xf.then(local)
    if "matrix" in s or "rotate" in s:
        raise AssertionError(f"unsupported transform primitive in {s!r}")
    return xf


# --------------------------------------------------------------------------
# The display list: a flat list of typed primitives in page coordinates.
# This is the artifact a GPU painter would consume (atlas quad per glyph,
# tessellated triangles per line/curve). Each carries the nearest MEI id.
# --------------------------------------------------------------------------
@dataclass
class Prim:
    kind: str  # "glyph" | "line" | "curve" | "rect" | "polygon"
    mei_id: str | None
    element: str | None  # verovio class: note, stem, slur, staff, clef, ...
    data: dict = field(default_factory=dict)


# codepoint out of a glyph-def id like "E050-n1sc384i" -> 0xE050
_CP_RE = re.compile(r"([0-9A-Fa-f]{4,6})")


def href_codepoint(href: str) -> int | None:
    m = _CP_RE.search(href.lstrip("#"))
    return int(m.group(1), 16) if m else None


def walk(node: ET.Element, xf: Xform, mei_id: str | None,
         element: str | None, out: list[Prim]) -> None:
    local = parse_transform(node.get("transform"))
    xf = xf.then(local)

    nid = node.get("id") or mei_id
    ncls = (node.get("class") or "").split()
    nelem = ncls[0] if ncls else element

    tag = node.tag.replace(SVG_NS, "")

    if tag == "use":
        href = node.get(XLINK_HREF) or node.get("href") or ""
        cp = href_codepoint(href)
        # the <use>'s own transform already carries translate+scale
        x, y = xf.apply(0.0, 0.0)
        out.append(Prim("glyph", nid, nelem,
                        {"codepoint": cp, "smufl": f"U+{cp:04X}" if cp else None,
                         "x": round(x, 1), "y": round(y, 1),
                         "scale": round(xf.sx, 4)}))
        return
    if tag == "path" and node.get("d"):
        d = node.get("d")
        # staff lines / stems / ledger lines are simple M..L polylines;
        # slurs / ties are cubic curves (C). Classify without a full parser.
        pts = _line_points(d)
        if pts is not None:
            world = [xf.apply(px, py) for px, py in pts]
            out.append(Prim("line", nid, nelem,
                            {"points": [[round(a, 1), round(b, 1)] for a, b in world]}))
        else:
            out.append(Prim("curve", nid, nelem, {"d": d, "xform": [xf.tx, xf.ty, xf.sx, xf.sy]}))
        return
    if tag in ("rect", "polygon"):
        out.append(Prim(tag if tag == "rect" else "polygon", nid, nelem,
                        {"raw": {k: v for k, v in node.attrib.items() if k != "transform"},
                         "xform": [xf.tx, xf.ty, xf.sx, xf.sy]}))
        return

    for child in node:
        walk(child, xf, nid, nelem, out)


_NUM = r"[-\d.eE]+"
_ML_RE = re.compile(rf"^M\s*({_NUM})\s+({_NUM})\s+L\s*({_NUM})\s+({_NUM})\s*$")


def _line_points(d: str) -> list[tuple[float, float]] | None:
    m = _ML_RE.match(d.strip())
    if not m:
        return None
    x1, y1, x2, y2 = map(float, m.groups())
    return [(x1, y1), (x2, y2)]


def build_display_list(svg: str) -> list[Prim]:
    root = ET.fromstring(svg)
    # the drawing lives inside the inner <svg class="definition-scale">; the
    # outer <svg> viewBox maps definition units to the page.
    inner = root.find(f".//{SVG_NS}svg[@class='definition-scale']")
    target = inner if inner is not None else root
    out: list[Prim] = []
    walk(target, Xform(), None, None, out)
    return out


def main() -> int:
    mei = (HERE / "sample.mei").read_text()
    tk = verovio.toolkit()
    tk.setOptions({"scale": 40, "adjustPageHeight": True, "svgViewBox": True})
    tk.loadData(mei)

    svg = tk.renderToSVG(1)
    (HERE / "sample.svg").write_text(svg)
    # the Python binding may return the timemap already parsed (a list) or as
    # a JSON string, depending on the build -- accept both.
    tm = tk.renderToTimemap({"includeMeasures": True})
    timemap = json.loads(tm) if isinstance(tm, (str, bytes, bytearray)) else tm

    prims = build_display_list(svg)

    # --- report ---
    from collections import Counter
    by_kind = Counter(p.kind for p in prims)
    by_elem = Counter(p.element for p in prims)
    glyphs = [p for p in prims if p.kind == "glyph"]

    print(f"MEI: {len(mei)} bytes -> SVG: {len(svg)} bytes")
    print(f"display list: {len(prims)} primitives")
    print(f"  by kind:    {dict(by_kind)}")
    print(f"  by element: {dict(by_elem.most_common(10))}")
    print(f"  glyphs (SMuFL): {len(glyphs)} distinct codepoints "
          f"{sorted({p.data['smufl'] for p in glyphs})}")
    print(f"timemap entries: {len(timemap)} "
          f"(first onsets ms: {[e.get('tstamp') for e in timemap[:6]]})")

    # note-onset <-> display-list bridge: every timemap 'on' id should appear
    # as a glyph mei_id. This is the hook to the arrangement layer's timeline.
    onset_ids = {i for e in timemap for i in e.get("on", [])}
    dl_ids = {p.mei_id for p in prims}
    matched = onset_ids & dl_ids
    print(f"note-onset ids present in display list: {len(matched)}/{len(onset_ids)}")

    out = [{"kind": p.kind, "mei_id": p.mei_id, "element": p.element, **p.data}
           for p in prims]
    (HERE / "display_list.json").write_text(json.dumps(out, indent=1))
    print(f"wrote {HERE / 'display_list.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
