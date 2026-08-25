// The client-owned server configuration, and what a running server reports
// (mirrors `clausters/defs/server/options.py`).
//
// `ServerSizing` is what this client's allocators are built against, and
// `ServerInfo` is the answer to `Server.queryInfo`: what the server it is
// talking to was actually built and booted with, which is not always the same
// thing. The `ServerOptions` half of the Python module — every flag a
// *launched* server takes, and the command line it becomes — has no
// counterpart here: a page cannot start a process, so a web client always
// meets a server that is already running.

// Server defaults, mirroring the Rust server's `DEFAULT_AUDIO_BUSES` /
// `DEFAULT_CONTROL_BUSES` (128 is the hard audio ceiling) and `--sample-rate`.
// They live here, on the server-configuration module, not in the bus one: how
// many buses exist is the server's property, and these are only the fallback
// when `/server_query` does not answer. The bus allocators carry no defaults
// of their own.
export const DEFAULT_AUDIO_BUSES = 128;
export const DEFAULT_CONTROL_BUSES = 16384;
export const DEFAULT_SAMPLE_RATE = 48000;
// Boot-time pre-allocated pool sizes, mirroring the Rust server's `Limits`
// defaults (`--max-nodes`/`--max-buffers`/`--max-graph-children`/
// `--max-ugen-inputs`). 32 is the hard ceiling on UGen inputs, like 128 for
// audio buses.
export const DEFAULT_MAX_NODES = 8192;
export const DEFAULT_MAX_BUFFERS = 4096;
export const DEFAULT_MAX_GRAPH_CHILDREN = 512;
export const DEFAULT_MAX_UGEN_INPUTS = 32;

/**
 * The server's default ring count (`--taps`), when it reports none.
 *
 * An audio bus is engine memory, one block at a time, so nothing outside the
 * audio thread can see it. The server copies a bus into one of a fixed set of
 * **sample rings** when asked (`/bus_tap bus 1`), which is what lets a scope
 * see the samples of a live signal.
 *
 * **The rings are the server's own bookkeeping.** A client asks for a *bus*
 * and never for a ring: the server picks one, counts watches so two views of a
 * bus share a ring, and frees it when the last one stops. All this client
 * needs from that region is how big it is, to size a sensible default before
 * `/server_query` answers.
 */
export const DEFAULT_TAPS = 8;

/**
 * Frames per audio-tap ring on a server started with the default `--tap-frames`
 * — the other half of `DEFAULT_TAPS`, and what sizes a window over a tap before
 * `/server_query` answers with the real shape.
 */
export const DEFAULT_TAP_FRAMES = 16384;

/**
 * The sizes a client's allocators need. They are a property of the *server*,
 * so `Server.attach`/`Server.boot` read them from `/server_query` rather than
 * guessing (`reconcile`);
 * pass them explicitly to skip that round trip.
 */
export interface ServerSizing {
    audioBuses: number;
    controlBuses: number;
    maxNodes: number;
    maxBuffers: number;
    /**
     * Hardware output channels — the audio buses reserved at the bottom of
     * the space, which the allocator never hands out.
     */
    channels: number;
    /** Audio-tap rings (`--taps`); 0 on a server with no tap region. */
    taps: number;
}

/** The static configuration a running server reports over `/server_query`. */
export interface ServerInfo extends ServerSizing {
    blockSize: number;
    nominalSampleRate: number;
    actualSampleRate: number;
    inputChannels: number;
    maxGraphChildren: number;
    maxUgenInputs: number;
    /** Audio-tap region shape; 0/0 when the server has no segment. */
    taps: number;
    tapFrames: number;
    /** The stream-transport frame ceiling in bytes. */
    maxFrame: number;
    /**
     * How many control buses one `/bus_stream` subscription may list
     * (`--max-stream-buses`), **as it applies to this client's carrier**: the
     * server's configured ceiling clamped by what one reply carries over the
     * transport asking. A subscription is one client's whole live picture — a
     * page of many canvases asks for a bus per meter — so a client that draws
     * a lot reads this rather than assuming. Falls back to the historical 128
     * against a server too old to report it.
     */
    maxStreamBuses: number;
}

/**
 * The server's configuration as the readable block `print` shows in the Python
 * client (`ServerInfo.__str__`) — same fields, same order, same wording.
 *
 * A free function rather than a method for the reason the record formatters in
 * `defs/info.ts` are: `ServerInfo` is an interface, so it carries none.
 */
export function formatServerInfo(info: ServerInfo): string {
    const g = (value: number): string => Number(value.toPrecision(6)).toString();
    const drift = info.actualSampleRate === info.nominalSampleRate
        ? ""
        : ` (nominal ${g(info.nominalSampleRate)})`;
    const taps = info.taps
        ? `${info.taps} x ${info.tapFrames} frames`
        : "none (no segment)";
    return [
        `server ${g(info.actualSampleRate)} Hz${drift}, ` +
            `${info.blockSize}-sample blocks, ` +
            `${info.channels} out / ${info.inputChannels} in`,
        `  buses   ${info.audioBuses} audio, ${info.controlBuses} control`,
        `  limits  ${info.maxNodes} nodes, ${info.maxBuffers} buffers, ` +
            `${info.maxGraphChildren} graph children, ` +
            `${info.maxUgenInputs} ugen inputs`,
        `  taps    ${taps}`,
        `  frame   ${info.maxFrame} bytes max`,
        `  stream  ${info.maxStreamBuses} buses per /bus_stream`,
    ].join("\n");
}
