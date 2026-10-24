/**
 * **The files of an editing application in a page**: a worker that answers
 * the application's threads' calls into the origin's storage (OPFS).
 *
 * A thread of the application (`apps-thread.ts`) reads and writes files
 * synchronously, as natively, through `clausters_core::files`: it writes a
 * request into the shared memory, posts its address here and waits on it.
 * This worker does the work with the storage's sync access handles, writes
 * the answer into the request and wakes the thread. A file is one sync
 * handle, shared by every thread that has it open, since the storage gives a
 * file one at a time; each thread keeps its own position on its side.
 *
 * The request's layout is `clausters_core::files::Request`, `repr(C)` on
 * wasm32: the offsets below are its fields'.
 *
 * @module
 */

const STATE = 0;
const OP = 4;
const HANDLE = 8;
const LEN = 12;
const BUF = 16;
const PATH = 20;
const PATH_LEN = 24;
const PATH2 = 28;
const PATH2_LEN = 32;
const AT = 40;
const RESULT = 48;
const MODIFIED = 56;
const IS_DIR = 64;
const ERROR = 68;
const ERROR_CAP = 72;
const ERROR_LEN = 76;

const DONE = 1;

/** What a request asks, as `clausters_core::files::Op` numbers it. */
const enumOp = {
    open: 1, create: 2, read: 3, write: 4, len: 5, close: 6, rename: 7, removeFile: 8,
    removeDir: 9, removeDirAll: 10, createDir: 11, createDirAll: 12, metadata: 13, readDir: 14,
    holdNew: 15, release: 16, sweep: 17,
} as const;

/**
 * The prefix of a working folder's Web Lock, before the folder's name: the
 * one `src/base/workspace.ts` holds a page's folders by, so either sweeps
 * what the other left.
 */
const TAB_LOCK = "clausters-work:";

/** What lets go of each working folder's lock, by its path. */
const releases = new Map<string, () => void>();

/** The failures a request answers by number, as `files.rs` reads them. */
const FAILED = -1;
const NOT_FOUND = -2;
const EXISTS = -3;
const NOT_EMPTY = -4;

/** A failure answered with its number and its reason. */
class Refused extends Error {
    readonly code: number;

    constructor(code: number, message: string) {
        super(message);
        this.code = code;
    }
}

let memory: WebAssembly.Memory | null = null;

/** One file open, shared by every handle on it. */
interface Open {
    path: string;
    sync: FileSystemSyncAccessHandle;
    refs: number;
}

const byPath = new Map<string, Open>();
const byHandle = new Map<number, Open>();
let nextHandle = 1;

/** The request's view onto the memory as it is now (it may have grown). */
const view = () => new DataView((memory as WebAssembly.Memory).buffer);
const bytes = (at: number, len: number) => new Uint8Array((memory as WebAssembly.Memory).buffer, at, len);
const text = (at: number, len: number) => new TextDecoder().decode(bytes(at, len).slice());

/** The `/`-separated path's parts, refusing a climb out of the root. */
function parts(path: string): string[] {
    const out = path.split("/").filter((p) => p.length > 0 && p !== ".");
    if (out.some((p) => p === "..")) throw new Refused(FAILED, `${path}: a path may not climb out of the root`);
    return out;
}

/** The directory `names` leads to, made on the way when `create`. */
async function directory(names: string[], create: boolean): Promise<FileSystemDirectoryHandle> {
    let dir = await navigator.storage.getDirectory();
    for (const name of names) dir = await dir.getDirectoryHandle(name, { create });
    return dir;
}

/** The directory holding `path`, and its own name. */
async function parent(path: string): Promise<[FileSystemDirectoryHandle, string]> {
    const names = parts(path);
    const name = names.pop();
    if (name === undefined) throw new Refused(FAILED, "an empty path names nothing");
    return [await directory(names, false), name];
}

/** The storage's failures, in the numbers a request answers with. */
function refusal(error: unknown, path: string): Refused {
    if (error instanceof Refused) return error;
    const name = (error as { name?: string })?.name;
    const message = `${path}: ${(error as Error)?.message ?? String(error)}`;
    if (name === "NotFoundError" || name === "TypeMismatchError") return new Refused(NOT_FOUND, message);
    if (name === "InvalidModificationError") return new Refused(NOT_EMPTY, message);
    return new Refused(FAILED, message);
}

/** A sync handle on `path`, shared when it is open already. */
async function openShared(path: string, create: boolean): Promise<number> {
    let open = byPath.get(path);
    if (open === undefined) {
        const [dir, name] = await parent(path);
        const file = await dir.getFileHandle(name, { create });
        open = { path, sync: await file.createSyncAccessHandle(), refs: 0 };
        byPath.set(path, open);
    }
    if (create) open.sync.truncate(0);
    open.refs += 1;
    const handle = nextHandle++;
    byHandle.set(handle, open);
    return handle;
}

function held(handle: number): Open {
    const open = byHandle.get(handle);
    if (open === undefined) throw new Refused(FAILED, `no open file ${handle}`);
    return open;
}

/** Lets go of a file's sync handle, when one is held on `path`. */
function letGo(path: string): Open | undefined {
    const open = byPath.get(path);
    if (open !== undefined) {
        open.sync.flush();
        open.sync.close();
        byPath.delete(path);
    }
    return open;
}

/** One request carried out: what goes in `result`, and the rest written. */
async function carry(at: number): Promise<number> {
    const v = view();
    const op = v.getUint32(at + OP, true);
    const path = text(v.getUint32(at + PATH, true), v.getUint32(at + PATH_LEN, true));
    switch (op) {
        case enumOp.open:
            return openShared(path, false);
        case enumOp.create:
            return openShared(path, true);
        case enumOp.read: {
            const open = held(v.getUint32(at + HANDLE, true));
            const into = bytes(v.getUint32(at + BUF, true), v.getUint32(at + LEN, true));
            return open.sync.read(into, { at: v.getFloat64(at + AT, true) });
        }
        case enumOp.write: {
            const open = held(v.getUint32(at + HANDLE, true));
            const from = bytes(v.getUint32(at + BUF, true), v.getUint32(at + LEN, true));
            return open.sync.write(from, { at: v.getFloat64(at + AT, true) });
        }
        case enumOp.len:
            return held(v.getUint32(at + HANDLE, true)).sync.getSize();
        case enumOp.close: {
            const handle = v.getUint32(at + HANDLE, true);
            const open = byHandle.get(handle);
            byHandle.delete(handle);
            if (open !== undefined && --open.refs === 0 && byPath.get(open.path) === open) {
                letGo(open.path);
            }
            return 0;
        }
        case enumOp.rename: {
            const target = text(v.getUint32(at + PATH2, true), v.getUint32(at + PATH2_LEN, true));
            // a file open is moved closed, and opened again where it went
            const open = letGo(path);
            letGo(target);
            const [from, name] = await parent(path);
            const [to, toName] = await parent(target);
            const file = await from.getFileHandle(name);
            try {
                await to.removeEntry(toName);
            } catch {
                // nothing there to replace
            }
            const movable = file as FileSystemFileHandle & {
                move?: (dir: FileSystemDirectoryHandle, name: string) => Promise<void>;
            };
            if (typeof movable.move === "function") {
                await movable.move(to, toName);
            } else {
                const copy = await (await to.getFileHandle(toName, { create: true })).createSyncAccessHandle();
                const source = await file.createSyncAccessHandle();
                const all = new Uint8Array(source.getSize());
                source.read(all, { at: 0 });
                source.close();
                copy.truncate(0);
                copy.write(all, { at: 0 });
                copy.flush();
                copy.close();
                await from.removeEntry(name);
            }
            if (open !== undefined && open.refs > 0) {
                const moved = await (await to.getFileHandle(toName)).createSyncAccessHandle();
                open.sync = moved;
                open.path = target;
                byPath.set(target, open);
            }
            return 0;
        }
        case enumOp.removeFile: {
            letGo(path);
            const [dir, name] = await parent(path);
            await dir.getFileHandle(name);
            await dir.removeEntry(name);
            return 0;
        }
        case enumOp.removeDir:
        case enumOp.removeDirAll: {
            const recursive = op === enumOp.removeDirAll;
            for (const held of [...byPath.keys()]) {
                if (held === path || held.startsWith(`${path}/`)) letGo(held);
            }
            const [dir, name] = await parent(path);
            await dir.getDirectoryHandle(name);
            await dir.removeEntry(name, { recursive });
            return 0;
        }
        case enumOp.createDir: {
            const [dir, name] = await parent(path);
            try {
                await dir.getDirectoryHandle(name);
                throw new Refused(EXISTS, `${path}: already there`);
            } catch (error) {
                if (error instanceof Refused) throw error;
            }
            await dir.getDirectoryHandle(name, { create: true });
            return 0;
        }
        case enumOp.createDirAll:
            await directory(parts(path), true);
            return 0;
        case enumOp.metadata: {
            const open = byPath.get(path);
            if (open !== undefined) {
                v.setFloat64(at + MODIFIED, Date.now(), true);
                return open.sync.getSize();
            }
            const [dir, name] = await parent(path);
            try {
                const file = await (await dir.getFileHandle(name)).getFile();
                v.setFloat64(at + MODIFIED, file.lastModified, true);
                return file.size;
            } catch (error) {
                if ((error as { name?: string })?.name !== "TypeMismatchError") throw error;
                await dir.getDirectoryHandle(name);
                view().setUint32(at + IS_DIR, 1, true);
                return 0;
            }
        }
        case enumOp.readDir: {
            const dir = await directory(parts(path), false);
            let lines = "";
            const listing = (dir as unknown as {
                entries(): AsyncIterable<[string, FileSystemHandle]>;
            }).entries();
            for await (const [name, handle] of listing) {
                lines += `${handle.kind === "directory" ? "d" : "f"} ${name}\n`;
            }
            const encoded = new TextEncoder().encode(lines);
            const cap = view().getUint32(at + LEN, true);
            if (encoded.length <= cap) bytes(view().getUint32(at + BUF, true), encoded.length).set(encoded);
            return encoded.length;
        }
        case enumOp.holdNew: {
            // a folder of this tab's, held by a lock the browser lets go of
            // however the tab goes
            const name = text(v.getUint32(at + PATH2, true), v.getUint32(at + PATH2_LEN, true));
            const leaf = `${name}-${crypto.randomUUID().slice(0, 13)}`;
            const folder = `${parts(path).join("/")}/${leaf}`;
            await new Promise<void>((granted) => {
                void navigator.locks.request(TAB_LOCK + leaf, () => {
                    granted();
                    return new Promise<void>((done) => releases.set(folder, done));
                });
            });
            await directory(parts(folder), true);
            const encoded = new TextEncoder().encode(folder);
            const cap = view().getUint32(at + LEN, true);
            bytes(view().getUint32(at + BUF, true), Math.min(encoded.length, cap))
                .set(encoded.subarray(0, cap));
            return encoded.length;
        }
        case enumOp.release: {
            const folder = parts(path).join("/");
            releases.get(folder)?.();
            releases.delete(folder);
            return 0;
        }
        case enumOp.sweep: {
            // every folder no tab holds or waits for goes
            const state = await navigator.locks.query();
            const held = new Set([...(state.held ?? []), ...(state.pending ?? [])].map((l) => l.name));
            let dir: FileSystemDirectoryHandle;
            try {
                dir = await directory(parts(path), false);
            } catch {
                return 0;
            }
            let gone = 0;
            const listing = (dir as unknown as {
                entries(): AsyncIterable<[string, FileSystemHandle]>;
            }).entries();
            const names: string[] = [];
            for await (const [name, handle] of listing) {
                if (handle.kind === "directory" && !held.has(TAB_LOCK + name)) names.push(name);
            }
            for (const name of names) {
                await dir.removeEntry(name, { recursive: true });
                gone += 1;
            }
            return gone;
        }
        default:
            throw new Refused(FAILED, `no file call ${op}`);
    }
}

/** **Answers the request at `at`**, and wakes the thread waiting on it. */
async function answer(at: number): Promise<void> {
    let result: number;
    try {
        result = await carry(at);
    } catch (error) {
        const path = text(view().getUint32(at + PATH, true), view().getUint32(at + PATH_LEN, true));
        const refused = refusal(error, path);
        result = refused.code;
        const reason = new TextEncoder().encode(refused.message);
        const cap = view().getUint32(at + ERROR_CAP, true);
        const len = Math.min(reason.length, cap);
        bytes(view().getUint32(at + ERROR, true), len).set(reason.subarray(0, len));
        view().setUint32(at + ERROR_LEN, len, true);
    }
    view().setFloat64(at + RESULT, result, true);
    const state = new Int32Array((memory as WebAssembly.Memory).buffer);
    Atomics.store(state, (at + STATE) >> 2, DONE);
    Atomics.notify(state, (at + STATE) >> 2);
}

/**
 * The requests are answered one at a time, in the order they came: a file a
 * thread writes and another then reads is written by then.
 */
let queue: Promise<void> = Promise.resolve();

const scope = globalThis as unknown as {
    onmessage: ((event: MessageEvent<{ memory?: WebAssembly.Memory; port?: MessagePort }>) => void) | null;
};

scope.onmessage = (event) => {
    if (event.data.memory !== undefined) memory = event.data.memory;
    const port = event.data.port;
    if (port !== undefined) {
        port.onmessage = (request: MessageEvent<number>) => {
            queue = queue.then(() => answer(request.data));
        };
    }
};
