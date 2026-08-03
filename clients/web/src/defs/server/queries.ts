// What a running server holds (mirrors `clausters/defs/server/queries.py`).
//
// The introspection round trips: the def table, the allocated buffers, the
// UGen catalogue, the server's own configuration and the node tree. Each one
// asks and awaits — they report the state of the server that is actually
// running, which is not necessarily the one this client set up (the def store
// persists across restarts, and other clients share the tree).
//
// A mixin, composed into `Server` beside `ServerStreams`, so no attribute path
// moves: `server.queryTree(...)` is the same call it was. Its methods are
// typed on `this: Server`, which is how they reach the request machinery that
// stays on the handle itself.

import { CommandError } from "../../errors.ts";
import type { MsgArg } from "../../base/osc.ts";
import {
    Tree,
    parseBufferList,
    parseDefInfo,
    parseQueryTree,
    parseUgenInfo,
} from "../info.ts";
import type { BufferInfo, DefInfo, UgenInfo } from "../info.ts";
import { Group, ROOT_NODE_ID, nodeId } from "../node.ts";
import type { NodeLike } from "../node.ts";
import {
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_GRAPH_CHILDREN,
    DEFAULT_MAX_NODES,
    DEFAULT_MAX_UGEN_INPUTS,
} from "./options.ts";
import type { ServerInfo } from "./options.ts";
import type { Server } from "./index.ts";

/** The introspection queries. Composed into `Server`; never used alone. */
export class ServerQueries {
    /**
     * The defs the server holds, each with its control surface (`/def_query`,
     * answered by one `/def_query.reply` per def). With `names`, details exactly
     * those — an unknown one comes back with an empty `family` rather than
     * failing; with none, every loaded def of every family.
     *
     * The def store persists across restarts, so a server may well hold defs
     * this client never sent: this is how you find out.
     */
    async queryDefs(this: Server, names: string[] = [], timeout?: number): Promise<DefInfo[]> {
        const replies = await this.requestBatch("/def_query", names, {
            reply: "/def_query.reply",
            timeout,
        });
        return replies.map((msg) => parseDefInfo(msg.args));
    }

    /**
     * Every **allocated** buffer with its shape (an argument-less
     * `/buffer_query`). Like `queryDefs`, this reports what the server holds
     * rather than what this client allocated.
     */
    async queryBuffers(this: Server, timeout?: number): Promise<BufferInfo[]> {
        const msg = await this.request("/buffer_query", [], {
            expect: ["/buffer_query.reply"],
            timeout,
        });
        return parseBufferList(msg.args);
    }

    /**
     * The server's UGen catalog (`/ugen_query`, answered by one `/ugen_query.reply` per
     * kind): every kind with its named inputs, defaults and rate rules, or
     * just `kinds` if given.
     *
     * This is the catalog **this** server was built with, which is why it is
     * worth asking instead of assuming: a build without the `synth` feature
     * has no UGens at all and returns an empty list (its defs would all be
     * FaustDefs, whose box vocabulary is Faust's own and lives client-side).
     */
    async queryUgens(this: Server, kinds: string[] = [], timeout?: number): Promise<UgenInfo[]> {
        const replies = await this.requestBatch("/ugen_query", kinds, {
            reply: "/ugen_query.reply",
            timeout,
        });
        return replies.map((msg) => parseUgenInfo(msg.args));
    }

    /**
     * The server's static configuration: bus counts, output/input channels,
     * block size, sample rate and the boot-time pool sizes. The appended
     * capacity fields degrade to the compiled defaults against a server too
     * old to report them.
     */
    async queryInfo(this: Server, timeout?: number): Promise<ServerInfo> {
        const msg = await this.request("/server_query", [], {
            expect: ["/server_query.reply"],
            timeout,
        });
        const a = msg.args;
        const at = (i: number, fallback: number) =>
            i < a.length ? Number(a[i]) : fallback;
        return {
            audioBuses: Number(a[0]),
            controlBuses: Number(a[1]),
            channels: Number(a[2]),
            blockSize: Number(a[3]),
            nominalSampleRate: Number(a[4]),
            actualSampleRate: Number(a[5]),
            inputChannels: at(6, 0),
            maxNodes: at(7, DEFAULT_MAX_NODES),
            maxBuffers: at(8, DEFAULT_MAX_BUFFERS),
            maxGraphChildren: at(9, DEFAULT_MAX_GRAPH_CHILDREN),
            maxUgenInputs: at(10, DEFAULT_MAX_UGEN_INPUTS),
            taps: at(11, 0),
            tapFrames: at(12, 0),
            maxFrame: at(13, 65536),
        };
    }

    /**
     * The node tree from `group` down (`/group_queryTree`) as a `Tree`: every
     * entry is the same `NodeInfo` that `Node.info` returns, so reading a
     * subtree needs no follow-up query. `String(tree)` draws it indented.
     */
    async queryTree(this: Server, group: NodeLike = ROOT_NODE_ID, timeout?: number): Promise<Tree> {
        const msg = await this.request(
            "/group_queryTree",
            [["i", nodeId(group)], ["i", 2]] as MsgArg[],
            { expect: ["/group_queryTree.reply"], timeout },
        );
        return parseQueryTree(msg.args);
    }

    /**
     * The group a path names (`/group_query`), or `undefined` when nothing
     * answers to it.
     *
     * A path is the group names from the root down, `/mixer/drums`; a group
     * with no name contributes its id instead (`/1000/drums`), so every group
     * is reachable whether it was labelled or not. Resolve once and keep the
     * handle: the id is the identity, the path is how you found it, and a group
     * that is renamed or freed leaves the handle pointing at the id it resolved
     * to.
     */
    async groupAt(this: Server, path: string, timeout?: number): Promise<Group | undefined> {
        const msg = await this.request("/group_query", [path], {
            expect: ["/group_query.reply", "/fail"],
            timeout,
        });
        if (msg.addr === "/fail") {
            throw new CommandError(`/group_query failed: ${msg.args.join(" ")}`);
        }
        const id = Number(msg.args[1]);
        return id >= 0 ? new Group(id, this) : undefined;
    }

    /**
     * The server's rendered node graph as text (`/group_dumpGraph`) — a
     * debugging aid; for machine use prefer `queryTree`.
     */
    async dumpGraph(this: Server, group: NodeLike = ROOT_NODE_ID, timeout?: number): Promise<string> {
        const msg = await this.request(
            "/group_dumpGraph",
            [["i", nodeId(group)]] as MsgArg[],
            { expect: ["/group_dumpGraph.reply", "/fail"], timeout },
        );
        if (msg.addr === "/fail") {
            throw new CommandError(`/group_dumpGraph failed: ${msg.args.join(" ")}`);
        }
        return String(msg.args[1]);
    }
}
