// The page's own filesystem: the origin private file system (OPFS).
//
// A tab has one, and it is where a soundfile a page reads actually lives —
// `/buffer_allocRead "take.wav"` in a window names the server's filesystem, and
// this is the only thing a tab has to mean by it. The path is resolved under
// the origin's root directory, `/`-separated, with no `..` and no escape: there
// is nothing above the root to reach.
//
// **Everything here runs in a dedicated Worker, and has to.** The synchronous
// access handle (`createSyncAccessHandle`) is restricted to dedicated workers
// by the File System standard, precisely so nobody blocks the main thread with
// it, and the AudioWorklet reaches no storage at all — its scope has no
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
 * Writes `bytes` as `path`, replacing whatever was there and creating the
 * directories above it.
 *
 * Two ways in, because the platform has two. In a dedicated Worker the
 * synchronous access handle is the one that is everywhere; on the main thread
 * it does not exist at all (by standard, so nobody freezes a page with it) and
 * the writable stream stands in — which is the pair that is *not* everywhere,
 * WebKit having been late to it. Whichever is present is used, and if neither
 * is, that is said rather than guessed at.
 */
export async function writeFile(
    path: string,
    bytes: Uint8Array<ArrayBuffer>,
): Promise<void> {
    const [dir, name] = await parentOf(path, true);
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

/** The file's extension, lowercased and without the dot — the decoder's format
 *  hint. Empty when there is none, which still probes by content. */
export function extensionOf(path: string): string {
    const name = path.slice(path.lastIndexOf("/") + 1);
    const dot = name.lastIndexOf(".");
    return dot <= 0 ? "" : name.slice(dot + 1).toLowerCase();
}
