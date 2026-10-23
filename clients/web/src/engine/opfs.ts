// The page's own filesystem: the origin private file system (OPFS).
//
// A tab has one, and it is where a soundfile a page reads actually lives --
// `/buffer_allocRead "take.wav"` in a window names the server's filesystem, and
// this is the only thing a tab has to mean by it. The path is resolved under
// the origin's root directory, `/`-separated, with no `..` and no escape: there
// is nothing above the root to reach.
//
// **Everything here runs in a dedicated Worker, and has to.** The synchronous
// access handle (`createSyncAccessHandle`) is restricted to dedicated workers
// by the File System standard, precisely so nobody blocks the main thread with
// it, and the AudioWorklet reaches no storage at all -- its scope has no
// `navigator`, the same minimality that already cost the engine a `TextDecoder`
// shim. So the filesystem lives where the NRT worker is, which is the same
// division the native server makes: file work belongs to the thread that owes
// neither audio nor the interface.

/** Splits a `/`-separated path, rejecting anything that could leave the root. */
function parts(path: string): string[] {
    const out = path.split("/").filter((p) => p.length > 0 && p !== ".");
    if (out.some((p) => p === "..")) {
        throw new Error(`${path}: a path may not climb out of the origin's root`);
    }
    if (out.length === 0) throw new Error("an empty path names no file");
    return out;
}

/** The directory holding `path`'s file, creating it when asked to. */
async function parentOf(
    path: string,
    create: boolean,
): Promise<[FileSystemDirectoryHandle, string]> {
    const segments = parts(path);
    const name = segments.pop() as string;
    let dir = await navigator.storage.getDirectory();
    for (const segment of segments) {
        dir = await dir.getDirectoryHandle(segment, { create });
    }
    return [dir, name];
}

/** The whole file's bytes. Throws if it is not there. */
export async function readFile(path: string): Promise<Uint8Array> {
    const [dir, name] = await parentOf(path, false);
    const handle = await dir.getFileHandle(name);
    const file = await handle.getFile();
    return new Uint8Array(await file.arrayBuffer());
}

/**
 * **Moves the file at `from` to `to`**, over whatever is there: the handle's
 * own `move` where the browser has one, else a copy and a removal, which a
 * page's files of an editor's working folder can afford.
 */
export async function moveFile(from: string, to: string): Promise<void> {
    const [source, name] = await parentOf(from, false);
    const handle = await source.getFileHandle(name);
    const [target, toName] = await parentOf(to, true);
    const movable = handle as FileSystemFileHandle & {
        move?: (dir: FileSystemDirectoryHandle, name: string) => Promise<void>;
    };
    if (typeof movable.move === "function") {
        try {
            await target.removeEntry(toName);
        } catch {
            // nothing there to replace
        }
        await movable.move(target, toName);
        return;
    }
    const bytes = new Uint8Array(await (await handle.getFile()).arrayBuffer());
    await writeFile(to, bytes);
    await source.removeEntry(name);
}

/** **Removes the file at `path`**; one that is not there is no error. */
export async function removeFile(path: string): Promise<void> {
    try {
        const [dir, name] = await parentOf(path, false);
        await dir.removeEntry(name);
    } catch (error) {
        if ((error as DOMException)?.name !== "NotFoundError") throw error;
    }
}

/**
 * **Removes directory `path`**: with `recursive`, and what it holds; without,
 * only when it is empty, and a directory that still holds something is left
 * as it is. One that is not there is no error.
 */
export async function removeDirectory(path: string, recursive: boolean): Promise<void> {
    try {
        const [dir, name] = await parentOf(path, false);
        await dir.removeEntry(name, { recursive });
    } catch (error) {
        const kind = (error as DOMException)?.name;
        if (kind !== "NotFoundError" && kind !== "InvalidModificationError") throw error;
    }
}

/**
 * **Makes directory `path`** and every one above it, as a script's
 * `mkdir -p` does on a disk: a directory already there is no error.
 */
export async function makeDirectory(path: string): Promise<void> {
    let dir = await navigator.storage.getDirectory();
    for (const segment of parts(path)) {
        dir = await dir.getDirectoryHandle(segment, { create: true });
    }
}

/**
 * Writes `bytes` as `path`, replacing whatever was there. A directory that is
 * not there fails the write, as a disk's does, for every writer: the server's
 * own (`/buffer_write`, a recording) and the client's (`SoundFile.write`,
 * `render({ path })`), so a page makes its directories with
 * {@link makeDirectory} where a script makes them with its standard library.
 *
 * Two ways in, because the platform has two. In a dedicated Worker the
 * synchronous access handle is the one that is everywhere; on the main thread
 * it does not exist at all (by standard, so nobody freezes a page with it) and
 * the writable stream stands in -- which is the pair that is *not* everywhere,
 * WebKit having been late to it. Whichever is present is used, and if neither
 * is, that is said rather than guessed at.
 */
export async function writeFile(path: string, bytes: Uint8Array<ArrayBuffer>): Promise<void> {
    let dir: FileSystemDirectoryHandle;
    let name: string;
    try {
        [dir, name] = await parentOf(path, false);
    } catch (error) {
        if ((error as { name?: string }).name !== "NotFoundError") throw error;
        throw new Error(`${path}: no such directory`);
    }
    const handle = await dir.getFileHandle(name, { create: true });
    if (typeof handle.createSyncAccessHandle === "function") {
        const access = await handle.createSyncAccessHandle();
        try {
            access.truncate(0);
            access.write(bytes, { at: 0 });
            access.flush();
        } finally {
            access.close();
        }
        return;
    }
    if (typeof handle.createWritable === "function") {
        const stream = await handle.createWritable();
        await stream.write(bytes);
        await stream.close();
        return;
    }
    throw new Error(
        `${path}: this context can neither open a sync access handle (a ` +
            `dedicated Worker can) nor a writable stream`,
    );
}

/**
 * **Writes each run at its own offset** into `path`, creating the file, and
 * emptying it first when `fresh`: how a file of any length is written a run
 * at a time, with only the run in memory. A gap a run leaves before it reads
 * as zeros. The handle is closed again before this returns, so the file is
 * free for a reader between two writes: a sync access handle holds the file
 * for as long as it is open.
 */
export async function writeAt(
    path: string,
    runs: { at: number; bytes: Uint8Array }[],
    fresh: boolean,
): Promise<void> {
    let dir: FileSystemDirectoryHandle;
    let name: string;
    try {
        [dir, name] = await parentOf(path, false);
    } catch (error) {
        if ((error as { name?: string }).name !== "NotFoundError") throw error;
        throw new Error(`${path}: no such directory`);
    }
    const handle = await dir.getFileHandle(name, { create: true });
    if (typeof handle.createSyncAccessHandle === "function") {
        const access = await handle.createSyncAccessHandle();
        try {
            if (fresh) access.truncate(0);
            for (const { at, bytes } of runs) access.write(bytes as Uint8Array<ArrayBuffer>, { at });
            access.flush();
        } finally {
            access.close();
        }
        return;
    }
    if (typeof handle.createWritable === "function") {
        const stream = await handle.createWritable({ keepExistingData: !fresh });
        for (const { at, bytes } of runs) {
            await stream.write({ type: "write", position: at, data: bytes as Uint8Array<ArrayBuffer> });
        }
        await stream.close();
        return;
    }
    throw new Error(
        `${path}: this context can neither open a sync access handle (a ` +
            `dedicated Worker can) nor a writable stream`,
    );
}

/**
 * A byte range of a file, for reading it a part at a time. `length` past the
 * end simply returns what is there -- a short read is the end of the file, not
 * an error.
 *
 * A slice of the `File` reads only the range, which is what makes a stream
 * possible at all: a whole-file read would put the file in memory, which is
 * the thing streaming exists not to do.
 */
export async function readRange(
    path: string,
    at: number,
    length: number,
): Promise<Uint8Array<ArrayBuffer>> {
    const [dir, name] = await parentOf(path, false);
    const handle = await dir.getFileHandle(name);
    // A `File` slices without reading the whole of it, and takes no lock: a
    // sync access handle would hold the file for as long as it is open, and
    // an offline render reading the same take holds one for the whole render.
    const file = await handle.getFile();
    return new Uint8Array(await file.slice(at, at + length).arrayBuffer());
}

/** How many bytes `path` holds. */
export async function sizeOf(path: string): Promise<number> {
    const [dir, name] = await parentOf(path, false);
    const handle = await dir.getFileHandle(name);
    return (await handle.getFile()).size;
}

/** Whether `path` names a file that exists. */
export async function exists(path: string): Promise<boolean> {
    try {
        const [dir, name] = await parentOf(path, false);
        await dir.getFileHandle(name);
        return true;
    } catch {
        return false;
    }
}

/** The file's extension, lowercased and without the dot -- the decoder's format
 *  hint. Empty when there is none, which still probes by content. */
export function extensionOf(path: string): string {
    const name = path.slice(path.lastIndexOf("/") + 1);
    const dot = name.lastIndexOf(".");
    return dot <= 0 ? "" : name.slice(dot + 1).toLowerCase();
}

/**
 * **Keeps the `keep` newest files of a directory** and removes the rest,
 * oldest first: the bound of a cache that would otherwise grow for as long as
 * the origin lives. A directory that is not there has nothing to trim.
 */
export async function trimDirectory(path: string, keep: number): Promise<void> {
    let dir: FileSystemDirectoryHandle;
    try {
        dir = await navigator.storage.getDirectory();
        for (const segment of parts(path)) dir = await dir.getDirectoryHandle(segment);
    } catch {
        return;
    }
    const files: { name: string; at: number }[] = [];
    const listing = (dir as unknown as {
        entries(): AsyncIterable<[string, FileSystemHandle]>;
    }).entries();
    for await (const [name, handle] of listing) {
        if (handle.kind !== "file") continue;
        const file = await (handle as FileSystemFileHandle).getFile();
        files.push({ name, at: file.lastModified });
    }
    if (files.length <= keep) return;
    files.sort((a, b) => a.at - b.at);
    for (const { name } of files.slice(0, files.length - keep)) await dir.removeEntry(name);
}
