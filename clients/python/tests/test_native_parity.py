"""The ctypes binding declares every function the C ABI exports, with the arity
the Rust signature has.

This is the check the compiler cannot make. `clausters-ffi` is verified against
`clausters-core` by cargo -- a function whose signature moves breaks the build
-- but `clausters/_native.py` re-states all of it by hand, in a language that
finds out at run time and only for the calls a test happens to make. So a
function added to the core and exposed through the C ABI reaches Python only if
somebody remembers to write a third declaration, and nothing notices when they
do not.

What it compares:

- **Every export is bound.** The exports come from the crate's own source (the
  `extern "C"` items), the bindings from the loaded library itself: `ctypes`
  caches each symbol on the `CDLL` instance the first time it is reached, so
  after `_configure` has run, the instance dictionary *is* the record of what
  the binding touched. Nothing in `_native.py` has to be kept in step for this
  to work.
- **Arity agrees.** Where the binding sets `argtypes`, its length must equal the
  parameter count of the Rust function. A wrong count is the failure ctypes will
  not report: it converts what it was told about and reads the rest as garbage.

What it deliberately does not compare: parameter *types*. Mapping `*const f32`
to `POINTER(c_float)` textually would re-encode the same knowledge a third time,
which is the problem, not the fix. Arity plus the ABI-version handshake in
`_configure` catches the drift that actually happens -- a function added, a
parameter appended -- and the value tests (`test_native.py` and the parity
vectors) cover whether the numbers come back right.
"""

import ctypes
import pathlib
import re

import pytest

from clausters import _native

# The repository's crate sources. An installed wheel has no crates/ next to it,
# and this test is about the repository's internal consistency, so it skips
# rather than fails there.
FFI_SRC = pathlib.Path(__file__).resolve().parents[3] / "crates" / "clausters-ffi" / "src"

SIGNATURE = re.compile(r'pub (?:unsafe )?extern "C" fn (\w+)\s*\(')
CFG_FEATURE = re.compile(r'#\[cfg\(feature = "(\w+)"\)\]')
# The attribute/doc block above an item. Walking it (rather than pinning one
# attribute order) is what keeps an added #[allow(...)] from hiding a function
# from this test -- which is how the first version of it missed one.
ATTRIBUTE = re.compile(r'\s*(#\[|///|//)')


def _param_count(text: str, open_paren: int) -> int:
    """Parameters between `open_paren` and its match, counting top-level commas.

    Balanced rather than regex-delimited so a parameter whose own type carries
    parentheses cannot end the list early.
    """
    depth, cuts, start = 0, [], open_paren + 1
    for i in range(open_paren, len(text)):
        c = text[i]
        if c in "([":
            depth += 1
        elif c in ")]":
            depth -= 1
            if depth == 0:
                cuts.append(text[start:i])
                # rustfmt puts a trailing comma on every multi-line list, so
                # the last piece is empty; an empty list gives one empty piece.
                return len([p for p in cuts if p.strip()])
        elif c == "," and depth == 1:
            cuts.append(text[start:i])
            start = i + 1
    raise AssertionError("unbalanced parameter list")


def exports() -> dict[str, tuple[int, str | None]]:
    """`{symbol: (parameter count, gating feature or None)}` from the crate."""
    found: dict[str, tuple[int, str | None]] = {}
    for path in sorted(FFI_SRC.glob("*.rs")):
        text = path.read_text()
        lines = text.splitlines()
        # notation.rs is behind the module-level `notation` feature; the
        # `verovio` items inside it carry their own cfg.
        module_feature = "notation" if path.stem == "notation" else None
        for m in SIGNATURE.finditer(text):
            line_no = text.count("\n", 0, m.start())
            feature = module_feature
            for above in reversed(lines[:line_no]):
                if not ATTRIBUTE.match(above):
                    break
                if cfg := CFG_FEATURE.search(above):
                    feature = cfg.group(1)
                    break
            found[m.group(1)] = (_param_count(text, m.end() - 1), feature)
    return found


def bound(lib: ctypes.CDLL) -> dict[str, object]:
    """The symbols the binding reached, read off the `CDLL` instance."""
    return {
        name: fn
        for name, fn in vars(lib).items()
        if not isinstance(fn, type) and hasattr(fn, "argtypes")
    }


@pytest.fixture(scope="module")
def surfaces():
    if not FFI_SRC.is_dir():
        pytest.skip("no crates/clausters-ffi source next to the package")
    try:
        lib = _native.lib()
    except OSError as e:
        pytest.skip(f"libclausters_ffi not loadable: {e}")
    return exports(), bound(lib)


def test_the_parser_sees_every_symbol_the_binding_reached(surfaces):
    """A guard on the parser itself, and the one that earns its keep.

    Everything below compares against a set this file extracts from Rust
    source, so a parser that quietly stops matching turns those checks into
    vacuous passes -- the failure mode of every test that reads its own
    reference. The first version of this file pinned one attribute order and
    lost a function to an `#[allow(...)]` between the attributes; the count
    check below said nothing, and this one is what found it.

    It works because the binding's own symbols are a lower bound: every name
    `_native.py` reached exists in the crate (the library would fail to load
    otherwise), so any of them the parser cannot see is a parser bug.
    """
    found, declared = surfaces
    assert len(found) > 50, f"parsed only {len(found)} exports from {FFI_SRC}"
    unseen = sorted(set(declared) - set(found))
    assert not unseen, (
        "the binding reached symbols this file failed to parse out of the "
        f"crate, so the parser is missing declarations: {', '.join(unseen)}"
    )


def test_every_export_is_bound(surfaces):
    found, declared = surfaces
    missing = []
    for name, (_, feature) in sorted(found.items()):
        if name in declared:
            continue
        # A feature the loaded library was not built with: absent by
        # construction, and `_native` already reports that through
        # has_notation()/has_engraver().
        if feature == "notation" and not _native.has_notation():
            continue
        if feature == "verovio" and not _native.has_engraver():
            continue
        missing.append(name)
    assert not missing, (
        "exported by clausters-ffi, never declared in clausters/_native.py: "
        + ", ".join(missing)
    )


def test_declared_arity_matches_the_signature(surfaces):
    found, declared = surfaces
    wrong = []
    for name, fn in sorted(declared.items()):
        if name not in found:
            continue
        arity = found[name][0]
        if fn.argtypes is None:
            # Leaving argtypes unset on a function that takes arguments is the
            # quiet one: ctypes then guesses from the Python values, so a
            # float lands in an int slot and a 64-bit handle is truncated to
            # 32 bits, with no error anywhere. Zero-argument functions have
            # nothing to convert and are fine as they are.
            if arity:
                wrong.append(f"{name}: no argtypes, crate takes {arity}")
            continue
        if len(fn.argtypes) != arity:
            wrong.append(f"{name}: binding declares {len(fn.argtypes)}, crate takes {arity}")
    assert not wrong, "argtypes disagree with the Rust signature:\n" + "\n".join(wrong)
