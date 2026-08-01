---
name: scsynth-osc
description: Reference for the scsynth (SuperCollider Server) OSC protocol and its model semantics — node tree, groups, add actions, buses, buffers, SynthDefs, replies and timetagged bundles. Consult when implementing any server command or the node tree logic. Clausters keeps this model but not these command names — the mapping is at the top of the skill.
---

# scsynth OSC protocol and server model

scsynth listens for OSC over UDP (default 57110) and TCP. This skill summarizes
the semantics our server replicates. Full reference: SuperCollider "Server Command
Reference".

> **The names below are scsynth's, not ours.** Clausters keeps this *model*
> — node tree, add actions, bus and buffer pools, the async barrier — and
> renamed every command to one rule, `/<resource>_<action>` (see
> `docs/schemas.md`, "How a command is named"). Read this skill for semantics
> and translate the address before writing any code:
>
> | scsynth | Clausters | | scsynth | Clausters |
> |---|---|---|---|---|
> | `/s_new` | `/synth_new` | | `/b_alloc` | `/buffer_alloc` |
> | `/n_set` | `/node_set` | | `/b_query` → `/b_info` | `/buffer_query` → `/buffer_query.reply` |
> | `/n_setn` | `/node_setRange` | | `/d_recv` | `/def_send synth` |
> | `/n_mapa` | `/node_mapAudio` | | `/d_free` | `/def_free` |
> | `/n_query` → `/n_info` | `/node_query` → `/node_query.reply` | | `/status` | `/server_status` |
> | `/g_new` | `/group_new` | | `/notify` | `/server_notify` |
> | `/g_queryTree` | `/group_queryTree` | | `/sync` → `/synced` | `/server_sync` → `/server_sync.reply` |
> | `/c_set` | `/bus_set` | | `/quit` | `/server_quit` |
> | `/c_getn` | `/bus_getRange` | | `/u_cmd` | `/node_ugenCmd` |
>
> The whole set, with the rule that generates it, is `docs/schemas.md`.

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
`/fail /synth_new "wrong argument type"` or duplicate node ID. Don't kill the server
on malformed commands.

## Manual testing

```bash
# with oscsend (liblo-tools package)
# (Clausters names, not scsynth's — see the mapping above)
oscsend localhost 57110 /server_status
oscsend localhost 57110 /synth_new siii default -1 0 0
oscsend localhost 57110 /node_set isf 1000 freq 220.0
```
