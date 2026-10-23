// A file by its path, wherever this client runs: the disk under node, the
// page's own storage (the origin private file system, `opfs`) in a tab.
//
// One door for both, because a path a caller names means the same thing to
// every verb that takes one -- `render`'s output, a `SoundFile`, a saved
// session -- and the server's `/buffer_allocRead` in a tab already reads that
// same storage.

import * as opfs from "../engine/opfs.ts";

/** Whether this is node rather than a browser (a real `process.versions.node`). */
export function underNode(): boolean {
    const proc = (globalThis as { process?: { versions?: { node?: string } } }).process;
    return typeof proc?.versions?.node === "string";
}

/** A file's bytes: from the disk under node, from the page's storage in a tab. */
export async function readFileAt(path: string): Promise<Uint8Array> {
    if (underNode()) {
        const { readFile } = await import("node:fs/promises");
        return new Uint8Array(await readFile(path));
    }
    return opfs.readFile(path);
}

/**
 * Writes a file's bytes where {@link readFileAt} reads them. A directory that
 * is not there refuses the write, on the disk and in the page's storage alike:
 * {@link makeDirectoryAt} makes one.
 */
export async function writeFileAt(path: string, bytes: Uint8Array<ArrayBuffer>): Promise<void> {
    if (underNode()) {
        const { writeFile } = await import("node:fs/promises");
        await writeFile(path, bytes);
        return;
    }
    await opfs.writeFile(path, bytes);
}

/**
 * **Makes directory `path`** and every one above it where {@link readFileAt}
 * reads: on the disk under node, in the page's storage in a tab.
 */
export async function makeDirectoryAt(path: string): Promise<void> {
    if (underNode()) {
        const { mkdir } = await import("node:fs/promises");
        await mkdir(path, { recursive: true });
        return;
    }
    await opfs.makeDirectory(path);
}

/**
 * **A file as it is**, where {@link readFileAt} reads it: when it was last
 * modified, in milliseconds since the epoch, and its size in bytes -- or
 * `null` for one that is not there.
 */
export async function statAt(path: string): Promise<{ modified: number; size: number } | null> {
    try {
        if (underNode()) {
            const { stat } = await import("node:fs/promises");
            const st = await stat(path);
            return { modified: st.mtimeMs, size: st.size };
        }
        return await opfs.stat(path);
    } catch {
        return null;
    }
}

/**
 * **Moves the file at `from` to `to`**, over whatever is there: on the disk
 * under node, in the page's storage in a tab.
 */
export async function moveFileAt(from: string, to: string): Promise<void> {
    if (underNode()) {
        const { rename } = await import("node:fs/promises");
        await rename(from, to);
        return;
    }
    await opfs.moveFile(from, to);
}

/** **Removes the file at `path`**; one that is not there is no error. */
export async function removeFileAt(path: string): Promise<void> {
    if (underNode()) {
        const { rm } = await import("node:fs/promises");
        await rm(path, { force: true });
        return;
    }
    await opfs.removeFile(path);
}

/**
 * **Removes directory `path`**: with `recursive`, and what it holds; without,
 * only when it is empty -- one that still holds something is left as it is.
 */
export async function removeDirectoryAt(path: string, { recursive = false } = {}): Promise<void> {
    if (underNode()) {
        const { rm, rmdir } = await import("node:fs/promises");
        try {
            if (recursive) await rm(path, { recursive: true, force: true });
            else await rmdir(path);
        } catch {
            // not empty, or not there
        }
        return;
    }
    await opfs.removeDirectory(path, recursive);
}

/**
 * **A folder for a client's temporary files, named `name`**: under the
 * system's temporary directory under node, at the root of the page's storage
 * in a tab. Not made: {@link makeDirectoryAt} makes it.
 */
export function temporaryFolder(name: string): string {
    if (underNode()) {
        const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
        const base = (env.TMPDIR || env.TEMP || env.TMP || "/tmp").replace(/\/+$/, "");
        return `${base}/${name}-${Date.now().toString(36)}`;
    }
    return `${name}-${Date.now().toString(36)}`;
}
