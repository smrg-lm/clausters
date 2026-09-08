"""Write the multitrack vector this package's parity test reads.

The piece itself is **not** defined here. It is built by the generator the Rust
suite already uses (`crates/clausters-document/tests/gen-multitrack-vector.py`)
and only written out again in this directory, because two definitions of "the
piece the parity tests use" is exactly the divergence these tests exist to
catch: the two would drift, each side would pass, and the thing neither covered
would be whatever the other one had.

So there is one piece, three readers -- the Python client that built it, the
Rust crate that parses it, and the web client that reads and rewrites it.

Run from the repo root, and commit whatever moves:

    python3 clients/web/tests/gen-multitrack-vectors.py
"""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "clients/python"))
sys.path.insert(0, str(ROOT / "crates/clausters-document/tests"))

import importlib.util  # noqa: E402

spec = importlib.util.spec_from_file_location(
    "gen_multitrack_vector",
    ROOT / "crates/clausters-document/tests/gen-multitrack-vector.py")
assert spec is not None and spec.loader is not None
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)


if __name__ == "__main__":
    here = pathlib.Path(__file__).resolve().parent
    (here / "multitrack-vectors.json").write_text(
        json.dumps(shared.build().write(), indent=1) + "\n")
    (here / "multitrack-session-vectors.json").write_text(
        json.dumps(shared.saved().write(), indent=1) + "\n")
    print(f"wrote {here / 'multitrack-vectors.json'} and its session")
