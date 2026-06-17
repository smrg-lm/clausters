# Clausters — Python client

High-level Python client for the [Clausters](../../README.md) audio server,
ported selectively from SuperCollider's class library
([sc3](https://github.com/smrg-lm/sc3)), **Faust-first**. Built in milestones;
see [`../PLAN.md`](../PLAN.md).

This is the **C1 scaffold**. In place now:

- `clausters.transport` — low-level transports (embedded server, shared memory,
  offline render); stdlib only. Its public names (`Clausters`, `ShmClient`,
  `render`) are re-exported from the top-level `clausters` package.
- `clausters._native` — ctypes binding over the shared native core
  (`clausters-ffi`): numeric builtins, seeded white noise and clock/sample
  math, matching the server by construction.
- `clausters.base._osclib` — minimal OSC wire encoding.
- `clausters.base` / `clausters.seq` / `clausters.defs` — placeholders for the
  base layer (C2), sequencing (C4) and Faust/SynthDef definitions plus server
  resources (C3).

## Building the native libraries

The package is pure Python at runtime but reaches Rust through two cdylibs that
**cargo** builds (not pip), found automatically under the workspace
`target/{release,debug}/`:

```sh
# from the repo root
cargo build -p clausters-ffi                      # libclausters_ffi (the core: _native)
cargo build --features embed,realtime             # libclausters    (transport: render/Clausters)
```

Override the locations with `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` if needed.

## Running the smoke tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python
floats/ints in, `array('f')`/bytes out.
