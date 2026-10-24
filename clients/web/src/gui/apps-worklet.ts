/**
 * **The engine of an editing application, in the page's AudioWorklet**: the
 * processing half of the engine the application holds, run block by block
 * on the worklet's own thread over the memory the page shares.
 *
 * The page makes it (`apps.ts`) when the application opens its engine: the
 * options are the compiled module, the shared memory and the engine's place
 * in it. The constructor instantiates the module over that memory, and each
 * render quantum is the engine's blocks, processed in place
 * (`apps_engine_process`) and copied out of the memory into the channels.
 * Commands reach the engine through its lock-free FIFOs in the same memory,
 * so nothing crosses the worklet's port.
 *
 * @module
 */

import "../engine/worklet-shim.ts";
import { apps_engine_block, apps_engine_process, initSync } from "../gui-host-apps/clausters_gui.js";

/** What the page hands the processor. */
interface EngineStart {
    module: WebAssembly.Module;
    memory: WebAssembly.Memory;
    engine: number;
    channels: number;
    stack: number;
}

class AppsEngineProcessor extends AudioWorkletProcessor {
    private readonly memory: WebAssembly.Memory;
    private readonly engine: number;
    private readonly channels: number;
    private readonly block: number;
    /** The block's samples, viewed where the engine writes them. */
    private view: Float32Array | null = null;

    constructor(options: { processorOptions: EngineStart }) {
        super();
        const { module, memory, engine, channels, stack } = options.processorOptions;
        initSync({ module, memory, thread_stack_size: stack } as never);
        this.memory = memory;
        this.engine = engine;
        this.channels = channels;
        this.block = apps_engine_block();
    }

    process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
        const out = outputs[0];
        if (out === undefined || out.length === 0) return true;
        const frames = out[0]!.length;
        for (let at = 0; at < frames; at += this.block) {
            const ptr = apps_engine_process(this.engine);
            const size = this.block * this.channels;
            // a view is made again only when the memory grew under it
            if (this.view === null || this.view.buffer !== this.memory.buffer || this.view.byteOffset !== ptr) {
                this.view = new Float32Array(this.memory.buffer, ptr, size);
            }
            const view = this.view;
            for (let c = 0; c < out.length; c++) {
                const channel = out[c]!;
                const from = Math.min(c, this.channels - 1);
                for (let i = 0; i < this.block && at + i < frames; i++) {
                    channel[at + i] = view[i * this.channels + from]!;
                }
            }
        }
        return true;
    }
}

registerProcessor("clausters-apps", AppsEngineProcessor);
