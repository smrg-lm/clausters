// An editor's working folder: where its renders are written, and gone when
// the session that made it is -- the core's `clausters_core::workspace`, by
// whatever this client can hold a folder with.
//
// A take of a day reversed writes tens of gigabytes, so a folder left behind
// by a session that did not close through its window is no small leftover.
// The core holds a folder with a file lock the operating system lets go of
// however the process ends, and the next editor to open sweeps what nobody
// holds. Neither half of that reaches this client as it is, so each place it
// runs holds a folder with what it has:
//
// - **Under node**, the folder is the core's: the same root, under the user's
//   cache, and the same lock file beside it. Node has no file lock, so the
//   lock file names its process and is touched while the folder is held: a
//   native sweep reads a lock touched lately as a session starting and leaves
//   it, and this one reads a lock whose process is gone as nobody's. A native
//   lock names its process too, so either client sweeps what the other left.
// - **In a tab**, the folder is in the origin's storage and held by a Web
//   Lock, which the browser lets go of when the tab goes however it goes: the
//   next editor to open in the origin removes every folder whose lock no tab
//   holds.

import * as opfs from "../engine/opfs.ts";
import { underNode } from "./files.ts";

/** The extension of the lock file beside a working folder, as the core's. */
const LOCK = "lock";

/**
 * **How old a lock or a folder must be before a sweep judges it**, as the
 * core's: a session makes its lock a moment before its folder, and a sweep in
 * between must not read that moment as a dead session.
 */
const SETTLED_MS = 60_000;

/** How often a held lock is touched under node: well inside {@link SETTLED_MS}. */
const TOUCH_MS = 20_000;

/** The prefix of a tab's Web Lock on a folder, before the folder's name. */
const TAB_LOCK = "clausters-work:";

/** The page's working folders, under the origin's root. */
const TAB_ROOT = "clausters/work";

type Env = Record<string, string | undefined>;

function env(): Env {
    return (globalThis as { process?: { env?: Env } }).process?.env ?? {};
}

/** Whether process `pid` is running: one this process may not signal is. */
function running(pid: number): boolean {
    try {
        process.kill(pid, 0);
        return true;
    } catch (error) {
        return (error as { code?: string }).code === "EPERM";
    }
}

/**
 * **Removes every working folder under `root` no session holds**, as the
 * core's `sweep` does, judged by what node can read: a settled lock whose
 * process has gone takes its folder with it, and a settled folder with no
 * lock beside it is nobody's. A lock that names no process is left to a
 * native sweep, which can try it. Answers how many went.
 */
export async function sweepNode(root: string): Promise<number> {
    const { readdir, readFile, rm, stat } = await import("node:fs/promises");
    const { join } = await import("node:path");
    let entries;
    try {
        entries = await readdir(root, { withFileTypes: true });
    } catch {
        return 0;
    }
    const names = new Set(entries.map((e) => e.name));
    let gone = 0;
    for (const entry of entries) {
        const path = join(root, entry.name);
        try {
            if (Date.now() - (await stat(path)).mtimeMs < SETTLED_MS) continue;
            if (entry.isDirectory()) {
                if (!names.has(`${entry.name}.${LOCK}`)) {
                    await rm(path, { recursive: true, force: true });
                    gone += 1;
                }
                continue;
            }
            if (!entry.name.endsWith(`.${LOCK}`)) continue;
            const pid = Number.parseInt((await readFile(path, "utf8")).trim(), 10);
            if (!(pid > 0) || running(pid)) continue;
            await rm(path.slice(0, -LOCK.length - 1), { recursive: true, force: true });
            await rm(path, { force: true });
            gone += 1;
        } catch {
            // gone already, or not ours to judge
        }
    }
    return gone;
}

/**
 * **Removes every working folder of this origin no tab holds**: one whose Web
 * Lock nobody holds or waits for. Answers how many went; nothing goes where
 * the page has no Web Locks, since then it cannot tell a live tab's folder.
 */
export async function sweepTabs(): Promise<number> {
    const locks = tabLocks();
    if (locks === null) return 0;
    const state = await locks.query();
    const held = new Set([...(state.held ?? []), ...(state.pending ?? [])].map((l) => l.name));
    let gone = 0;
    for (const name of await tabFolders(TAB_ROOT)) {
        if (held.has(TAB_LOCK + name)) continue;
        await opfs.removeDirectory(`${TAB_ROOT}/${name}`, true);
        gone += 1;
    }
    return gone;
}

/** The handle of the origin's directory `path`, or `null` for one not there. */
async function tabDirectory(path: string): Promise<FileSystemDirectoryHandle | null> {
    try {
        let dir = await navigator.storage.getDirectory();
        for (const segment of path.split("/")) dir = await dir.getDirectoryHandle(segment);
        return dir;
    } catch {
        return null;
    }
}

/** The names of the directories the origin's directory `path` holds. */
async function tabFolders(path: string): Promise<string[]> {
    const dir = await tabDirectory(path);
    if (dir === null) return [];
    const names: string[] = [];
    const listing = (dir as unknown as {
        entries(): AsyncIterable<[string, FileSystemHandle]>;
    }).entries();
    for await (const [name, handle] of listing) {
        if (handle.kind === "directory") names.push(name);
    }
    return names;
}

function tabLocks(): LockManager | null {
    return (globalThis as { navigator?: { locks?: LockManager } }).navigator?.locks ?? null;
}

/** The folders this process holds, removed if it exits with them still held. */
const live = new Set<string>();
let exitHooked = false;

let next = 0;

/**
 * **A working folder, held**: made under the working root by {@link ready},
 * after a sweep of the folders nobody holds any longer, and removed by
 * {@link close} -- or, when this session ends without one, by the next sweep.
 */
export class Workspace {
    /** The folder: a path on the disk under node, in the origin's storage in a tab. */
    readonly path: string;
    /** Resolves once the folder is made and held. */
    readonly ready: Promise<void>;
    #release: (() => void) | null = null;

    /**
     * **A working folder named after `name`.** Its path is known at once and
     * the folder is not there until {@link ready} resolves.
     */
    constructor(name: string) {
        const token = `${Date.now().toString(36)}${(next++).toString(36)}`;
        if (underNode()) {
            this.path = `${nodeRoot()}/${name}-${process.pid}-${token}`;
            this.ready = this.#holdOnDisk();
        } else {
            this.path = `${TAB_ROOT}/${name}-${token}`;
            this.ready = this.#holdInTab();
        }
    }

    async #holdOnDisk(): Promise<void> {
        const { mkdir, utimes, writeFile } = await import("node:fs/promises");
        const { dirname } = await import("node:path");
        const root = dirname(this.path);
        await mkdir(root, { recursive: true });
        await sweepNode(root);
        const lock = `${this.path}.${LOCK}`;
        // the lock first, so the folder is never there unheld
        await writeFile(lock, `${process.pid}\n`, { flag: "wx" });
        await mkdir(this.path);
        const touch = setInterval(() => {
            const now = new Date();
            utimes(lock, now, now).catch(() => {});
        }, TOUCH_MS);
        touch.unref?.();
        live.add(this.path);
        hookExit();
        this.#release = () => {
            clearInterval(touch);
            live.delete(this.path);
        };
    }

    async #holdInTab(): Promise<void> {
        const locks = tabLocks();
        if (locks !== null) {
            const leaf = this.path.slice(TAB_ROOT.length + 1);
            await new Promise<void>((granted) => {
                void locks.request(TAB_LOCK + leaf, () => {
                    granted();
                    return new Promise<void>((done) => { this.#release = done; });
                });
            });
            await sweepTabs();
        }
        await opfs.makeDirectory(this.path);
    }

    /**
     * **Removes the folder when it is empty, and lets go of it then.** A folder
     * that still holds something -- the renders a clipboard names -- stays
     * held as long as this session lives, and the next sweep after it takes
     * it. Answers whether it went.
     */
    async close(): Promise<boolean> {
        await this.ready;
        if (underNode()) {
            const { rmdir, rm } = await import("node:fs/promises");
            try {
                await rmdir(this.path);
            } catch {
                return false;
            }
            this.#release?.();
            // the lock goes only with its folder, as the core's
            await rm(`${this.path}.${LOCK}`, { force: true });
        } else {
            await opfs.removeDirectory(this.path, false);
            if ((await tabDirectory(this.path)) !== null) return false;
            this.#release?.();
        }
        this.#release = null;
        return true;
    }
}

/**
 * **Where working folders are made under node**, by the core's rule:
 * `$CLAUSTERS_CACHE_DIR` where it is set, else the platform's cache folder --
 * `%LOCALAPPDATA%` on Windows, `~/Library/Caches` on macOS, `$XDG_CACHE_HOME`
 * or `~/.cache` elsewhere -- under `clausters/work`, and the system's
 * temporary directory only where none of those can be found.
 */
export function nodeRoot(): string {
    const vars = env();
    const sep = process.platform === "win32" ? "\\" : "/";
    const join = (...parts: string[]) => parts.join(sep);
    if (vars.CLAUSTERS_CACHE_DIR) return vars.CLAUSTERS_CACHE_DIR;
    const home = vars.HOME;
    let cache: string | undefined;
    if (process.platform === "win32") cache = vars.LOCALAPPDATA || undefined;
    else if (process.platform === "darwin") cache = home ? join(home, "Library", "Caches") : undefined;
    else cache = vars.XDG_CACHE_HOME || (home ? join(home, ".cache") : undefined);
    if (cache) return join(cache, "clausters", "work");
    const temp = (vars.TMPDIR || vars.TEMP || vars.TMP || "/tmp").replace(/[\\/]+$/, "");
    return join(temp, "clausters-work");
}

/**
 * Removes the held folders when the process exits with them, as dropping the
 * core's `Workspace` does: exit hooks are synchronous, so this reaches for
 * the synchronous half of `node:fs`, loaded with the first folder.
 */
function hookExit(): void {
    if (exitHooked) return;
    exitHooked = true;
    void import("node:fs").then(({ rmSync }) => {
        process.once("exit", () => {
            for (const path of live) {
                try {
                    rmSync(path, { recursive: true, force: true });
                    rmSync(`${path}.${LOCK}`, { force: true });
                } catch {
                    // what could not go is swept later
                }
            }
        });
    });
}
