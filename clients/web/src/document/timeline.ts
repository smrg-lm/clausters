/**
 * The timeline's entries: a tempo, a meter, a span.
 *
 * Private: the names are `clausters/document`'s.
 *
 * @module
 */

import { type Extra, num, unknownFields } from "./format.ts";

/**
 * One entry of the tempo map: from here on, this tempo.
 *
 * `at` is a beat and `tempo` is beats per **second**, the unit every tempo in
 * clausters is in; beats per minute is only how a ruler or a text field may show
 * it. `ramp` says the tempo runs from here to the next entry rather than
 * stepping. A ritardando is a ramp; a section change is a step.
 */
export class Tempo {
    at: number;
    tempo: number;
    ramp: boolean;
    extra: Extra;

    constructor(fields: { at: number; tempo: number; ramp?: boolean; extra?: Extra }) {
        this.at = fields.at;
        this.tempo = fields.tempo;
        this.ramp = fields.ramp ?? false;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        const out: Extra = { at: this.at, tempo: this.tempo };
        if (this.ramp) out.ramp = true;
        return { ...out, ...this.extra };
    }

    static read(written: Extra): Tempo {
        return new Tempo({
            at: num(written.at),
            tempo: num(written.tempo),
            ramp: Boolean(written.ramp),
            extra: unknownFields(written, "at", "tempo", "ramp"),
        });
    }
}

/**
 * One entry of the meter map: from here on, this time signature.
 *
 * It says how beats make **bars**, which is what a ruler draws and what a
 * snap-to-bar means. It never moves a beat: a meter change does not shift what
 * is placed, it re-bars it.
 */
export class Meter {
    at: number;
    beats: number;
    unit: number;
    extra: Extra;

    constructor(fields: { at: number; beats: number; unit: number; extra?: Extra }) {
        this.at = fields.at;
        this.beats = fields.beats;
        this.unit = fields.unit;
        this.extra = fields.extra ?? {};
    }

    write(): Extra {
        return { at: this.at, beats: this.beats, unit: this.unit, ...this.extra };
    }

    static read(written: Extra): Meter {
        return new Meter({
            at: num(written.at),
            beats: num(written.beats),
            unit: num(written.unit),
            extra: unknownFields(written, "at", "beats", "unit"),
        });
    }
}

/**
 * A span of the timeline: the loop, the punch, a named region of the multitrack.
 *
 * Half-open, so two spans that meet cover no instant twice. In seconds.
 */
export class Span {
    start: number;
    end: number;

    constructor(start: number, end: number) {
        this.start = start;
        this.end = end;
    }

    get length(): number {
        return Math.max(0, this.end - this.start);
    }

    write(): Extra {
        return { start: this.start, end: this.end };
    }

    static read(written: Extra): Span {
        return new Span(num(written.start), num(written.end));
    }
}

