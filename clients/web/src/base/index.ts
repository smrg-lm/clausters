// `clausters/base`: the client's foundations, the Python client's
// `clausters.base` -- clocks and time, the default session, streams and
// routines, the random context, the destinations, the OSC and MIDI
// carriers. The names a script types without a namespace are also the
// package root's, as in the Python client; the rest are reached here.
//
// What the Python module has and this one does not is the platform's: a
// page binds no port and opens no socket of its own kind, so there is no
// `NetAddr` and no UDP, TCP or embedded interface; the carriers a page has
// beside the WebSocket one are in `clausters/page`.

export { AbstractObject } from "./absobject.ts";
export type { Composed, Fan } from "./absobject.ts";
export { AppClock } from "./appclock.ts";
export type { AppItem } from "./appclock.ts";
export { TempoClock } from "./clock.ts";
export { TempoMap } from "./time.ts";
export { LogicalTimebase, ManualTimebase, MonotonicTimebase, SampleClockTimebase } from "./timebase.ts";
export type { Timebase } from "./timebase.ts";
export { Environment, RandomContext } from "./environment.ts";
export { WHOLE_SHARE, shareOf } from "./ids.ts";
export type { IdShare } from "./ids.ts";
export { Main, main } from "./main.ts";
export { Moment } from "./moment.ts";
export {
    Rng,
    betarand,
    bilinrand,
    cauchy,
    choose,
    coin,
    currentRng,
    exprand,
    gauss,
    hprand,
    irand,
    irand2,
    irrand,
    linrand,
    lprand,
    meanrand,
    poisson,
    rand2,
    rrand,
    scramble,
    seed,
    spawnRng,
    sputter,
    sum3rand,
    tablerand,
    wchoose,
} from "./rand.ts";
export { OscDestination } from "./destination.ts";
export type { Destination } from "./destination.ts";
export { FunctionStream, Routine, StopStream, Stream, YieldAndReset } from "./stream.ts";
// The transports. `Connection` is the Python client's `OscInterface`: what
// every carrier is, a type here, where the Python one is a base class.
export { OscNrtInterface, OscScore, OscWsInterface } from "./connection.ts";
export type { Connection, SampleClock } from "./connection.ts";
export { OscReceiver } from "./receiver.ts";
export type { OscHandler } from "./receiver.ts";
export { MidiNrtInterface, MidiReceiver, MidiRtInterface, MidiScore, MidiServer, parseMidi } from "./midi.ts";
export type {
    MidiHandler,
    MidiInputPort,
    MidiInterface,
    MidiMessage,
    MidiOutputPort,
    MidiPortOptions,
    MidiPorts,
    MidiReceiverOptions,
    MidiServerOptions,
    MidiWriteOptions } from "./midi.ts";
