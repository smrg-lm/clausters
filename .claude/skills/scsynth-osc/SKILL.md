---
name: scsynth-osc
description: Reference for the scsynth (SuperCollider Server) OSC protocol and its model semantics — node tree, groups, add actions, buses, buffers, SynthDefs, replies and timetagged bundles. Consult when implementing any /x_y server command or the node tree logic.
---

# scsynth OSC protocol and server model

scsynth listens for OSC over UDP (default 57110) and TCP. This skill summarizes
the semantics our server replicates. Full reference: SuperCollider "Server Command
Reference".

## Model

- **Nodes**: every synth or group is a node with a unique client-assigned `i32`
  ID. The **root group has ID 0**; by convention sclang creates the "default
  group" with ID 1 hanging from 0. ID -1 in `/s_new` = "let the server choose"
  (ours: internal counter starting high, e.g. 2_000_000).
- **Execution order**: depth-first traversal of the tree; earlier nodes process
  first in each block. Order matters because buses are read/written sequentially
  (an effect must come *after* its source).
- **Add actions** (in `/s_new` and `/g_new`): `0` = head of the target group,
  `1` = tail of the target group, `2` = just before the target node, `3` = just
  after, `4` = replace the target node (frees it).
- **Buses**: global arrays. Audio buses (default 1024): indices `0..numOutputs`
  are the hardware outputs, the next `numInputs` are the inputs. Control buses
  (default 16384): one float each.
- **Buffers**: indexed pool (default 1024) of sample arrays, allocated/loaded by
  asynchronous commands.
- **Done actions** (argument of EnvGen, Line, etc.): what happens when the
  envelope ends. The essential ones: `0` nothing, `1` pause the synth, `2` **free
  the synth** (the most used — this is how notes die), `14` free the enclosing
  group.

## Essential commands (with their arguments)

### Server

- `/status` → replies `/status.reply` with: `1, #UGens, #synths, #groups,
  #loaded_synthdefs, avg_cpu, peak_cpu, nominal_sample_rate, actual_sample_rate`.
- `/notify i` (1/0) → registers the client to receive `/n_go`, `/n_end`, etc.
  notifications. Replies `/done /notify clientID`.
- `/quit` → replies `/done /quit` and shuts down.
- `/dumpOSC i` → 0 off, 1 print received commands (parsed).
- `/sync i` → replies `/synced i` once all previous asynchronous commands have
  finished. Key for clients that load buffers and then play.

### Nodes

- `/s_new s i i i [controls...]` → def name, new ID, add action, target ID, then
  control/value pairs (control by name `s` or index `i`, value `f`).
- `/n_free i...` → frees nodes. `/n_run i i...` → pause(0)/resume(1).
- `/n_set i [s|i f]...` → sets controls on a node (by name or index).
- `/n_map i [s|i i]...` → maps controls to **control buses** (read live every
  block); `/n_mapa` maps to **audio buses**. `-1` unmaps; a later `/n_set`
  also clears the mapping. In Clausters controls are block scalars (UGen
  controls and Faust zones alike), so `/n_mapa` samples one frame per block
  (control rate) — there are no audio-rate controls; feed audio through `In`.
  (scsynth also has the multi forms `/n_mapn`/`/n_mapan`; not implemented.)
- `/n_before i i`, `/n_after i i` → reorder: node A before/after node B.
- `/g_new i i i...` → ID, add action, target (like s_new without controls).
- `/g_freeAll i` → frees all children of the group. `/g_deepFree i` → frees all
  descendant synths (keeps the group structure).
- Notifications (if `/notify` is on): `/n_go`, `/n_end`, `/n_off`, `/n_on`,
  `/n_move` with `nodeID, parentID, prevID, nextID, isGroup`.

### SynthDefs

- `/d_recv b [optional completion msg bytes]` → receives a serialized
  definition. **Asynchronous**: replies `/done /d_recv`. (Our own format in the
  blob, not SC's binary `.scsyndef` — document the divergence.)
- `/d_free s...` → forgets definitions (live synths keep sounding).

### Buffers (all asynchronous → reply `/done /b_xxx index`)

- `/b_alloc i i i` → index, frames, channels.
- `/b_read i s i i i i` → index, path, start frame in file, frame count
  (-1 = all), start frame in buffer, leaveOpen.
- `/b_write i s s s i i i` → write to file (header format and sample format).
- `/b_free i`, `/b_zero i`.
- `/b_query i...` → replies `/b_info` with `index, frames, channels, samplerate`.

### Control buses

- `/c_set [i f]...`, `/c_get i...` → replies `/c_set` with the pairs.

## Bundles and time

- An OSC bundle carries an **NTP timetag** (64 bits: 32 of seconds since 1900,
  32 of fraction). Timetag `1` = "immediately".
- scsynth semantics: the bundle's messages execute **at the exact sample**
  corresponding to the timetag. Implementation: convert NTP → stream sample time;
  if it falls inside the current block, split processing at that offset; if it is
  in the future, enqueue into an ordered (pre-allocated) queue consulted every
  block.
- Bundles arriving "late" execute right away (scsynth also prints "late").
- Bundles may nest bundles.

## Errors

Reply `/fail s s` with the command and the reason, e.g.
`/fail /s_new "wrong argument type"` or duplicate node ID. Don't kill the server
on malformed commands.

## Manual testing

```bash
# with oscsend (liblo-tools package)
oscsend localhost 57110 /status
oscsend localhost 57110 /s_new siii default -1 0 0
oscsend localhost 57110 /n_set isf 1000 freq 220.0
# or from sclang: Server("mine", NetAddr("127.0.0.1", 57110))
```
