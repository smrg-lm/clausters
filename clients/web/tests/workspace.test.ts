// An editor's working folder under node: made under the user's cache, held
// by a lock file naming its process, removed by its close, and swept by the
// next one where its process ended: the same rules as the core's
// `clausters_core::workspace`, whose root and lock files it shares.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { nodeRoot, sweepNode, Workspace } from "../src/base/workspace.ts";

async function inRoot(body: (root: string) => Promise<void>): Promise<void> {
    const root = await mkdtemp(join(tmpdir(), "clausters-work-"));
    const before = process.env.CLAUSTERS_CACHE_DIR;
    process.env.CLAUSTERS_CACHE_DIR = root;
    try {
        await body(root);
    } finally {
        if (before === undefined) delete process.env.CLAUSTERS_CACHE_DIR;
        else process.env.CLAUSTERS_CACHE_DIR = before;
        await rm(root, { recursive: true, force: true });
    }
}

/** `path` as old as a sweep needs. */
async function aged(path: string): Promise<void> {
    const old = new Date(Date.now() - 120_000);
    await utimes(path, old, old);
}

/** A process id no process has any longer: a child's, once it has exited. */
function deadPid(): number {
    return spawnSync(process.execPath, ["-e", ""]).pid as number;
}

test("the root is the core's", async () => {
    await inRoot(async (root) => {
        assert.equal(nodeRoot(), root);
    });
});

test("a working folder is held by its process and goes with its close", async () => {
    await inRoot(async (root) => {
        const ws = new Workspace("audio");
        assert.ok(ws.path.startsWith(root));
        await ws.ready;
        assert.ok(existsSync(ws.path));
        const lock = `${ws.path}.lock`;
        assert.equal((await readFile(lock, "utf8")).trim(), String(process.pid));
        assert.equal(await ws.close(), true);
        assert.ok(!existsSync(ws.path));
        assert.ok(!existsSync(lock), "its lock goes too");
    });
});

test("a folder still holding something stays held", async () => {
    await inRoot(async () => {
        const ws = new Workspace("audio");
        await ws.ready;
        await writeFile(join(ws.path, "copied.w64"), "samples");
        assert.equal(await ws.close(), false);
        assert.ok(existsSync(ws.path), "what the clipboard names is kept");
        assert.ok(existsSync(`${ws.path}.lock`), "and held");
        await rm(join(ws.path, "copied.w64"));
        assert.equal(await ws.close(), true);
    });
});

test("a sweep takes what no process holds and leaves what one does", async () => {
    await inRoot(async (root) => {
        const left = async (name: string, pid: string, old = true) => {
            const dir = join(root, name);
            await mkdir(dir);
            await writeFile(join(dir, "reversed.w64"), "samples");
            await writeFile(`${dir}.lock`, pid);
            if (old) await aged(`${dir}.lock`);
            return dir;
        };
        const dead = await left("audio-1-0", `${deadPid()}\n`);
        const live = await left("audio-2-0", `${process.pid}\n`);
        // a lock that names no process is a native session's to judge
        const unnamed = await left("audio-3-0", "");
        // a lock just made is a session starting
        const young = await left("audio-4-0", `${deadPid()}\n`, false);
        // a settled folder whose lock went before it did is nobody's
        const orphan = join(root, "audio-5-0");
        await mkdir(orphan);
        await aged(orphan);
        assert.equal(await sweepNode(root), 2);
        assert.ok(!existsSync(dead) && !existsSync(`${dead}.lock`), "the dead process's folder went");
        assert.ok(!existsSync(orphan));
        for (const kept of [live, unnamed, young]) assert.ok(existsSync(kept), kept);
        // a new folder sweeps on its way in
        await aged(`${young}.lock`);
        const ws = new Workspace("audio");
        await ws.ready;
        assert.ok(!existsSync(young));
        await ws.close();
    });
});
