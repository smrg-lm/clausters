# clausters-apps — the applications over the document: the closed tracks

The tracks of [`crates/clausters-apps/PLAN.md`](../../../crates/clausters-apps/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

### The turn: an application's takes live on disk

*Opened 2026-10-22 by the user, after successive pastes of a long selection
failed natively with `no reply to /buffer_stitch`.* The audio editor's model
does not reach a file of hours, and the failure showed why: every open file is
read whole into a pool buffer (`/buffer_allocRead`), a copy is made in the
server's memory (`/buffer_gen copy`), and a server that shares its buffers
copies each new join whole into a region a peer maps and summarizes it. So the
cost of an edit grows with the take and its memory with the history; once one
copy passed the client's 5 s, the reply was given up for lost. Making each of
those cheaper (`G44`, the region copied part by part) does not change their
order. The model below replaces it, for the audio editor and the multitrack
alike, and is what the milestones of this section build.

**The model** (decided with the user, 2026-10-22):

- **Two kinds of samples.** A **pool buffer** is finite and short and lives in
  a server's memory: a sampler's samples, an oscillator's table, a stroke
  being drawn. A **file** is any length its format admits (W64, RF64: hours or
  days) and lives on disk, never read whole. An application's takes are files.
- **Segments over either.** A segment list (a part names a source, a frame
  range, its fades and its channel map) is one logic whatever its parts
  name; only where the data is differs. A sampler reads segments of pool
  buffers with random access; an editor and a multitrack read segments of
  files as a stream.
- **An edit is a list.** The audio editor works as the multitrack does: an
  edit is a new segment list over files that never change. Cut, copy, paste, move, undo and redo move no samples at any length,
  and the clipboard holds segments.
- **Two servers, by the kind of work.** The **RT server** plays a list,
  reading the files ahead of the playhead. The **NRT
  server** processes: a gain, a mix, a stroke, an effect, a consolidation each
  read their spans from disk and write a new file in the editor's working
  folder, which the list then names. A render is temporary, and so is the
  folder: what outlives the editor is the file a save consolidates the list
  into (the seventh decision below).
- **Several NRT servers, and the RT server beside them.** A render reads
  files that never change and writes a new one, so renders share no state and
  run on as many NRT instances at once as the cores and the disk allow: two
  files processed at the same time, or a long render split into spans where
  the process keeps no state across them (one with a tail or memory overlaps
  its spans). They finish in any order, and an edit names its render only once
  it is done, which is the coordinator's per-completion wait. The RT server
  goes on playing while they work, and when the render lands the list names
  the new file with no break in what is heard. It
  can also play a render still being written, up to how far it is written
  (the frontier `S20` publishes); the other way round, the RT server records
  and an NRT instance measures what it recorded.
- **The server holds audio functions, not editing.** No command exists for
  one application's verb: an edit's processing is a def rendered offline, so
  `/buffer_gain`, `/buffer_reverse`, `/buffer_mix` and `/buffer_render` go
  (root `PLAN.md`, `S23`).
- **Drawing reads the files.** Each file has a peaks file (the CLPK
  format), written once, by a peaks UGen beside the `DiskOut` that writes the
  samples or by an NRT render over an imported file. The host composes a list's
  picture from its parts' peaks and reads a short span of a file when a view
  is zoomed to samples: from disk natively, from OPFS in a page.
- **Memory is bounded by the work in hand, never by a take's length**: the RT
  reader's ring, the NRT's blocks, the window's columns.

**What stays.** The document's `Location::File` and `Location::Segments`, and a
`Part` that already names a source and a range; the segment logic of
`dsp::stitch`, generalized; G44's composition of a join's summary, moved from
the server to `clausters-core`; the host's asking at the drawn level and
placing whole (`G44c`); `DiskIn`'s thread and ring; the NRT session; and, for
pool buffers, `/buffer_stitch`, `/buffer_parts` and `/buffer_peaks`.

**What goes.** Reading a take whole to open it, `/buffer_gen copy` as the
editor's copy, the region copy of a join and its overview, and the four edit
commands above.

**The decisions**, refined with the user on 2026-10-22 after the plan was
written; none is left open:

1. ✅ **Reverse** *(decided 2026-10-22 by the user)*. As an edit it is
   processing, so it is an NRT render like a gain, in `X28`. Reading
   backwards is the RT reader's capability, a playhead that runs backwards,
   and is filed in the root plan's Future directions ("A list reader plays
   backwards"); once it exists, a part read backwards can be the editor's
   optimization of a reverse over a long file.
2. ✅ **Who writes a file's peaks** *(decided 2026-10-22 by the user)*. A
   UGen of its own, beside `DiskOut` and not inside it: each writes its own
   data to its own file. A render or a recording that wants its file drawn
   puts both on the same signal, and an imported file is measured by an NRT
   render of `DiskIn` into the peaks UGen alone. No command is added, and the
   audio is not copied.
3. ✅ **The base bucket of a peaks file on disk** is the one the format
   already has, 256 frames: no decision was needed. With its levels a peaks
   file is about 2.3 % of an f32 file, 3.1 % of a 24-bit one and 4.7 % of a
   16-bit one (a day of 48 kHz stereo is about 0.78 GB of peaks).
4. ✅ **Saving over the file that was opened** *(decided 2026-10-22 by the
   user)*. Undo keeps working after a save. The save renders to a new file
   and replaces the original by a rename; the original's content moves to the
   working folder and is deleted by `O34`'s rule, when nothing names it.
   Another editor editing the same file is concurrent editing, as with two
   desktop applications over one file: when the file changes on disk the
   other is told in the standard way, and nothing is shared between them.
   Two windows of one editor are views of one document and are not this
   case.
5. ✅ **The working folder, and where peaks live** *(decided 2026-10-22 by
   the user)*. The audio editor's working folder is temporary: its renders
   and the peaks of what it opened live there, nothing is written beside the
   user's file, and the folder goes when the editor closes. A render is
   deleted earlier, as soon as no history entry and no clipboard names it.
   The multitrack keeps its peaks, as the field does, beside the files or in
   a tree of their own; which of the two is `X29`'s.
6. ✅ **How the RT reader follows the transport** is not a decision: `S21`
   states it (positioned by the transport, refilled on a locate), and what is
   left, its ring size and how far ahead of a seam it reads, is measured
   there against comping parts a quarter second long from different files.
7. ✅ **The audio editor has no session** *(decided 2026-10-23 by the
   user; superseded the same day by the second turn below, where it had a
   session file per file being worked on, and holding again since that file
   was withdrawn on 2026-10-25)*. Each file is independent: an audio editor edits a file, as a
   text editor does, and a multitrack mixes many files into one, so the two
   are not compatible applications and the audio editor's takes do not live
   in a multitrack's session. Saving with edits half done means nothing
   here; a save consolidates the list into the file.
8. ✅ **The gain envelope has no preview** *(decided 2026-10-23 by the
   user)*. Drawing a preview of an envelope means a processed picture of
   the take, and over hours of multichannel audio that is not a picture to
   make; the curve is edited over the take and heard once it is applied.
   Its insert, its table and its processed picture went. Hearing an effect
   live before applying it, which a real-time effect allows with nothing
   drawn or with a graph animated as it plays (the spectrum an equalizer
   leaves), is a different thing, filed in Future directions ("An effect
   heard live before it is applied").

**The order**: `S21` and `S22` (root `PLAN.md`), `O34`
(`crates/clausters-document/PLAN.md`), `G45` (`crates/clausters-gui/PLAN.md`), then
`X28`, `X29` here, and `S23` last, once nothing calls what it removes. The
turn comes before `X25`, which ports the audio editor's lifecycle to the other
editors and should port this one.

- ✅ **X28 - The audio editor edits files on disk.** *(Replaced by `X30`–`X32` for the application model and by `X25`
  for the remaining lifecycle ports. Closed 2026-11-02: everything below
  is in the tree and measured; the eye and ear passes over the audio editor
  on files are the user's, as every visual check is, and they ran in
  `G44c`'s native pass over long takes (2026-10-28) and in `X25`'s
  cross-application pass.)* After `S21`, `S22`,
  `O34` and `G45`. Opening a file reads its header and its peaks (measured
  once if it has none) and nothing else; every edit is a list; the clipboard
  holds segments; processing (gain, reverse, mix, a stroke,
  an effect) is an NRT render into the working folder; playing is the RT
  server's list reader; saving consolidates the list into a file by an
  NRT render. The editor's shared crate, both
  clients and the standalone host change together; nothing reads a take
  whole. **Acceptance:** on a two-hour stereo file and a synthetic W64 file of
  more than 4 GB, ten successive pastes of an hour each answer in the same
  time as one paste into a two-second take, reading no samples but those
  under the seams' fades; the server's and the host's memory stay flat across the
  ten and across undo and redo; copy and paste between two windows and two
  files move no samples; a gain over an hour is an NRT render whose progress
  is shown and which leaves the RT server playing; two renders over two files
  run on two NRT instances at once and land in either order; a consolidation's file reads back sample
  for sample; a save over the opened file keeps undo working past it, and a
  second editor over that file is told the file changed on disk. The
  example pair `editors/audio_lifecycle` runs as before, natively and in a
  page; by eye and by ear with two windows. *Opened 2026-10-22: done towards it, a page's
  recording at an offset, a list's spectrogram, an offline render in a
  page that reads and writes files (`clients/web/PLAN.md`, Found by use), a
  render step every endpoint carries out, and (2026-10-23) the editor's takes
  as files in both clients: opening reads the header, every verb is a list or
  a render, the clipboard holds segments, a save renders and moves aside the
  file the list reads, and the renders nothing names are deleted. Since
  done: a render's progress on the status bar, the changed-on-disk notice,
  a list replaced while it plays fading into the new one instead of a
  silence (the reader's, for the multitrack; the audio editor edits no file
  that plays, `docs/app-behavior.md`), and the measurement of the edits (`tests/long_takes.rs`, in
  release: ten pastes of an hour into two hours and into 4.8 GB, and their
  undo and redo, answer in 0.05 ms at the median and 0.3 ms at worst, as
  into two seconds, render nothing, and leave the editor's and the server's
  memory where it was), and renders over two files at once: a run that stops
  on a render hands it out, so an edit of another file runs meanwhile and
  the two land in either order, each committed to its own file. Left: the eye
  and ear pass. The session save the acceptance named first was dropped
  (2026-10-23, the seventh decision), and so was the effect previewed live
  and then applied (the eighth).*

- ✅ **X29 - The multitrack and the standalone host read takes from disk.**
  *(Replaced by `X34` for track reading and by `X25` for session lifecycle.
  Closed 2026-11-02, its acceptance met there: hour-long takes open in the
  time of their peaks and a comp of quarter seconds plays with nothing
  starved (`X34`, "Measured"); the session's peaks are kept in its `peaks/`
  (`X34`, "A session opens from its files"); a saved session opens and
  sounds natively, in a page (`multitrack-editor.html?isolated`) and in the
  standalone host (`--session`), and the pair `edit_multitrack` and
  `load_multitrack` runs in both clients (`X25`).)*
  After `X28`. Loading a session (`clausters-editing`, `load.rs`) stops
  reading files into pool buffers and stitching joins in memory: a track
  plays its regions through the RT list reader, and a bounce or a freeze is an
  NRT render into the session's folder. Its peaks are kept, beside the files or
  in a tree of their own (the turn's fifth decision leaves the choice here).
  *Noted by the user, 2026-10-23:* the peaks a measure writes say where
  they go, as a property of the measure: temporary, in the working folder,
  as the audio editor's are, or kept somewhere relative to the file or to
  the multitrack's project, as a multitrack's may be. **Acceptance:** a session of several
  tracks of hour-long takes opens in the time it takes to read their peaks,
  plays from any position, and a comping pass of quarter-second parts plays
  without a click or an underrun; the server's memory does not grow with the
  takes; the multitrack example pair and a saved session open and sound
  natively, in a page and in the standalone host.
