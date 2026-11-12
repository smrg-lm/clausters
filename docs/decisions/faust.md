# Faust

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Faust embedding: decisions and upstream bugs

- **Ubuntu's `libfaust` is unusable for embedding** — built without the LLVM
  backend and shipped without headers. libfaust must be built from source with
  the LLVM backend; the reproducible recipe lives in `BUILD.md`.
- **A hand-written FFI, not a crate.** `faust-build`/`faust-types` do
  Faust→Rust codegen at build time (they need the `faust` compiler and static
  DSP source) and do not embed the JIT; there is no maintained libfaust binding.
  We bind ~30 functions by hand against the real headers, avoiding a libclang
  build dependency. Because dynamic linking is lazy, a mistyped FFI symbol only
  fails when called — so a "kitchen sink" test exercises every schema op once.
- **libfaust does not tolerate concurrent compilation.** Two compiler threads in
  one process SIGSEGV — Faust's global compiler state is not thread-safe, even
  for `createCDSPFactoryFromString`. Fix: a process-global `compiler::ffi_lock()`
  around every compilation call. A server has a single compiler thread, but the
  test harness (and any multi-server embedder) needs the lock. With every FFI
  compilation path behind it, the faust suites now run in the ordinary parallel
  harness; the historical `--test-threads=1` rule is retired (a SIGSEGV there
  would mean a path skipped the lock).
- **Instantiation does *not* take the lock.** Creating instances from an
  already-compiled factory is independent of the compiler's global state (JIT
  code + malloc; FaustLive does it concurrently with compilations). The lock
  stays only for compiling. (Open caveat: `deleteDSPFactory` may touch the global
  factory table and can flake under parallel test load — non-blocking for a real
  single-compiler server.)
- **Upstream bug — `boxFmod()` returns `abs`.** `boxFmod()` in
  `compiler/box_signal_api.cpp` returns the abs primitive (a copy-paste bug,
  present through master-dev). Workaround: build `fmod` from a
  `CDSPToBoxes("process = fmod;")` fragment instead of the binding.
- **Upstream bug — the `cos` box returned abs** likewise; fixed the same way.

## Faust is a default feature, and the wheel ships it

`faust` used to be an opt-in Cargo feature, purely to keep a development build
free of the libfaust dependency. The cost of that convenience landed on the
*user*: the packaged artifacts were built with the default features, so an
installed wheel could not compile a `FaustDef` at all — `/def_send faust` replied
`/fail`, and one of the two def families was, in practice, unavailable. Two
families we document as peers cannot have one of them missing from the product.

So `faust` joins the default set, and the packaging bundles it:

- **A default build needs libfaust** (built with the LLVM backend, a one-time
  from-source install; `BUILD.md`). Building without it stays supported and
  tested — `--no-default-features --features synth,realtime,…` is a SynthDef-only
  server with no libfaust on the machine — but it is now the explicit choice,
  not the default one.
- **The wheel carries libfaust** in `clausters/_libs/`, staged by
  `clients/python/build_native.py` off the built artifacts (keyed by the exact
  soname the loader asks for). It is the heaviest thing in the wheel because the
  Faust JIT *is* LLVM. It carried a separate ~130 MB libLLVM beside it until
  LLVM was linked *into* libfaust; see "The LLVM in the wheel is the one the JIT
  reaches" below for what that changed and why the reasoning here was wrong.
- **`DT_RPATH`, not `DT_RUNPATH`.** `build.rs` emits `-Wl,--disable-new-dtags`
  with an rpath of `$ORIGIN`, `$ORIGIN/../_libs` and the build prefix. The
  `$ORIGIN` entries make the artifacts relocatable (the wheel's `_bin/clausters`
  finds `../_libs/libfaust.so.2`), and `DT_RPATH` is required because only it is
  inherited by *transitive* dependencies: libfaust itself carries no rpath, so
  the libraries it needs are resolved through ours. With `RUNPATH` the loader
  would fall back to the system copies — or find none.

## CI with a default `faust`: one shared libfaust, and a baseline JIT CPU

Making `faust` default turned every job that links the default build (clippy,
the default-set tests, the Python client, the release wheel/server) into a
libfaust consumer, where before only one dedicated job needed it. Two choices
keep that cheap and green:

- **One `libfaust` job builds it; everyone else restores a cache.** A single
  job runs the from-source build, and what it caches is self-contained because
  LLVM is linked into `libfaust.so`. Downstream jobs `needs:` it and restore the
  cache through the `.github/actions/libfaust` composite; the same `DT_RPATH`
  that makes the wheel self-contained (`<prefix>/lib`, inherited transitively)
  then resolves libfaust, so a consumer needs **no LLVM runtime installed** —
  only the restore. A warm cache makes every
  downstream job free. `release.yml` uses the same composite, so the wheel build
  no longer sets up libfaust by hand.
- **The build is vendored and pinned, so the bundle is reproducible.** The Faust
  source is too heavy to commit (it stays git-ignored under `third_party/faust`),
  so reproducibility lives in two committed files: `third_party/faust.pin` (the
  exact commit — the unpatched base, keeping the boxcos/boxfmod canaries valid —
  and the LLVM version) and `third_party/build-faust.sh` (the one recipe). The
  composite, `release.yml` and a developer all run *that* script and read *that*
  pin — the CI cache key included — so the bundled libfaust is the same
  everywhere, not whatever the build host defaulted to. **LLVM is pinned to 18**:
  it is Ubuntu 24.04's default (CI installs it from the distro repos, no
  apt.llvm.org), and the distro build targets a baseline x86-64, which keeps the
  LLVM linked into the wheel's libfaust portable — it runs on any x86-64 CPU. A newer upstream
  build (e.g. apt.llvm.org's 21) is built for a higher baseline and can itself
  hit an illegal instruction on older machines, so it is the wrong thing to
  ship; a developer who happens to have another LLVM builds locally with
  `LLVM_CONFIG=llvm-config-NN` (the FFI surface is version-independent).
- **CI pins a baseline JIT target (`CLAUSTERS_FAUST_TARGET`).** The Faust factory
  JITs for `""` = the host machine, and LLVM tunes the code to the detected CPU —
  correct and fast on a *real* machine, which is why production leaves it unset.
  But virtualized CI runners can report CPU features the hypervisor then traps,
  so host-tuned JIT code hits an illegal instruction at run time on some Azure
  SKUs (seen intermittently, VM-dependent). The override forces a baseline
  x86-64 target. The string is a Faust `triple:mcpu` pair
  (`x86_64-unknown-linux-gnu:x86-64`); what actually pins the CPU is the **mcpu
  half**, since an empty mcpu is what sends Faust to `sys::getHostCPUName()`.
  (An earlier revision credited the *triple* for this, on the theory that an
  empty one let LLVM re-detect host features. That was wrong, and it hid the
  real defect for longer than it should have — see the resolution below.) The
  override is a plain env var read by `faust::compiler::host_target`, so only CI
  opts in — and it only works against a libfaust carrying the `setTarget()`
  ordering fix.
  - *Resolved (2026-07-20): the override never took effect — it is a Faust
    bug.* In `llvm_dynamic_dsp_aux.cpp`, both source-based entry points
    (`createCDSPFactoryFromString`, `...FromSignals`) call
    `factory_aux->setTarget(target)` **after** `factory_aux->initJIT()`. But
    `initJIT()` is what reads `fTarget` to pick the triple and mcpu, and with an
    empty mcpu it falls back to `builder.setMCPU(sys::getHostCPUName())` — so
    every factory was JITed host-tuned no matter what target we passed. The
    bitcode/IR entry points on the same file pass the target through the factory
    *constructor*, i.e. before `initJIT()`, which is the intended order and makes
    the source paths an oversight rather than a design choice.
    Measured on the emitted JIT pages (not the input bitcode — see the caution
    below), same DSP and same LLVM, counting VEX/EVEX instructions:
    unpatched with the override, 139 (including `vfnmsub231ss`, FMA, absent from
    baseline x86-64); patched with the override, 0 — only scalar SSE; patched
    without the override, 139 again, so production keeps its host tuning.
    The fix is to hoist `setTarget()` above `initJIT()` in both paths. It has not
    been sent upstream.
    Two SIGILLs are on record, both with the override nominally active:
    `auto_order::faust_synths_sort_by_their_reserved_buses` (run 29492474371) and
    `test_defs::test_faustdef_renders_through_the_seam` (run 29711088891). The
    core captured for the third (run 29714651299) traps on `kmovd %ecx,%k1` —
    AVX-512 — in anonymous executable memory, i.e. JIT pages, with masked
    `vmovss` and `vroundss` around it. Host-tuned code on a runner that cannot
    run it, exactly as the original entry supposed.
    - **Caution, learned the hard way.** This bug was briefly "ruled out" on
      three separate arguments, all wrong in the same way: they measured a stage
      the bug does not live in. Running the *input bitcode* through `llc` at the
      pinned target shows clean SSE2 — but that tests whether `llc` honours
      `--mcpu`, not whether MCJIT ever received the target. Reading
      `EngineBuilder::selectTarget()` shows no feature autodetection — true, and
      irrelevant, because the host tuning entered one level up through an empty
      `fTarget`. A tell was visible and misread: bitcode written by a supposedly
      pinned factory carried `target triple = "x86_64-pc-linux-gnu"`, LLVM's host
      default, not the triple we passed. When a pin appears not to work, verify
      the stage that actually fails — here, disassemble the JIT pages.
    - **Also found, unrelated and open:** valgrind on `auto_order` reports a
      use-after-free at process exit — `llvm::MCJIT::~MCJIT` reading a block
      already freed by an earlier exit handler, reached from libfaust's global
      `dsp_factory_table` destructor during `_dl_fini` — plus uninitialised reads
      in `global::initDirectories`. Our own `Arc<FaustDef>` refcounting audits
      clean. Neither is the SIGILL, but the first can still crash a run at exit.
    - **Not a bug, do not "fix" it:** `FaustArgs` passes `-ftz` as `argv[0]` and
      does not NULL-terminate. libfaust prepends its own `"faust"` `argv[0]` and
      NULL-terminates its copy (`libcode.cpp`), so the arguments arrive correctly.

## The LLVM in the wheel is the one the JIT reaches

The wheel used to ship a pair: a ~9 MB `libfaust.so` plus the distro's
monolithic `libLLVM.so`, 137 MB of it, dragging libxml2, libedit, libtinfo,
libffi, libbsd and libmd along as vendored dependencies. Three quarters of the
package was one file. It is now a single `libfaust.so` with LLVM linked into it,
and the measurements are the argument (stripped, on this machine, against
LLVM 21 — the pinned 18 comes out smaller still, below):

| | raw | gzip |
|---|---|---|
| `libfaust.so` + `libLLVM.so` (what it was) | 146 MiB | 45 MiB |
| static LLVM, all Faust backends | 60 | 23 |
| + only the LLVM backend in the `.so` | 56 | 22 |
| + `--gc-sections` with the archives' symbols hidden | **43** | **18** |

The wheel went from 194 MiB installed / 64 MiB packed to **90 / 35**.

**What ships is the pinned LLVM 18, and it is smaller than the table.** The
figures above are this machine's LLVM 21; the release build is 18, and the
release workflow's dry run now packages a wheel and prints what it weighs, so
this is measured rather than inferred:

| | libfaust, stripped into the wheel | wheel installed | wheel packed |
|---|---|---|---|
| LLVM 21, here | 43 MiB | 90 MiB | 35 MiB |
| **LLVM 18, shipped** | **39 MiB** | **87 MiB** | **33 MiB** |

Comparing the two takes some care, because there are two libfaust sizes and they
are four MiB apart: what `make install` puts in the prefix (50 MiB on 21, 46 on
18) and what `build_native.py` strips into the wheel (43 and 39). Reading one
against the other is how 18 briefly got written down as the *larger* of the two.

One vendored dependency does come back with 18: its Support library still links
terminfo, so `libtinfo.so.6` — 0.2 MiB — is staged beside libfaust
(`build_native.py` reads `ldd`, so nothing had to decide this). LLVM 19 removed
that dependency, which is why 21 shows none; the other five — libxml2, libedit,
libffi, libbsd, libmd — are gone on both.

Four trims, and each one answers a different question about what a *shipped*
JIT actually needs. `third_party/build-faust.sh` explains them where they are
applied; what belongs here is why they were available at all:

- **The distro libLLVM is one library for a whole toolchain.** Every target
  backend (AArch64, AMDGPU, Hexagon, RISCV, SPIRV, Xtensa…), BOLT, the DWARF
  linkers, Exegesis, ObjectYAML. Measured over its static components, the
  backends that are not the host account for 41% and the non-JIT tooling for a
  further 11% — more than half of it is material libfaust never calls. A
  `NEEDED` link takes all of it anyway; a static link takes the archive members
  it references and nothing else.
- **`-ULLVM_BUILD_UNIVERSAL`, which is upstream's oversight, not ours.** Faust's
  `build/CMakeLists.txt:141` defines `LLVM_BUILD_UNIVERSAL` on every platform,
  outside the `if (UNIVERSAL)` at line 169 that was clearly meant to gate it
  (and which is therefore dead). That define is what compiles the
  `InitializeAllTargets()` call in `llvm_dynamic_dsp_aux.cpp` and pins a
  reference to all twenty backends — so the monolithic libLLVM was not merely
  convenient, it was *required*. Undefined, `initJIT` is left with
  `InitializeNativeTarget()`, which is all a JIT compiling for its own machine
  ever needed. What it gives up is emitting machine code for a different triple
  (`writeDSPFactoryToMachineFile`), which Clausters does not use. There is no
  silent failure mode: if the flag stops taking, the link fails on an undefined
  `LLVMInitialize<other-arch>TargetInfo`.
- **A shared object exports everything, so nothing could be collected.** 26k of
  the 37k exported symbols were LLVM's, every one of them a root that
  `--gc-sections` had to keep. `--exclude-libs,ALL` hides what came from the
  archives — the Faust API is compiled into the target, not pulled from one, so
  it stays exported — and the exported set drops to 1.4k. This trim alone is
  13 MiB, and it only exists once the link is static.
- **One Faust backend in the shared library.** The server only ever JITs, so the
  other seventeen (C, C++, Rust, Julia, WASM…) stay in the CLI compiler, where
  `faust -lang c` reads them. Worth 4 MiB, the smallest of the four, and the
  only one a reader would have guessed at first.

**What this supersedes.** The earlier record claimed static LLVM "is no lighter
— the server binary and the embed cdylib would each embed their own copy". That
confused two different things: linking LLVM into *our* binaries, which would
indeed duplicate it, and linking it into `libfaust.so`, which is still one
shared library loaded once by both. The claim was never measured, and it was
wrong by a factor of three.

**What is not settled.** Building LLVM from source with
`LLVM_TARGETS_TO_BUILD=<host>` would trim further, and is deliberately not done:
the pin buys the property that CI installs LLVM from the distro repos with no
`apt.llvm.org` and no LLVM build, and that is worth more than the remaining
megabytes. The static link needs no such thing — the same `llvm-N-dev` package
ships the archives.

## The page's Faust is a second wasm module linked into the engine's own memory

*2026-08-25.*

The in-page engine is the `synth,embed` build: no libfaust, no LLVM JIT, so
`/def_send faust` answers `/fail` in a tab and `/done` in a window. That is a
capability one client has and the other does not, which this project treats as a
defect rather than as a platform's quirk, and closing it is the half of W7 that
a compiler artifact does not solve on its own. `libfaust-wasm` gives us a
*compiler* in the page; what it does not give us is a way to **instantiate** what
it compiled inside a node of our tree.

**The shape of the answer is fixed by what a Faust node already is here.**
`FaustSynth` (`src/faust/synth.rs`) holds an opaque instance pointer, a `Vec` of
`f32` parameter zones reaching into it, and private non-interleaved block
buffers it stages I/O through; per block it does one FFI call,
`computeCDSPInstance(dsp, frames, in**, out**)`, and `/node_set` is a plain
aligned store into a zone. Faust's **wasm backend emits exactly that ABI**:
`compute(dsp: i32, count: i32, inputs: i32, outputs: i32)`, pointers being byte
offsets into linear memory, with each parameter's offset inside the DSP struct
published in the JSON the compiler returns beside the binary (`"size"` for the
struct, `"index"` per parameter). On `wasm32` a `*mut f32` *is* a `u32` offset,
so `in_ptrs`/`out_ptrs` and `zones` are already the right types. The node's
`process` needs no restructuring at all — three `ffi::` calls change hands.

**So the Faust module is linked into the engine, not run beside it.** Compiled
with `-lang wasm-e` (external memory: the compiler documents it for polyphony,
and what it means is that the module allocates nothing), the emitted module is
instantiated with `env.memory` bound to **the engine's own
`WebAssembly.Memory`** and its `compute` export appended to the engine's
`__indirect_function_table`; Rust calls it through that table by transmuting the
slot index to an `extern "C" fn`, which on `wasm32` is what a function pointer
already is. The DSP struct is allocated by our allocator, at an offset we choose,
and the I/O buffers are the `Block`s the synth already owns. **No copies, no
JavaScript frame on the audio path, and the node stays a node** — `/node_set`,
`/node_map`, the bus summing and the done actions are the same code in both
builds.

**The transcendentals are ours too.** A Faust wasm module imports what the
instruction set lacks — `env._sinf`, `env._powf`, `env._fmodf` and their
neighbours. A wasm function exported by one instance is a legal import of
another, so these are bound to the engine's own exports rather than to
`Math.sin` closures: no JS on the audio path, and Faust and our UGens go through
one libm, which is what keeps a parity vector meaningful.

**The compiler gets a scope of its own, and the protocol does not learn about
it.** Natively, `/def_send faust` compiles on the compiler thread and replies
`/done` later; the audio thread never sees a compiler. In a page that thread is
the **NRT worker** — this first said the main thread, and the second decision
below moved it, for the reason that decision gives: the main thread owes the GUI
host its frames. So the worklet, on `/def_send faust`, posts the payload out, the
Worker compiles with `libfaust-wasm`, posts back the module bytes and the JSON,
and the worklet instantiates and replies `/done`.
This is the same asynchronous shape the command already has, which is why
**nothing on the wire changes**: no precompiled-payload form, no second command,
no client-side special case. `docs/schemas.md` is untouched, and the one
observable difference is that the reply stops being `/fail`.

It also settles the packaging rule W7 states without needing a rule: the
compiler's assets are fetched by the engine's glue **on the first `/def_send
faust`**, so a page that mounts a prebuilt bundle of SynthDefs downloads none of
them, and one whose bundle carries a FaustDef downloads them because it needs
them.

**The one hazard, named because it is invisible until it corrupts something.**
The wasm backend writes the DSP's JSON into a data segment at **absolute offset
0**, unconditionally — external memory included. Instantiating such a module
against the engine's memory therefore writes over the engine's own first bytes.

*This paragraph first said to move the engine's data out of the way with
`--global-base`, and that is not available.* Building the thing found out why,
and the correction is worth keeping because the reasoning that produced it was
sound and still wrong: rustc links wasm with `--stack-first`, so the low
megabyte is the **stack**, not `.rodata` — and `--global-base` must be at least
the stack size when `--stack-first` is used, so it cannot open a gap *below* the
stack, which is exactly where the segment lands. A reserved page there is not
something the linker will give.

So the segment goes instead of being worked around. Nothing reads it — the JSON
we use is the one `createDSPFactory` returns beside the binary — so the copy
inside the module is dead weight, and `faust::wasm_module::strip_data_section`
removes it before instantiation. It walks the binary's own top-level framing (an
id byte and a LEB128 length, stable since the MVP) and **refuses rather than
guesses**: more than one segment, or one that does not start at zero, means the
backend grew a use for that memory this has not accounted for, and dropping it
blind would be the kind of corruption nobody traces back here.

**What was rejected, and why it is not a near miss.**

- **A Faust wasm module as a WebAudio node beside the engine** — the shape the
  phrase "run behind our AudioWorklet" first suggested. It puts the Faust DSP
  *outside* the node tree: no `/node_set` by control index, no group ordering, no
  bus summing, no done action. The browser would gain a Faust that is not the
  same thing the window's Faust is, which is the divergence this milestone
  exists to remove, not a cheaper way to remove it.
- **A separate memory per instance, with JS copying blocks across** — the
  conservative version of the choice above, and the fallback if the linker
  constraints below ever stop holding. It keeps the node in the tree, but it puts
  a JavaScript frame and two copies in the audio callback, per Faust node, per
  block. The B track's own topology note already draws the line in exactly this
  place: OSC translation may allocate on the worklet thread, **the DSP may not**.
- **Faust's interpreter backend, executed inside `libfaust-wasm`** — the other
  path the plan named. It moves the audio compute into the Emscripten module
  entirely, which means the whole compiler resident in the worklet, an
  interpreted inner loop, and the same cross-module boundary as the option above
  with worse numbers on the other side of it. Its only advantage is one artifact
  instead of two.
- **A wasm interpreter in Rust inside the engine** (`wasmi` and its family) —
  everything stays in Rust and nothing else does: interpreting wasm inside wasm,
  on the audio thread, allocating.

**Two things the build found, both invisible until something is silent.**

*A `WebAssembly.Module` posted into an AudioWorklet is dropped on arrival.*
Compiling in the Worker and cloning the finished module across is the obvious
shape — a Module is serializable, and it keeps the last piece of compilation off
the thread that owes a block. It appears to work: the `postMessage` succeeds,
nothing throws on either side, and the message simply never arrives, because a
Module can only be cloned inside one agent cluster and a worklet is not in the
Worker's. So the **bytes** travel and the worklet compiles them itself, which is
microseconds once per def against a silence with no error anywhere. The port now
carries an `onmessageerror` for the same reason: that is where a dropped message
does show up, and without it the engine just never answers.

*wasm-bindgen drops the exports the linker synthesized*, `__indirect_function_table`
among them, so `--export-table` on the Rust side is not enough — the whole design
rests on that table and it was gone from the staged bundle. `--keep-lld-exports`
on the engine's bundle alone keeps it; the other three bundles link no second
module and keep the smaller surface.

**The interpreters are the server's, and they run in the page.** A def sent as a
box or a signal tree is read by `faust::boxes` and `faust::signals` — the same
Rust a native server runs, compiled to wasm and living in the NRT worker — and
what they issue is Faust's own C API. The alternative was to read the schema
again in TypeScript, or to print it back to Faust source; both are a second
implementation of one schema, and the failure they produce is a def that means
something slightly different in a tab than in a window, with nothing to report
it. Two things had to be built for the interpreters to reach the compiler:

- **Faust's Emscripten bindings expose only `createDSPFactory`.** The wasm
  backend has `createWasmDSPFactoryFromBoxes` and `...FromSignals` already; what
  was missing was the JS-facing surface over them and the C API being exported at
  all. `third_party/faust-wasm-bindings.patch` adds four embind methods, and the
  link exports the C API — the list read out of `src/faust/ffi.rs`, so a call the
  interpreter grows and the artifact does not export fails at load, by name,
  rather than later.
- **The two modules do not share an address space.** A box handle is an integer
  and crosses as it is; a `const char*` is a pointer into whichever module made
  it. `faust-shim.js` is that boundary and nothing else: it reads a C string out
  of the interpreter's memory and writes it into the compiler's, copies a
  NULL-terminated handle array the same way, and gives `CDSPToBoxes` its `argv`
  and its three out-pointers. Which arguments are strings is a table of
  nineteen entries; everything else passes through.

**Two more silences, both found by building it and both worth the words.** They
have the same shape as the two above — the thing works, then a *later* def fails
for no reason anyone would connect to it.

- **A label freed when its call returns breaks a def compiled afterwards.**
  Faust keeps what it was handed, so the native path holds every `CString` until
  the factory exists and says so. Freeing per call passes every small test and
  then, once the heap has churned enough to hand the block out again, breaks the
  term merging hash-consing does: a graph that shares a subterm stops sharing it,
  and a recursion over it never terminates. It is reported as a stack overflow
  inside the compiler. So the shim holds one arena per def and frees it when the
  factory exists.
- **A destroyed lib context poisons the next one**, the same way and with the
  same report. The native path brackets every def with
  `createLibContext`..`destroyLibContext`; here that bracket is what breaks the
  *following* def — reproduced down to two, a box with a `rec` and then anything
  recursive. So a page opens one arena and keeps it, which also costs less than
  one per def. That in turn decides how **source** is compiled: through the box
  schema's own escape hatch (`{"op": "faust", "src": …}`, which is
  `CDSPToBoxes`) rather than through `createDSPFactory`, because that entry
  point allocates and destroys a context of its own and would take the page's
  arena down with it. One consequence worth naming: all three formats now reach
  the compiler by one path, which is a better shape than the one this started
  from.

**A third silence, found by measuring the first two.** Keeping one arena for the
tab's life has a bill nobody had asked for: the *call stack*. A wasm frame sits
on the JavaScript engine's stack — about a megabyte, against the eight a native
thread gets, and unrelated to the `STACK_SIZE=8MB` the compiler is linked with,
which is only its shadow stack — and libfaust recurses over the term graph of
everything compiled so far. So a page compiling **distinct recursive signal**
defs runs the stack out at roughly every other def, reported as
`stack overflow (Maximum call stack size exceeded)` on a def that is perfectly
well formed. It is browser-only: eight of the same defs compile in one context
natively, which is what says the interpreter is not at fault.

The way out is not a smaller def and not a destroyed context. It is a **fresh
compiler**: a def that exhausts the stack is compiled again in a new Emscripten
instance — new memory, new arena, new context — and succeeds. This does not
contradict the entry above, and the distinction is the whole point: what poisons
a page is a context that was *destroyed*, not a second one that exists. The cost
is one instantiation, and the fetch is in cache and the module already compiled,
so twelve such defs in a row (six replacements between them) average 18 ms each
against the 9 ms a def that never overflows costs. `tests/faust-arena.html` is
the measurement and the numbers move with the compiler's pin, so it is a page to
re-run when `third_party/faust.pin` does.

Defs written as **box** trees or as **source** never hit it, which is worth
knowing but is not a reason to steer anyone: the retry makes the signal API work
without a caveat, and a client surface that is fine in one language and
conditional in another is the divergence this project does not keep.

**Three assumptions carry this, and all three were checked before anything was
built on them** — a probe that instantiates a hand-emitted second module against
a Rust one. They hold: `-C link-arg=--export-table -C link-arg=--growable-table`
makes rustc export `memory` and `__indirect_function_table`; a second module
instantiates with `env.memory` bound to the first's memory and a Rust export
bound as its `_sinf`; and Rust calls that module's export **through the shared
table by index**, transmuting the index to an `extern "C" fn` — which on
`wasm32` is what a function pointer already is.

Each also fails loudly rather than subtly, which is what makes them safe to
depend on. A wrong index or a changed pointer representation traps on
`call_indirect`'s type check, and the fallback is a JS shim doing
`table.get(i)(…)`, one frame per block. The link flags are checked by the wasm
build gate. And a Faust release where `-lang wasm-e` stopped meaning "allocates
nothing" would be caught by the def-send path failing to find its parameters
where the JSON says they are. None of the three is a reason to prefer a design
that is slower on purpose.

## A page has one number type, and Faust's promotion makes that cost nothing

*2026-08-27.*

Porting the box API found that a page cannot write a Faust constant that is
*real* and integral. A def builder's numeric constant travels as a bare JSON
number and the server reads it back with `serde_json`: an integral one becomes a
Faust **int**, anything else a **real** (`src/faust/boxes.rs`, `number_box`; the
signal schema does the same). Python separates them by the literal's type —
`box(2)` is `int 2`, `box(2.0)` is `real 2.0` — and JavaScript has one number
type, so `2.0` *is* `2` by the time `boxes.box`/`signals.signal` sees it. It is
older than the box API (`signals.ts` has had it since W1) and the frozen parity
vectors cannot see it, because they are compared as parsed JSON where `2.0` and
`2` are the same value — worth knowing about what those vectors prove.

The obvious repair is a spelling: both schemas already accept an explicit
`{"op": "real", "value": x}` node. It was not taken, and the reason is a
measurement rather than a preference. Compiling both spellings of every operator
that could care (`faust -lang c`, the pinned compiler) says the constant's type
never decides a **value**:

- **`/` is real whatever its operands are.** `7 / 2` folds to `3.5f`; there is no
  integer division to fall into.
- **Any real operand promotes the whole expression.** An audio signal is real, so
  `sig % 3` and `sig % 3.0` are the same `fmodf`, and so on through the arithmetic.
- **Where int is *required*** — a table index, `@`, `select2`, the bitwise family
  — a real is cast down silently, so the int a page emits is what those wanted.
- **What is left is an operator pair chosen by type against an *int* signal**:
  `%`, `min`, `max` and the comparisons compile to `%`/`min_i`/`>` against an int
  and `fmodf`/`fminf`/`>` on floats. Both compute the same number for an integral
  constant; they diverge only past 2²⁴, where the exact one is the int.

So the gap is a spelling, not a behaviour — and the spelling already exists, in
a verb both clients have: `asfloat`. `box(2).asfloat()` is `float(2)` in Faust,
which the compiler folds to `2.0f`, and `int(x) % float(4)` is byte-identical to
`int(x) % 4.0`. Adding a `real()` builder would put a name in one client that the
other has no use for — Python has the float literal — which is the divergence the
non-divergence rule exists to prevent, paid for a distinction nothing can hear.
Recorded in the web client's book, under the platform's own limits.
