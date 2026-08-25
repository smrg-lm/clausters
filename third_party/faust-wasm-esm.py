#!/usr/bin/env python3
"""Turns the staged libfaust-wasm glue into an ES module.

Emscripten ends the file with a CommonJS export. A module Worker imports it, so
the tail is replaced with `export default FaustModule;` -- the same thing
upstream's own `make wasm` appends for its worklet variant.

Split out of `build-faust-wasm.sh` rather than inlined so the heredoc nesting
stays readable, and so a broken assumption fails with a message naming it.
"""
import sys

path = sys.argv[1]
src = open(path, encoding="utf-8").read()
for marker in ('if(typeof exports=="object"&&typeof module=="object")',
               'if(typeof exports==="object"&&typeof module==="object")'):
    at = src.find(marker)
    if at >= 0:
        open(path, "w", encoding="utf-8").write(
            src[:at] + "\nexport default FaustModule;\n")
        sys.exit(0)
sys.exit("the glue's CommonJS tail is not where this expects it; check what "
         "emscripten emitted before changing this")
