// What the server's MIDI plays: its bindings, and MIDI 2.0 packets sent in
// (mirrors `clausters/defs/server/midi.py`).
//
// A **binding** says which instrument a MIDI channel plays and how its
// messages reach that instrument's controls. It is the server's, not this
// handle's: the same binding hears the server's live MIDI input (`--midi`) and
// the MIDI messages a transport's event lane plays (`laneSet`'s `midi` and
// `ump` lists), and a server with a data directory keeps it across restarts.
// So a binding is made once, wherever the notes come from, and `midiQuery`
// reads back what a server already has.
//
// `midiUmp` is the other half: MIDI 2.0 packets played now, as the live input
// plays its messages.
//
// A mixin, composed into `Server` beside `ServerQueries`, `ServerStreams` and
// `ServerTransport`, so no attribute path moves.

import type { MsgArg } from "../../base/osc.ts";
import { AddAction, ROOT_NODE_ID, nodeId } from "../node.ts";
import type { NodeLike } from "../node.ts";
import type { Server } from "./index.ts";

/**
 * One binding, as {@link ServerMidi.midiQuery} reads it back.
 *
 * `kind` is `"channel"` for a binding of one channel and `"zone"` for an MPE
 * zone, whose `channel` is its master and whose `members` is how many member
 * channels the device last said it has (0 for a channel). `controls` maps each
 * selector that names a control -- `note`, `vel` and `gate` always, then
 * `bend`, `pressure`, `poly`, `timbre`, `lift`, `ccN` and `progN` where mapped
 * -- to the name it drives (a `progN` names an instrument). `timbreCc` is a
 * zone's timbre controller, `null` on a channel.
 */
export interface MidiBinding {
    kind: string;
    channel: number;
    members: number;
    instrument: string;
    target: number;
    action: number;
    gate: boolean;
    controls: Record<string, string>;
    timbreCc: number | null;
}

/** Where a binding's voices go and how they end: `midiBind`'s options. */
export interface MidiBindOptions {
    /** The node its voices are added against: the root group unless given. */
    target?: NodeLike;
    /** How: `AddAction.HEAD` unless given. */
    action?: AddAction;
    /** A note-off sets the voice's `gate` to 0 rather than freeing it. */
    gate?: boolean;
}

/** A binding as one line: where, the instrument, and what each selector drives. */
export function formatMidiBinding(binding: MidiBinding): string {
    const where = binding.kind === "zone"
        ? `zone on ${binding.channel} (${binding.members} members)`
        : `channel ${binding.channel}`;
    const mapped = Object.entries(binding.controls)
        .map(([selector, name]) => `${selector} -> ${name}`)
        .join(", ");
    return `${where}: ${binding.instrument} [${mapped}]`;
}

/**
 * One `/midi_query.reply`: `kind channel members instrument target addAction
 * gate`, then `selector name` pairs and, on a zone, a trailing `timbreCc
 * <int>`. A `"none"` reply -- a channel asked for and bound to nothing -- is
 * `null`.
 */
export function parseMidiBinding(args: readonly unknown[]): MidiBinding | null {
    if (String(args[0]) === "none") return null;
    const binding: MidiBinding = {
        kind: String(args[0]),
        channel: Number(args[1]),
        members: Number(args[2]),
        instrument: String(args[3]),
        target: Number(args[4]),
        action: Number(args[5]),
        gate: Number(args[6]) !== 0,
        controls: {},
        timbreCc: null,
    };
    for (let i = 7; i + 1 < args.length; i += 2) {
        const selector = String(args[i]);
        if (selector === "timbreCc") binding.timbreCc = Number(args[i + 1]);
        else binding.controls[selector] = String(args[i + 1]);
    }
    return binding;
}

/** A packet's 32-bit word as the signed int32 OSC carries, the same bits. */
function int32(word: number): number {
    return Math.trunc(word) | 0;
}

/**
 * The MIDI half of `Server`. Composed into `Server`; never used alone.
 *
 * The bindings are sent and not awaited, like `Node.map`: the server takes
 * them in order, so a lane set after a bind plays through it, and one it
 * refuses -- an instrument it does not hold, a channel a zone covers --
 * answers `/fail`.
 */
export class ServerMidi {
    /**
     * Binds MIDI `channel` to the def named `instrument` (`/midi_bind`): each
     * note on it makes a node of that def at `target` by `action`, its note
     * number on `freq` and its velocity on `amp` until `midiMap` says
     * otherwise. A SynthDef, a FaustDef and a GraphDef are bound alike (a
     * GraphDef's shared members are made here, and each note is a voice of
     * them). `channel` is 0-255: the 16 MIDI 1.0 channels and the extended
     * MIDI 2.0 group x channel space.
     *
     * A note-off frees the node, or, with `gate`, sets its `gate` control to
     * 0, for a def whose envelope releases. The binding reaches both the live
     * input and a transport lane's MIDI messages; binding a channel again
     * replaces it.
     */
    midiBind(this: Server, channel: number, instrument: string, options: MidiBindOptions = {}): Server {
        const { target = ROOT_NODE_ID, action = AddAction.HEAD, gate = false } = options;
        this.sendMsg(
            "/midi_bind",
            ["i", Math.trunc(channel)],
            ["s", String(instrument)],
            ["i", nodeId(target)],
            ["i", action],
            ["i", gate ? 1 : 0],
        );
        return this;
    }

    /**
     * Binds an **MPE zone** to `instrument` (`/midi_bindZone`): the lower zone
     * on master channel 0 (members ascending from 1) or the upper on 15
     * (descending from 14), `members` member channels, 0 to wait for the
     * device's own layout -- which wins when it arrives. Each note is a voice
     * whose own channel's bend, pressure and timbre are its own: a voice
     * starts with `freq`, `amp`, `press` (the pressure) and `slide` (the
     * timbre) -- the built-in `default`'s controls -- so a def with those
     * controls plays all three with no `midiMap`. Refused when the zone would
     * reach a channel `midiBind` holds.
     */
    midiBindZone(
        this: Server,
        master: number,
        members: number,
        instrument: string,
        options: MidiBindOptions = {},
    ): Server {
        const { target = ROOT_NODE_ID, action = AddAction.HEAD, gate = false } = options;
        this.sendMsg(
            "/midi_bindZone",
            ["i", Math.trunc(master)],
            ["i", Math.trunc(members)],
            ["s", String(instrument)],
            ["i", nodeId(target)],
            ["i", action],
            ["i", gate ? 1 : 0],
        );
        return this;
    }

    /**
     * Removes `channel`'s binding (`/midi_unbind`), freeing every voice still
     * sounding on it; on a zone's master, the zone and its voices.
     */
    midiUnbind(this: Server, channel: number): Server {
        this.sendMsg("/midi_unbind", ["i", Math.trunc(channel)]);
        return this;
    }

    /**
     * Routes one kind of message on `channel` to the control `name`
     * (`/midi_map`). `selector` is `"note"` (the frequency control), `"vel"`
     * (the amplitude), `"gate"` (the gate control), `"bend"`, `"pressure"`
     * (channel aftertouch), `"poly"` (per-note aftertouch, to the note's
     * voice), `"lift"` (note-off velocity), `"ccN"` (control change `N`) or
     * `"progN"` (program `N`, whose `name` is an instrument to switch to); on
     * a zone's master, `"timbre"` too, with `cc` the controller that carries
     * it (74 unless given). On a zone's master the map is the zone's.
     */
    midiMap(this: Server, channel: number, selector: string, name: string, cc?: number): Server {
        const args: MsgArg[] = [["i", Math.trunc(channel)], ["s", String(selector)], ["s", String(name)]];
        if (cc !== undefined) args.push(["i", Math.trunc(cc)]);
        this.sendMsg("/midi_map", ...args);
        return this;
    }

    /**
     * Plays MIDI 2.0 packets now (`/midi_ump`), each number one 32-bit word of
     * a Universal MIDI Packet, as the live input plays its messages: a note,
     * controller, pressure, bend or program change through its channel's
     * binding at its own resolution (a 16-bit velocity, 32-bit values), and a
     * per-note bend or controller to the voice sounding on its channel and
     * key. Any other packet plays nothing.
     */
    midiUmp(this: Server, ...words: number[]): Server {
        this.sendMsg("/midi_ump", ...words.map((w): MsgArg => ["i", int32(w)]));
        return this;
    }

    /**
     * The server's MIDI bindings (`/midi_query`), by channel: every one, or
     * those of `channels`, a channel bound to nothing left out. A zone is
     * listed under its master.
     */
    async midiQuery(this: Server, channels: number[] = [], timeout?: number): Promise<Map<number, MidiBinding>> {
        const replies = await this.requestBatch(
            "/midi_query",
            channels.map((c): MsgArg => ["i", Math.trunc(c)]),
            { reply: "/midi_query.reply", timeout },
        );
        const out = new Map<number, MidiBinding>();
        for (const msg of replies) {
            const binding = parseMidiBinding(msg.args);
            if (binding !== null) out.set(binding.channel, binding);
        }
        return out;
    }
}
