"""The verbs a window answers, and the client API each one is, checked against
a manifest.

A verb is written once, in ``crates/clausters-editing/src/verbs.rs``; a menu,
a key and a tool are callers of it, and so is a client's handler -- which is
the one caller nothing ties to the table. ``docs/verbs.md`` says, verb by
verb, which members of the two clients do what the verb does, or that none
does yet. This reads the three surfaces and fails when one of them differs
from it:

* the verb table is read **statically**, scope by scope, and the manifest's
  rows are exactly its rows;
* Python is read by **importing it**: every member a row names is on its
  class;
* TypeScript is read **statically**: every member a row names is in its
  class's body, spelled as the Python one in camelCase.

A row is ``api`` with both columns filled, or ``gap``/``n/a`` with both empty
-- never one client with a verb the other lacks.
"""

import importlib
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
TABLE = ROOT / "crates/clausters-editing/src/verbs.rs"
MANIFEST = ROOT / "docs/verbs.md"
WEB = ROOT / "clients/web/src"

sys.path.insert(0, str(ROOT / "clients/python"))

#: Each scope of the table, by the constant that holds its rows.
SCOPES = {"WINDOW": "window", "MULTITRACK": "multitrack", "SCORE": "score",
          "NOTE_ENTRY": "note_entry"}

#: Where each class a row may name lives, in each client.
CLASSES = {
    "Editor": ("clausters.gui.editing.editor", "gui/editing/editor.ts"),
    "MultitrackEditor": ("clausters.gui.editing.multitrack", "gui/editing/multitrack.ts"),
    "AudioEditor": ("clausters.gui.editing.audio", "gui/editing/audio.ts"),
    "NotesEditor": ("clausters.gui.editing.events", "gui/editing/events.ts"),
    "ScoreEditor": ("clausters.gui.editing.score", "gui/editing/score.ts"),
    "Multitrack": ("clausters.multitrack", "multitrack.ts"),
    "Tracks": ("clausters.multitrack", "multitrack.ts"),
}


def camel(name: str) -> str:
    """``notes_view`` -> ``notesView``."""
    return re.sub(r"_([a-z])", lambda m: m.group(1).upper(), name)


def table() -> set:
    """``(scope, verb)`` for every row of the verb table."""
    source = TABLE.read_text()
    rows = set()
    for const, scope in SCOPES.items():
        body = re.search(rf"pub const {const}: &\[Verb\] = &\[(.*?)\n\];", source, re.S)
        assert body, f"no {const} in {TABLE.name}"
        rows |= {(scope, verb) for verb in re.findall(r'verb\(\s*"([a-z0-9_]+)"', body[1])}
    return rows


def manifest() -> dict:
    """``(scope, verb)`` -> ``(python members, web members, verdict)``."""
    rows = {}
    for line in MANIFEST.read_text().splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 5 or not cells[0].startswith("`"):
            continue
        scope, verb, python, web, verdict = cells
        members = lambda cell: re.findall(r"`([A-Za-z]+\.[A-Za-z_]+)`", cell)
        verdict = re.match(r"\*\*([a-z/]+)\*\*", verdict)[1]
        rows[(scope.strip("`"), verb.strip("`"))] = (members(python), members(web), verdict)
    return rows


def class_body(path: pathlib.Path, name: str) -> str:
    """The text of TypeScript class ``name`` in ``path``, up to the next
    top-level declaration."""
    source = path.read_text()
    start = re.search(rf"^export (?:abstract )?class {name}\b", source, re.M)
    assert start, f"no class {name} in {path.name}"
    rest = source[start.end():]
    end = re.search(r"^(?:export )?(?:abstract )?(?:class|function|const|interface) ", rest, re.M)
    return rest[: end.start()] if end else rest


def test_the_manifest_is_the_verb_table_row_for_row():
    rows = manifest()
    assert set(rows) == table(), (
        f"in the table and not here: {sorted(table() - set(rows))}; "
        f"here and not in the table: {sorted(set(rows) - table())}")


def test_a_row_is_whole_in_both_clients_or_in_neither():
    for (scope, verb), (python, web, verdict) in manifest().items():
        if verdict == "api":
            assert python and web, f"{scope}.{verb}: api names members in both"
            assert [f"{c}.{camel(m)}" for c, m in (p.split(".") for p in python)] == web, (
                f"{scope}.{verb}: the web members are the Python ones in camelCase")
        else:
            assert verdict in ("gap", "n/a"), f"{scope}.{verb}: {verdict}"
            assert not python and not web, f"{scope}.{verb}: a {verdict} names no member"


def test_every_member_a_row_names_is_there_in_python():
    for (scope, verb), (python, _, _) in manifest().items():
        for member in python:
            cls, name = member.split(".")
            module = importlib.import_module(CLASSES[cls][0])
            assert hasattr(getattr(module, cls), name), f"{scope}.{verb}: no {member}"


def test_every_member_a_row_names_is_there_in_the_web_client():
    for (scope, verb), (_, web, _) in manifest().items():
        for member in web:
            cls, name = member.split(".")
            body = class_body(WEB / CLASSES[cls][1], cls)
            found = re.search(
                rf"^\s+(?:(?:public|override|async|static|readonly)\s+)*(?:get\s+|set\s+)?"
                rf"{name}\s*[(<]", body, re.M)
            assert found, f"{scope}.{verb}: no {member} in {CLASSES[cls][1]}"
