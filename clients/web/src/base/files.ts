// A file by its path, wherever this client runs: the disk under node, the
// page's own storage (the origin private file system, `opfs`) in a tab.
//
// One door for both, because a path a caller names means the same thing to
// every verb that takes one -- `render`'s output, `readSoundfile`, a saved
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

/** Writes a file's bytes where {@link readFileAt} reads them. */
export async function writeFileAt(path: string, bytes: Uint8Array<ArrayBuffer>): Promise<void> {
    if (underNode()) {
        const { writeFile } = await import("node:fs/promises");
        await writeFile(path, bytes);
        return;
    }
    await opfs.writeFile(path, bytes);
}
