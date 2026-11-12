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

- **Types agree.** Each declared parameter type, and the declared return
  type, is compared with the C type of the header the crate generates
  (`crates/clausters-ffi/include/clausters_ffi.h`, kept equal to the crate by
  `crates/clausters-checks/tests/c_header.rs`), through a fixed table of
  equivalents: `uint32_t` is `c_uint32`, `const float *` is
  `POINTER(c_float)`, any pointer may cross as `c_void_p`. The header is not a
  third statement of the signatures, it is the first one read back, which is
  why this compares types and the first version of this file did not
  (`docs/decisions.md`, "The ctypes declarations are checked against a header
  the crate generates"). A width that disagrees is the failure ctypes never
  reports: it truncates or pads, and the number is wrong far from the call.
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
HEADER = FFI_SRC.parent / "include" / "clausters_ffi.h"

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


# The C types of the header, as the ctypes types that carry them. Fixed and
# small on purpose: a type the header gains that is not here fails the test
# below, and is added here once, rather than a declaration guessing.
SCALARS = {
    "double": ctypes.c_double,
    "float": ctypes.c_float,
    "bool": ctypes.c_bool,
    "int8_t": ctypes.c_int8,
    "uint8_t": ctypes.c_uint8,
    "int16_t": ctypes.c_int16,
    "uint16_t": ctypes.c_uint16,
    "int32_t": ctypes.c_int32,
    "uint32_t": ctypes.c_uint32,
    "int64_t": ctypes.c_int64,
    "uint64_t": ctypes.c_uint64,
    "size_t": ctypes.c_size_t,
    "ptrdiff_t": ctypes.c_ssize_t,
}

C_FUNCTION = re.compile(
    r"^(?P<ret>[A-Za-z_][\w \*]*?)\s*\b(?P<name>clausters_\w+)\((?P<params>[^;]*?)\);",
    re.MULTILINE | re.DOTALL,
)


def header_signatures() -> dict[str, tuple[str, list[str]]]:
    """`{symbol: (return type, [parameter types])}` from the generated header."""
    text = HEADER.read_text()
    found = {}
    for m in C_FUNCTION.finditer(text):
        ret = " ".join(m.group("ret").split())
        params = []
        for param in m.group("params").split(","):
            param = " ".join(param.split())
            if param in ("", "void"):
                continue
            # the parameter's name is the last word; its type is the rest
            ctype = re.match(r"^(.*?)\s*\b\w+$", param).group(1)
            params.append(ctype)
        found[m.group("name")] = (ret, params)
    return found


def carries(ctype: str, declared) -> bool:
    """Whether a ctypes declaration carries the C type `ctype`."""
    ctype = ctype.replace(" *", "*").strip()
    if ctype.endswith("*"):
        if declared is ctypes.c_void_p:
            return True  # an address, whatever it points at
        inner = ctype[:-1].removeprefix("const ").strip()
        if inner == "char":
            return declared is ctypes.c_char_p
        if inner == "uint8_t" and declared is ctypes.c_char_p:
            return True  # bytes handed over as they are
        target = getattr(declared, "_type_", None)
        if not (isinstance(declared, type) and issubclass(declared, ctypes._Pointer)):
            return False
        if inner in SCALARS:
            return target is SCALARS[inner]
        return inner.startswith("struct ") and issubclass(target, ctypes.Structure)
    if ctype == "void":
        # an unset restype is ctypes' int, read and dropped
        return declared is None or declared is ctypes.c_int
    return ctype in SCALARS and declared is SCALARS[ctype]


def test_declared_types_match_the_header(surfaces):
    _, declared = surfaces
    if not HEADER.is_file():
        pytest.skip("no generated header next to the crate")
    header = header_signatures()
    assert len(header) > 50, f"parsed only {len(header)} functions from {HEADER}"
    wrong = []
    for name, fn in sorted(declared.items()):
        if name not in header:
            wrong.append(f"{name}: not in the generated header")
            continue
        ret, params = header[name]
        if fn.argtypes is not None:
            for i, (ctype, decl) in enumerate(zip(params, fn.argtypes)):
                if not carries(ctype, decl):
                    wrong.append(f"{name}: parameter {i} is {ctype}, declared {decl.__name__}")
        if not carries(ret, fn.restype):
            got = getattr(fn.restype, "__name__", fn.restype)
            wrong.append(f"{name}: returns {ret}, declared {got}")
    assert not wrong, "ctypes types disagree with the C ABI:\n" + "\n".join(wrong)

