// An offline render that writes its file as it goes, and reads and writes the
// files its score names the moment it needs them.
//
// The one-call render (`./render.ts`) hands back every sample it rendered and
// reaches no file. This one is the native render (`renderFile`, the engine's
// `render_to_wav_with`): it writes its output a run at a time, and serves its
// own file streams between two runs -- the lists and `DiskIn`s it reads, the
// `DiskOut`s and `PeaksOut`s it writes -- through `clausters_core::files`, as
// it does natively. What the Rust needs of this side is that each file call
// is answered before it returns, so the files the score names are opened
// first (opening is asynchronous; reading and writing an open file is not),
// and each call is carried out on them (`claustersRenderFileCall`).
//
// Two places run it: a dedicated Worker in a page, where the page's files
// (OPFS) are read and written synchronously through sync access handles, and
// node, where the disk is read and written through `fs`. Each gives the run
// the files it reaches ([`RenderFiles`]); the render is the same.

import { FAILED, FileRequest, NOT_FOUND, Refused, field, fileOp } from "../base/file-requests.ts";
import { prepareFaust, rendererMemory, type EngineModule } from "./render.ts";

/** What a render to a file is: the score and how it is rendered. */
export interface FileRenderRequest {
    score: Uint8Array;
    sampleRate: number;
    channels: number;
    seed: bigint;
    /** The file the render's output is written to; empty for none. */
    path: string;
    sampleFormat: string;
}

/** What the render came to. */
export interface FileRenderResult {
    frames: number;
    events: number;
    seed: bigint;
    peak: number[];
    rms: number[];
    /** What the render logged, one `LEVEL<TAB>message` line per entry. */
    log: string;
}

/**
 * **The files a render reads and writes**, as the place running it reaches
 * them. Opening is asynchronous (a sync access handle is created with a
 * promise); reading and writing an open file is synchronous, which is what
 * lets the render's Rust read and write inside the call that needs it.
 */
export interface RenderFiles {
    /** Opens `path` to be read for the length of the render. */
    openRead(path: string): Promise<void>;
    /** Creates `path`, empty, to be written for the length of the render. */
    create(path: string): Promise<void>;
    size(path: string): number;
    read(path: string, at: number, length: number): Uint8Array;
    write(path: string, at: number, bytes: Uint8Array): void;
    /** Flushes and closes every file the render opened. */
    close(): Promise<void>;
}

/**
 * **Renders `request` into its file**, reading and writing through `files`.
 * `onProgress` is told the frames done and the frames the render comes to,
 * after each run.
 */
export async function renderToFile(
    engine: EngineModule,
    request: FileRenderRequest,
    files: RenderFiles,
    onProgress?: (done: number, total: number) => void,
): Promise<FileRenderResult> {
    await prepareFaust(engine, request.score);
    // Every file the render touches is opened before it starts: the ones
    // the score reads, the ones its synths write, and its own output.
    const reads = new Set(JSON.parse(engine.scoreFiles(request.score)) as string[]);
    const writes = new Set(JSON.parse(engine.scoreOutputs(request.score)) as string[]);
    if (request.path !== "") writes.add(request.path);
    const scope = globalThis as unknown as Record<string, unknown>;
    try {
        for (const path of reads) await files.openRead(path);
        for (const path of writes) await files.create(path);
        const memory = rendererMemory();
        const calls = renderCalls(files, reads, writes);
        scope.claustersRenderFileCall = (at: number) => {
            const call = new FileRequest(memory, at);
            try {
                call.answer(calls(call));
            } catch (error) {
                call.answer({ error });
            }
        };
        const stats = JSON.parse(engine.renderFile(
            request.score,
            request.sampleRate,
            request.channels,
            request.seed,
            request.path,
            request.sampleFormat,
            (done, total) => onProgress?.(done, total),
        )) as { frames: number; events: number; seed: string; peak: number[]; rms: number[]; log: string };
        return {
            frames: stats.frames,
            events: stats.events,
            seed: BigInt(stats.seed),
            peak: stats.peak,
            rms: stats.rms,
            log: stats.log,
        };
    } finally {
        delete scope.claustersRenderFileCall;
        await files.close();
    }
}

/**
 * **A render's file calls carried out** on the files opened for it: an open
 * or a create of one of them answers a handle on it, and a read, a write or
 * a length goes to the file the handle names. A file the score did not name
 * is refused, since it could not be opened inside the call.
 */
function renderCalls(
    files: RenderFiles,
    reads: Set<string>,
    writes: Set<string>,
): (call: FileRequest) => number {
    // The Rust names a path from the storage's root, with no leading `/`
    // (`clausters_core::files`); under node it is the disk's, absolute, so
    // a call is matched to the path it was opened under.
    const key = (path: string) => path.replace(/^\/+/, "");
    const opened = new Map<string, string>();
    for (const path of [...reads, ...writes]) opened.set(key(path), path);
    const handles = new Map<number, string>();
    let next = 1;
    const named = (handle: number) => {
        const path = handles.get(handle);
        if (path === undefined) throw new Refused(FAILED, `no open file ${handle}`);
        return path;
    };
    const handle = (path: string) => {
        handles.set(next, path);
        return next++;
    };
    return (call) => {
        const path = opened.get(key(call.path)) ?? call.path;
        switch (call.op) {
            case fileOp.open:
                if (!reads.has(path) && !writes.has(path)) {
                    throw new Refused(NOT_FOUND, `${path}: a render reads only the files its score names`);
                }
                return handle(path);
            case fileOp.create:
                if (!writes.has(path)) {
                    throw new Refused(FAILED, `${path}: a render writes only the files its score names`);
                }
                return handle(path);
            case fileOp.read: {
                const into = call.buffer;
                const got = files.read(named(call.u32(field.handle)), call.f64(field.at), into.length);
                into.set(got);
                return got.length;
            }
            case fileOp.write: {
                const from = call.buffer;
                files.write(named(call.u32(field.handle)), call.f64(field.at), from);
                return from.length;
            }
            case fileOp.len:
                return files.size(named(call.u32(field.handle)));
            case fileOp.close:
                handles.delete(call.u32(field.handle));
                return 0;
            case fileOp.metadata:
                if (!reads.has(path) && !writes.has(path)) {
                    throw new Refused(NOT_FOUND, `${path}: a render reads only the files its score names`);
                }
                return files.size(path);
            default:
                throw new Refused(FAILED, `${path}: a render does no file call ${call.op}`);
        }
    };
}

/**
 * **The page's files, through sync access handles** -- what a dedicated
 * Worker reaches. A file read is opened read-only where the browser can share
 * it, so a render reading a take does not lock it from the live engine or
 * another render reading the same take.
 */
export function opfsFiles(): RenderFiles {
    type Handle = {
        getSize(): number;
        read(buffer: Uint8Array, options: { at: number }): number;
        write(buffer: Uint8Array, options: { at: number }): number;
        truncate(size: number): void;
        flush(): void;
        close(): void;
    };
    const open = new Map<string, Handle>();
    const handleOf = async (path: string, create: boolean): Promise<FileSystemFileHandle> => {
        const parts = path.split("/").filter((p) => p.length > 0 && p !== ".");
        if (parts.some((p) => p === "..")) throw new Error(`${path}: a path may not climb out of the origin's root`);
        const name = parts.pop();
        if (name === undefined) throw new Error("an empty path names no file");
        let dir = await navigator.storage.getDirectory();
        for (const part of parts) {
            try {
                dir = await dir.getDirectoryHandle(part);
            } catch {
                throw new Error(`${path}: no such directory`);
            }
        }
        try {
            return await dir.getFileHandle(name, { create });
        } catch {
            throw new Error(`${path}: no such file`);
        }
    };
    const access = async (file: FileSystemFileHandle, mode?: string): Promise<Handle> => {
        const make = (file as unknown as {
            createSyncAccessHandle(options?: { mode: string }): Promise<Handle>;
        }).createSyncAccessHandle.bind(file);
        if (mode === undefined) return make();
        try {
            return await make({ mode });
        } catch {
            return make();
        }
    };
    const get = (path: string): Handle => {
        const handle = open.get(path);
        if (handle === undefined) throw new Error(`${path}: the file was not opened for this render`);
        return handle;
    };
    return {
        async openRead(path) {
            if (!open.has(path)) open.set(path, await access(await handleOf(path, false), "read-only"));
        },
        async create(path) {
            if (open.has(path)) return;
            const handle = await access(await handleOf(path, true));
            handle.truncate(0);
            open.set(path, handle);
        },
        size: (path) => get(path).getSize(),
        read(path, at, length) {
            const handle = get(path);
            const out = new Uint8Array(Math.max(0, Math.min(length, handle.getSize() - at)));
            if (out.length > 0) handle.read(out, { at });
            return out;
        },
        write: (path, at, bytes) => {
            get(path).write(bytes, { at });
        },
        async close() {
            for (const handle of open.values()) {
                try {
                    handle.flush();
                } catch {
                    // a read-only handle has nothing to flush
                }
                handle.close();
            }
            open.clear();
        },
    };
}

/** **The disk, through `fs`** -- what node reaches. */
export async function nodeFiles(): Promise<RenderFiles> {
    const fs = await import("node:fs");
    const open = new Map<string, number>();
    const get = (path: string): number => {
        const fd = open.get(path);
        if (fd === undefined) throw new Error(`${path}: the file was not opened for this render`);
        return fd;
    };
    return {
        async openRead(path) {
            if (!open.has(path)) open.set(path, fs.openSync(path, "r"));
        },
        async create(path) {
            if (!open.has(path)) open.set(path, fs.openSync(path, "w+"));
        },
        size: (path) => fs.fstatSync(get(path)).size,
        read(path, at, length) {
            const fd = get(path);
            const out = new Uint8Array(Math.max(0, Math.min(length, fs.fstatSync(fd).size - at)));
            if (out.length > 0) fs.readSync(fd, out, 0, out.length, at);
            return out;
        },
        write: (path, at, bytes) => {
            fs.writeSync(get(path), bytes, 0, bytes.length, at);
        },
        async close() {
            for (const fd of open.values()) fs.closeSync(fd);
            open.clear();
        },
    };
}

/**
 * **A render to a file, run where the place it runs reaches files**: a
 * Worker of its own in a page (one per render, so two renders run at once),
 * this thread under node.
 */
export async function renderFile(
    request: FileRenderRequest,
    onProgress?: (done: number, total: number) => void,
): Promise<FileRenderResult> {
    const { underNode } = await import("../base/files.ts");
    if (underNode()) {
        const { loadRenderer } = await import("./render.ts");
        return renderToFile(await loadRenderer(), request, await nodeFiles(), onProgress);
    }
    const worker = new Worker(new URL("./nrt-worker.js", import.meta.url), { type: "module" });
    try {
        return await new Promise<FileRenderResult>((resolve, reject) => {
            worker.onmessage = (event: MessageEvent) => {
                const reply = event.data as {
                    type: string; ticket: number; done?: number; total?: number;
                    result?: FileRenderResult; error?: string;
                };
                if (reply.type === "render-progress") {
                    onProgress?.(reply.done ?? 0, reply.total ?? 0);
                } else if (reply.type === "render") {
                    if (reply.error !== undefined) reject(new Error(reply.error));
                    else resolve(reply.result as FileRenderResult);
                }
            };
            worker.onerror = (event) => reject(new Error(event.message || "the render's Worker failed"));
            worker.postMessage({ type: "render", ticket: 1, ...request });
        });
    } finally {
        worker.terminate();
    }
}
