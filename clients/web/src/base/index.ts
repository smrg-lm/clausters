// `clausters/base`: the client's foundations, the Python client's
// `clausters.base` -- clocks and time, the default session, streams and
// routines, the random context, the destinations, the OSC and MIDI
// carriers. Every name here is also exported by the package root; the
// module is the same one in both clients, so a script imports a name from
// where the other client's script does.
//
// What the Python module has and this one does not is the platform's: a
// page binds no port and opens no socket of its own kind, so there is no
// `NetAddr` and no UDP, TCP or embedded interface; the carriers a page has
// are its `Connection`s (docs/bindings.md).

export { AbstractObject } from "./absobject.ts";
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
export { OscNrtInterface, OscScore } from "./connection.ts";
export { OscReceiver } from "./receiver.ts";
export { MidiNrtInterface, MidiReceiver, MidiRtInterface, MidiScore, MidiServer, parseMidi } from "./midi.ts";
