// How many sample rings the server carries.
//
// An audio bus is engine memory, one block at a time, so nothing outside the
// audio thread can see it. The server copies a bus into one of a fixed set of
// **sample rings** in the shared segment when asked (`/tap bus 1`), which is
// what lets a scope see the samples of a live signal.
//
// **The rings are the server's own bookkeeping.** A client asks for a *bus*
// and never for a ring: the server picks one, publishes the choice in the
// segment, counts watches so two views of a bus share a ring, and frees it
// when the last one stops. All this client needs from that region is how big
// it is, to size a sensible default before `/server_info` answers.

/** The server's default ring count (`--taps`), when it reports none. */
export const DEFAULT_TAPS = 8;
