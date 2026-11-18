#!/usr/bin/env python3
"""Rebuilds ``src/font/lucide.rs``: the subset of Lucide the GUI host draws
its icons with, each an SVG path the host strokes as Lucide does (a width of
2 in its box of 24, round caps and joins).

Every element of an icon (a path, a circle, a rect, a line, a polyline, an
ellipse) is written as one path of absolute moves, lines and cubic curves --
an arc becomes the cubics that draw it -- so the host's path scanner reads
it with nothing more than it reads an engraver's outline with.

    crates/clausters-gui-draw/assets/icons/lucide.py \\
        clausters-editor/node_modules/@lucide/svelte

The argument is an unpacked ``@lucide/svelte`` package (1.47.0 at the time
of writing; ISC, see LICENSE beside this file). Run from the repository
root.
"""

import json
import math
import re
import sys
from pathlib import Path

# The icons the host carries, by Lucide's name: the editor's tools and its
# components', the host's chrome, and the transport an application's
# toolbar shows.
SUBSET = [
    "arrow-down-to-line", "arrow-left", "arrow-right", "check",
    "chevron-down", "chevron-left", "chevron-right", "chevron-up", "circle",
    "copy", "download", "ellipsis-vertical", "eraser", "file-text", "folder",
    "folder-open", "grip-vertical", "house", "list-video", "menu", "minus",
    "pause", "play", "plus", "refresh-cw", "repeat", "rotate-ccw", "search",
    "skip-back", "skip-forward", "square", "trash", "x",
]

TOKEN = re.compile(r"[MmLlHhVvCcSsQqTtAaZz]|-?(?:\d+\.?\d*|\.\d+)(?:e-?\d+)?")


def fmt(v):
    s = f"{v:.3f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def arc_to_cubics(x1, y1, rx, ry, phi, large, sweep, x2, y2):
    """The SVG arc from (x1, y1) to (x2, y2) as cubic segments, each
    ``(c1, c2, end)`` (the standard endpoint-to-centre conversion)."""
    if rx == 0 or ry == 0 or (x1 == x2 and y1 == y2):
        return [((x1, y1), (x2, y2), (x2, y2))]
    rx, ry = abs(rx), abs(ry)
    phi = math.radians(phi)
    cp, sp = math.cos(phi), math.sin(phi)
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = cp * dx + sp * dy, -sp * dx + cp * dy
    lam = x1p ** 2 / rx ** 2 + y1p ** 2 / ry ** 2
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx ** 2 * ry ** 2 - rx ** 2 * y1p ** 2 - ry ** 2 * x1p ** 2
    den = rx ** 2 * y1p ** 2 + ry ** 2 * x1p ** 2
    coef = math.sqrt(max(0.0, num / den)) if den else 0.0
    if large == sweep:
        coef = -coef
    cxp, cyp = coef * rx * y1p / ry, -coef * ry * x1p / rx
    cx = cp * cxp - sp * cyp + (x1 + x2) / 2
    cy = sp * cxp + cp * cyp + (y1 + y2) / 2

    def angle(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    t1 = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dt = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not sweep and dt > 0:
        dt -= 2 * math.pi
    elif sweep and dt < 0:
        dt += 2 * math.pi
    n = max(1, math.ceil(abs(dt) / (math.pi / 2) - 1e-9))
    step = dt / n
    k = 4 / 3 * math.tan(step / 4)
    out = []

    def point(t):
        x, y = rx * math.cos(t), ry * math.sin(t)
        return (cp * x - sp * y + cx, sp * x + cp * y + cy)

    def deriv(t):
        x, y = -rx * math.sin(t), ry * math.cos(t)
        return (cp * x - sp * y, sp * x + cp * y)

    t = t1
    for _ in range(n):
        a, b = point(t), point(t + step)
        da, db = deriv(t), deriv(t + step)
        out.append(((a[0] + k * da[0], a[1] + k * da[1]),
                    (b[0] - k * db[0], b[1] - k * db[1]), b))
        t += step
    out[-1] = (out[-1][0], out[-1][1], (x2, y2))
    return out


def absolute(d):
    """The path ``d`` as absolute M, L, C and Z commands."""
    toks = TOKEN.findall(d)
    out, i = [], 0
    cx = cy = sx = sy = 0.0
    last_ctrl = None
    cmd = None

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    def flag():
        # a flag may be written run together with what follows it ("01")
        nonlocal i
        t = toks[i]
        if len(t) > 1 and t[0] in "01" and not t.startswith(("0.", "-")):
            toks[i] = t[1:]
            return int(t[0])
        i += 1
        return int(float(t))

    while i < len(toks):
        if re.fullmatch(r"[A-Za-z]", toks[i]):
            cmd = toks[i]
            i += 1
            if cmd in "Zz":
                out.append("Z")
                cx, cy = sx, sy
                last_ctrl = None
                continue
        rel = cmd.islower()
        c = cmd.upper()
        if c == "M":
            x, y = num(), num()
            if rel:
                x, y = x + cx, y + cy
            out.append(f"M{fmt(x)} {fmt(y)}")
            cx, cy, sx, sy = x, y, x, y
            cmd = "l" if rel else "L"
            last_ctrl = None
        elif c in "LHV":
            if c == "L":
                x, y = num(), num()
                if rel:
                    x, y = x + cx, y + cy
            elif c == "H":
                x, y = num() + (cx if rel else 0), cy
            else:
                x, y = cx, num() + (cy if rel else 0)
            out.append(f"L{fmt(x)} {fmt(y)}")
            cx, cy = x, y
            last_ctrl = None
        elif c in "CS":
            if c == "C":
                x1, y1 = num(), num()
                if rel:
                    x1, y1 = x1 + cx, y1 + cy
            else:
                x1, y1 = (2 * cx - last_ctrl[0], 2 * cy - last_ctrl[1]) if last_ctrl else (cx, cy)
            x2, y2, x, y = num(), num(), num(), num()
            if rel:
                x2, y2, x, y = x2 + cx, y2 + cy, x + cx, y + cy
            out.append(f"C{fmt(x1)} {fmt(y1)} {fmt(x2)} {fmt(y2)} {fmt(x)} {fmt(y)}")
            last_ctrl = (x2, y2)
            cx, cy = x, y
        elif c in "QT":
            if c == "Q":
                qx, qy = num(), num()
                if rel:
                    qx, qy = qx + cx, qy + cy
            else:
                qx, qy = (2 * cx - last_ctrl[0], 2 * cy - last_ctrl[1]) if last_ctrl else (cx, cy)
            x, y = num(), num()
            if rel:
                x, y = x + cx, y + cy
            c1 = (cx + 2 / 3 * (qx - cx), cy + 2 / 3 * (qy - cy))
            c2 = (x + 2 / 3 * (qx - x), y + 2 / 3 * (qy - y))
            out.append(f"C{fmt(c1[0])} {fmt(c1[1])} {fmt(c2[0])} {fmt(c2[1])} {fmt(x)} {fmt(y)}")
            last_ctrl = (qx, qy)
            cx, cy = x, y
        elif c == "A":
            rx, ry, phi = num(), num(), num()
            large, sweep = flag(), flag()
            x, y = num(), num()
            if rel:
                x, y = x + cx, y + cy
            for c1, c2, e in arc_to_cubics(cx, cy, rx, ry, phi, large, sweep, x, y):
                out.append(f"C{fmt(c1[0])} {fmt(c1[1])} {fmt(c2[0])} {fmt(c2[1])} {fmt(e[0])} {fmt(e[1])}")
            cx, cy = x, y
            last_ctrl = None
        else:
            raise ValueError(f"unknown command {cmd} in {d}")
    return "".join(out)


def element(tag, a):
    f = lambda k, default=0.0: float(a.get(k, default))
    if tag == "path":
        return absolute(a["d"])
    if tag in ("circle", "ellipse"):
        cx, cy = f("cx"), f("cy")
        rx = f("r") if tag == "circle" else f("rx")
        ry = f("r") if tag == "circle" else f("ry")
        return absolute(f"M{cx - rx} {cy}A{rx} {ry} 0 1 0 {cx + rx} {cy}"
                        f"A{rx} {ry} 0 1 0 {cx - rx} {cy}Z")
    if tag == "rect":
        x, y, w, h = f("x"), f("y"), f("width"), f("height")
        rx = f("rx", a.get("ry", 0))
        ry = f("ry", a.get("rx", 0))
        if rx == 0:
            return absolute(f"M{x} {y}H{x + w}V{y + h}H{x}Z")
        return absolute(
            f"M{x + rx} {y}H{x + w - rx}A{rx} {ry} 0 0 1 {x + w} {y + ry}"
            f"V{y + h - ry}A{rx} {ry} 0 0 1 {x + w - rx} {y + h}H{x + rx}"
            f"A{rx} {ry} 0 0 1 {x} {y + h - ry}V{y + ry}A{rx} {ry} 0 0 1 {x + rx} {y}Z")
    if tag == "line":
        return absolute(f"M{f('x1')} {f('y1')}L{f('x2')} {f('y2')}")
    if tag in ("polyline", "polygon"):
        pts = a["points"].replace(",", " ").split()
        d = "M" + " ".join(pts[:2]) + "".join(f"L{pts[i]} {pts[i + 1]}" for i in range(2, len(pts), 2))
        return absolute(d + ("Z" if tag == "polygon" else ""))
    raise ValueError(f"unknown element {tag}")


def main():
    package = Path(sys.argv[1])
    version = json.loads((package / "package.json").read_text())["version"]
    rows = []
    for name in SUBSET:
        text = (package / "dist" / "icons" / f"{name}.svelte").read_text()
        data = json.loads(re.search(r"const iconData = (\{.*?\});", text, re.S).group(1))
        d = "".join(element(tag, attrs) for tag, attrs in data["node"])
        rows.append(f'    ("{name}", "{d}"),')
    out = Path(__file__).resolve().parents[2] / "src" / "font" / "lucide.rs"
    out.write_text(
        "//! **The Lucide icons the host carries**, generated by\n"
        "//! `assets/icons/lucide.py` from Lucide "
        f"{version} (ISC; `assets/icons/LICENSE`):\n"
        "//! each its name and its elements as one path of absolute moves, lines\n"
        "//! and cubic curves in Lucide's box of 24, `y` downward, to be stroked 2\n"
        "//! wide with round caps and joins. Not edited by hand.\n\n"
        "/// `(name, path)`, by name.\n"
        "pub const ICONS: &[(&str, &str)] = &[\n" + "\n".join(rows) + "\n];\n")
    print(f"{len(rows)} icons from Lucide {version} -> {out}")


if __name__ == "__main__":
    main()
