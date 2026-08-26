// The bridge between the one Faust interpreter and the compiler the page
// carries.
//
// `faust::boxes` and `faust::signals` are the *server's* JSON interpreters, in
// Rust: they read a box or signal tree and issue the corresponding calls of
// Faust's C API. Natively those calls go straight into libfaust. In a page the
// interpreter runs in the NRT worker's wasm module and the compiler is a
// *second* module -- `libfaust-wasm`, an Emscripten build -- so the calls have
// to cross between them, and **the two do not share an address space**. A box
// handle is an integer and crosses as it is; a `const char*` is a pointer into
// whichever module made it, and that is the whole of what this file does.
//
// So each import is one of three shapes:
//
//  - **plain** -- every argument is a handle or a number: hand it over.
//  - **strings at known positions** -- a slider's label, an `fconst`'s name:
//    read the C string out of the worker's memory, copy it into the compiler's
//    heap, call, free. libfaust copies a label as it takes it, so the temporary
//    dies with the call.
//  - **special** -- `CDSPToBoxes` alone, which takes an `argv` and writes back
//    through three out-pointers.
//
// The names and their string positions are read off `src/faust/ffi.rs`, which
// is the one place either side declares them; `clients/web/build.sh` generates
// the export list from the module's own declared imports, so an interpreter
// that grows a call the artifact does not export fails at load with that name
// rather than silently later.

/** The Emscripten module: `libFaustWasm`'s own `Module`. */
let lib = null;
/** The NRT worker module's linear memory, where the interpreter's strings are. */
let host = null;

/**
 * Everything copied into the compiler's heap for the def being built, freed
 * only when the factory exists.
 *
 * **Not an optimization.** A label handed to the C API has to outlive the
 * whole construction, not the one call that took it — the native path says so
 * in as many words (`faust::compiler`'s `cstrings`, kept "until the factory is
 * created"). Freeing each one at the end of its call passes every small test
 * and then breaks a *later* def, once the heap has churned enough to hand the
 * block out again: what fails is the term merging Faust's hash-consing does,
 * so a graph that shares a subterm stops sharing it and a recursion over it
 * never terminates. The report is a stack overflow inside the compiler, with
 * nothing to connect it to the def that actually wrote the label.
 */
let scope = null;

/**
 * Points the shim at the two modules. Called once, before the first def is
 * built, by whoever loaded them.
 */
export function attach(faustModule, hostMemory) {
    lib = faustModule;
    host = hostMemory;
}

/** Opens the arena for one def. Pairs with {@link endScope}. */
export function beginScope() {
    endScope();
    scope = [];
}

/** Frees everything the def needed, once its factory exists. */
export function endScope() {
    if (scope === null) return;
    for (const at of scope) lib._free(at);
    scope = null;
}

/** Holds one allocation for as long as the def is being built. */
function keep(at) {
    if (scope === null) {
        throw new Error("a Faust def is being built outside beginScope/endScope");
    }
    scope.push(at);
    return at;
}

/** Which arguments of which functions are `const char*`. Everything absent
 *  here passes its arguments through untouched. */
const STRING_ARGS = {
    CboxHSlider: [0],
    CboxVSlider: [0],
    CboxNumEntry: [0],
    CboxButton: [0],
    CboxCheckbox: [0],
    CboxHGroup: [0],
    CboxVGroup: [0],
    // (type, name, file)
    CboxFConst: [1, 2],
    CboxFVar: [1, 2],
    CsigButton: [0],
    CsigCheckbox: [0],
    CsigHSlider: [0],
    CsigVSlider: [0],
    CsigNumEntry: [0],
    CsigHBargraph: [0],
    CsigVBargraph: [0],
    CsigFConst: [1, 2],
    CsigFVar: [1, 2],
};

/** Which take a NULL-terminated array of handles, in the worker's memory. */
const HANDLE_ARRAY_ARGS = {
    CboxWaveform: [0],
    CsigWaveform: [0],
};

/** `ffi::ERROR_MSG_SIZE`: what the C API promises to write no more than. */
const ERROR_MSG_SIZE = 4096;

const decoder = new TextDecoder();

/** One NUL-terminated string out of the worker's memory. */
function readCString(ptr) {
    if (ptr === 0) return null;
    const bytes = new Uint8Array(host.buffer);
    let end = ptr;
    while (bytes[end] !== 0) end++;
    return decoder.decode(bytes.subarray(ptr, end));
}

/** The same string in the compiler's heap. The caller frees it. */
function writeCString(text) {
    const size = lib.lengthBytesUTF8(text) + 1;
    const at = lib._malloc(size);
    lib.stringToUTF8(text, at, size);
    return at;
}

/** A NULL-terminated handle array copied into the compiler's heap. */
function copyHandles(ptr) {
    const words = new Uint32Array(host.buffer);
    let n = 0;
    while (words[ptr / 4 + n] !== 0) n++;
    const at = lib._malloc((n + 1) * 4);
    for (let i = 0; i <= n; i++) lib.HEAP32[at / 4 + i] = words[ptr / 4 + i];
    return at;
}

/**
 * The import for one C function, by name. Everything the compiler does not
 * export is refused here rather than at the first call, where the failure
 * would be a box that is quietly null.
 */
export function bind(name) {
    if (name === "CDSPToBoxes") return dspToBoxes;
    const strings = STRING_ARGS[name];
    const arrays = HANDLE_ARRAY_ARGS[name];
    return (...args) => {
        const entry = entryPoint(name);
        if (strings === undefined && arrays === undefined) return entry(...args);
        const marshalled = args.slice();
        for (const at of strings ?? []) {
            const text = readCString(marshalled[at]);
            if (text === null) continue;
            marshalled[at] = keep(writeCString(text));
        }
        for (const at of arrays ?? []) {
            marshalled[at] = keep(copyHandles(marshalled[at]));
        }
        return entry(...marshalled);
    };
}

/** The compiler's own export, looked up when it is first needed: the shim is
 *  imported while the glue loads, long before the compiler exists. */
function entryPoint(name) {
    if (lib === null) throw new Error("the Faust compiler is not attached yet");
    const fn = lib["_" + name];
    if (typeof fn !== "function") {
        throw new Error(`the Faust compiler exports no ${name}`);
    }
    return fn;
}

/**
 * `CDSPToBoxes(name, source, argc, argv, *inputs, *outputs, error_msg)` --
 * the box schema's escape hatch to the Faust standard library, and the one
 * call that both takes an argv and answers through out-pointers. Everything it
 * touches is copied across and the answers are copied back, so the interpreter
 * reads them out of its own memory as it does natively.
 */
function dspToBoxes(namePtr, sourcePtr, argc, argvPtr, inputsPtr, outputsPtr, errorPtr) {
    const entry = entryPoint("CDSPToBoxes");
    const name = keep(writeCString(readCString(namePtr) ?? ""));
    const source = keep(writeCString(readCString(sourcePtr) ?? ""));
    let argv = 0;
    if (argc > 0 && argvPtr !== 0) {
        const words = new Uint32Array(host.buffer);
        argv = keep(lib._malloc(argc * 4));
        for (let i = 0; i < argc; i++) {
            const arg = keep(writeCString(readCString(words[argvPtr / 4 + i]) ?? ""));
            lib.HEAP32[argv / 4 + i] = arg;
        }
    }
    // The three answers, on the other hand, are read back here and done with.
    const inputs = lib._malloc(4);
    const outputs = lib._malloc(4);
    const error = lib._malloc(ERROR_MSG_SIZE);
    try {
        lib.HEAPU8[error] = 0;
        const result = entry(name, source, argc, argv, inputs, outputs, error);
        const words = new Int32Array(host.buffer);
        if (inputsPtr !== 0) words[inputsPtr / 4] = lib.HEAP32[inputs / 4];
        if (outputsPtr !== 0) words[outputsPtr / 4] = lib.HEAP32[outputs / 4];
        if (errorPtr !== 0) {
            const bytes = new Uint8Array(host.buffer);
            let n = 0;
            while (n < ERROR_MSG_SIZE - 1 && lib.HEAPU8[error + n] !== 0) n++;
            bytes.set(lib.HEAPU8.subarray(error, error + n), errorPtr);
            bytes[errorPtr + n] = 0;
        }
        return result;
    } finally {
        lib._free(inputs);
        lib._free(outputs);
        lib._free(error);
    }
}
