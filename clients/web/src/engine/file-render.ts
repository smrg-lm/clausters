// An offline render that writes its file a run at a time, and reads the files
// its score reads the moment it needs them.
//
// The one-call render (`./render.ts`) hands back every sample it rendered, and
// serves none of the streams that read and write files: in a page the host is
// a stream's reader (`dsp::disk_web`), and nothing serves it while one
// synchronous call runs. This drives the engine's `OfflineRender` instead, a
// run of frames at a time:
//
//  - the files the score reads (its joins' and its `DiskIn`s') are opened
//    first, and read synchronously through two functions this module defines
//    on `globalThis` for the render -- so an input stream reads its next span
//    the moment it needs it, inside the block, as a native render's streams
//    wait for their threads, and what is rendered is what a native render
//    renders;
//  - each run comes back and is written where the file's body ends, its header
//    written again at the end, so only the run is held;
//  - between two runs the outputs (`DiskOut`, `PeaksOut`) are drained and
//    written, as the live engine's are.
//
// Two places run it: a dedicated Worker in a page, where the page's files
// (OPFS) are read and written synchronously through sync access handles, and
// node, where the disk is read and written through `fs`. Each gives the run
// the files it reaches ([`RenderFiles`]); the render is the same.

import { soundFraming } from "./codec.ts";
import { prepareFaust, type EngineModule } from "./render.ts";

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
 * lets a stream read inside the block that needs it.
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

/** Frames per run: under the slack an input stream's ring keeps, and short
 *  enough that a progress moves. */
const RUN_FRAMES = 4096;

/** A `DiskOut` being drained into its file. */
interface Recording {
    path: string;
    format: string;
    bytes: number;
}

/** A `PeaksOut` being drained, its base buckets kept to the end. */
interface Summary {
    path: string;
    runs: Float32Array[];
}

type Row = {
    id: number;
    direction: "in" | "out" | "list" | "peaks";
    path: string;
    channels: number;
    looping: boolean;
    start: number;
    format: string;
    samples: number;
};

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
    const framing = await soundFraming();
    await prepareFaust(engine, request.score);
    for (const path of JSON.parse(engine.scoreFiles(request.score)) as string[]) {
        await files.openRead(path);
    }
    // An empty path keeps nothing of the output: the render's work is what
    // its synths write (a `PeaksOut` measuring a file).
    const keep = request.path !== "";
    if (keep) await files.create(request.path);
    const scope = globalThis as unknown as Record<string, unknown>;
    scope.claustersRenderFileSize = (path: string) => files.size(path);
    scope.claustersRenderFileRead = (path: string, at: number, length: number) =>
        files.read(path, at, length);
    const recordings = new Map<number, Recording>();
    const summaries = new Map<number, Summary>();
    let render: InstanceType<EngineModule["OfflineRender"]> | null = null;
    try {
        render = new engine.OfflineRender(
            request.score,
            request.sampleRate,
            request.channels,
            request.seed,
        );
        const head = framing.header(request.path, request.channels, request.sampleRate,
            request.sampleFormat, 0).byteLength;
        let body = 0;
        for (;;) {
            const run = render.step(RUN_FRAMES);
            if (keep && run.length > 0) {
                const bytes = framing.body(run, request.sampleFormat);
                files.write(request.path, head + body, bytes);
                body += bytes.byteLength;
            }
            await drain(render, files, framing, recordings, summaries, request.sampleRate);
            onProgress?.(render.framesDone(), render.totalFrames());
            if (render.done()) break;
        }
        const stats = JSON.parse(render.finish()) as {
            frames: number; events: number; seed: string; peak: number[]; rms: number[]; log: string;
        };
        // What the synths that just went left behind.
        await drain(render, files, framing, recordings, summaries, request.sampleRate);
        if (keep) {
            files.write(request.path, 0, framing.header(request.path, request.channels,
                request.sampleRate, request.sampleFormat, body));
        }
        return {
            frames: stats.frames,
            events: stats.events,
            seed: BigInt(stats.seed),
            peak: stats.peak,
            rms: stats.rms,
            log: stats.log,
        };
    } finally {
        render?.free();
        delete scope.claustersRenderFileSize;
        delete scope.claustersRenderFileRead;
        await files.close();
    }
}

/**
 * Empties the render's outputs into their files: each `DiskOut`'s run where
 * its body ends with its header after it, each `PeaksOut`'s buckets kept and
 * its file written once its synth is gone.
 */
async function drain(
    render: InstanceType<EngineModule["OfflineRender"]>,
    files: RenderFiles,
    framing: Awaited<ReturnType<typeof soundFraming>>,
    recordings: Map<number, Recording>,
    summaries: Map<number, Summary>,
    rate: number,
): Promise<void> {
    const rows = JSON.parse(render.diskPoll()) as Row[];
    for (const row of rows) {
        if (row.direction === "out") {
            let take = recordings.get(row.id);
            if (take === undefined) {
                await files.create(row.path);
                take = { path: row.path, format: row.format || "int16", bytes: 0 };
                recordings.set(row.id, take);
            }
            const head = framing.header(take.path, 1, rate, take.format, 0).byteLength;
            if (row.samples > 0) {
                const bytes = framing.body(render.diskPull(row.id, row.samples), take.format);
                files.write(take.path, head + take.bytes, bytes);
                take.bytes += bytes.byteLength;
            }
            files.write(take.path, 0,
                framing.header(take.path, 1, rate, take.format, take.bytes));
            if (row.looping) {
                render.diskRelease(row.id);
                recordings.delete(row.id);
            }
        } else if (row.direction === "peaks") {
            let summary = summaries.get(row.id);
            if (summary === undefined) {
                summary = { path: row.path, runs: [] };
                summaries.set(row.id, summary);
            }
            if (row.samples > 0) summary.runs.push(render.diskPull(row.id, row.samples));
            if (!row.looping) continue;
            // The synth is gone and its buckets are all here: the file, as a
            // native `PeaksOut` writes it, unless a bucket was lost.
            render.diskRelease(row.id);
            summaries.delete(row.id);
            if (row.format === "lost" || row.start === 0) continue;
            const total = summary.runs.reduce((n, run) => n + run.length, 0);
            const stats = new Float32Array(total);
            let at = 0;
            for (const run of summary.runs) {
                stats.set(run, at);
                at += run.length;
            }
            await files.create(summary.path);
            files.write(summary.path, 0, framing.peaks(row.start, row.channels, stats));
        }
    }
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
