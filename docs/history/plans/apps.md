# clausters-apps — the applications over the document: the closed tracks

The tracks of [`crates/clausters-apps/PLAN.md`](../../../crates/clausters-apps/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

The entries of the plan's "Found by use" and "Future directions" lists that
were fixed or done are at the end of this file, each moved whole once it was
ticked.

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
  (`crates/clausters-server/PLAN.md`, `S23`).
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
   and is filed in the server plan's Future directions ("A list reader plays
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

**The order**: `S21` and `S22` (`crates/clausters-server/PLAN.md`), `O34`
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

## Application state and services

*Opened 2026-10-18 by the user after recurring audio-editor defects exposed
unclear ownership and call ordering. This plan owns the work, including its
host and client adapters; the GUI plan's completed AP track is the precedent,
not a second implementation plan. The checkboxes track the staged migration,
done with `X27`; `docs/architecture.md`, "Application state and services:
implementation contract", is the contract as built.*

**The purpose: one modular application framework.** Applications follow one
consistent design, even if this project remains its only consumer. State,
services, actions, component relationships and lifecycle are composable
modules with explicit contracts. The audio editor is the first composition
that proves them, not a special architecture for the other editors to copy.
An application's options come from capabilities, configuration and domain
operations over its contents; adding an option does not require designing
its own history, clipboard, menu, view synchronization or resource lifecycle.
Domain semantics still need definition when new; their integration into the
application follows the common contracts. Extend a shared contract when a
real capability requires it, with all consumers kept consistent.

**The design.** Compose an application from hierarchical instance state,
scoped services and explicit transitions. `AppState` describes what the
instance holds; an application coordinator processes actions and service
results. Document/View separates edited contents from presentation; Command
and undo contexts keep reversible edits together; a unidirectional update
path and state machines make pending operations explicit; ports and adapters
keep platform I/O outside the shared logic. Build the internal framework
with Rust composition and enums over existing mechanisms, without requiring
an external application framework, inheritance hierarchy or global service
locator. Applications select the modules they need through explicit
dependencies; a single all-purpose state object is not the framework.

**Ownership is not the widget tree.** The application holds open objects,
their presentation state, windows and pending operations. Objects reference
their editing contexts; a context may span several objects and views, and
must not be copied into each window. Each view keeps its own conversation
and stale-event checks. Host-owned navigation and interaction remain the
host's; application-owned state restored per file has an explicit projection
to the host, never a second independently mutable copy. A redraw does not
change the lifetime of edited contents, playback or history.

**Services have scopes, not global instances.** Start from `History` and
`Editing`, `AudioEditorPlayback`, the typed clipboard, and the existing
source, spill and step mechanisms. Audit their contracts before adopting
them as services: reuse does not require retaining defective ownership or
representations. Clipboard contents, storage and resource ownership are
separate responsibilities. A service may serve several applications while
retaining separate state per context, server or object. Pass dependencies
explicitly at construction. Track
resource references from contents, history, clipboard, playback and pending
operations before freeing a source; buffer ids are local to their server.

**History policy is explicit application composition.** Menus and shortcuts
resolve undo/redo to the active document's history, after resolving text-input
editing where applicable. An empty order disables the action; it never falls
back to another document or the application context. Independent audio files
have independent orders, including the initial file. Sharing a service does
not merge orders. An application must explicitly compose a shared context;
scripts may explicitly address that context. Its entries restore their
registered objects even out of focus, updating playback graphs without
starting playback or changing the active document or its view bindings.

**Placement and compatibility.** Application states and coordination live in
`clausters-apps`; reusable projections, playback and step mechanisms stay in
`clausters-editing`; document structures, edit semantics and `History` stay
in `clausters-document`. The crate never links the renderer. Host services
are reached through ports; native/browser shells and language adapters
execute effects and report results. Existing Python/TypeScript `Application`
classes are adapters to reconcile with this design, not a second owner of
application policy. Preserve `edit`'s contract and supported public verbs;
a defective behavior may change under an explicit migration and parity
contract. Compatibility must not freeze a bug into the new design.

**A GuiDef remains an interface definition.** Its running instance can use
services without becoming an audio editor. Neither undo entries nor live
resources are serialized into the widget tree. Persisting authored GuiDefs
and persisting application/session data remain separate contracts.

**Boundaries to settle first.** Saving one audio file, persisting a
multi-file workspace and crash recovery are different contracts. This track
implements the first; it defines what state is transient and records any
proposal for the other two under Future directions, without silently adding
a session format or persistent undo. Applications initially use a supplied
server connection. Launching or supervising different server processes is
outside this track and needs its own ownership/lifecycle design.

**Order and closure.** Take `X15` through `X27` in order, with the audio
editor as the reference through the acceptance gate in `X24`. Port the
proven contracts to other editors only in `X25`, and to simple GuiDef
instances in `X26`. Each implementation lands with every affected endpoint
in the same commit: Python and TypeScript bind the same Rust behavior, and
the standalone host uses it wherever that application is supported. No
milestone adds an unsupported standalone application. Parity, regression
checks and documentation accompany each change; final acceptance does not
postpone them. Concrete type and module names are settled against existing
code in `X15`. The reported bugs motivate tests, not presumed diagnoses.

- ✅ **X15 - Audit ownership, boundaries and governing instructions.** Map
  the audio editor, `Editing`, `closing`, both clients' `Application`, audio
  drivers/outcome handlers, host services and standalone `Owner`. Record
  each state's owner, lifetime, identity, persistence, undo scope and every
  adapter-held projection. Classify the server commands used for samples,
  storage, playback and resource management: semantics, consumers, waits and
  failure behavior. Server buffers store/process samples; clipboard/history
  policy belongs to application services. Identify protocol/ABI gaps before
  adding commands, preferring existing general operations where suitable.
  Reconcile stale `clausters.form`/`FormEditor` skill guidance, the renderer
  versus standalone document owner, and the difference between executing a
  resource operation and owning its policy. Check higher-priority project
  rules before changing guidance. Define transient workspace, file-save and
  recovery boundaries, and preserve `docs/app-behavior.md`'s valid rules.
  **Acceptance:** the architecture has an ownership/service table and action,
  effect and completion contracts; conflicting guidance is corrected at its
  source; an audit names justified reuse, changes and remaining decisions.
  Define the module composition and extension recipe: required contracts,
  optional capabilities and where domain-specific operations attach. Common
  infrastructure neither enumerates application classes to decide which
  edits exist nor grows a separate lifecycle per application option.
  Build regression cases for reported symptoms without claiming reproduction
  or a cause where none was demonstrated. No implementation depends on
  private agent state, a speculative registry or a new process supervisor.
  Contract and audit: `docs/architecture.md`, "Application state and services:
  implementation contract".

- ✅ **X16 - Separate audio files, windows and view relationships.** After
  `X15`, introduce explicit file identities, open-file collection, focus,
  per-file contents/presentation, window bindings and playback association.
  Replace whole-editor swaps and `bind_window`'s copied field list. Define
  each component's observed object, shared axes, dimensions, subscriptions
  and rebinding behavior. Distinguish the viewed file, playing file and
  output infrastructure: allocated bus capacity does not set visible meter
  channels. Settle which signal the meter represents and project its channel
  count accordingly; draw a playhead only over the file whose time it names.
  **Acceptance:** two files restore parts, envelope, selection, cursor,
  layers and history independently; playing A while viewing B preserves A's
  pass without drawing its cursor over B. Mono/stereo/multichannel switches
  show the specified meter shape in both directions. Two windows keep their
  conversations, meters and navigation apart. Rebinding drops old observers;
  redraw retains resources and stale gestures remain refused.

- ✅ **X17 - Compose editing, playback and resource services.** After `X16`,
  pass scoped dependencies explicitly. Generalize history-scope routing out
  of `Editing`'s audio-file bookkeeping while preserving the initial file's
  shared context and additional files' independent orders. Keep the history
  engine and playback implementation where their audited contracts fit.
  Coordinate source retention, budgets and scratch storage through one
  resource mechanism. Contents, history, clipboard, playback and pending
  operations are independent roots; identify a remote resource by server
  instance and local id, including invalidation on reconnect/restart.
  **Acceptance:** a shared context undoes across members in one order;
  independent files select independent orders; no retained source is freed;
  spill/restore and final release work on disk and OPFS. Services work
  without a renderer and two applications do not share transport state.

- ✅ **X18 - Define and implement the clipboard exchange contract.** After
  `X17`, separate immutable copied contents, their storage, resource leases,
  destination compatibility and edit policy. Audit `Content::Samples`,
  `Content::Buffer`, host `Clip`, system-text mirroring and the clients'
  per-server buffer tables; one service owns publication and replacement.
  Define Copy, Cut, Paste and Paste mixed, insertion/selection behavior,
  channel/rate handling, and accepted conversion or refusal paths. Specify
  same-file, cross-file/window and cross-server behavior, including closing
  the origin, replacing clipboard contents and losing a server connection.
  A buffer-backed copy may avoid transferring samples but carries a valid
  server-instance reference and lifetime; its number alone is insufficient.
  Publish a copy only when ready; cut must retain a valid copy before removal
  is committed. Pasting produces destination-owned contents independent of
  later clipboard replacement. Platform text and typed payloads have an
  explicit synchronization policy and do not silently reinterpret each other.
  **Exchange policy:** server-backed publications are scoped to one client
  connection incarnation; cross-connection transfer refuses explicitly.
  Insertion preserves source rate through the existing parts ratio; mixing
  requires matching rates, and both require matching channels. Publications
  outlive source windows; pending readers defer replacement release. New
  external system text stays text, including JSON-looking text. Reconcile
  unused remote publications at the next clipboard action instead of polling;
  at most one otherwise unused publication remains per endpoint. Pasted
  sources keep the destination's existing history budgets and disk/OPFS
  spill/restore path; clipboard ownership is separate from undo ownership.
  **Acceptance:** repeat copy/cut/paste/mix across files and windows, close
  the origin, replace typed contents with external text, vary channels/rates,
  and disconnect/reconnect the server. Every case follows the compatibility
  matrix, succeeds or refuses without data loss, and retains/releases sources
  correctly. Test semantic contents as well as lifetime in both clients.

- ✅ **X19 - Make pending operations and their results explicit.** After
  `X18`, represent opening, sample-producing edits, clipboard publication,
  saving, spilling, restoring and closing with operation identities,
  completions, errors and cleanup. Reuse `Step` waits; define commit points,
  dependencies and queue/refusal rules. A delayed result addresses its
  original object, not the file now in focus; freed/recreated views and
  duplicate replies cannot receive another operation's completion.
  Confirm saved destination and clean checkpoint after successful I/O.
  Content/history identity is separate from the conversation version: undo
  to the saved state is clean, while an edit during save remains dirty.
  **Acceptance:** failed save remains dirty/retryable; failed spill retains
  samples; restore precedes stitch; save-and-close waits for success. Failure
  after partial allocation, cancellation, late and duplicate replies leave
  recoverable contents/history and release resources once. Platform-specific
  scheduling does not decide commit or cleanup policy.

  - ✅ Non-generic shared services, read-only typed preparation and
    per-application operation registries over one resource graph; shared
    completion, repair and history-walk orchestration.
  - ✅ Audio open/add-file, sample edits, clipboard publication, save snapshots
    and spill/restore use prepared deltas; menu/shortcut undo waits for
    confirmation. Retiring sources keep exact cleanup tickets until
    acknowledgement, including after file close.
  - ✅ Headless regression coverage for rejected commits, distinct application
    commit types sharing services, failure/retry, saved revisions,
    original-object routing, invalidation and exactly-once id return in the
    Rust core and both adapters.
  - ✅ Execute lifetime and spill/restore decisions against the native NRT
    endpoint and browser NRT/OPFS, asserting samples after undo, clipboard
    source close and publication replacement.
  - ✅ AudioEditor classifies operation-producing gestures; Editing only
    supplies endpoint-scoped clipboard reads, without a second gesture
    whitelist.
  - ✅ Native delayed-completion review accepted by the user (2026-10-19):
    cut/paste, view replacement before save confirmation, then selection and
    zoom work without a spurious refusal. Playback cursors and viewed-file
    meters were also accepted. Browser visual acceptance is separate.
  - ✅ Native save-and-close script over the existing Python API: a real
    write failure keeps the window open and dirty; retry with delayed I/O
    keeps it open until save confirms, then closes it. Reading the saved WAV
    verifies the edited samples. This tests sequential save/close, without
    adding a dialog or changing ownership of client-held objects.

- ✅ **X20 - Give actions one definition and contextual state.** After
  `X19`, extend the existing `clausters-editing::verbs` and chrome mechanisms
  with target resolution, availability/reason, command/toggle/choice kind,
  checked/value state and pending status. An action is not a history entry:
  play, navigation and dialogs are actions without undoable content edits.
  Menus, tools, shortcuts, the key sheet and script calls invoke the same
  action; the presentation chooses its layout and reads state from it.
  Resolve text-field focus versus edited-content targets explicitly, and
  keep user shortcut overrides and the host's direct bound-control path.
  Apply the history policy above: remove the legacy implicit enrollment of
  an audio editor's initial file in the shared context, preserved in `X17`.
  Shared history must be selected by explicit composition rather than file
  opening order; document actions never fall back to the context's order.
  **Acceptance:** menu/key/script paths resolve the same intended target;
  disabled and checked entries reflect current file, selection and clipboard;
  pending operations update availability; shortcut help matches actual
  bindings. With edits in A and an empty focused B, undo is disabled and A
  stays unchanged, whether A or B was opened first. Explicit shared-context
  undo restores its recorded objects without switching focus or starting
  playback. No menu/button keeps a second copy of application state.

  - ✅ One definition per verb: `verbs` rows carry their kind (command,
    toggle, choice and its group); `ActionState` reads target, availability
    and reason, checked/value and pending, computed on each call
    (`Editing::action_states`, `AudioEditor::actions`) and published as the
    window's `actions` prop by both clients only when it moves.
  - ✅ The host reads that one object: menu lists open with disabled,
    checked and grouped entries; tools draw held or greyed; a key, a tool, a
    menu entry and `/gui_verb` refuse a disabled reading alike. A focused text
    field answers its own editing first, and a paste is read against the
    clipboard's shape, rate and server connection.
  - ✅ An audio editor's first file has an independent order
    (`Editing::open_independent`); `shared_history`/`sharedHistory` composes
    it into the context's order explicitly. Directed tests cover empty B with
    edits in A in both opening orders, across two windows and two files of
    one window, in the Rust core and both clients.
  - ✅ General host rules, for every window: a selection is read from the
    addressed view (`Element::holds_selection`, else its axis), so a reading
    predating a sweep or a pick never refuses; a dialog up keeps every verb
    from what is behind it; a text field keeps its own undo history; a
    window with readings offers no other verb of the host's table, on keys
    or on its key sheet. Readings and their publication live in both
    clients' base `Editor`; the paste destination is tagged by contents kind.
    Other applications' readings are `X25`.
  - ✅ Native and browser eye pass over the `edit_audio` pair: disabled and
    checked entries follow file, selection, clipboard and pending saves; the
    key sheet matches the bindings. First native pass (2026-10-21) found the
    entries below in "Found by use"; the second pass (2026-10-21) accepted
    them, with a delete that removes and is history but copies nothing.

- ✅ **X21 - Consolidate shared visual compositions and file selection.**
  After `X20`, compose existing `forms`, `files`, chrome and shortcut views
  through common contracts. Separate directory/read/write services, chooser
  interaction state and application decisions about opening selected paths.
  Specify filters, multiple selection, pending/error/cancel states, focus,
  dimensions and disposal; a stale directory listing cannot replace the
  directory now being viewed. Reuse the same host component on disk and
  OPFS, with I/O adapters at the platform boundary. Keep musical rulers and
  data views in their existing shared host/domain mechanisms.
  **Acceptance:** the audio editor uses the reusable chooser/forms and
  action-backed menu/key sheet; navigation while loading, cancellation,
  errors, multiple selection and reopening restore only intended state.
  Test the composition in isolation and inside the audio editor, without
  adding a second renderer or an editor-specific file browser.

  - ✅ Three layers with one owner each: the front's listing service answers
    a ticketed `Ask` (disk natively, OPFS in a page); `host::files::Chooser`
    holds the directory, filter, the ask it waits on and the entries or the
    error, and the `files` element only draws it with the table's selection,
    focus and scrolling; the application reads the choice through
    `forms::Dialogs`. Reading a chosen file stays the client's (`openFile`,
    `SoundFile`).
  - ✅ A stale listing replaces nothing: the chooser takes only the answer to
    its current ask, so climbing while a listing is in flight, a re-pathed
    chooser and a window drawn again refuse late answers even for the same
    directory; a freed widget or window takes nothing. Tested on the chooser,
    the element and the host's delivery.
  - ✅ A form field can be the host's chooser (`forms::Spec::chooser`: filter,
    multiple selection, size). `forms::read` reports `Chosen`, `Picked` and
    `Entered`; `forms::Dialogs` holds the ids, the form up, the paths chosen
    and the directory each chooser reopens in. Opening a form lists the
    chooser afresh and focuses it; cancel and accept forget the selection
    and keep the directory; a redraw shows the form up with its values; a
    closed window drops it. Tested with a form of the module's own.
  - ✅ The audio editor's Open form is that chooser field, filtered to what a
    `SoundFile` reads; a pick opens the selection as `OK` does. Its menu and
    key sheet were already action-backed (`X20`). Directed tests cover
    reopen, pick, directory change, redraw and close inside the editor; the
    score editor's forms keep their behavior and move to `Dialogs` in `X25`.
  - ✅ Native eye pass over the `edit_audio` pair's File > Open (2026-10-21):
    filter, keyboard, pick, reopen in the last directory and multiple
    selection accepted. The pass also found File > Close file naming the
    wrong file (fixed) and the rule that an application opens with no file
    and outlives its last, which the audio editor now keeps; the other
    applications are in "Found by use".
  - ✅ Browser eye pass over the same (2026-10-21, with the `X22` pass).

- ✅ **X22 - Centralize audio transitions and effect ordering in Rust.**
  After `X21`, make menu, key and scripted actions enter one coordinator.
  It dispatches edits to `Editing`, updates instance state, orders service
  requests and publishes host corrections. Move policy from Python's `_take`,
  TypeScript's `take` and audio drivers into that shared path; keep socket
  operations, language-object projection and awaiting in adapters. The
  client holds a handle to the application and does not assemble its internal
  lifecycle. Service results re-enter the coordinator to determine next work.
  Reuse the existing generic `application::walks::finish` and `WalkHost`;
  do not introduce a second participant-routing abstraction. Consolidate the
  remaining common repair construction and finish/walk finalization in the
  runtime. Keep domain command decoding, completion decoration and JSON
  projection in the application adapters; file length is not the boundary.
  **Acceptance:** shared action/completion traces produce equal state and
  semantic effect order through both clients and supported host adapters;
  readiness precedes reload, preview reattachment follows restructure, and
  release follows the last dependent effect. Adapters contain no duplicate
  focus/playback/clipboard/close policy; `edit` and supported verbs work.

  - ✅ **The coordinator** (`editing::audio_session`): one per audio member of
    `Editing`, owning that member's `AudioEditorPlayback` (a client no
    longer holds one), the transport facts a script and a hand share (span,
    loop switch, paused) and a stack of runs, so an operation started while
    another run waits (a release that stores, a walk that restores) is a run
    of its own. One door, `audio`: `do` starts a run, `run` and `result`
    resume it; the call carries the server's id spaces, which the playback
    allocates from.
  - ✅ **The effect vocabulary**, answered in order: run steps on the
    server, set props, publish the window drawn again, echo an answer, put a
    document on the clipboard, release a lease, release freed and stored
    takes, project a binding, the dirty flag or the layers onto the language
    object, tell a locate, close the window. A run stops on an **await**
    whose result re-enters it: whether the transport rolls, the files read
    into buffers, an operation's batch, a history step carried out. A failed
    await resumes the run that asked for it; with no handler for the
    failure, the run ends with its error and the adapter raises it.
  - ✅ Ported into it, in Python's order (the reference): event, action and
    member-verb outcomes; focus, close and restructure; sound, meter and
    transport corrections; playback updates and the preview; play, pass,
    follow and cue; new and opened files; clipboard publication and the cut
    continuation; the operation loop with repair; the transport verbs.
  - ✅ Both clients interpret the vocabulary and nothing else: sockets,
    awaiting, reading a `SoundFile`, the language objects. Each records the
    effects it performs, and a trace the Rust tests write is replayed by
    both client suites against the same order
    (`clients/web/tests/audio-trace-vectors.json`, written by the Python
    client's run of the script). The trace found the web client answering a
    window undo before the restore it reads had landed.
  - ✅ Common repair construction and finish/walk finalization move into
    `application::runtime`, beside `walks::finish`: `refuse_turn`,
    `refuse_walk`, `finish_walk` and `Repair::echo`.
  - ✅ Native eye and ear pass over the `edit_audio` pair (2026-10-21): play,
    stop, loop, files opened and closed, cut, paste and undo, save.
  - ✅ Browser eye and ear pass over the same (2026-10-21). The pass found
    the entries of 2026-10-21 in `crates/clausters-gui/PLAN.md` and the `crates/clausters-server/PLAN.md` "Found by use": eight tracks, the page's speed and dialogs,
    a stale spectrogram after undo, a long take that distorts, a profiler.

- ✅ **X23 - Define activity, subscriptions and invalidation.** After
  `X22`, connect application activity to existing host demand/tick machinery.
  Define redraw, clock queries, meter/tap subscriptions and preview analysis
  needs for stopped, playing, processing, hidden and closing states. Separate
  visibility from resource lifetime: hiding a view need not stop playback,
  but must not keep unnecessary drawing or data transfers alive. Audit live
  demand and native/browser scheduling before introducing new machinery.
  **Acceptance:** instrument redraws, queries, transfers and pending work;
  a settled static editor has no recurring work without a documented reason.
  Measure CPU against a recorded baseline for stopped/playing, one/many files
  and visible/hidden views. Use deterministic demand tests plus runtime
  profiles, not an arbitrary CPU percentage or a presumed cause of the report.
  Reactivation updates the view, and closing releases subscriptions/timers.

  - ✅ **The audit** found the host's demand machinery sound and three things
    keeping a settled audio editor busy, in both fronts: the meter of a file
    that was not playing still declared its (negative) buses, so the window
    ticked and drew its live layer thirty times a second; every fed window
    drew a frame per tick whether or not a value moved; and a page polled
    `/transport_query` every tick for a transport that stood. A stage's
    canvas in a page never reported leaving the viewport.
  - ✅ **Activity is the host's** (`crates/clausters-gui/src/host/activity.rs`), one
    rule for both fronts: a window ticks for named reasons (animated, fed,
    playhead, recording, fetching, timer, edge scroll, read-back, node tree,
    MIDI); an element's `tick` answers whether what it shows changed, and a
    tick draws only what moved; a meter that reads no bus declares nothing;
    a transport's pushes say when it rolls, so a playhead over a stopped one
    is watched only for a moment after a push. The application states map
    onto the meter's bus and the playhead the coordinator already corrects
    (`docs/architecture.md`, "Activity: what an open application keeps
    doing"); no client or application schedules a timer.
  - ✅ **Hidden and closing**: an occluded native window and a canvas out of
    view or in a background tab are not ticked, read back or queried for,
    and draw whole when seen; playback goes on. Closing releases the window's
    demand and counters.
  - ✅ **Instrumented** with `/gui_activity` (`host.activity(id)`,
    `editor.activity()` in both clients): ticks, frames by kind and time,
    queries, download bytes, streams, visibility and the reasons in force.
    Deterministic tests drive the editor's own window through the host:
    stopped asks for nothing, playing for its meter and transport, a locate
    is watched briefly, closing forgets.
  - ✅ **Measured against a baseline** (2026-10-21, one 10 s stereo take,
    6 s windows, two runs each). Native host: stopped 1.2% of a core to
    0.0% (no tick, no frame), with one file or four; playing 5.0-5.3% before
    and after (thirty ticks and live-layer frames a second: the meter and
    the play cursor). Page, headless on a software adapter: stopped 3.3% to
    0.3-0.6% and no `/transport_query` (thirty a second before); playing
    about 65% before and after, the page's drawing cost already filed in
    `crates/clausters-gui/PLAN.md`; scrolled away 0.3%. The server's idle cost per
    open file did not move and is filed in the `crates/clausters-server/PLAN.md`, "Found by
    use".
  - ✅ Native eye pass over the `edit_audio` pair (2026-10-21): play, stop,
    loop and locate draw as before, and the activity log read stopped
    windows settle to nothing within two seconds. A covered or minimized
    window was not reported hidden: the windowing library the host uses
    reports occlusion on X11 but never on Wayland, where the pass ran. Filed
    in `crates/clausters-gui/PLAN.md`, "Found by use".
  - ✅ Browser eye pass over the same (2026-10-21): the page behaves as the
    native window, and a tab put in the background or an editor scrolled
    away comes back drawn and still playing.

- ✅ **X24 - Accept the audio editor as the reference application.** After
  `X23`, close the audio lifecycle before migrating another editor. Remove
  superseded audio orchestration and state mirrors. Replay shared traces with
  fake services that delay, fail and reorder completions; use the real server
  to verify edited samples, copy/paste/mix, undo/redo, save and spill/restore.
  **Acceptance:** paired examples cover two windows, multiple files, playback
  across focus changes, meter channel changes, independent/shared history,
  clipboard lifetime/type changes, failed save, edit-during-save and close
  during pending work. Check demand/CPU as well as data correctness. Run
  affected Rust, binding and client checks, native/browser gates and docs;
  inspect the paired examples by eye and playback by ear with refreshed
  native artifacts. An outstanding manual gate prevents reference acceptance.

  - ✅ **Superseded orchestration and mirrors removed.** The editor's JSON
    door lost the verbs the operations replaced -- `save` (which marked a
    file clean before its write), `open`, `apply`, `kept`, `acknowledge`,
    `preview` and a duplicated `binding` arm; a window's save only names the
    destination of the operation it becomes. A stored take always carries its
    spill operation, and both clients lost the spill path without one. The
    coordinator keeps no span or loop switch of its own (they are the focused
    file's), and `dirty` is read from the editor in both clients instead of
    projected by a `dirty` effect.
  - ✅ **Fake services that delay, fail and reorder.** The coordinator's tests
    hold, fail and reorder awaits: an edit while a save's write is out, a
    refused write and its retry, two files' completions in the other order, a
    duplicate completion, a close while a write is out, a cut whose copy is
    refused. Both clients replay one shared fault trace (`faults` in
    `audio-trace-vectors.json`) with a server that refuses and is slow,
    comparing each step's effects and what it leaves.
  - ✅ **The real server.** `tests/audio_coordinator.rs` plays a client of the
    coordinator against an offline server: cut, paste, mix, undo and redo
    read back as samples, a save read back from its WAV, a refused write and
    its retry, a history on disk read back for undo and redo.
  - ✅ **The paired examples** `editors/audio_lifecycle` (`.py`, `.html`):
    two windows, several files, playback across a change of focus, the
    meter's channels, independent and shared history, the clipboard's
    lifetime and a change of type, a refused save, a save then an edit, a
    save then a close, and what each window costs. The scripted steps were
    run against the refreshed native artifacts and in headless Chrome, with
    the same states step by step. The walk found and fixed: a script's or a
    menu's cut, copy and delete refused over a selection set on the take's
    pane, and a paste with no pointer lost in a page whose window fetches
    nothing; a page's window left on an old version after a shared undo
    restored another window's file, refusing its next gesture; and a page's
    `/buffer_write` making the directory a native server refuses.
  - ✅ **Native eye and ear pass** over `editors/audio_lifecycle.py`, by the
    user (2026-10-22): every step as the example says. It found a window
    looping a selection spinning a core on empty frames (fixed) and a paste
    refused in silence when another program's text replaced the copy
    (filed below, in "Found by use").
  - ✅ **Browser eye and ear pass** over `editors/audio-lifecycle.html`, by
    the user (2026-10-22): every step as the example says, once a closed
    window's canvas left the page (fixed). It also found an edit of a short
    take shown about 1.6 s late in a page (`crates/clausters-gui/PLAN.md`, Found by
    use). A write too short for a hand to edit through is checked by the
    fault traces, not by either pass.

- ✅ **X25 - Port the proven lifecycle to the other existing editors.** After
  `X24`, compose multitrack, notes, points and score editors with the same
  contracts, retaining domain-specific state and verbs. Preserve external
  client objects and standalone `Owner` session data. Closing a view does
  not free an object the client retains; the last holder with unsaved work
  follows the common save/discard/cancel rule. Keep notation optional.
  **Acceptance:** roll and symbolic-score views over one sequence share
  edits/history with separate conversations; standalone save/close uses the
  same completion rules; shared actions, forms and clipboard compatibility
  follow the same mechanisms without inventing domain conversions. A points
  editor requires no audio-only service; the common layer builds without
  notation. Each port passes its affected example pair before the next one.
  Demonstrate a second real application through Editing with its typed
  runtime over the same Services, including failure/repair and history walks,
  without copying audio lifecycle orchestration. The synthetic distinct-commit
  runtime test proves service sharing, not this integration acceptance.
  Each editor is a composition of the established modules, with its domain
  adapters and configuration identified. A missing general capability
  extends the shared contract rather than introducing a private mechanism.

  The port runs on the host applications of `X30`–`X34`; both clients are
  handlers. Adapt an existing mechanism, add a real consumer, then remove
  the replaced path. Preserve `docs/app-behavior.md`. The following parts
  are separate review boundaries; this milestone stays open until their
  automated and manual acceptance is complete.

  - ✅ **Common coordination, then the multitrack consumer.** Keep
    non-generic `Services` and each application's `Runtime<Commit, Outcome>`.
    Extract preparation, dispatch, confirmation, refusal, repair and release
    from audio without moving its commands, deltas, playback or result
    decoration. Reuse `Application`, `WalkHost` and `walks::finish`.
    Decode JSON at the client door, use typed calls within the process.
    Identify session opens and source measurements by operation and original
    session; a late response cannot modify a replacement. Save through the
    same confirmation protocol and a general storage operation over disk or
    OPFS. Keep `Editing::start`, `land`, `query` and the application driver;
    results go to their originating runtime, never to handler-owned policy.
    **Acceptance:** unchanged audio traces/results; a real multitrack
    operation over the same services with a distinct delta covers partial
    failure, repair, cancellation, duplicate completion and
    invalidation. Undo/redo uses common coordination and preserves order on
    failure. Prove isolated builds with and without `notation`.
    - ✅ Extract common preparation and turn settlement before changing
      multitrack behavior; audio keeps its result shape and domain decisions.
    - ✅ Integrate typed multitrack operations, storage and history walks;
      retire the replaced routes after the real consumer passes.
      - ✅ Session open/measurement/save and history use their own typed
        runtime over the shared services; verify fault traces, storage
        read-back on disk/OPFS, and both client handlers.
      - ✅ Decode the remaining multitrack action and handle requests at the
        boundary and replace internal JSON dispatch with typed calls.

  - ✅ **Each editor opened on its own is a unit of editing** *(decided
    with the user, 2026-10-30, reviewing this milestone's first branch: one
    `clausters.Session` opens as many editing units as it asks for, each
    window a different thing to its user (two multitracks' sessions, two
    rolls over different sequences, later two plots), and the same
    session in two windows is one session with several views, not two)*.
    The unit is the open document, not the window: an audio editor's window
    holds several files, and a multitrack's session is drawn by its window
    and by the rolls opened from its boxes. No type is added for it and the
    word "session" is not stretched to name it: in a client it is the
    handler `edit()` answers, in the application a member with an order of
    its own. A host multitrack joined the context's one shared order, so
    an undo in one window undid the last edit of another, and a roll's key
    (`sequence:{source}`) named one structure under two multitracks. A
    session now has its own order unless `sharedHistory` composes it, and a
    roll joins its multitrack's order (`Editing::join_view`,
    `open_notes_in`). Opening a document already open in the same host
    brings its window forward: `edit()` already does in both clients;
    the host's own opens (File > Open, `/gui_app_open`, `--session`) are
    part of the file lifecycle below.

  - ✅ **The multitrack's file lifecycle.** Distinguish no session, a new
    session with no path, and a saved session; an empty multitrack is open.
    Open without a session and keep chrome/forms after closing the last.
    Content verbs refuse with `no file is open` without choosing another
    history. Add New/Open/Close file through chrome and `Dialogs`, keeping
    Save/Save as and read-only behavior. Change the one active binding
    explicitly. A save confirms its captured content revision; subsequent
    edits stay dirty and failure preserves path, contents and retry.
    Save/discard/cancel follows retention: unbinding a window does not free
    the client's object or retarget its handles. Preserve opening over a
    supplied `Multitrack`, a session file and standalone.
    **Acceptance:** empty open, New/Open, read/save failure, save-and-close
    and last-session close on disk and OPFS; read saved sessions and source
    references back. Run and review `edit_multitrack` and `load_multitrack`
    in both clients before the next port.
    - ✅ One session file per window. The first branch kept every session
      a window had left as a retained object the client could go on
      editing through the window (object addressing on `query` and `do`,
      every retained session on `/gui_closed`). It is withdrawn: when a
      window lets go of its session, the session's order closes with it,
      the client that opened the window is handed what the window left it
      as (`/gui_app_released`) and settles its object as a close settles
      it, and the window goes on as the host's own, asking before a close
      loses work. The `snapshot` a handler opens over again stays: it is
      one session's. *Done 2026-10-30.*
    - ✅ A document already open in another window of the host is not
      opened twice. The context answers which window holds a session file
      (`Editing::window_holding_file`, the same file however its path
      is written); an in-window open of it (the door's `open`, which the
      window's File > Open will call) refuses and brings that window
      forward, and so does the host's own open. A client's
      `/gui_app_open` of it is answered that window, which it holds from
      then on when no client did, and is refused naming the window when
      another client holds it. `--session` opens the host's first window,
      so it finds nothing; it goes through the same open. *Done
      2026-10-30.*
    - ✅ Wire the empty host opening and New/Open/Close file chrome and dialogs,
      including holder-aware save/discard/cancel and confirmed deferred closes.
      Preserve all three opening paths and verify disk/OPFS read-back.
      The File menu has New session, Open session... (an Open form of the
      shared mechanism, its chooser showing session files) and Close
      session, read as window verbs that are never stale. One change of
      session (`Editing::change_file`) serves the menu, the door's verbs
      and the handlers' `new_file`/`open_file`/`close_file` (`newFile`,
      `openFile`, `closeFile`). A window the host holds alone asks with the
      close form, which now remembers what it is up for; its Save defers
      the change (or the close) to the confirmed write and drops it when
      the session moved meanwhile. A change tells the window the version
      it draws, so a gesture after it is not refused as overtaken.
      `--session` takes no file for an empty host, and the read-only of a
      session opened without `--save-to` stays with that session. A form's
      events reach an empty window. Disk read-back is the host's test
      (`an_empty_multitrack_opens_a_session_from_its_file_menu`), OPFS the
      applications' page (`tests/app-window.html`: an empty window opens a
      stored session, and a second one is refused). *Done 2026-10-30.*
    - ✅ Refresh artifacts and finish the automatic example-pair acceptance
      after the complete binding path is in place. The installed client
      and its binaries refreshed; `edit_multitrack` and `load_multitrack`
      run to their end against a headless host (`[gui] headless`), and the
      pair audit finds the two languages' calls the same. Both describe
      the File menu. *Done 2026-10-30.*

  - ✅ **Roll review and points port.** Review roll actions, clipboard,
    history, conversations, close and invalidation. Extend its existing host
    application to share a sequence with a host-executed symbolic score.
    Keep MIDI/Hz axes, view-local selection and supported playback targets;
    shared contents/history do not merge transports or conversations.
    Compose points with chrome, action definitions/readings and `Dialogs`
    where required, editing the client's curve without adding file verbs.
    Require only its capabilities: `sample_rate` does not require an engine,
    device or playback. Use existing clipboard compatibility and explain
    incompatibility without conversions.
    **Acceptance:** two views share edits/history with independent selection
    and conversations; close/reopen retains the object; points runs without
    audio services. Run and review `edit_notes` and `edit_env` separately in
    both clients before proceeding.
    *Done 2026-10-31.* The review found a host roll's File > Close did
    nothing, since its coordinator never read the close a turn asks for, and
    a roll outliving the session its multitrack let go, drawing and walking
    what no window held; a session let go now takes its rolls and its order
    with it (`Histories::close_order`, also when the window is forgotten).
    Points has its menu bar (`chrome::App::Points`, no tools) and readings
    for every window verb, what a curve does not do refused with why, and
    runs in a context with no engine. Both clients publish an editor's
    readings as its window opens. Sharing a sequence with the host-executed
    symbolic score is the score's own part below.

  - **Where a symbolic score executes** *(decided with the user, 2026-10-31;
    `docs/decisions.md`, "A symbolic score executes in the host")*: in the
    host's application, its engraver with it, and both clients are handlers,
    as for the multitrack. Natively the host links libverovio; in a page the
    application's thread asks an engraver worker synchronously, over the shared
    memory as it asks the file worker, and verovio loads on first use. A host
    with no engraver refuses the open and says why. The parts below run in this
    order:
    - ✅ The application: a score member through `Editing::start`, its
      playback the roll's, and `AppKind::Score` on the application's thread.
      `/gui_app_open score` opens it, natively, with a handler's verbs, its file
      and File > Close through the door; a file a window holds is not opened
      twice, and a build with no engraver refuses it. *Done 2026-10-31.*
    - ✅ The door and both handlers: `/gui_app_open score` and `notes`, the
      client's `Score` and `EventSequence` remote handles while a window holds
      them. The roll came too, since a roll and a page over one sequence are
      one `Editing` (`docs/decisions.md`). `over` opens a view beside a window
      showing a sequence, one undo order with a transport, a selection and a
      conversation each; a close hands back what it leaves; Python reads the
      window's at each read and a page holds a copy it follows, as a
      multitrack's does. Until the engraver worker lands a page's host refuses
      a score, saying why. *Done 2026-10-31.*
    - ✅ The page's engraver worker, which also gives a page's multitrack its
      boxes as pages. The pinned verovio in a worker (`apps-engraver.ts`) the
      application's thread asks synchronously over the shared memory, fetched
      the first time it is asked. *Done 2026-10-31.*
    - ✅ Standalone on the same application, the replaced paths removed:
      `--score` opens `AppKind::Score` over its file, its Save where
      `--save-to` says, and plays on the application's engine rather than
      through a player beside an on-demand session. `document::Owner`, the
      host's own score editor and its playback went. *Done 2026-10-31.*

  - ✅ **Symbolic score: actions and execution.** *Done 2026-10-31*: execution
    is the host's, in the parts above, and a window reads its actions
    (`ScoreEditor::actions`, read by `action_states`). Its menu bar, tools and
    palettes keep their own composition, since no reading is lost and their
    layout is preserved; a roll and a page over one sequence are tested
    through the door in both views. Compose shared chrome
    and action readings, preserving tools, palettes, keys and layout. Menu,
    keyboard, tools and client calls use the same actions. Move coordination
    and playback into the host application, reusing notes playback and
    existing rules. Extend application doors; clients retain public verbs as
    handlers. Register roll/page over one sequence in one `Editing` under one
    structure identity. Reuse multitrack remote handles, preserving object
    identity and the web client's documented asynchronous adaptation.
    **Acceptance:** extend `a_roll_and_a_page_over_one_sequence_are_one_order`
    for edits and undo/redo from both views, separate conversations, stale
    events, close/reopen, and the host route through both handlers. Run and
    review the `notation/score_editor` pair before the next part.

  - ✅ **Symbolic score: dialogs.** *Done 2026-10-31*: the score's window
    holds a `forms::Dialogs`, and a window drawn again now shows the form
    that was up, including a question of a dialog several forms share (the
    mechanism draws a page as the form up, not the one naming the page). Move
    `forms::Spec` forms into `forms::Dialogs`, removing duplicate
    open/value/accept/cancel state. Preserve content, semantics, modality,
    focus, redraw restoration and rejection of late answers. **Acceptance:**
    run and review the `notation/score_editor` pair before the next part.

  - ✅ **Symbolic score: MEI files and standalone.** *Done 2026-10-31*:
    `score::operations` puts Save and Close file on the shared runtime, its
    settling walk shared with the multitrack (`settle_operation`). A window
    opened over nothing has no document, and `--score` takes no file. Save
    confirms the prepared revision before what waits for it. **New and Open are
    not edits** (changed on the user's word, 2026-10-31, from this entry's
    "keep their history contracts"): which document a window holds has no
    history, in any application, so they replace the document and its order, by
    the rule `closing::change_file` shares with the multitrack; only a script's
    `Score.load(mei)` stays an entry. Distinguish no
    document from a new symbolic score, with one active document per window.
    Use common runtime open/save/checkpoint/close. Read or engraving failure
    leaves the current document intact. Close file explicitly unbinds. Save writes the
    prepared revision and confirms it before a pending close proceeds.
    Adapt standalone to the same application while preserving `Owner` data
    and responsibilities; remove replaced coordination and playback from
    clients and standalone. **Acceptance:** read back saved MEI and contents;
    run and review the `notation/score_editor` pair in both clients.

  - ✅ **Cross-application verification and manual acceptance.** Exercise
    delayed, failed and reordered service replies; assert contents, history
    and resource retention as well as effects. Use existing activity and
    playback instrumentation for stopped, hidden and closed windows, with no
    client timers. Run affected suites, bindings/ASCII, fmt, warning-free
    clippy, applicable matrices, wasm gates, docs, Python pyright and web
    build before tests. Refresh native artifacts before manual review. Read
    each Python/web example pair side by side, call by call; ordinary browser
    acceptance is Chrome. Check two windows, time limits, vertical navigation,
    selection, playback, saving and history. Each reviewed part commits its
    endpoints, tests, docs and examples together; unresolved findings go at
    the end of their owning plan with checkboxes. Close this milestone and
    remove its ROADMAP entry only after every acceptance passes.
    *Automatic part done 2026-10-31.* Fault traces cover each application and
    three of them at once (`saves_of_different_applications_land_each_on_its_own`,
    the score's slow, failed, late and invalidated stores, the audio close form
    end to end on a real host); stopped and closed windows of every application
    ask for nothing (`a_stopped_or_closed_window_of_any_application_asks_for_nothing`),
    and a hidden one streams nothing (`components.html`); every application's file
    verbs run against the real host from both clients; the affected pairs were read
    call by call and run to their end. What it found is fixed: a save over a file
    on another filesystem than the working folder failed, a second move that did
    not land left the original in that folder, a stored document was not written
    whole, a refused operation's corrections never reached its window, the audio
    close form stayed up through the write where the others go down, and File >
    Close on a multitrack with no session was refused.
    *Manual part closed 2026-11-01, by the user's decision.* The multitrack, the
    notes editor and its page, the curve and the score editor were gone through
    by eye and ear in their windows, the audio editor's having been accepted in
    `X24`; the feature matrix is the user's. What it found is fixed, each in its
    commit: a tool holding the keys, a click not letting go of a time range, a
    control and the curve over its parameter disagreeing, a window not told the
    version a script's edit moved it to, a page over a sequence not playing that
    sequence, a curve window out of step with its script, a window's handle
    hanging the encoder on File > Close, a combobox keeping the keys, a box under
    its text, and the note a locate lands on not sounding on an engine the
    application holds. Left open at the end of their plans: this file's "A
    multitrack's window does not say whether its session is unsaved" and "A new
    session's first Save or track failed once in the eye pass", and
    `crates/clausters-gui/PLAN.md`'s "A pass can play with no play cursor until a frame
    happens to come" and "A control's size does not follow what it says, in a row
    or across one".

  Out of scope: new notation operations, domain conversions, the roll's
  pending recording, new session formats and persistent recovery. The second
  real application is the multitrack; symbolic scores execute in the host
  while preserving shared objects and views.

- ✅ **X26 - Use scoped services from simple GuiDef instances.** After
  `X25`, provide the lightweight instance context for synthesis panels
  without editor state. Before parameter undo, define its owner/baseline,
  gesture coalescing, bound-target identity, external/MIDI changes, missing
  targets and what reversing a live parameter means. Use the ordinary binding
  path; widget value and server parameter cannot become two undo targets.
  Ordinary GuiDefs remain valid without history; compatible subtree redraws
  retain instance services. Wire extensions land with reference and clients.
  **Acceptance:** one paired panel example exercises opt-in undo and the
  shared clipboard; instances stay independent, a drag is one entry and
  external updates/target loss follow the specified policy. Loading an
  authored GuiDef starts a fresh runtime context without silently restoring
  previous session resources or history. No new server process is launched.

  **The decisions** *(taken 2026-11-02 with the user; `docs/decisions.md`,
  "A live parameter's undo writes the value it had")*. What exists under
  them: a `/gui_bind` sends a widget's value straight to the server or to
  another widget, the script out of the loop (`host::bind`); a binding to a
  node's control is read back every 200 ms, leaving alone the widget a hand
  holds and the field the keyboard is in, and stops at the node's
  `/node_end` (`host::readback`); `undo` and `redo` on a plain window are
  reported to its owner, the host holding no history (`docs/gui-protocol.md`,
  the events table); the host-wide clipboard is the document crate's typed
  one (`host::clipboard`); and the orders are `application::Histories` over
  `clausters_document::history::History`, which the host links already.

  - ✅ **The owner, and how a window asks for it.** A window's `history:
    true` gives that instance an order of its own, kept by the host (the
    document crate's `History`, one per window, not a second history type),
    since a binding never passes through the script that would otherwise
    keep it. With it, the host performs `undo` and `redo` on
    that window instead of reporting them; a script asks for them through
    the door it already has, `/gui_verb id undo`. Without it nothing
    changes. Two instances are two orders; composing several into one is
    not offered until a use asks for it.
  - ✅ **The baseline.** An entry holds the value the target had when the
    gesture began, as the widget showed it, and the value it left. No state
    is read from the server to make one.
  - ✅ **A gesture is one entry.** A hold, from press to release, is one
    entry however many values it sent. A run of wheel or key steps on one
    control is one entry while it stays the order's newest and no other
    target, undo or redo comes between, as a run of typing is one step of a
    field. A text field keeps its own history while it holds the focus
    (`docs/app-behavior.md`); what it changed from focus to blur is one
    entry of the instance. A command (a button, a click) is never an entry.
  - ✅ **The target is the parameter, never the widget.** A server binding's
    target is its address and prefix (`/node_set 1001 freq`: the node's
    control), the key the read-back already matches replies by, so two
    widgets bound to one control are one target and a widget and its server
    parameter are never two. A widget bound to a widget is its own target,
    and its binding applies the walked value to the other as a hand's change
    would. An unbound control is its own target too; walking it delivers
    the `/gui_event` a hand would, so the script cannot tell the two apart.
  - ✅ **A walk writes the stored value, absolute.** An undo writes the
    entry's first value and a redo its second, through the widget's
    ordinary delivery (its binding, or its event), whatever the parameter
    holds by then. Another writer that keeps driving it (an automation, a
    bus, a controller) takes it back on its next write, as the field's
    convention has it.
  - ✅ **What another writer does is not an entry.** A value that reaches a
    widget by read-back (MIDI through the server, another client, a
    plugin's own window, a bus) or by the script's `/gui_set` makes no
    entry and moves no entry. A recording pass is one entry of the
    application that records it, never of a panel.
  - ✅ **A lost target leaves the order.** At a node's `/node_end` the
    entries naming its controls are dropped; a node later made under the
    same number is another target. So are the entries of a target no widget
    of the instance writes any more (a widget freed, or bound elsewhere),
    since the instance writes only through its widgets.
  - ✅ **The instance lives as long as its window.** A `/gui_def` over the
    open window of the same id keeps its order, and an entry whose target a
    rebuilt widget still writes stays walkable. Freeing the window ends it.
    `/gui_load` of an authored GuiDef, and any new window, start a fresh
    one; `history` is authored, the entries never are.
  - ✅ **The clipboard is the host's one.** Copy on a focused control puts
    its value on the host-wide clipboard as text, which crosses to the
    platform's; paste on a control reads a number from text and is an
    entry. Any other kind held (samples, elements) is refused, naming what
    is held, with no conversion.

  - ✅ The instance's order and the walk, in the host, with its tests.
    *Done 2026-11-02* (`crates/clausters-gui/src/host/panel.rs`, its tests in
    `gestures/panel_tests.rs`): the `history` prop, an entry per hold
    recorded where every value is delivered (`effects::deliver`; a walk
    goes through `route` and records nothing), the run closed at a press, a
    release and a walk, the baseline kept from the def, every set and every
    entry, the walk in the window's verb before its owner is told, the
    target lost at its node's `/node_end` or when no widget writes it, and
    the order kept across a redefinition and dropped with the window. The
    prop's reference is in `docs/gui-protocol.md`.
  - ✅ What the first part left. *Done 2026-11-02*: a run of wheel or key
    steps is one entry until another target comes between or the keys
    leave the control, and a text field's typing is one entry from focus to
    blur, a press that moves its caret included (the run closes as the
    focus leaves, in `Host::focus` and `Host::clear_focus`), each with its
    test.
  - ✅ The clipboard on controls. *Done 2026-11-02*: copy and paste on a
    focused `knob`, `slider` or `number` (`elements/control.rs`, shared by
    the three), the paste one entry of a window's order and closed off from
    the runs around it, other kinds refused naming them; the reference is in
    `docs/gui-protocol.md`, "The clipboard, and who may read what".
  - ✅ A load starts a fresh instance. *Done 2026-11-02*: `/gui_load` drops
    the order of the window it reopens before defining it, so loading over
    the open window of its id restores the authored `history` and none of
    the hand's entries (`host::wire`, with its test).
  - ✅ The `history` prop in `docs/gui-protocol.md` and both clients'
    builders, the paired panel example and the books. *Done 2026-11-02*:
    `view(history=...)` and `view({ history })`, their sweep vectors, the
    pair `panels/undo.py` and `panels/undo.html` (two bound panels: a drag,
    a field bound to the knob's control, a script's set, the shared
    clipboard, a synth freed), and a section in each client's GUI chapter.

- ✅ **X27 - Finish cross-application validation and documentation.** After
  `X26`, check there is one owner and one policy implementation per contract,
  and remove remaining obsolete coordination. Complete the architecture,
  behavior rules, binding table and affected user docs; curate non-obvious
  decisions and keep unresolved designs at the end of their owning plan.
  **Acceptance:** relevant crate tests, fmt/clippy, feature/wasm gates, Python
  call-site checks, web build before tests, binding parity and all three
  books pass. Existing examples show equivalent behavior in both clients;
  native/browser visual and audible gates are named and completed where
  applicable. The audit names no required work deferred to private notes;
  no shipped claim rests solely on automated checks.
  The developer guide explains how to compose an application and add an
  action, view or service capability using the existing contracts. Verify
  that an additional option in a migrated editor requires no new lifecycle
  orchestration in clients or application-specific shared infrastructure.

  *Done 2026-11-02.* The audit found one contract with two owners left: the
  clients still carried the clipboard-exchange and resource-cleanup
  adapters of the time the audio editor ran in them, reached through six
  verbs of the context's JSON door, though the host's application had
  called both typed since the coordinator moved there. They and the verbs
  are removed. `docs/architecture.md` now states the contract as built: who
  owns each state and service (one row each), every regression scenario of
  the audit with the test that checks it, what stays outside (the two
  Future directions entries on workspaces and recovery), and a guide to
  composing an application and adding an action, a view or a service
  capability, checked against the spectrogram settings added to the audio
  editor after its migration (application crate and the handlers' door
  only). `docs/bindings.md` no longer lists the removed verbs. The two
  "Found by use" entries left to this milestone are fixed: the clients'
  walk of steps refuses the file steps only the application carries out,
  and the audio editor and the multitrack are `HostedEditor`s in both
  clients. No required work is left outside the plans.

## Future directions: the entries that were done

- ✅ **The roll and the score editor as applications of their own** *(the
  user, 2026-10-29, after the eye pass over the multitrack on its
  application)*. The audio editor and the multitrack run on the
  application's thread, with its engine; a roll opened from a box runs
  there too, as a notes member with a transport of its own, but a roll or
  a score opened from a script, and the score a standalone host opens
  (`--score`), still run where they did -- a client's context, the host's
  own owner and its server. Porting both onto the application is what
  would let a box drawn as its page open the score editor again (Found by
  use, "A box drawn as its page opens its roll"). To weigh after `S23`.
  *Done 2026-10-31 in `X25`* ("Where a symbolic score executes"): a roll
  or a score a script opens runs in the host's application
  (`/gui_app_open notes`, `/gui_app_open score`), a roll and a page over one
  sequence are one order there, and `--score` opens `AppKind::Score`; the
  host's own owner and its score editor went.

- ✅ **From the roll to the score** *(the user, 2026-10-05, planning `X5`:
  the conversion to a roll goes one way, and the way back takes decisions
  that can be made later)*. `X5.8` renders a score into an `EventSequence`;
  reading one into a `Sheet` is the other direction, and each part of it is a
  decision: snapping onsets to written values (and to tuplets), spelling
  pitches the events do not spell, finding voices and staves, reading curves
  back as dynamics and hairpins (a hand-drawn curve is no hairpin), and what a
  sequence the roll edited means to the score that rendered it -- whether the
  score is re-read from it, or the edit stays the sequence's. With it, `edit`
  opens a sequence in the score editor (`view`, the presentation a sequence
  has beside the roll), including one built from a client's plain numbers.
  The notation keys (`X5.7`) are what makes the exact case exact: a sequence
  whose events carry them is read back with nothing to decide.

  **Decided 2026-10-06, with the user, and taken with the entry below and
  with `N9` (`crates/clausters-gui/PLAN.md`) as one sequence of work:**

  - **The sequence is the structure, and the page is a reading of it.** The
    reading is a function in Rust, beside the render
    (`clausters_document::events::score`), that decides only what the events
    do not say; it replaces the reduction each client wrote
    (`sheet_from_timeline`, `sheetFromTimeline`). **It never rewrites the
    sequence**: what was played from a keyboard keeps the times and the
    lengths that arrived, and how finely it is snapped is a parameter of the
    reading, changed without touching the data. The meter is given, not
    found, and tuplets are asked for: finding a meter or an irregular value
    in a performance takes an algorithm this does not attempt.
  - **An edit on the page changes in the sequence only what it changed on
    the page.** The page is rendered before the edit and after it, and an
    event the two renders agree on is a note the edit did not touch: its
    event in the sequence stays as it was, with its time, its level and its
    curves. A note the edit touched takes the render's event, notation keys
    and all, so what was decided on the page persists in the sequence. The
    other answer -- the sequence replaced whole by the render -- was weighed
    and left: the first edit on the page would snap every note of a
    performance.
  - **A curve is a note's or a channel's, and that is the criterion both
    ways**, whatever MIDI specification the sequence names: a note's curve is
    a note's mark (a bend straight to the next note a glissando, a level
    moving inside the note a hairpin over it), a channel's is its voice's (the
    dynamics, the hairpins, the pedal), and a curve that is not the shape a
    mark renders as stays a curve of the sequence. In MIDI 1.0 and 2.0 a
    channel says the voice; in MPE a channel is a note's, so the zone is the
    part and its voices are found.
  - A score held as itself is as it was: saved as MEI, and `render_events`
    one way.

  **The steps:** the reading, bound in both clients as the score's
  constructor from a sequence; `edit(sequence, view="score")` with the edit
  written back as above; then `N9`. *(All of it done 2026-10-06; `N9` is
  built, and closed in its own plan with its eye pass on 2026-10-28. What each step left
  open is listed under it.)*

  - ✅ **The reading.** *(Done 2026-10-06.
    `clausters_document::events::transcription::read`, behind the crate's
    `notation` feature and bound as `clausters_core_sheet_read_events`: a
    sequence and a `Transcription` -- the meter, the key, the clef, the beat,
    the smallest value (`division`, a sixteenth), the tuplets admitted, the
    most voices on a staff, whether levels are read -- answered as the sheet
    and which item each event became. An onset is snapped to its beat's grid,
    and a beat is a tuplet's only where one is asked for (or stated by an
    event's `value`) and fits its onsets clearly better, the ends weighing a
    quarter of an onset; a note is cut and tied where such a beat ends, since
    a tuplet fills its beat. A staff is a channel and a voice is found, a
    note that starts under another going to the next; in an MPE zone the
    staff is the zone. A number is spelled by the key -- the signature most
    notes are in where none is given -- on the line of fifths, four under the
    tonic to seven over. A level is read from a curve of steps and straight
    ramps on the dynamics' controller, else from the notes where it changes;
    a line at one level is written with no dynamic. **The exact case**: the
    render's section gained the staves as they are written, `controls` and
    `groups`, and `marks` -- what a note carries that no notation key says --
    so a rendered score reads back the same MEI. `Score.from_events` and
    `sheet_from_events` in both clients are this reading, and the reduction
    each client wrote is deleted; `from_timeline` and `from_notes` are the
    same reading under the names they had, so a chord under a melody is now a
    second voice where it was clamped to one layer, and each note of a chord
    keeps the spelling its own event states. **The emitter changed with
    it**: a run of tuplet values is a group to each written value it fills,
    where it was one group as long as the run, which no barline could cross.
    Core ABI 85.)*
    **Left open by it**, each for when it is met:
    - a level is named by the interpretation's own table, whose amplitudes
      are a synth's: a take's MIDI velocities all fall at its top, so a
      caller reading one hands in a table of its own. It is the entry
      "Dynamics need a model of the instrument and of hearing".
    - a score's repeats are played out in its sequence, so one with repeats
      is read written out, on its first meter alone;
    - a control standing on a rest has no event to name and is not in the
      section;
    - an end that falls in a beat with no onset is snapped to `division`,
      tuplets or not;
    - the keys are the fifteen major signatures, as the model's are.
  - ✅ **`edit(sequence, view="score")`**, the edit written back.
    - ✅ **The write-back.** *(Done 2026-10-06.
      `clausters_document::events::writeback::write_back`: the page rendered
      before the edit and after it, compared item by item. An item both
      render alike leaves its events as the sequence has them; one they
      render differently takes the difference alone -- each key the edit
      changed, written with its family's coherence and the written pitch
      last; a move in time added to where the event was played; its curves
      where they differ. An item only the page after has is new events,
      played by what plays the sequence, and one only the page before had is
      events removed. A curve the two renders write alike stays the
      sequence's own, and one they write differently is the render's, found
      by what it drives and on which channel, whatever it is called. What
      the page says of itself goes into the `notation` section part by part
      where the edit changed it, what has two ends naming the sequence's own
      events; a sequence that had no section gains one only then, and never
      an `items` list, which is what says a sequence was rendered. With it,
      the reading lets an event that states no `staff` take its channel's
      beside others that state theirs.)*
    - ✅ **The editor over a sequence.** *(Done 2026-10-06.
      `ScoreEditor::over` in the crate: the score loaded with the sequence
      read, and from then on an edit of the page written back and recorded
      **as the sequence's** -- its leg is in the events' vocabulary, put back
      by restoring the sequence -- under the sequence's own key, so the page
      and every roll over the sequence are one structure in the order and one
      undo. The score is held while the two agree, so the ids a selection
      names stand across the page's own edits; it is read again when the
      sequence is no longer the one it agreed with -- a step of the history,
      a note moved in a roll, a script's change -- which every verb of the
      editor checks first. `clausters_apps_editing_open_score_over`, core ABI
      86; `edit(sequence, view="score")` in both clients, with the
      transcription's keys as its options, `ScoreEditor.over` the same as a
      constructor, and `view` refused for a presentation a structure does
      not have. What plays there is the sequence itself.)*
      **The page is on the sequence's time** *(found by the user the same
      day, at the first look: the page's line ran twice as fast as the
      rolls')*: an engraver counts a page at 120 quarters a minute where the
      score states no tempo, and what plays over a sequence is the sequence
      on its own map -- a beat a second where it states none. The editor
      takes each time of the page to the beat the page puts it at, and from
      there to where the sequence's map puts that beat.
      **Left open by it**: the standalone host opens no sequence on a page,
      having no door that asks for one.

- ✅ **A sequence as primitive data, by the keys chosen** *(the user,
  2026-10-05, planning `X5`)*. A sequence the score editor or the roll made
  holds everything in its events, and a client wants it back as its own plain
  data -- `[(note, dur), ...]`, or any other set of keys. So the verb chooses
  the keys and yields a list of tuples (or of rows) in that order, in both
  clients, over the events. It waits for `X5.7`, since the keys it chooses
  from are the ones that table settles. Open: what an event without one of the
  chosen keys yields, and whether a chord is one row or several.
  **Done 2026-10-06.** `sequence.to_rows(*keys)` (`toRows(keys)`), over
  `clausters_document::events::rows` through the door the sequence already
  had, so no symbol is new. **A key is read as the event means it** -- a
  `midinote` from a `degree`, a `freq` or `pitches` alone, an `amp` from a
  `velocity`, a `sustain` through `dur` -- `at` and `id` are the event's own,
  and **a key the event does not hold is null**. **A chord is several rows or
  one, by the shape asked for**: a row per event is the sequence as it is
  placed, and `line=True` is one line played back to back, as a pattern
  writes it -- notes that start together one row whose differing keys hold
  lists, `dur` the time to the next row, a silence a row of its own.
  `EventSequence.from_rows` reads either back, which is what makes a list of
  plain numbers a sequence. **With it, by the user the same day**: the voices
  of a score are sequences of their own -- `sequence.separate(by)`, by voice,
  staff or channel, each part with its curves and what the whole says of its
  page -- and the MIDI specification follows one rule
  (`EventSequence::midi_fit`): MIDI 1.0 where no note carries a curve of its
  own, MPE where one does and the sequence is one line, MIDI 2.0 where it is
  several. The render names its sequence by that rule, so a one-voice score
  with a glissando is MPE now, where it was 2.0.

- ✅ **The applications' window chrome in standalone** *(recorded 2026-09-24,
  the user: "to be able to run the host's applications standalone, the window
  will need to have chrome, and that is not implemented yet")*. A standalone
  host runs the applications with no client beside it, so whatever a script
  would set — a switch, a mode, a setting — has to be reachable from the window
  itself, and today the windows have no chrome for it. The audio editor's loop
  is a key and a status line precisely because there is none (X8). What the
  chrome is, and which of it is the application's rather than the widget's, is
  the design. *(2026-10-04: the elements it is composed from exist in the host
  — a window's menu bar, a context menu, tools, tips, tabs, sections and a
  dialog (`G37`–`G40`, `crates/clausters-gui/PLAN.md`; the `panels/chrome` example holds
  one of each). Which entries an application puts in them is still this
  entry's.)* *(2026-10-06, the user, with the score editor the one application
  that has chrome: an application run standalone has its chrome, since that is
  how it is handled there; run from a client it is optional -- the entry below
  -- so what this entry designs for each remaining application is the
  standalone window, and a client opening it bare is part of the acceptance.)*

  **Done 2026-10-06** for the audio, multitrack and notes editors, with the
  score editor as the model and the field's menu conventions (the user asked
  for both): **File** (Save where the window writes its work, Close), **Edit**
  (Undo, Redo; Cut, Copy, Paste -- and Paste mixed in the audio editor --,
  Delete; Select all; Split, Join, Quantize in the multitrack and the roll),
  **View** (Zoom to fit; the multitrack's Reset track heights and Compact
  tracks), **Transport** (Play or stop; the multitrack's Pause and Stop; Go to
  start, Go to end; Loop), **Track** in the multitrack (Add track) and **Help**
  (Keyboard shortcuts). A toolbar under it holds the transport -- the host's
  verbs as tools, or the multitrack's own row by name with its clock -- and
  the edit tools. Composed once in `crate::chrome` and dressed onto each
  window with its `main` view; the vocabulary is the one decided in
  `crates/clausters-gui/PLAN.md` ("The whole interaction vocabulary is provisional").
  **Bare from a client**: `"chrome": false` in each open request, passed by
  both clients from the editor's `chrome`; the multitrack's transport row then
  stays under it as before. A pick reported as `"menu" <verb>` is read as the
  verb (`Converse::reads_menu_as_verbs`; the score editor keeps reading its
  own). The standalone host numbers the tools of its own windows, which a
  client does on the way out.

- ✅ **An application opens without its chrome from a client** *(asked for by
  the user 2026-10-06, over the score editor, as a rule for every editor once
  it has chrome: it must be possible to run one from a client with no chrome,
  the client editing through the handle; the application is lighter that way;
  the keys are kept; and what matters most in editing a roll or a score from
  a client is reading back the notes or the events a hand edited, to add to
  or process in code)*. **Done 2026-10-06 for the score editor**, where the
  chrome is: `"chrome": false` in the open request (`ScoreEditor::set_bare`)
  opens the page in its scroll and nothing else. The switch is in the crate,
  where the window is composed, so a client passes one option through: a bare
  editor names no tool, palette entry or dialog for the caller to number, and
  composes no menu bar, no status line and no symbol table. **Absent rather
  than hidden** -- the user's condition for hiding was that it cost nothing,
  and what is not composed is not engraved for, sent or corrected: the
  example's window is 3 widgets where the whole one is 233, a def of 26 kB
  where it is 113 kB, and a turn that moved the input state corrects the
  window's `keys` alone rather than every tool and the whole menu bar. The
  keys stay because they were never the menu's: the window's `keys` prop
  names scopes of the host's table. Both clients: `chrome` on `Editor` and on
  `edit` (`chrome=False`, `chrome: false`), on the base class so the option
  is every editor's and an editor with no chrome opens the same either way;
  the default is the whole window. Reading back was already there and is now
  the example's step (`alone()`, the page's *page alone*): the score the
  client holds is the edited one (`sheet`, `render_events`), and `on_change`
  is told once per gesture. What it left is the entry below.

- ✅ **A bare window does not say why a verb was refused** *(left by the
  entry above, 2026-10-06)*. The reason a refused verb has is shown on the
  status line, and a bare window had none: the verb answered `false` and the
  reason reached nothing. A gesture's refusal reached the host's own status
  bar, through the acknowledgement of its stamp; a verb a client calls has no
  stamp, so its answer settled nothing and the host dropped the reason.
  **Decided 2026-10-06, by the user**: the bare window keeps the status line
  the whole window has.
  **Fixed 2026-10-06**: a bare editor composes the status line under the page
  (the page, its scroll and the line), and a verb refused through the handle
  corrects the line with its reason -- in the whole window as well, where the
  same reason was dropped the same way. Pinned in the crate and in both
  clients' tests.

- ✅ **A window shows its keys with F1** *(asked for by the user 2026-10-06:
  a window with no chrome shows its shortcuts with F1, in a modal window that
  scrolls, to look them up quickly; the modal closes with its own close mark
  or Escape)*. **Done 2026-10-06, in the host**, where the key table is: `keys`
  is a host verb bound to F1, and it opens a sheet in the popup layer, in the
  middle of the window, under a title strip that ends in its close mark. It
  lists every key in force in the window -- a section per scope the window
  names, the one read first at the top, then the table's own rows -- with a
  chord a scope takes over listed only where it holds; the table's own verbs
  read in words and any other verb as its name. Nothing on it is picked: a
  press anywhere but the close mark is swallowed, the arrows, Home and End
  scroll it, and the close mark and Escape take it down. Every window has it,
  since the table is every window's; a bare one has no menu to show a chord
  beside an entry, which is why it was asked for there.

- ✅ **The multitrack's stop-at-end in standalone: a key, saved in the
  session** *(decided by the user 2026-09-24; out of X8; done 2026-10-07)*. The switch exists
  (`MultitrackPlayback::set_end`, bound in both clients as `Playback.end`); the
  standalone host needs a key that flips it and the session to keep it, so a
  reopened session stops where it stopped before. *(The chrome it waited for
  landed 2026-10-06: the switch can be a Transport entry and a tool beside
  Loop.)*
  *(Done, with two things the user fixed on the way, 2026-10-07. **Where it is
  kept**: a field of the session, `end`, in the three forms a playback's end
  already has -- absent for a pass that rolls on, `"contents"`, a number of
  seconds for an end marker. Not in a view, since it changes what is heard,
  and not in the multitrack, since it is no edit. The type is the document's
  now (`clausters_document::End`), and both clients' `Session` carry the
  field. **An end marker is carried by the contents**: a region placed past
  one takes the marker with it (`End::carried`, applied by the playback at
  each sync), while a marker put inside the contents is an early stop and
  stays. **An end bounds no view**: it is where a pass stops and nothing
  else, sent to the server's transport and read by no window, so the time
  axis scrolls and zooms out past it.
  The switch is the window's verb `stop_at_end`: `Shift+L`, declared by the
  window in a scope of its own of the key table, *Stop at end* in the
  Transport menu and a tool beside Loop. It flips the end between open and
  the contents; the editor keeps no copy, and whoever holds the playback
  flips it there. The standalone host reads the end with the session, tells
  the playback when it makes one, says the switch on the status line and
  writes it on a save, with no server needed; both clients flip their
  playback's `end`, and a script copies it into `Session.end` when it
  saves.)*

- ✅ **Effects in preview in the audio editor** *(out of X7; removed
  2026-10-23 with the gain envelope's preview, the turn to disk's eighth
  decision)*: the first
  insert is the take's gain envelope, heard through `ReplaceOut` and a
  linear-ASR `LinXFade2` bypass, then paused at exact dry. Preview on/off,
  remove and Apply are distinct verbs. The processed picture uses a scratch
  buffer through the existing signal's `preview_buffer` prop.

  **Resume and order.** Resume preserves effect state; transport-addressed
  automation reads the current position immediately, including after a locate
  or loop wrap. A future stateful insert retains its frozen tail as well;
  removing and enabling it creates fresh state. Chain order is the stored
  insertion order of the read/replace nodes. A reorder rebuilds the affected
  suffix in its requested order, with the same port values and fresh DSP
  state, rather than issuing a forbidden manual move in an auto group.
  The first chain has one gain insert and no reorder control.

  **Ownership.** The playback owns the dry bus and gain synth id. Readers
  and pass use that bus through their ordinary ports; auto order supplies
  their dependency order. A synth paused by its ASR can then be resumed by
  its own id, unlike a private member hidden under a slot group. The gain
  table and scratch picture are made with the same buffer gain operations
  as Apply and freed with their insert. Preview switches are outside undo;
  applying and the curve edits retain the existing take history.

- ✅ **Several files in one audio editor** *(out of X7; closed 2026-10-17)*.
  The Rust application holds the open files; a hideable `table` left of the
  editing view lists them (View -> Files). `open_file` takes a path or a list;
  the Open form is the host's `files` chooser with Ctrl/Shift selection; New
  file adds to the editor. Each file keeps its parts, gain curve, cursor,
  selection, layers and its own undo order (`docs/decisions.md`, "The files
  an audio editor adds walk orders of their own"). A switch stops the one
  server transport and locates it to the file's cursor.

- ✅ **A roll with no sequence: whether the bare `pianoroll` stays** *(the
  user, 2026-10-01, deleting the bare `multitrack` builder in
  `clients/python/PLAN.md`, `C57.0`; one question of `X6`; decided and done
  2026-10-07)*. Both clients
  still build a `pianoroll` widget by hand, over no `EventSequence`: in
  `editors/pianoroll` and `editors/pianoroll_midi`, and in a column of
  `panels/gestures`. The two editor examples predate the notes editor (`X3`)
  and teach what it no longer is — their docstring had the OSC markers
  edited by hand, which they no longer are (`C57.0` names them `OscMarker`) — and `edit_notes` and
  `edit_midi_file` show the same through the application. Open: whether a
  roll drawn with no sequence has any use (a multitrack's had none: an edit
  has nowhere to live), and so whether the builder and those two examples go,
  as the bare `multitrack` did, or stay as a view.
  *(Decided by the user 2026-10-07: it goes -- "it is old code that is no longer used that way". Removed from both clients in one commit: the `pianoroll` builder
  and its model name `notes`, the curve-point helper only it called, the two
  editor examples and their pages, and the roll column of `panels/gestures`,
  which now shows the table over a ruler and a waveform. The host's `notes`
  element stays, as the notes editor's; a `Source` still carries `notes` and
  `osc` through the generic `node`. What that left open is the entry below.)*

- ✅ **A roll that paints what is played has no door and no example** *(left
  2026-10-07 by the entry above; done the same day)*. The host's roll takes `midi_in`: it opens
  a MIDI port and paints incoming notes into the roll, reported as ordinary
  `"notes"` events. The bare builder was its only named door and
  `editors/pianoroll_midi` its only example, and both are gone; the notes
  editor's window states no `midi_in`. Reaching it now takes the generic
  `node("notes", midi_in=True)`, over no sequence. It belongs with the
  editor's recording (`X3.10`, through `PLAN.md` `T10`): whether a roll that
  records shows the notes as they arrive through this prop, or only the take
  the server wrote.
  *(Done: the door is the notes editor's. `midi_in` is an option of the
  editor -- `edit(sequence, midi_in=True)`, `midiIn` in the web client -- which
  the crate's editor states on its roll and in every correction of it. What the
  host paints comes back as the `"notes"` a hand's edit reports, so a played
  note is an edit of the sequence: in the history, undone with the rest, and
  drawn in every other window over it -- which the bare roll, over no sequence,
  could not say. `editors/edit_notes` opens its first roll that way, in both
  clients. Painting from a script needs no door at all now: a `MidiFunc` that
  adds events to the sequence redraws the roll, which is what the removed
  example did by hand with `/gui_set`. The recording question stays where it
  was, with `X3.10`: this is the host's live input, at the play line, and a
  take the server records is another thing. The host in a page opens no MIDI
  input, so there the option is carried and nothing is painted:
  `crates/clausters-gui/PLAN.md`, Found by use, "The host in a page opens no MIDI
  input".)*

- ✅ **The roll as an editor of everything a sequence holds** *(designed
  with the user, 2026-11-10, after studying three rolls: one that writes
  articulations and a continuous pitch over its notes, and two conventional
  MIDI editors)*. What the roll lacks is not new structures but the editing
  and the seeing of the ones a sequence already has: its notes, their
  written data, the curves of its channels and those of each note. The
  principles first, then the work, in two groups. It carries items 1 and 10
  of "The roll's two modes are not yet an editor a hand can trust", in Found
  by use. *Folded into `X37`-`X42` on 2026-11-10, which keep these
  principles; what stays open here is the drawing of slurs, which `X42`
  waits on.* *(The drawing of slurs is taken by `X43`, and done with it
  2026-11-10: nothing of this entry is open.)*

  The principles:
  1. **Verbs first.** Every action below is a verb of the one table,
     defined for each of the roll's cases: MIDI or notation, monophonic or
     polyphonic. Its keys and modifiers are the key table's, which a user
     changes, and their defaults are chosen last.
  2. **One format at a time.** MIDI 1.0, MPE, MIDI 2.0 and the written page
     each decide which keys are shown and edited and which curves exist at
     which level. Going from one to another is a conversion, already an
     undoable edit; the roll never edits two formats at once.
  3. **Colour is the voices', the staves' and the parts'**, never a note's
     parameter.
  4. **Every curve but one is an automation**: a channel's in a lane under
     the plane, a note's drawn against its box, as the bend already is.
     Showing another is choosing which parameter is shown. The two new
     pictures are the continuous pitch line and the page's glyphs in
     notation mode.
  5. **What is general is the library's.** Respelling, scales and harmony
     are the core's, used by the roll, by `crates/clausters-notate` and by
     both clients alike; quantization is `crates/clausters-notate/PLAN.md`'s;
     the roll calls them.
  6. **Choosing what is shown is a panel, never a dialog.** Which curve a
     lane shows, which note parameter is drawn, the event list and the
     inspector are sections of `crate::panels` beside the plane, shown or
     hidden, as the audio editor's spectrogram settings are. A modal dialog
     asks before an act (unsaved work, a conversion's options) or reports
     work in progress, and nothing else.
  7. **No new widget, and no floating bar.** The roll composes what exists:
     the menus (as the score editor's page options and the audio editor's
     spectrogram settings are reached), `panels`, the `table`, a `choice`
     in its `tabs` view over a `stack`, the `curve`. What a roll's engine
     does that is its own (a phrase's variations, a rendered waveform) is
     not the roll's; reading MIDI and MEI or MusicXML files exists already.

  Basic editing:
  - **The verbs per case**: create, move, resize, select, delete, split,
    join and transpose, in one table in `docs/app-behavior.md` with a test
    per rule.
  - **A split is the document's.** Today the host cuts the drawn note and
    the second half comes back as a new note holding only what the roll
    draws: its curves, its marks and its written value are lost. As a
    region is split in the multitrack, an event split at a time becomes
    two, the second a new event carrying the first's keys, each of its
    curves cut at the point, and in notation mode the written values of
    both halves read again.
  - **Selection by data**: by pitch, channel, voice or staff, as filters of
    the editing crate that both clients call; a drag that erases what it
    crosses; the whole selection resized by one note's edge.
  - **Transposition** by a semitone and by an octave; in notation mode by a
    step of the key, respelled by the core.
  - **Velocity in a lane of bars** again, one bar per note at its start: a
    bar or the selection dragged, and a drag across the lane setting a
    ramp. It shows only the voice or the channel chosen, when one is. This
    reverses the decision of 2026-09-27 that moved velocity into the note
    (`docs/decisions.md`), which is corrected with it.
  - **The page's marks on a note**: staccato, tenuto, accent, marcato and
    the breath, which takes length from a note before the next attack; and
    the dynamics. Not a trill, which a page writes as notes, nor a fermata,
    which belongs to the tempo map.
  - **Curves by format**: a channel's or a note's curve added or cleared
    among the targets the format allows. Under MIDI 1.0 a channel's
    controllers, bend and pressure, and a note's poly pressure; under MPE
    the master channel's controllers and each note's bend, pressure and
    CC74; under MIDI 2.0 each note's own controllers and pitch as well.
  - **Tools that change the pointer** (draw, erase, stretch): the host
    cannot set the pointer's shape. An element would say which pointer the
    point under it means and each front would set it; the multitrack's
    grips stand where that capability was missing.

  Visualization:
  - **Colour by voice, staff or part.**
  - **What is under the pointer**: the name, the MIDI number, the velocity
    and the channel, and in notation mode the written value, the spelling,
    the dynamic and the marks. On the status band now, as a reading of the
    host's beside its history (as `progress` is beside it), and as a hover
    tooltip once the host has one.
  - **The continuous pitch line**: the pitch that sounds over time (the
    note's, its own bend and, under MIDI 1.0, the channel's bend over the
    note that sounds), drawn over the boxes, joining the boxes of a line
    where they are joined and cut where they are not. It replaces the
    straight links joins are drawn with today. A function of the library
    computes it, so the score editor can read it too. A channel's bend
    stays a lane, and the two can be seen at once.
  - **The page's glyphs in notation mode**, from the SMuFL outlines the page
    already sends: the marks over a box, the breath at its end, a dynamic as
    its glyph. No ties: a box is continuous across a barline, where a
    page's notehead is not. Slurs are open: a monophonic legato is already
    the pitch line and a piano's is the overlap of its notes, so a drawing
    is shown to the user before anything is decided.
  - **Lanes that read.** The keyboard's column is too narrow for a label,
    so the lanes' labels are the tabs of a tab view (a `choice` in its
    `tabs` view bound to a `stack`) over the curves that stand apart from
    the notes, a channel's and the spanners, one shown at a time with its
    value ruler. The fixed `CURVE_H` gives way to the height a lane needs.
  - **The event list and the inspector**, as panels. The events as text in
    a `table` (the time in bar:beat, the kind, the pitch, the length, the
    velocity, the channel, and the written keys in notation mode; a curve
    as a tree of its points); the selection's data, a value that differs
    between the selected notes shown as such. Both follow the roll's
    selection both ways, and an edit there is an intent of the document,
    with its history.

- ✅ **Quantize on a page** *(raised by the user, 2026-11-01, reading the score
  editor's key sheet)*. A page has no grid, so its window offers no quantize; a
  page's written values could still be quantized as a verb of its own, which is
  a design of its own rather than the grid's.
  *Settled 2026-11-09 by the user*: the score editor gets no quantize of its
  own. A played rhythm becomes written values by the roll's quantize to its
  grid, or by the quantization step of `crates/clausters-notate/PLAN.md`
  (`Q5`), which is where reading a performance as a page is designed.

- ✅ **Whether the multitrack keeps its score boxes** *(left to the user
  2026-11-09, when engraving a box by parts was measured and not taken:
  `crates/clausters-gui/PLAN.md`, Found by use, "What a score box of the multitrack
  left open")*. A box of notes drawn as its page is engraved whole after
  every edit of its sequence (about 190 ms at a thousand notes and 2.3 s at
  five thousand, on the application's thread), and the project follows the
  engraver rather than replacing part of its layout.
  *Settled 2026-11-09 by the user, for now*: the multitrack does not keep
  its score boxes; a box of notes is drawn as a roll, and the page is the
  score editor's. This is where the project stands, not a rule against
  them: the question may be opened again, and the measurements above are
  what it would start from. The removal is "The multitrack's score boxes
  come out" in Found by use.

- ✅ **What a sequence is, as a thing that is edited, and what a change of
  representation keeps** *(the user, 2026-11-10, reading the open question
  of "A performance's joins are lost when the roll writes it as
  notation")*. The user changed their mind on principle 2 of "The roll as
  an editor of everything a sequence holds" ("one format at a time, and
  going from one to another is a conversion"): **going between
  representations and editing modes must not lose the keys of the other
  formats**. Today a conversion renders the page afresh, and what the
  performance held that the page does not write (the joins first, but any
  key outside the target's set) is gone. The user's point is that this is
  not clear conceptually, and the reason is that the **file as an editable
  object** was never defined: a MIDI 1.0 file, a MIDI 2.0 file, an MPE file,
  an MEI or MusicXML score each hold a different set of data, and the roll
  treats a sequence as though it were all of them at once. Two ways out,
  both open, to be decided before any fix of the joins:
  1. **Define the compatibility of the data sets**: which keys each format
     holds, which a conversion carries over to the same notes, which it
     keeps aside untouched for the way back, and which it cannot carry;
     a switch that would lose something says so, or is not offered.
  2. **Make the format the file's**: a sequence is opened *as* one of those
     files, a change of format is an explicit conversion of the file into
     another (a new document, or a Save as), and the roll offers only the
     editing mode the file's format supports.
  Whichever is taken, the joins' question below is answered by it, and
  the roll's modes, the conversion dialog and `docs/app-behavior.md` ("The
  roll's verbs, case by case") are read again against it.
  *Decided 2026-11-10 by the user: the second way* (`docs/decisions.md`, "A
  file says which application it belongs to, and the format is the
  file's"). What is open is its design, to be taken into a milestone: the
  file as the roll's object (what it holds per format, how it is opened,
  saved and saved as), the editing mode each format allows, the conversion
  as a file into another (and what it carries to the same notes), the
  roll's File menu, and the clients' surface for all of it. *Taken into
  `X44` the same day, and done with it 2026-11-11.*

- ✅ **The applications as modules that read apart** *(a refactoring to
  evaluate, asked by the user 2026-11-12, once the workspace's review is
  in; done 2026-11-12)*. The crate is 53 thousand lines and already has a module per
  application (`audio`, `multitrack`, `notes`, `points`, `score`) beside
  what they share (`application`, `editing`, `chrome`, `forms`, `panels`,
  `closing`, `file`, `ruler`, `samples`, `turn`). A crate per application is
  not the direction: the applications are composed of components written
  once, and what they share is the larger half. What is to evaluate is
  whether the modules are as separate as their names say: which application
  reaches into another's module, what a shared module knows of one
  application, and whether the shared half has an order of its own. To be
  measured by reference first, as the host's layers were (`crates/clausters-server/PLAN.md`,
  `R19`), and only then decided.

  *Measured.* Tests apart, the modules already had an order, with four
  kinds of reference against it. The audio editor and the multitrack named
  the editing context that opens them, for three things that were never the
  context's: what an editor hands the engine the context holds
  (`editing::held`'s take and its lists), a sound file's header, and
  whether two paths name one file. The notes editor reads its marks'
  symbols from the score's palettes. No application reaches into another
  except to place its view (the multitrack's boxes are the audio editor's
  take and the notes editor's roll, the audio editor's envelope is the
  points editor's), which is the rule and points one way. `closing` and
  `turn` name each other and are one layer.

  *Decided.* The order is written as six layers in `docs/architecture.md`
  ("The applications' modules and their layers") and held by
  `crates/clausters-checks/tests/apps_layers.rs`, which shares the host
  check's reader (`clausters_checks::modules`). The three things moved down:
  the take and its lists and the header to a module of their own, `takes`,
  and `same_file` to `file`; `editing::held` keeps the engine's trait. The
  notes editor's reference to the score's palettes is the test's one
  exception, entered below under Found by use.

## Found by use: the entries that were fixed

- ✅ **The multitrack's score boxes come out** *(2026-11-09, from the
  user's decision in Future directions, "Whether the multitrack keeps its
  score boxes")*. The View menu's *Notes as scores*
  (`clausters_editing::verbs`, `notes_score`), the multitrack editor's
  pages, its engraver and `refuse_pages`
  (`clausters_apps::multitrack::editor`), the host's binding of an engraver
  to a multitrack window (`crates/clausters-gui/src/host/audio_app.rs`), the page a
  box opens in, their tests, and the step of `editors/edit_multitrack` that
  shows it, in both clients wherever they name it. A box of notes is drawn
  as a roll and opens in the notes editor. Taken out so that it can come
  back: the engraving stays the score editor's, and what the box used
  (`Transcription::of`, the anchors) stays where the score editor uses it.
  *Done 2026-11-09*: the View menu's two notes entries, the editor's
  `notes_view` and its pages, the engraver it was handed, the host's page
  window over a box and its proportional drawing, the clients'
  `notes_view`/`notesView`, and the widget's `notes_view` and `scores`
  props are gone; a double click on a box of notes opens its roll. The
  reading a box used (`Transcription::of`) stays with the score editor,
  and the proportional drawing is in the history if the boxes come back.

- ✅ **A failed endpoint walk leaves unissued steps queued** (2026-10-30,
  reading the document-store adapters). `clausters_editing::run::Runner`
  deliberately releases its queue after a refusal; its
  `a_refused_command_releases_the_queue_with_its_reason` test preserves
  that low-level policy. Python `run_steps` and web `runSteps` raise on
  failure without retiring the unissued tail. Reusing that runner can then
  dispatch the previous batch's remaining sends before the new work.
  Keep the raw runner's policy explicit and give the aborted endpoint walk
  an explicit retirement boundary. **Acceptance:** reuse one runner after a
  refused render, store and server wait; none of the failed walk's unissued
  messages goes out, and independent queued work follows its stated policy.
  *Fixed 2026-11-02.* `Runner::retire` (the JSON door's `retire`) drops
  what is left unsent, the awaited step included; Python `run_steps` and
  web `runSteps` call it on whatever ends a walk early, so a runner is idle
  after a failure. The raw runner still releases its queue on a refusal,
  which the host's shared queue relies on, and the policy is written on
  `Reply::Refused`. Tested in the runner and in both clients over a
  refused wait, a failed store and a failed render.

- ✅ **The audio meter is corrected while the window does not draw it**
  (2026-10-21, the eye pass over the editor with no file). After a file is
  closed or opened, the `playing` answer corrects the meter widget with
  `{"bus", "channels"}` (a negative bus for a file not playing) when the
  window was drawn without a meter, and the host warns `/gui_set <id>: no
  such widget`. Reproduced on the Python client's fake host by closing files
  and opening one again; the correction should follow whether the window
  drew the meter. *Fixed 2026-10-21:* a switch of focus read the meter
  before the playback was told the file viewed, so the window was drawn
  without one; the client now tells the playback first, and the editor
  corrects only a meter it drew.

- ✅ **The other applications that open files open without one and outlive
  their last** (the user, 2026-10-21, in the X21 eye pass: an editor is an
  application that opens with no file, and closing the last file does not
  close it; this holds for other applications too, though not all). The rule
  is in `docs/app-behavior.md`; the audio editor keeps it. Pending where an
  application holds files or documents of its own: the score editor (it
  opens, saves and closes MEI files, in the standalone host too) and the
  multitrack where it holds a session file. It does not apply to an editor
  over a structure a client handed it (the notes and points editors, a GuiDef
  panel). Port with `X25`, reusing the audio editor's shape: an `empty` open,
  the window's own verbs answered and the rest refused with `no file is
  open`, and a `binding` the clients unbind.
  **The score editor keeps it since 2026-10-31**: it opens with no document
  (`--score` with no file, an empty `/gui_app_open`), its File menu makes, opens
  and closes one, and Close file leaves the window open with the score handed
  back.
  **The multitrack keeps it since 2026-10-30**: it opens with no session
  (`--session` with no file, an empty `/gui_app_open`), its File menu makes,
  opens and closes one, and closing the last leaves the window open.

- ✅ **First native eye pass over X20's actions** (2026-10-21). A copy
  announced "loop off": a completion re-sends the view's loop switch, and
  the host now says the switch only when it moves. Delete/Backspace did
  nothing: the audio editor now removes the span as a cut that copies
  nothing. Preview on/off became one `toggle_preview` switch, "Preview
  edits", ticked while on, and the edit layers moved to Edit. With a dialog
  up and its field unfocused Ctrl+Z undid the file, and in the field it did
  nothing: a dialog now keeps verbs from what is behind it and a field has
  its own history. The key sheet listed `quantize`, `split`, `join` and
  `snap`: a window with readings no longer offers table verbs it does not
  read. Directed host and core tests cover each.

- ✅ **A copy right after a sweep could be refused on a stale reading**
  (2026-10-21, forced in the host harness). The host gated keys on the
  owner's last reading, which may predate the sweep. Readings now mark an
  empty selection (`unselected`), the host reads its own, and a key is never
  refused on that objection alone; regressions mutation-checked.

- ✅ **Audio undo corrected a component absent from the tree**
  *(native clipboard acceptance, 2026-10-18).* After closing a clipboard
  origin, undo restored samples but emitted `/gui_set 1004: no such widget`.
  The simpler reproducer needs no close: the samples view registered its
  optional stacked lane even in a single-view tree, and undo corrected every
  registered id. Both clients now restrict the common view registry to the
  composed tree while retaining reserved component ids across layout changes.
  Regression tests walk undo/redo through single and stacked views.

- ✅ **Closing an audio clipboard origin could free its loaded buffer twice**
  *(found in the native example, 2026-10-18).* The resource service could
  collect a file-owned loaded buffer after its private edited copy was made,
  while close still owned its release. Keep loaded-buffer ownership rooted
  for every open file, including inactive ones, until close; clipboard
  publications retain their separate snapshots.

- ✅ **Concurrent Python editing queries could corrupt FFI responses**
  *(found while validating the audio clipboard, 2026-10-18).* Script queries
  and host callbacks could interleave the size/fill calls on one EditingCore
  handle. Serialize the complete handle call and disposal in the adapter;
  serialize endpoint clipboard-service lookup/publication as well. Keep this
  transport synchronization separate from application operation policy.

- ✅ **Audio focus corrections addressed retired widgets** *(found
  2026-10-18 during the native multi-file review)*. Focus answers were built
  against the previous file's component ids, then delivered after a redraw
  gave the new file different ids. The native host reported `no such widget`,
  and restored cursor/selection corrections could miss their views. Window
  bindings now retain instance-scoped component ids, including across file
  switches, while context versions reject old gestures. Preview corrections
  follow publication; hidden spectrogram lanes receive no cursor correction.
  Both clients test with a real allocator and with two windows on one host.
  The web example's second editor also gets its own canvas; reusing the
  first editor's explicit stage had hidden one window behind the other.

- ✅ **The audio editor's switches are menu entries, not checks** *(the user,
  2026-10-17)*. The preview of the gain envelope is two entries of the Edit
  menu, *Preview on* and *Preview off*, where it is one state; it belongs in
  View as one checked entry, and so do the editor's other view switches. Open:
  the menu needs a checked entry the owner keeps in step with the state, the
  same in the host and in both clients.
  *Closed 2026-11-02, with a test.* Two commits after the entry had already
  done it and left it open: the actions' readings (2026-10-21) give every
  toggle its `checked` and every choice its group, which the host turns into
  a check or a radio wherever the verb appears, and the preview was removed
  from the editor (2026-10-23), so no on/off pair is left. A host test now
  reads the window's menus marked: Files, Spectrogram settings and Loop are
  checks, the views and the body the hand edits radios, and a switch turned
  from a script reads turned.

- ✅ **Cut, Copy, Paste and Paste mix in the audio editor do not agree with
  the selection and the position cursor** *(the user, 2026-10-17)*. A paste
  landed at the start of the selection rather than at the position cursor,
  a cut left the selection standing over other samples, and a paste from the
  menu after a click reached no view, so it worked only with something
  selected. A cut over a long take was refused, the host holding a summary
  of it and no samples. Fixed the same day: a paste lands at the position
  cursor, a cut or a paste clears the selection, a paste with no pointer
  goes to the view the cursor is on, and the audio editor copies on the
  server.

- ✅ **An overlaid gain curve needs its own scale named.** Found in the X14 example capture: unity sits halfway up a zero-to-two curve over a waveform whose ruler measures amplitude. Selecting the curve now states `gain 0..2 (unity 1)` in the pane header; selecting the take restores its caption. The signal ruler continues to measure samples. *(Superseded 2026-10-17: the pane's vertical ruler and read-out are the body in hand's, so the curve in hand is ruled in gain. The same day its range became 0..1, silence to unity: an envelope attenuates and does not boost.)*

- ✅ **A box of notes stretched from the front loses where its notes start**
  *(the user, 2026-10-06, at the first look at a box drawn as its page: with
  the box moved, its notes do not sound as they should -- and then the cause:
  stretched from the front, the box loses the relation to where its notes
  start)*. The document and what plays were right: a trim moves the box's
  edge and its window's start together, and the notes were placed at their
  own seconds. The picture was not. Each box's notes, and its page's anchors,
  were stated **from the box's edge**, and a trim is answered with an
  acknowledgement alone, so the host went on drawing them from the new edge:
  they travelled with it, and the picture stopped being what was heard.
  **Fixed 2026-10-06** by stating a box's notes as a take's samples are
  stated, in **the sequence's own frames**, and drawing them through the
  box's window (its `start`, and `rates`): a trim leaves every note where it
  was played, in the picture the moment the edge moves, and a note that
  starts before the window is not drawn, as it is not heard.

- ✅ **The notes editor's playback does the transport's work** *(found
  2026-09-28, writing `X3.9`'s sound; the user: "A piano roll with MIDI/OSC
  events should run on the server's transport, otherwise we would be
  duplicating functionality")*. `X3.8` stamps every event on the transport's
  clock (`/sched_atTransport`) and plans again from the crate on every play,
  locate and edit, with the client querying the clock first: a second
  implementation of what the transport does for a take, and wrong across a
  loop, since the clock does not wrap. A roll holds concrete data as a clip of
  audio does, so it plays on the server's transport: `PLAN.md`, `T8`, the
  event lane, which the notes editor and the multitrack's notes regions move
  onto. `clausters_editing::notes_playback` is what goes. **Fixed 2026-09-28
  by `T8`**: the playback is an event lane on its transport, an edit is the
  lane's new data (`update`), and nothing asks for the transport's clock.

- ✅ **A roll's edit made every note new** *(the user's log of `edit_notes`,
  2026-09-29: the notes came back with ids #53 to #56, each with a
  `velocity` and an amplitude the author never wrote)*. The crate corrects a
  roll with `notes` and `note_ids`, in a `serde_json` map that sorts its
  keys, so `note_ids` reached the host first and the `notes` behind it
  cleared the ids it had just been given. The next report named every note
  0, as one the hand made, and each gesture rewrote the sequence as new
  events that had lost what the roll cannot draw.
  **Fixed 2026-09-29** in the host's `notes` element: the ids set last are
  kept for a list that arrives after them, applied when the lengths agree and
  consumed there, so a list is named by the ids beside it whichever came
  first.

- ✅ **A roll's OSC markers were sent to the server** *(the same log: "lane
  1001: event 3 was not built: /mark cannot be scheduled in a timed bundle",
  on every edit)*. An OSC event is a message to another application, which
  the server cannot send, and `notes_playback::data` wrote it as a lane
  message. **Fixed 2026-09-29**: a lane's data holds the notes only; the
  lane's own `messages` are commands for the server, and a sequence holds
  none.

- ✅ **Where a pass ends, and the space bar** *(the user, 2026-09-29, trying
  `edit_notes`: the roll should be able to play to the last note's end or
  roll on as the multitrack does, better the second; "For the multitrack the
  same rule as the roll's can be applied for the end of what exists, or an
  end marker can be used"; and "the space bar should be play and stop, so
  that the play head goes back to the position head")*. The notes editor
  marked its sequence's end on every play, the multitrack had a switch, and
  the space bar paused both. **Done 2026-09-29**:
  `clausters_editing::playback::End` -- open (the default), the contents, or
  an end marker -- is how both playbacks say where a pass ends
  (`Playback.end` and `NotesEditor.end` in both clients; core ABI 77), and
  the space bar is play/stop over both windows, a stop going back to the
  position cursor. The end marker is a number from a script; drawing it and
  placing it by hand is open.

- ✅ **A roll navigates only the window it opened on, and a note in hertz
  grows with the zoom** *(the user, 2026-09-29, trying `edit_notes`: the roll
  should scroll to octaves outside its range as the piano widget does, MIDI 0
  to 127 and hertz past MIDI 127, opening on the contents with a margin; and
  "zooming in vertically with the frequency representation should not change
  the boxes' width", since a box's frequency is at the middle of its height
  and nothing says so)*. The crate sent the window fitted to the notes as the
  roll's compass, so nothing past it could be reached; and a note was a
  semitone row high in hertz too. **Fixed 2026-09-29**: the compass is the
  domain's whole range (MIDI 0 to 127; the hertz domain from MIDI note 0 to
  20 kHz) and the fitted window is the slice the view opens on
  (`axes.y.start`/`len`); a wheel over the keyboard scrolls the window, a
  tenth of it a notch, and Ctrl and the wheel zoom it
  (`OnAxis::wheel_pans_y`, the roll's alone). In hertz a note is a bar of a
  fixed height with a line at its centre (`pianoroll::note_height`). Home and
  End place the cursor over a roll and a multitrack too.

- ✅ **A time range over a roll and a multitrack** *(the user, 2026-09-29:
  "The multitrack and the roll need the ability to mark a time range, as in
  the audio editor", with a modifier since the plain drag selects notes and
  boxes, and anywhere on the view; Alt chosen, an Alt click still toggling,
  and the range played as the audio editor plays one)*. **Done 2026-09-29**:
  the host's `range` gesture step sweeps the time range alone and hands a
  press that never moved to the element as a click; a roll and a multitrack
  give it Alt. The range is kept by the editor (beats for a roll, seconds for
  a multitrack) and the space bar plays it as the audio editor does -- from
  its start to its end, back to the position cursor, and the loop switch `L`
  over it or over everything (`play_pass` on both playbacks,
  `clausters_editing_playback_play_pass`; `play(range=, looping=)` in both
  clients). The standalone host plays the same, and learns from the engine
  when a pass stopped on its end mark.

- ✅ **Two windows of one role over one structure draw on one widget**
  *(found 2026-09-28, extending `edit_notes` with a roll in hertz beside the
  one in MIDI notes: "gui_def: widget id 1000 already in use, skipping")*. A
  view names its widgets by `(structure, role, key)` in the core's registry
  (`Application.id_for`), deliberately without the drawer, so that two views
  of one thing agree about which widget draws which part of it. Two editors
  opened over the same structure -- `edit(seq)` twice, which the history
  supports, one pile per context -- then ask for the same id, and the host
  skips the second window's widget. The hertz roll is keyed by its axis, so
  the pair in `edit_notes` works; two rolls in one axis still collide, and so
  would any editor opened twice over one structure, since every view names
  its widgets through that one door. Open: whether the window belongs in the name,
  or a second editor over a structure is refused and hands back the first.
  **Decided 2026-10-06, by the user**: the first is handed back.
  **Fixed 2026-10-06**: `edit` over a structure with an editor of its kind
  open on it -- for a roll, over the same axis, so a roll in hertz beside one
  in MIDI notes is still two windows -- answers that editor and opens nothing,
  found among the views of the structure's editing context, in both clients.
  A closed editor is not handed back, and a timeline is rendered into a new
  sequence by each call, so it never finds one.

- ✅ **The roll has no play cursor, and its ruler places nothing** *(the
  user, 2026-09-28, trying `X3.9`: "The roll has no play cursor or it does
  not work, it cannot be positioned, there is no cursor advancing with
  time")*. `X3.7` and `X3.8` gave the notes editor a window and a playback
  and never joined the two: the roll's axis anchored no playhead, the editor
  asked the host for no clock, and a click on the ruler reached the crate as
  a tag it did not read, so no cursor was kept and a play always started
  from the top. The audio editor had all three. **Fixed 2026-09-28**: the
  roll anchors its playhead at 0 and the editor asks for the notes
  transport's position when it opens (both clients, and the standalone
  host's roll window); the ruler's `locate` is the outcome's `locate`, a
  beat of the sequence, which the editor keeps as its cursor and cues a
  stopped transport on (`NotesPlayback::cue`). The standalone host's roll
  also plays now -- the space bar, a cue, an edit heard from either window
  -- where before it drew and edited only.

- ✅ **The play cursor of a take at another rate than the engine's runs
  ahead** *(found building X7, 2026-09-24)*. The audio editor's playback
  converts every frame to the engine's samples and the reader scales its
  phase, so the take is heard at its pitch and ends where it ends; the play
  cursor is drawn from the transport's position read as frames of the take,
  so over a 44.1 kHz take in a 48 kHz session it runs ahead by the ratio. The
  view has to scale the position by its rate over the engine's.
  **Fixed 2026-09-25**: `Host::head_clocks` — the one place a playhead's
  reading is resolved, for both fronts and whoever plays — reads a transport
  on a view that declares its samples' rate as that view's own frames, the
  position times its rate over the engine's. The device clock is left alone,
  since an anchor on it is in the engine's samples. The reading is rounded to a
  whole frame: a locate sends the frame as the nearest engine sample, and
  scaled back unrounded it stood a fraction of a frame beside the position
  cursor at a zoom that draws samples.

- ✅ **A looping selection does not follow a new selection** *(the user,
  2026-09-25)*. Redrawing the selection while it loops leaves the loop on
  the old span. It was never otherwise in the audio editor opened from a
  client: the live follow (`transport_follows_selection`) is the host
  monitor's, and it acts only in a host that owns the transport -- a
  standalone session (`bin/clausters-gui.rs`) -- while the editor's
  `selection` arm, before X7 and after it, keeps the span for the next play
  and tells the playback nothing. It has to answer a new selection, while a
  loop plays, with the playback's `set_loop`, bound for both clients; and
  in a window that plays itself the host's sweep should not touch the
  monitor's loop.
  *(Fixed 2026-10-07. The editor answers a `selection` with the span a loop
  would repeat over -- the selection, or the take with none, as `space`
  says -- and the `locate` a sweep is let go with, which lands on the
  selection's start, with the same span and `place`; both clients hand it
  to the playback's `follow` verb, which moves the loop from where the
  transport stands and, with `place`, locates the head on the span's first
  frame. That placing is not in the entry above and is why `set_loop` alone
  was not enough: a transport past its loop's end runs on and never wraps,
  so a span drawn behind the head would have left the take playing on
  outside it. The playback acts only for the file in focus, rolling, on a
  pass that loops, which it now remembers from its last `play` or `pass`;
  a pass that runs to its end keeps the end it was played with. In the
  host, `transport_follows_selection` stays out of a window whose owner
  plays it. The start is matched within what single precision keeps of a
  frame: `crates/clausters-gui/PLAN.md`, "A selection is reported in single
  precision".)*

- ✅ **A roll over a rendering was offered the drag it could not keep**
  *(found 2026-09-27, writing the notes editor's window over the catalogue's
  roll)*. `catalogue::pianoroll` marked a roll that cannot be edited with
  `notes_editable: false`, which is the **clip's** prop for the notes layer
  inside a box; the `notes` element standing on its own reads `editable`, and
  read nothing, so a roll over what a generator produced offered every drag
  and the owner unwound it after. Fixed the same day: the catalogue writes
  `editable`.

- ✅ **Each editor writes the conversation's turn again** *(audit 2026-09-25,
  with the server's)*. `audio/editor.rs::event` and `multitrack/editor.rs::event`
  build the same conversation `Message` from the event and map
  `Turn::{Nothing, Closed, Step, Stale, Route}` onto an `Outcome` with
  `conversation::answer`, differing only in who owns a widget, the resync and
  the route. The score editor would be a third copy: one
  `conversation::turn(event, version, window, owns, route, resync)`.

  **Fixed 2026-09-27, before the notes editor would have been the third copy
  (`X3.4`).** `turn::turn` reads the message and answers every turn but the
  gesture, which it hands to the editor through the `Converse` trait: the
  conversation, the window, who owns a widget, the resync, the route, and a
  window verb answered before the conversation (the audio editor's save and
  play). `turn::Turned` fills the fields every outcome shares. Both editors
  are the one call now.

- ✅ **A roll's edit sends every note at its velocity's amplitude** *(found
  2026-09-27 by the user, by ear, in `editors/edit_notes` with its timeline
  looping: after one drag the whole melody came back much louder and the high
  note saturated the speakers)*. The server's own log (`RUST_LOG=clausters::osc=trace`)
  settles what reached it: before the edit every note arrived with the
  author's `amp` (0.1, and 0.4 on the last one); after it, **every** note --
  not only the one that was moved -- arrived as `amp 0.78740156`, `velocity 100`,
  which is 100/127, about +18 dB. Nothing was duplicated: one `/synth_new` and
  one `gate 0` per note, from one pass.

  So the write-back rebuilt every note from what the roll can say (start,
  length, pitch, velocity, channel) and the amplitude the author wrote was
  replaced by the roll's velocity. Where exactly is not pinned down: an offline
  replay of `NotesDomain.project` with a hand-built `notes` payload, before and
  after the timeline had played, kept every `amp`, so the difference is in what
  the live host reports, in the crate's `events` reading of it, or in how an
  `Event` turns a `velocity` into an `amp` -- one of the three seams this
  milestone exists to settle. It is left for `X3` on purpose: patching today's
  `NotesDomain`/`NotesEditor` would be work on the part it replaces.

  **What it blocks:** `C54`'s by-ear acceptance over `editors/edit_notes`
  (`clients/python/PLAN.md`), since any edit in that window changes the level of
  what the pass plays. The pass itself was checked without the roll -- edits
  from another thread, and the server's output recorded.

  **Fixed 2026-09-27, in the host.** The document's catalogue holds a roll's
  quintuples as `Vec<f64>` and serializes the velocity as `12.0`; the host read
  it with `as_i64()`, which refuses a float, and fell to its default of 100 --
  on every note, and the channel to 0 the same way. So the roll drew every note
  at 100 and the first drag reported 100 for all of them, and the crate's
  intake, seeing each velocity differ from the one it projected (0.1 * 127 ->
  12), wrote `velocity 100` and its amplitude onto every note. The host now
  reads both as numbers and rounds them; the intake was right, and compares the
  integer it projected with the integer that comes back, so an untouched note
  keeps the amplitude its author wrote.

- ✅ **The graphs a note plays in are never freed** *(found 2026-09-30,
  writing `X3.11c`)*. A channel's graph and each note graph are named by what
  they hold (`clausters_core::event_graph`), so every new shape -- a curve
  added, a note started with another control -- is a new def sent to the
  server, and none is ever freed: they accumulate for as long as the server
  runs, and on a server with a data directory they are persisted, so they
  outlive it. What is missing is giving a def back once no instance and no
  channel graph names it (`/def_free`), and deciding whether generated defs
  are persisted at all. **Related:** "A GraphDef carries the buffers it
  plays" (`crates/clausters-server/PLAN.md`, Future directions): if a def keeps its buffers,
  persisting one persists them too. **Decided and fixed 2026-10-01** with the
  user: generated defs are **not persisted**, and whoever generates one frees
  it. Every generated name begins with `tmp_`, the server's existing mark of a
  def it never writes (`tmp_ev.` for the curves' readers and graphs,
  `tmp_mt.` for the mixer); a note or channel graph is named for the lane it
  plays from (`CurvePlan::scoped`, the lane's node id), so two playbacks on
  one server never share one; and `NoteCurves` frees, with `/def_free`, every
  graph no instance still alive plays, on each pass and at teardown. The four
  readers and the mixer's defs are a bounded set every playback shares, and
  are only kept out of the store.

- ✅ **A curve added while a sequence plays cuts its channel** *(found
  2026-09-30, writing `X3.11c`)*. A slot's members are fixed by its def, so a
  channel whose set of shapes or of curves changes is a new graph, and its
  instance is made again: the notes sounding in the old one stop at once,
  releases included. An edit of a curve's points does not do this -- a table
  is replaced under its reader -- only one that changes which curves there
  are. Keeping the old instance until its notes end, and adding the new one
  beside it, would make the change as seamless as a point dragged. **Fixed
  2026-09-30** that way: `NoteCurves` makes the new instance under the
  channel's next generation and keeps the old one -- its notes' releases are
  the lane's, sent to their own nodes -- with every table it may read, until
  the next pass gives both back; a channel whose last curve goes finishes its
  notes the same way.

- ✅ **A note cannot be written into the second voice** *(found 2026-10-05,
  building `X5.5.2`'s voice tool)*. The page's `insert` gesture names the
  item the new note **follows**, and an insertion after an item goes into
  that item's voice and adds its time to it, so there is no press that writes
  a second line against music already there. The voice tool is therefore the
  **selection's**: it shows the voice of what is selected and moves it
  (`voice`), which is how two lines written as one come apart, and it is not
  an input state. Writing into a voice directly needs the gesture to name a
  **moment** on the staff -- the measure and the onset the press fell at --
  and an operation that writes there without moving what the other voice
  holds. *(2026-10-05: that is the edit cursor of "Writing notes has no
  cursor, and a note written pushes the rest along", below -- a place in a
  voice, and an entry that replaces there. This closes with it.)*
  **Fixed 2026-10-05 with note entry**: `Ctrl+Alt+2` puts the edit cursor in
  the second voice, and what is entered there is written against the first,
  which does not move; the voice is made, and padded with a rest to where the
  entry starts. The bars past a second voice's end are left empty rather than
  given a rest each (`<mSpace/>`), which is what the page drew the first time
  one was written.

- ✅ **The value and dot tools do not act on the selection** *(found
  2026-10-05, the same step)*. They are the input state alone: with notes
  selected, picking an eighth changes what the next press writes and leaves
  the selection as it was, where the accidental tool, beside them, is the
  selection's when there is one. One rule for the three, or the difference
  said in the tool's tip, is to be decided with the palettes (`X5.6`).
  **Decided 2026-10-05 with `X5.6`**: the toolbar is what a hand writes with
  and the palettes are what is done to what is written. The value, the dot
  and the rest stay the input state, as their tips say; a selection's values
  change by `scale` (Notes, Longer and Shorter), and its accidentals by the
  Accidentals palette, which always acts on the selection. The accidental
  *tool* keeps both readings, since arming one for the next note is its
  reason to be on the toolbar.

- ✅ **A text of the page cannot be edited where it is drawn** *(found
  2026-10-05, closing `X5.5.4`)*. A press on a title selects it and the status
  line says its field and its place; changing it is the page text dialog, or
  `set_text`. Editing it in place needs two things the page does not do: to
  report a **double click** on an element, as against the press that selects
  it, and to hold a text field over the box the text is drawn in, which moves
  with the scroll and the zoom. Both are the host's `score` element's, and the
  field's commit is then the `text` verb that exists.
  **Fixed 2026-10-05**, without the field: the state is the page's own. A
  double click on a text of an `editable` page takes it up, all of it
  selected; the page draws the string as it stands, with its caret and what
  is selected, at the text's own anchor -- so it moves with the scroll and
  the zoom because it is the page drawing it; the keys are a field's (the
  one function both now call); Enter writes it, Escape leaves it, and so does
  going elsewhere -- a press on the page, or the focus leaving it, which the
  host now tells an element (`Element::blur`). What is reported is
  `"text" <id> <string>`, and the editor reads it as the `text` verb. A page
  that edits takes the keyboard focus for this, and says nothing about it.

- ✅ **The dialogs' labels and the toolbar are not laid out by eye yet**
  *(the same day)*. The widths of a form's captions and fields, the height of
  the footnotes' field, and the size an outline glyph is drawn at were set by
  reasoning, with the window not on screen; they are to be judged in the eye
  pass that closed `X5`.
  **Done 2026-10-05**, looking at the window in a browser at the user's
  display factor (1.33), with a system face and with the built-in one. A
  form's captions are 170 wide (the longest, "Bottom margin (mm)", was cut at
  150), its fields 340, its rows carry no margin of their own -- the pitch
  went from 50 to 28 -- and the footnotes' field holds four lines. The
  toolbar's symbols are drawn at `tools::SYMBOL_SIZE` and the palettes' at
  `palettes::SYMBOL_SIZE`; see the entry on the symbols, below.

- ✅ **Two lines of the page's head run into each other** *(found
  2026-10-05, the first look at the page)*. In the example the subtitle,
  centred under the title, and the composer, right-aligned on nearly the
  same line, overlap where the one ends and the other begins. Not yet read
  in the code; the first thing to check is whether the cells were sized with
  the engraver's text metrics while the host draws the text in its own,
  wider, face.
  **Fixed 2026-10-05**, and it was not the face's width: the host took the
  font size the engraver names -- the em -- for the height of its own line of
  capitals, so every text of a page was drawn at 1.4 times the size the
  engraver had laid it out for, and two cells it had kept apart ran together.
  The page's text is drawn at 0.7 of the em now
  (`graphics::score::CAP_PER_EM`), and its hit box is as wide as the string
  is in the host's face.

- ✅ **The symbols of the toolbar and the palettes cannot be read** *(the
  same look, and the user's own: "it is not clear what each glyph is")*.
  Four things, in the toolbar and the palettes alike:
  - **They are drawn too small.** An outline is drawn in the body text's
    cell, which is right for a letter and small for an articulation dot, an
    accent or an accidental.
  - **At that size they deform.** The toolbar's sharp is missing one of its
    lines: a stroke thinner than a pixel is lost, or two fall on one. An
    outline wants a size at which its thinnest stroke is a pixel, and
    placement on the pixel grid.
  - **Some are text where a symbol exists.** The two grace notes of Notes
    show their name cut to a letter and an ellipsis, in a cell one glyph
    wide: the engraver handed no outline for their codepoint, so the label is
    the fallback. The slur and the tie of Lines, and the toolbar's tie, are
    words as well.
  - **Some have no symbol at all**: the barlines of Measures, and the voice.
  **Fixed 2026-10-05**, each of the four:
  - *Size.* A symbol's size is its text's, so a tool that shows one sets
    `text_size`: 2.5 on the toolbar (an em of 22 logical pixels) and 3.5 in
    the palettes (30). What made them look small was as much the words: the
    host's default text was capitals as high as the whole line, half again a
    desktop's own text. It is three quarters of the line now, for every
    window (`crates/clausters-gui/PLAN.md`, "Found by use").
  - *Deformation.* The host keeps a stroke a pixel wide: under the size where
    a music font's uprights are a pixel it draws the outline's edges over the
    fill. And an outline steps by its own width, so a dynamic of two letters
    is not run into its neighbour or cut to nothing.
  - *Words.* `score::icons` draws what the engraver's face does not hold,
    and completes the table the window carries: the grace notes (the face's
    eighth, smaller, with its stroke), the slur, the tie, the hairpins.
  - *No symbol.* The same module: the barlines and the repeats (under SMuFL's
    own codepoints, so a face that has them wins), the voice, the measures'
    three verbs, the breaks, "none", and the transport's back-to-start --
    these under codepoints of the editor's own, in a private plane.
  The palettes' column scrolls, which showed up as soon as the symbols had a
  size: its last groups were under the window's foot.
  Left as they are: the voice selector and the layout selector of the
  toolbar are words (`v1`, `v2`; `page`, `line`) -- symbols since
  2026-10-06, the entry below.

- ✅ **An edit to a sounding pitch does not rewrite the written one** *(found
  2026-10-05, closing `X5.7`)*. The pitch family is coherent -- moving a note's
  `midinote` in a roll rewrites the `freq` and the `degree` it holds -- and
  `pitches` is outside it: an event that holds both keeps the written pitch it
  had after the note was dragged, so the page and the sound part ways. The
  family's rule would be to respell `pitches` by `spelling` when a sounding key
  is set, and to set `midinote` when `pitches` is. It waits on the way back
  from the roll to the score ("From the roll to the score"), which is what
  reads a sequence edited as numbers.
  **Fixed 2026-10-06**, by the family's rule alone: setting a sounding key
  on an event that holds `pitches` spells them again by `spelling` (by whole
  octaves they keep their letters and a forced sign), and setting `pitches`
  on an event that states a sounding pitch moves its `midinote`. What a
  sequence edited in the roll means to the score that rendered it is still
  "From the roll to the score".

- ✅ **The score's page was in no window** *(found 2026-10-05, the user's
  first look at `notation/score_editor`: "It is not shown, and the score can
  be neither created nor opened")*. The crate wrote the container the page
  sits in as `"type": "scroll"`, a name the wire dropped when it took the
  model's (`plane`). The host builds a type it does not know as nothing and
  a leaf carries no children, so the page was never placed -- while every
  message to its id was still answered by the editor, which is why the
  host's tests (they deliver to the id) and both clients' (they read the
  tree) all passed. Fixed by the name, and pinned where the two meet: the
  host's test of the window it opens now refuses a tree with an unknown type
  in it and asks that the page be found in the window.

- ✅ **A written system break was laid out as if it were not there** *(the
  same look)*. The page view asked the engraver for `breaks: auto`, on a note
  saying it had been measured to honour a written break; it does not --
  the example's break before the fifth of eight bars was drawn before the
  seventh. `smart` honours one only on a system at least `breaksSmartSb`
  full (two thirds by default: a break after one bar of four is ignored), so
  the page view is `smart` with that threshold at zero, pinned against the
  engraver in `clausters-notation`.

- ✅ **A written page break does not turn the page** *(the same day, measured
  while fixing the entry above)*. The engraver's system cast-off reads `sb`
  and has no visit for `pb`: under `auto` and `smart` a page break is neither
  a new page nor a new system, and only `encoded` honours it -- the mode that
  breaks nowhere else. So Measures, Break, Page writes a `pb` the page view
  does not show. Either the engraver is taught it (a `VisitPb` beside
  `VisitSb`, carried as a patch of the vendored build) or the break is
  written as something the cast-off does read. **Decided 2026-10-05, by the
  user: the engraver is not patched** -- reading `pb` only when the encoding
  is followed whole is its design, not a defect. The page view honours the
  break on its own side of the engraver (for instance by casting off the
  stretch between two page breaks as its own run of pages); which way is
  settled when it is taken.
  **Fixed 2026-10-05, that way**: a score with page breaks is written as one
  document per run (`notation::sheet_to_mei_pages`), each engraved on its own,
  and the page view stacks their pages, numbered as the whole score's
  (`docs/decisions.md`, "A page break is honoured by laying the score out
  twice"). Checked in the browser: a break before the third bar of the example
  leaves two on the first page and opens the second, numbered 2, with the
  third. **Left open**: a slur or a hairpin that crosses a page break is drawn
  in neither run.
  **Fixed 2026-10-06**: a line across a page break is written once in each
  run it is in -- to the end of the run it starts in, through one it passes,
  from the start of the one it ends in -- each end a beat of the run's own
  measure, which the engraver matches and draws open at the edge as it does
  across a system break. A slur, a hairpin, a phrase mark, a bracket and an
  octave line are; pinned against the engraver. Still open: a glissando and a
  beam across a barline, which are lines between two notes and have no end a
  page could stand in for.

- ✅ **The example's page had no play cursor** *(the same look: "The play
  cursor cannot be seen either")*. Two causes. The example opened the editor
  on the ambient host, which stands alone: the cursor is the position of the
  score's transport, and only a host that is a client of the server reads
  one. The page had `session.gui()` and the script did not; closing `X5.9`
  the pair was made alike by taking the call out of the page, the wrong way
  round. Both have it now, as the notes editor's examples do, and the books
  say which host draws a cursor. And the line itself ran from the title into
  the next system: the room it takes past a system was 0.6 of the *system's*
  height, a staff's stem room on one staff and three times it on a grand
  staff; it is a measure of the staff now (`notation::cursors::stem_room`).

- ✅ **Writing notes has no cursor, and a note written pushes the rest along**
  *(found 2026-10-05, the user's first sitting at the page: "There has to be
  an edit cursor ... The pitches entered cannot displace the others, neither
  within the bar nor across bars")*. Today a press on empty staff, with
  `entry` on, calls `notation::edit::insert`, whose own doc says what is wrong
  with it here: "everything after it moves later by its value". So a note
  written into finished music shifts every note after it in the bar and
  re-bars the ones after that; the place it goes is wherever the pointer fell,
  after the nearest item; and the same press selects and drags when it lands
  on a note, so writing and moving are one gesture told apart by a pixel. What
  it is to be is the settled principle of notation programs:
  - **Note entry is a mode with a cursor.** Entering it puts an **edit
    cursor** on a staff, at a time and in a voice, drawn on the page; pitches
    are entered *at the cursor*, and outside the mode nothing is written.
    The cursor is a second line beside the play cursor, and is the host's
    `score` element's to draw from a place the editor names.
  - **An entry replaces; it never adds time.** A pitch entered over a
    **rest** makes a note of the value in hand there, and what is left of the
    rest stays a rest. Nothing after the cursor moves, in the bar or across
    bars: the bars keep their length and every other note its time.
  - **A pitch entered where a note already is joins it as a chord**, rather
    than replacing it or pushing it.
  - **Outside the mode a press selects and a drag moves**, as now; inside
    it a press is an entry and never a drag.

  **Decided 2026-10-05**, by the user, before it is taken:
  - **A value longer than what is left of the bar goes on into the next
    one**: it replaces the positions it reaches there, and the two parts are
    joined by a tie. The bar never grows.
  - **A chord is built two ways**: a press on another line or space where
    the note is, or a pitch entered from the keyboard with a modifier key
    held.
  - **Pitches are entered from the keyboard too, and the keyboard moves the
    cursor.**
  - **The window opens outside the mode.**
  - **The mode is entered with a key or with a button of the toolbar.**
  - **The cursor is put where the selection is**: entering the mode with a
    note or a rest selected puts it on that note or rest, and with a staff
    selected at the staff's start.
  - **The cursor goes when the mode is left, and playing leaves the mode**:
    a play switches note entry off, and the cursor goes with it.
  - **`insert` stays, as its own verb, and it moves everything after it.**
    It is for measures above all (inserting bars) and may serve for notes;
    it is never what an entry does.
  - **The cursor is how the second voice is written**: it stands in a voice,
    and an entry replaces in that voice alone -- which closes "A note cannot
    be written into the second voice", above, when this is built.

  **Decided later the same day**, by the user:
  - **With nothing selected, the cursor goes where it was last left**, and
    to the first beat of the score when it has not been anywhere yet.
  - **The pitches are the letters `a` to `g`.**
  - **The cursor advances by the value after each entry**, and the arrows
    take it back or on.
  - **`N` switches the mode on and off, and `Escape` switches it off**, as
    is the convention (the toolbar's button beside them).
  - **Every key of note entry is a binding**, set by file like the rest of
    the host's key table (`[gui.keys]`, `--keys`), never spelled in an
    element.

  **The default keys, agreed the same day** (each a binding like the rest):
  - **`Shift` + a pitch letter adds that pitch to the chord** at the cursor,
    which then does not advance.
  - **`Left`/`Right` move a note or rest at a time, `Ctrl` + `Left`/`Right`
    a bar at a time, `Alt` + `Up`/`Down` to the staff above or below.**
  - **`Up`/`Down` move the note just entered a step, `Ctrl` + `Up`/`Down` an
    octave**; they do not move the cursor.
  - **`Ctrl` + `Alt` + `1` to `4` move the cursor to that voice.**
  - **The digits pick the value in hand.**
  - **A letter writes its pitch in the octave nearest the note before it.**

  Still to settle when it is taken: the letters are bindings already (`E` is
  `split` in the host's table), and inside the mode its keys go first, so
  the table grows a binding scoped to a mode rather than a second name for
  `E`. What it
  touches: the model gains the verb an entry is (a write over a stretch of a
  voice, where `insert` adds time), bound in both clients; the host's `score`
  element draws the cursor and reads a press by the mode; the example's
  prose and both books' paragraph on a gesture change with it.
  **Built 2026-10-05**, as decided. The model's verb is `enter`
  (`notation::edit::enter`, `Op::Enter`, in both clients): a stretch of one
  voice written over, at a time or at the time an item starts, or a pitch
  added to the chord there. The crate's `score::entry` keeps the cursor's
  arithmetic, and the editor the mode, the cursor, where it was left and the
  note just written. The host's `score` element reads a plain press on a staff
  in the mode as `"enter" <column> <position> <staff>` -- the element whose
  column it fell in -- and draws the cursor the editor names (`edit_cursor`);
  the key table grew **scopes** (`[gui.keys.score]`, `[gui.keys.note_entry]`, a
  window's `keys`), read first in a window that names them, so the letters are
  pitches in the mode and `E` is still `split` elsewhere. The toolbar has the
  pencil, the menu's entry is Note entry, and the window opens outside the
  mode. Checked in the browser: entering, chords by key and by press, the
  second voice, the arrows, Escape. Left as found: overwriting the note a slur
  or a hairpin starts on takes the spanner with it, as a deleted note does.

- ✅ **The ○ entries are notation the model does not hold** *(listed by
  `X5.6`; taken 2026-10-05)*. Each is the model growing an item or a field,
  with its emission, its reading, its `Op` and its palette entry.
  **Decided 2026-10-05, by the user: notation and sound** -- what an entry
  means to a performance is the render's as well: a tempo mark sets the
  tempo, the pedal is controller 64, an octave line moves the sounding
  pitch, a tremolo is its repeated notes, an arpeggio staggers its chord, a
  glissando is a curve, and repeats and endings are played out.
  **Built 2026-10-05**, every ○ entry of the list above: the model's marks
  (a tremolo, a roll, a breath or a caesura, let it ring, a beat repeat, a
  fingering, a chord symbol, lyrics), its spanners (a phrase mark, a
  glissando, the pedal, the four octave lines, a bracket, a beam across a
  barline, two notes alternating), what is written at a point (`Control`: a
  tempo mark, a direction, a rehearsal mark), the grid's changes of key,
  endings, navigation marks, measure repeats and numbered rests, a staff's
  changes of clef, lines, names and transposition, and the groups of staves
  -- each written and read through the engraver's own document, a verb of the
  palettes (three new groups: Text, Repeats and jumps, Keys and clefs, and
  Staves), a method in both clients and an operation in both shells. The
  interpreter plays them (`notation::performance`), the render carries the
  tempo map, the pedal lane and a glissando's bend, and the page's cursor is
  drawn over the same reading (`docs/decisions.md`, "What a page writes is
  what it plays"). Checked in the browser, each drawn as the engraver draws
  it. **Left open**: the sound, which nobody has listened to; let it ring
  drawn under nothing visible on a note with a caesura; a slur or a hairpin
  into a measure that becomes a repeat is dropped with the items the repeat
  replaces.
  **Fixed 2026-10-06, the last**: what is written at an item of a measure
  drawn as a repeat -- a line's end, the pedal, a tempo, a direction, a
  rehearsal mark -- is written at its beat of that measure, the sign standing
  where the item's element would be, and read back onto the item the measure
  holds there. Pinned in the model and against the engraver.
  **Fixed 2026-10-06, let it ring**: the caesura was where it was seen and
  not the cause. The tie was written to the next item, and the engraver
  draws one only when it ends inside its own measure and starts on a note:
  so it drew none on the last note of a measure -- where a caesura stands --
  none on the last note of a voice, and none on a chord. It is now written
  as what it is (the user, the same day: "a tie that leaves the notehead and
  goes nowhere; a rest after the note is the common case"; "one never
  arrives at a note"): from each notehead of the item's last written part
  to a beat of that measure -- a beat on, or half the way to the next note
  or the barline where either is nearer. Pinned against the engraver.

- ✅ **A channel group is not edited as one** *(`X5.8.1`: "editing a group
  as one is not built")*. The render writes a staff's dynamics as the same
  lane on each channel of its group, each naming the group in its target;
  an edit to one of them in the roll changes that channel alone.
  **Fixed 2026-10-06**: the sequence's edit to a lane gives every other lane
  of its group -- the same target but for the channel -- the edited lane's
  points, and removing one removes the group.

- ✅ **New and Close are in the File menu and do nothing** *(`X5.8.2`)*.
  **Decided 2026-10-05, by the user**: New replaces the score in the same
  window, as Open does and as one entry of the history -- one staff, treble
  clef, common time, C major, four empty bars; Close closes the window, and
  with changes not saved asks first: save, do not save, or cancel.
  **Fixed 2026-10-06**: both are the File menu's. What its file holds is the
  document the editor last opened, saved or made new, so an undo back to it
  is nothing to save; a holder that wrote the file a save named says so
  (`saved`). Close with something to lose opens a form -- Don't save,
  Cancel, Save -- and a Save with no file asks for one first; the turn names
  the close (`close`), and its holder closes the window once the file is
  written. An Open now makes the file it read the score's. The window's own
  close mark still closes without asking.

- ✅ **The host's `L` and the toolbar's loop are two switches** *(left
  open by `X5.8.3`: "a press of `L` after the toolbar turned it may ask for
  the state it already has")*. The editor's switch was the one a play read
  and the host's the one `L` turned, and nothing told the host when the
  toolbar or the menu turned the editor's.
  **Decided 2026-10-06, by the user**: a set of measures or of notes is
  selected and a play loops it -- "the same behaviour as the other editors,
  in another domain, the notation's".
  **Fixed 2026-10-06**: a turn of the toolbar's switch or of the menu's
  carries `looping` on the window, which is the host's own switch, so the
  three are one and `L` starts from where it stands. And a selection is **a
  stretch or a place**, as a swept range and the position cursor are in the
  other editors: a measure pressed on its staff, a selection extended with
  Shift, everything, or several items picked, is the stretch a play plays
  once and a loop repeats -- from its first note to the end of the one that
  ends last, though the measure holds one note; one item picked is where a
  pass starts.

- ✅ **A window's widgets asked one by one engrave the score once each**
  *(found 2026-10-06, looking at a selection in the browser)*. A client asks
  the editor for a window's widgets one at a time (the door's `props`), and
  the score editor answered each with the whole window corrected -- the page
  engraved again -- whichever widget was asked for. Measured in a page: one
  `select` made from the script engraved the score 165 times, once for every
  tool and palette entry, three loads each with a page break written. A
  press on the page does not take that path, so a hand never saw it.
  **Fixed 2026-10-06**: only the page and its scroll are answered with an
  engraving; the status line is answered with its sentence, a tool with the
  chrome's state. Pinned by counting what the engraver draws.

- ✅ **A run of pages after the first carries the engraver's own credit**
  *(found 2026-10-06, the same look)*. The engraver signs the foot of a
  document's first page where no foot is written, and a run that opens on a
  page break is a document of its own: the score's second page had the
  credit the first one, which has a foot, does not.
  **Fixed 2026-10-06**: a run that does not start the score is written with a
  foot, an empty one where the score has nothing on every page. Pinned
  against the engraver.

- ✅ **The page number stood in the middle of the head** *(the user,
  2026-10-06, looking at a second page: "page numbers do not go at the top in
  the middle")*. The running head wrote it centred, on every page from the
  second.
  **Fixed 2026-10-06**: it stands at the page's outer corner, as a score's
  page numbers do. The head writes it against the right margin, which is the
  outer one of an odd page, and the page view turns an even page's over to
  the left, as far from the paper's left edge as it was from the right --
  where it already writes the number over the engraver's count. Pinned
  against the engraver.

- ✅ **The window's close mark loses unsaved changes without asking**
  *(found 2026-10-06, building Close)*. The File menu's Close asks; the mark
  on the window's frame frees the window at once, since the host closes it
  before any owner hears. Asking there means the host holding a close until
  an owner that wants to ask has answered -- a window prop saying so, and a
  close request the owner confirms -- which is the host's, for every
  editor, and not the score editor's alone.
  **Fixed 2026-10-06**: a window carrying `ask_close` is not closed by its
  close mark, nor by a desktop window's Escape: it reports `close`, as its
  menu bar's entry for it where the bar has one, and its owner frees it once
  nothing is left to lose. A second close the owner has answered nothing
  since closes it anyway, so an owner that stopped answering cannot keep it
  open; and a standalone host whose owner freed its last window ends, as the
  close mark ends it. The score editor's window asks when it has its forms
  to ask in; a bare one closes at once, since the score it edits is the one
  its holder keeps. *(Narrowed the same day, by the user: a window asks only
  where it is the work's one holder -- the entry after next.)*

- ✅ **The standalone session's windows close without asking** *(found
  2026-10-06, after the entry above)*. A window asks only where it is the
  work's one holder, which is a standalone host's, and there only the score
  editor's asked: the multitrack and the rolls edit the session the host
  saves with Ctrl+S, and their windows closed and let unsaved changes go.
  This entry first said it waited for the chrome in standalone; it did not,
  since the close form is a dialog any window holds on a page of its own,
  with no menu bar or toolbar around it. And a roll is not a holder of its
  own: the user, the same day, on an application inside another -- the outer
  one governs, the multitrack here, and when its window closes every window
  that depends on it closes too (`crates/clausters-document/PLAN.md`, "An
  application inside another").
  **Fixed 2026-10-06**: the session knows whether it has changes its file
  does not hold (its history's version against the one last written or
  opened); the multitrack's window holds the close form the closing module
  builds (`closing::stack`), numbered by the host, and says `ask_close`; its
  editor answers the rule like every editor, and the form's Save writes the
  session and closes -- a save that fails says why on the status bar and
  keeps the window. A roll opened from the multitrack depends on its window
  (`Host::depend`), so the multitrack's window takes its rolls with it
  whichever way it closes, and closing a roll alone loses nothing.

- ✅ **Closing and the key sheet are every application's** *(asked for by
  the user 2026-10-06: F1 and the close are a general mechanism, and have
  to be available equally to any application; F1 works with no chrome; and,
  decided the same day, an editor a client holds does not ask at all --
  "the client can close it, and it already has the data in the handle, which
  can open the window again; what matters is that the edited information is
  not lost, and if it is already in the client's objects it is not, and the
  client is responsible")*. The host half was general already; the
  application half was the score editor's alone. **Done 2026-10-06**:
  - **The close is one rule** (`closing`): at once, unless the window is the
    work's one holder and has something to lose, and then the editor's form
    asks. The shared turn reads the window's `close` for every editor; an
    application adds whether it has something to lose, whether its holder
    said the window is the only one (`asksToClose`, which the standalone host
    says and no client), and the form it asks in, which the module builds;
    every editor's outcome carries `close`; both clients' `Editor` and the
    standalone host free the window on any editor's. So a client's score
    editor, whole or bare, closes at once by its mark and by File, Close, and
    the standalone host's asks.
  - **A scope is the application's** (`score::keys`): the window brings its
    scopes, their default chords and the words F1 shows them with (its
    `verbs` prop, bare or whole), and the host lays them into its table under
    what the user bound -- three layers, so a config's
    `[gui.keys.note_entry]` still wins whenever the window opens. The host's
    table names no application now.

- ✅ **The standalone host's score neither plays nor exports**
  *(`X5.8.3`, `X5.8.4`)*. **Decided 2026-10-05, by the user**: with
  `--score` the host boots its embedded server, as `--session` does, and
  plays the score with a simple def of its own; it exports with the MIDI
  writer the clients use.
  **Fixed 2026-10-06**: `--score`, in a build with `standalone`, opens the
  on-demand server and a player as `--session` does. The score's render
  plays as a roll's sequence plays in that host -- the editor's playback on
  a transport of its own, through the player -- and the page's cursor is
  bound to that transport, as a client's score editor binds it; a pass, a
  stop, the loop switch, a rewind and an edit heard on are the turn's, as
  in both clients. The def is the server's built-in `default`, which a
  render's events name by naming none. An export writes the render with
  `clausters-midi`, the clients' writer, which the `score` feature now
  pulls in. Nobody has listened to it.

- ✅ **The toolbar's voice and layout selectors are words** *(the symbols'
  entry, above: `v1`, `v2`; `page`, `line`)*. They take symbols, as every
  other tool has.
  **Fixed 2026-10-06**: the voices are the quarter with its stem up and
  with it down, as a page tells two voices apart -- SMuFL's two, the second
  drawn from the face's first turned over where the face lacks it -- and
  the layout is a sheet with its systems and a system running on, both the
  editor's own. Found with them, by eye: **the latches read as on when
  off.** A toggle drawn as a button took, off, the fill a segmented
  choice's chosen option has, so the toolbar's note entry, dot, rest and
  loop all looked pressed; the host now draws a latch as an option chosen
  or not -- the field's well off, the chosen fill on.

- ✅ **A glissando's pitch, and how it is heard** *(the user, 2026-10-06,
  in the score editor: a glissando must be logarithmic in frequency to sound
  right, or go through midi-to-hertz, depending on how it is built)*.
  Checked, and it already is: a glissando (MEI's `gliss`) renders as the
  note's own curve over its pitch, played as a pitch bend, straight in
  **semitones** from its onset to its release
  (`events::score::render`), and every player turns a pitch bend into a ratio --
  `midiratio` of the channel's and the note's bend on the server
  (`event_graph::pitch_def`), `2^(s/12)` where a client sends the values
  itself (`event_curves`), and a per-note pitch bend in semitones in a MIDI
  2.0 file -- so the frequency moves geometrically: halfway up an octave's
  glissando is a tritone, not the mean of the two frequencies. Two tests hold it
  there (`a_glissando_is_straight_in_pitch_and_geometric_in_frequency`, and
  the render's glissando in semitones).
  **Found while listening, and fixed 2026-10-06**: a glissando written from
  the last note of a tied chain -- where it is written -- was lost, since the
  chain sounds as its first note and the glissando was looked up on that one.
  It is now the chain's, and starts at its last note (`Note::gliss_from`):
  the tied part holds its pitch. The terms were also put back to the
  engraver's, at the user's call (bend and glissando are two notations: a
  bend is a curve out of a stretched string's note, a glissando a straight
  line between orchestral notes): the model and the performance say
  `gliss`, as MEI and verovio do; "pitch bend" names only the MIDI message
  a glissando is played as; and the curve of a held note under a hairpin is
  `hairpin`, not a word of the project's own.

- ✅ **A hairpin's level** *(the user, 2026-10-06, in the score editor: a
  crescendo or a diminuendo grows straight from the note it starts on to the
  one it ends on -- in amplitude for now, from the first note's onset to the
  last one's -- and whether it grows inside a note or between notes is the
  instrument's)*. The attacks were already straight in amplitude over that
  stretch. Two things were not. **A hairpin that ended on no dynamic let
  go**: the notes after it dropped back to the level before it, where the
  level it reached holds until the next dynamic -- the curve and the attacks
  now hold it. **Nothing grew inside a held note on a synth**: the level's
  curve is a controller (11, expression), which a synth does not listen to,
  so a crescendo over a held note was heard only at the next attack. Where
  the reading hears a dynamic in the attack and the curve both (`both`, the
  default), a note held under a hairpin now carries it as its own curve
  over `amp` (`Note::hairpin`, rendered as the note's
  automation), from its level at the attack to where the hairpin has taken
  it at its release, straight between; with `attack` -- a struck sound --
  no note carries it. What a good reading of dynamics needs beyond this is
  "Dynamics need a model of the instrument and of hearing", under Future
  directions.

- ✅ **A track's gain automation does not reach its notes** *(the user,
  2026-10-06, trying the standalone session: the track's envelope -- its
  gain -- has to act on a sequence of notes too, whatever the MIDI
  automations and the velocities say; a track's envelopes apply over them,
  and here they do not act, because one is the audio's and the other the
  MIDI's)*. The notes of every box over a sequence play on the multitrack's
  one event lane, and its voices sound through their own `out`, **outside
  the tracks' strips** (`clausters_editing::playback`, `notes`): the strip is
  where a track's gain, its gain curve, its mute and its meters are, so none
  of them reaches a track of notes -- only mute and solo do, by deciding
  what is placed at all. What a track produces is what its strip shapes,
  whether it reads samples or plays notes: the notes' own velocities and
  note-level curves shape each voice, and the track's envelopes apply after
  them, as over a take.
  *(The same day, the user, on the shape: the box of notes makes its sound
  with the synth it has, and the track takes that sound as its input the
  way it takes a segment of audio -- and the node graph may have to be
  reviewed for it.)* So a box of notes is a **source of sound inside its
  track**, as a box of samples is: a slot of the track's group whose
  output is what the strip reads, where today a box of samples is a slot
  holding readers and a box of notes is nothing of the track's. What the
  review has to settle, against the graph as it is
  (`clausters_document::multitrack::nodes::plan`, `clausters_editing::instance`):
  - **a slot whose contents are voices**: a box's slot holds readers made
    once and kept, and a box of notes holds voices made and freed as the
    transport plays -- whether a slot can be a group the event lane makes
    voices in, writing to the slot's own out, or the voices stay on the
    lane and name the bus of the track they sound into;
  - **one lane or one per box**: the multitrack plays every sequence on one
    event lane in the transport's group; a box's voices sounding into its
    track either splits the lane per box (each made in its slot) or gives
    each voice its box's bus;
  - **the curves inside a note**: a note's own curves are graphs the lane
    makes beside its voices (`clausters_editing::note_curves`), and they go
    where the voices go;
  - **a box moved to another track** carries its sound with it, as a slot of
    samples is moved and re-wired (`/graph_moveSlot`);
  - **the synth**: what plays a box is what its events name (their
    instrument, the server's `default` when they name none) -- a box's own
    choice of synth, beside what its events say, is a design of its own.

  **Built 2026-10-06, and what the review settled.** A box of notes is a
  **clip of its track** over a bus of its own -- the clip's strip, with an
  input where the readers are (`clausters_core::mixer::voices_graph`,
  `VOICE_SLOT`) -- so the box has its own gain, mute and curves and the
  track's strip, its curves and its meter are after it, as after a take.
  - **The voices stay on the lane and name their box's bus.** A slot's
    members are fixed by its def and a note's def is whatever its events
    name, so a voice is not a member of the clip: it is told the bus as its
    `out`, the control an event's `out` key already sets
    (`MultitrackPlayback::notes`; a key of that name on a note of a box is
    not read). The bus is the client's (`Op::AudioBus`), handed to the clip
    on `in0`/`in1`.
  - **One lane**, as before, its notes made in a group **before the
    multitrack** (`instance::VOICES`): a voice has to have written its bus by
    the block the clip reads it. It was the tail of the transport's group,
    after the multitrack.
  - **The curves inside a note** go with the voice: a note's graph has the
    controls it is started with as ports, `out` now among them, and the
    channels' instances are made in the same group.
  - **A box moved to another track** is moved as a clip of samples is
    (`Op::Move`), with the bus it had.
  - **Which boxes play notes is the notes' to say.** The document names a
    source and not what is behind it, so `nodes::plan` cannot know; the
    playback plans each box the placed notes name
    (`nodes::plan_voiced`), against the multitrack the last `sync` was of.
    No door changed, so neither client did.
  - **Mute is the strip's.** `placed_notes` no longer leaves a silenced
    box's notes out: they sound into a muted strip, and a note that began
    under a mute is there when it is lifted. A MIDI message and a command
    are still left out of a silenced box, since no strip reaches them.
  - **Two things the built-in `default` needed**, both the server's: an
    `out` control (it wrote buses 0 and 1 as constants, so an event's `out`
    did nothing on it), and a bus analysis that reads `out + 1` as the
    static index it is -- arithmetic over controls and constants -- rather
    than as a barrier the group's sort cannot cross (`docs/auto-order.md`).
  Heard by a render: `tests/multitrack_playback.rs` (a track's gain curve
  over a plain note and over one its own curves shape) and
  `tests/mixer_graph.rs`.

  **What is left**, each on its own:
  - **A def with no `out` control** sounds where it was written to, past the
    track. It is the event convention's own limit and it is stated in the
    books; nothing reports it.
  - **A MIDI message or a command in a box** goes where it names (a
    channel's binding, a node). Routing a bound instrument into the track of
    the box that plays it is not designed.
  - **The roll's own play** (`NotesPlayback`, a transport of its own) is the
    sequence alone, outside any track. Whether a roll opened from a box
    plays through that box's track is part of
    `crates/clausters-document/PLAN.md`, "An application inside another".
  - **A box's own choice of synth** -- the entry in "Future directions"
    below, "A box of notes chooses its synth".
  - **The clients' builders refuse a channel list on a control's bus**
    (`clients/python/PLAN.md`, Found by use), so a stereo instrument with an
    `out` control is written one `out` per side.

- ✅ **A stereo take plays both its sides on the left** *(found 2026-10-06,
  by a render, writing the test of a box of notes on a track)*. A clip's
  readers are one per channel of its source, and its slot wired every one of
  them to the first channel of the clip's bus: the take's right side was
  summed onto the left and the right was silent before the balance. No test
  held a stereo take -- the mixer's were all over a mono one. **Fixed**: a
  reader is handed both channels of the clip's bus (`out0`, `out1`) and
  writes each at a gain its channel says (`clausters_core::mixer::reader_def`).
  Two buses named by controls rather than one worked out as `out + chan`,
  which is what the audio editor's reader does: a bus index a UGen computes
  made its node a barrier for the group's sort, and inside a track that put
  the meters before the strip they read (`tests/mixer_graph.rs`,
  `a_stereo_take_keeps_its_two_sides`). *(The sort reads that arithmetic
  since the entry above was closed, the same day; the two controls stayed.)*

- ✅ **A history step is whole on each structure, not across them**
  *(found 2026-10-08, making a gesture land whole: `domain::edit_all` and
  `clausters_document::apply_all`)*. A gesture now lands all or none, and so
  does each structure's share of an undo or redo step -- the multitrack
  editor's (`MultitrackEditor::apply_step`), the host's multitrack and tree
  (`host::document::carry`). What is not whole is a step whose entry spans
  **several** structures -- a multitrack and a curve edited by one gesture:
  `Editing::step` hands each structure its legs in turn, so a refusal on the
  second leaves the first stepped. A refusal there means the history and a
  structure disagree, which no path is known to produce today. The fix is a
  step that puts back the structures it already stepped when a later one
  refuses -- each leg's opposite direction is in the same entry -- and
  answers the step as not taken.

  **Closed 2026-10-08, the same day**, by keeping rather than inverting:
  `Editing::step` keeps what each member holds before handing it its legs
  (`before_step`), and a structure held by a member that moves nothing puts
  every structure already stepped back (`put_back`), moves the cursor back
  and answers the step as not taken. Keeping is used over the entry's
  opposite legs because a redo's opposite may be a step only its owner can
  run (`a_step_refused_on_one_structure_puts_back_the_others`).

- ✅ **Most of the windows' verbs have no client API** *(found 2026-10-08,
  writing `docs/verbs.md`)*. An application is an API its clients' handlers
  call, and a menu, a key or a tool is one more caller of it -- but of the 76
  verbs in the one verb table (`clausters_editing::verbs`), 42 are reachable
  only from the window: no member of either client does what they do. The
  window's clipboard and editing verbs (`cut`, `copy`, `paste`, `mix`,
  `delete`, `select_all`, `split`, `join`, `quantize`), its view and
  transport verbs (`view_all`, `loop`, `to_end`; `to_start` is the
  multitrack's `rewind` and no other editor's), the multitrack's
  `stop_at_end`, `reset_heights` and `compact_tracks`, and note entry's
  writing and moving keys (`pitch_*`, `chord_*`, the cursor's, `voice_*`,
  `enter_rest`) and the score's `select_left`/`select_right`. The two
  clients agree -- every one is missing from both -- so this is not a
  divergence, and `clients/python/tests/test_verbs.py` keeps it that way.
  **What it needs is a decision before code**: most of these act on what the
  host holds (the selection, the focus, the clipboard, the view), so a
  handler asks the host to perform them, which no command does today -- a
  `/gui_*` command that performs a verb on a window would be one door for all
  of them, against a member per verb per editor. Each row that closes moves
  to `api` in `docs/verbs.md` in the same commit.

  **Closed 2026-10-08, the same day, by the one door.** `/gui_verb <window>
  <verb>` reaches the host's one dispatch -- the one a key, a menu entry and a
  tool end in -- with no pointer, so a verb a script asks for does what the
  window does. Both clients have `editor.verb(name)` over it and a named
  member per verb, one line each (`select_all`, `cut`, `split`, `write("C")`,
  `entry_voice(2)` ...), the way a UGen builder is one line over the node it
  makes; `docs/verbs.md` has no `gap` left. `clients/python/examples/editors/window_verbs.py`
  and its page are the by-eye check.

- ✅ **Quantize does nothing in the notes editor or the multitrack: no
  application gives its views a grid** *(found 2026-10-08, writing
  `editors/window_verbs`)*. The host quantizes to a view's `snap`
  (`structures::boxes::quantize`), and a roll or a lane with no `snap` has no
  grid: the verb is refused as "these notes are already on the grid", from Q,
  the Edit menu, the toolbar and `editor.quantize()` alike. No application sets
  one (`grep '"snap"' crates/clausters-apps`), so a drag does not snap either.
  **A decision first**: the applications' axes are in beats over a tempo map
  and a view's `snap` is one number in its own units, so the grid is either
  carried in beats and resolved through the map where a box lands, or the
  units become beats -- and then whether the grid is the document's (one per
  session, like its defaults) or the window's (a View-menu choice, like the
  notes' view).

  **Closed 2026-10-08, the same day: the grid is in beats, the data the
  document's** (decided with the user). The axis carries a `grid` in beats and
  the host resolves it through the axis' tempo map (`boxes::Grid`,
  `TempoMap::snap_secs`), for every placement a view makes -- a drag, a
  paste, a split, step entry and the quantize. The notes editor and the
  multitrack state a sixteenth until a script or a client chooses another
  (`editor.grid`, both clients); the grid is the window's, like its zoom. What
  a quantize leaves is written into the document, on its beat exactly
  (`TempoMap::settle_beat` on every beat the notes axis reads back). The record
  is `docs/decisions.md`, "A grid is a length in beats".

- ✅ **Inactive-file history and resource routing** (2026-10-18). A shared
  undo could not find an audio file while its window showed an independent
  file, and storage/release methods inspected only the focused file. Route
  history legs by registered object, update that object's playback without
  rebinding the window, account for all orders and keep consumers as separate
  resource roots. A source without a scratch port must not prevent another
  source from spilling. Clipboard replacement must not free a previous
  connection incarnation's reused buffer number.

- ✅ **Audio file navigation carries another file's presentation**
  (2026-10-19, native paired-window review). In the `take` window with
  three mono files, zoom, selection and other presentation changes appear
  in the other files too. Restore each file's own viewport, selection and
  position cursor across file-list navigation, including the host's retained
  widget state. Review the actual window path as well as the core file swap.
  Fixed by retaining observed horizontal and vertical viewports per file and
  restoring them with the incoming file's cursor and selection. Directed
  file-state tests and a native headless host probe cover navigation; the
  native real-window acceptance is recorded above.

- ✅ **Eight-channel playback starts late and stalls the GUI**
  (2026-10-19, native paired-window review). Starting playback in the
  `eight channels` window is delayed and makes interaction slow or stalled.
  Interval samples in this debug run show about 60-62% of one logical CPU
  in the server, including about 44% in `pw_out`, and GUI samples reaching
  about 66%. Separate idle, redraw and playback work, locate the expensive
  path and compare an optimized build before attributing the cause. Acceptance
  requires responsive real windows and prompt playback, not only log success.
  Removed full sample clears and peak scans for unwritten buses, fixed the
  multichannel output alias below and refreshed the release artifacts used by
  the example. The directed native run starts in 128 ms, answers host queries
  within 1 ms and captures the signal with eight matching meters. That run
  measures about 5.2% server / 0.6% GUI CPU idle and 7.4% / 4.6% looping,
  with 100% meaning one logical CPU. Closing the windows stops the example's
  session. Combined hand review remains under the open acceptance above.

- ✅ **Audio paste uses the selection end and leaves the selection visible**
  (2026-10-19, native paired-window review). Copy correctly keeps the
  selection, but paste was observed at its end instead of the position
  cursor, with the old selection still visible. Follow the existing behavior
  rule: paste at the position cursor, clear selection after successful paste
  and keep the cursor at the edit's landing position. Check both host verb
  dispatch and the confirmed operation's projection into the current view.
  Fixed the host's implicit cursor move on sweep release. Release now confirms
  the selected loop span independently, while copy retains selection and paste
  uses the last click. Gesture regressions and the native headless copy/paste
  probe check the position and the cleared selection after confirmation.

- ✅ **Multichannel output aliases private audio buses and corrupts meters**
  (2026-10-19, eight-channel window review). An eight-channel output used
  `OUT:2` through `OUT:7` even on a two-output server. Those indices overlapped
  client audio buses and summed channels back into the editor's input. Connect
  only negotiated device outputs; remaining channels use the output graph's
  private discard bus. A native NRT regression reproduces the wrong amplitude
  before the fix and checks eight distinct channel levels and stereo output
  after it. Waveform capture confirms that the multichannel samples are drawn.

- ✅ **The interactive review launcher keeps the server alive after windows close**
  (2026-10-19). A persistent Python prompt kept the review session, GUI host
  and server alive after the visual review. The example's `run()` now waits
  for all its windows and closes its session in `finally`; review launches use
  that blocking entry point instead of an interpreter left waiting for input.

- ✅ **Meters show the previously playing file after navigation**
  (2026-10-19, native window review). Switching files during playback kept
  the previous file's levels and channel count. The meter now always takes
  its width from the viewed file and reads live buses only when that file
  plays. Viewing another file shows zero without stopping playback; returning
  restores live levels. Rebinding the meter bus also clears held peaks and
  clip lamps from the previous source. Directed transport, host and paired
  client regressions cover navigation and source changes; native manual
  window acceptance is recorded above.

- ✅ **Structural redraw enables an inactive file's play cursor**
  (2026-10-19, native window review). Focus corrections hid the cursor of
  another file's running transport, but the next window definition restored
  an unconditional zero anchor. Use the same viewed/playing-file rule for
  both the full definition and incremental corrections. A failing regression
  reproduced the zero anchor before the fix; directed tests cover repeated
  waveform/stacked redraws, file switches and stopping. The native host probe
  checks the projected anchor after file-list visibility changes during play.
  Native visual acceptance is recorded above.

- ✅ **Saving one audio window rejects the gesture in another**
  (2026-10-19, native window review). Operation classification bypassed the
  ordinary conversation's target ownership check, so a broadcast Save was
  prepared for unrelated editors and emitted a false refusal alongside the
  real completion. Check window/component ownership before classification,
  and unwrap menu verbs only for the owning window. A failing regression
  reproduced the duplicate acknowledgement; directed coverage checks key and
  menu saves across independent contexts and leaves the other file dirty.
  The delayed-completion native visual review is accepted above.

- ✅ **An invalid undo target steps the previous history order**
  (2026-10-19, action-target review). A scripted `step` naming an unknown
  member kept the last selected order; malformed ids selected the shared
  context, and oversized ids could wrap to a valid member. Validate an
  explicit member before selecting or preparing undo/redo, and return an
  ordinary refused step without effects or version changes. Omitted/null
  members retain the existing explicit context behavior. Directed tests
  reproduce the failure and cover invalid numeric, string and boolean ids,
  undo/redo and a valid step after refusal. This does not complete `X20`.

- ✅ **A failed repair leaves the audio file locked without a client retry** (2026-10-20, branch review). When both an edit and its repair failed, adapters discarded the ticket while the runtime retained the lock. Member-scoped repair discovery and paired client retry methods now retain access to recovery without repeating the edit. Exclusive retry claims, repeated failures, original gesture acknowledgement, successful release and invalidation are covered by directed tests.

- ✅ **Pending content confirmation restores old envelope visibility** (2026-10-20, branch review). Showing the gain envelope during a sample edit was overwritten by the prepared contents, leaving an active but hidden points body. Content confirmation and history restoration now preserve current visibility by curve identity; directed core and paired client regressions cover delayed confirmation and redraw.

- ✅ **An undo after a spilled audio stroke does not step**
  (2026-10-21, found running the server's suite while adding its
  profiler). `tests/audio_editor_graph.rs`,
  `application_sources_spill_restore_and_release_without_a_renderer`: after
  two draw strokes on a resident application source, the second spilling
  the first, `{"verb":"step","direction":"undo"}` answers `stepped: false`.
  Bisected to "Read application actions from one definition in every
  window" (`e25ae291`); the test passes on its parent. Nothing else in the
  workspace suites runs this path, which is why it went unnoticed. Undo and
  spill/restore are part of `X24`'s acceptance, so it is fixed before that.
  *Fixed 2026-10-21:* the test was stale, not the editor. That commit gave
  an audio editor's first file an undo order of its own, so a step without
  a member addresses the context's order, which no longer holds it; the
  test now steps its member, as the file's other undo test does.

- ✅ **The `edit_audio` pair reads differently at its end** *(found
  2026-10-22, accepting the audio editor)*.
  `scripts/audit-example-pairs.py editors/edit_audio` reports one call: the
  script waits on `session.gui().wait()`, every window the session opened,
  and the page on `editor.wait()`. Both are the examples' endings (`wait` is
  dropped on both sides), but the script's `gui()` is counted. Either ending
  the script on the editor, as the page does, or teaching the audit that the
  host a `wait` is reached through is part of the ending, makes the pair read
  as one.
  *Fixed 2026-10-22:* the script ends on `editor.wait()`, as the page does,
  and the audit reads the pair as one.

- ✅ **A paste refused for what the clipboard holds says nothing**
  *(found 2026-10-22, the eye pass over `editors/audio_lifecycle`)*. Text
  copied in another program after a copy of samples is what a paste pastes,
  and over a take it is refused -- correctly -- but in silence: Ctrl+V does
  nothing, a script's `paste()` returns as though it had pasted, and the
  window's readings still say `samples` until it publishes them again, since
  the host reads the platform's clipboard only at the paste. Found when the
  steps were copied from one terminal into another, which put each command's
  text on the clipboard between `copy_first()` and `paste_second()`. The
  refusal should be said where a refusal is said, and the readings follow
  the clipboard.
  *Fixed 2026-10-22:* an action the host refuses at its key or a script's
  verb is said on the window's status line with its reason (`paste: the
  clipboard holds no samples`). The fit is the host's reading, made against
  the platform's clipboard as it is at the paste and when a menu opens; the
  owner's readings carry only where a paste lands, so they had nothing to
  follow.

- ✅ **A compressed file opens through a buffer of all of it**
  *(found 2026-10-23, moving the audio editor onto files)*. Since the
  clients became the handler (2026-10-24), a compressed file is not opened
  at all: the application reads WAV, RF64 and W64 in place and refuses the
  rest, and so does `edit`. A part streams
  PCM only (WAV, RF64, W64), so an MP3 or a FLAC is decoded whole into a
  server buffer by `edit`, which the open then writes to the working folder:
  an hour of it costs the memory and the time the files model removes for
  everything else. It should be decoded into the working folder a run at a
  time, as an offline render reads a stream, without a buffer between.
  **Decided with the user (2026-10-28):** the window opens at once and the
  take appears when the decode ends, as a take appears with its peaks; what
  is edited is the decoded copy, and a save of a compressed file is a save
  as WAV, never a write over the original, since nothing here encodes a
  compressed format. Saving in other formats is a later feature (FLAC first:
  lossless encoders exist with licenses this project can take; MP3's
  encoder's license has to be weighed first).
  *Fixed 2026-10-28:* a file whose codec a part does not read in place is
  decoded at open by an offline render that streams it through `DiskIn`
  into `<serial>-decoded.w64` of the working folder with its peaks
  (`renders::decode`); the take is that copy, untitled, named after the
  file opened, and the file opened is left as it was. Neither client
  refuses a compressed file now; `reads_in_place` / `readsInPlace` still
  say which files are read where they are. Heard end to end over an AIFF,
  which takes the same path (`a_file_not_read_in_place_is_decoded_and_heard`).
  A page does not decode yet: see "A page decodes no compressed file when
  it opens".

- ✅ **What the clients' audio coordinator leaves behind** *(found
  2026-10-24, making both clients the audio editor's handler)*. With the
  coordinator gone from the clients, these have no caller in them: the
  Python `_native.Workspace` and `_native.AudioEditorPlayback` bindings,
  `SamplesView` (still exported by both clients' `editing`), and the node
  half of the web client's `src/base/workspace.ts` with its decision ("The
  web client holds its working folder by what it has"), since under node
  the folder is now the native host's. The page half of that module is
  what the page step can hold the application's folder with, so the sweep
  waits for it and closes `X30`.
  *Fixed 2026-10-24:* the page step held the folder through the core
  (`clausters_core::workspace`'s page side), so nothing calls
  `src/base/workspace.ts` and it went with its test and its decision
  (superseded). `_native.Workspace` and `_native.AudioEditorPlayback` went
  with the C ABI symbols behind them and the wasm `AudioEditorPlayback`
  (core ABI v94), and `SamplesView` from both clients' `editing`.

- ✅ **Two playback tests of the application's engine fail now and then under
  a parallel run** *(found 2026-10-24, moving the application's threads
  onto one door)*. `a_loop_moved_while_it_plays_is_crossfaded` and
  `two_loop_switches_close_together_at_its_end_are_not_heard`
  (`crates/clausters-app-engine/tests/playback.rs`) failed once each in a
  few runs of the crate's suite, before the change and after it, and pass
  alone: they drive the engine block by block against a take's reader on a
  thread of its own, so a loaded machine changes what the reader has filled
  by a given block. They should wait for the reader as the engine's cue does
  rather than for time.
  *Closed 2026-10-28:* the tests were right and the engine was not. The
  loop switch's failures were a loop change that landed a block late at
  the loop's end, which a loaded machine made likely by moving the block
  its readers were ready on; fixed in the engine (see "A loop switched to
  another loop near its end plays silence on a machine of one or two
  cores", below). The crossfade test, which turns a loop the same way,
  has not failed since, held to one or two cores; reopen it if it does.
  *Reopened 2026-10-28:* `a_loop_moved_while_it_plays_is_crossfaded`
  failed once more (`playback.rs`, the one jump it expects), in a run of
  the crate's suite beside a build of other crates, and passed six times
  alone right after. The change being tested did not reach it (its take
  is mono into a mono list, read the same by either channel rule), so the
  crossfade has a load-dependent failure of its own.
  *Closed again 2026-10-28, reproduced first:* with every core busy the
  suite failed about one run in three. Two causes, neither the crossfade.
  The test read a wrap landing on a block's last frame as a step in the
  wrong block (its step is the first frame of the next), now allowed. And
  the readers fell short of a machine that busy: the silence was the ring
  out of lead, which the engine did not always count -- a replacement whose
  kept frames ran out before its refill landed was silence counted as
  nothing, natively and in a page. It is counted now (`crates/clausters-server/src/dsp/stream.rs`,
  `crates/clausters-server/src/dsp/disk_web.rs`; `a_replacement_whose_refill_is_late_is_counted_as_starved`),
  and the three loop tests run again a run the engine reports starved, up
  to four times, failing if every run starved (`unstarved`). Under full
  load the suite then failed once in twelve, on the case below.

- ✅ **A peaks read lands after its working folder is gone** *(found
  2026-10-24, the audio editor's acceptance page)*. Closing an editor
  removes its working folder while a level of a peaks file in it is still
  being read for the window, and the host warns `peaks <path>: ...
  NotFoundError` as the read lands. The window is gone, so nothing is
  drawn wrong; the host should forget a file's pending reads with the last
  window reading it rather than warn about them.
  *Fixed 2026-10-28:* the host's answerer forgets what it read of a file,
  its peaks and its spectral summary when the last list naming them is
  forgotten (`Answerer::forget`), so a read still on its way lands on
  nothing and is dropped without a warning; a file another window's list
  names keeps what was read (`a_read_landing_after_its_last_list_went_is_dropped`).

- ✅ **A file of fewer channels is drawn silent where it is played**
  *(found 2026-10-24, moving the take onto a tree)*. A part with no channel
  map over a file narrower than the take is drawn by the host with the
  file's channels in order and silence past them, and played by the
  application's reader (and the server's stitch) with the file's channels
  repeated (`c % channels`): a mono file pasted into a stereo take sounds on
  both channels and draws on one. Two rules for one map, in two places; the
  map a part reads should be one function the drawing and the reading both
  call.
  **The rules, decided with the user (2026-10-28).** The implicit map is
  the mono one: a mono part under a take of any width is heard and drawn on
  every channel, and otherwise a channel reads the file's channel of the
  same number, silence past it. Pasting a block whose width differs from the
  take's, which is refused today, goes by a rule:
  - **mono into N channels duplicates** it on every channel (a map of
    zeros);
  - **N channels into mono mixes** them, the average of the channels so a
    paste never clips;
  - **N channels into M channels takes the channels as tracks**: the block
    lands from a chosen channel of the take (the first when none is chosen),
    only the channels that fit are pasted, nothing is duplicated or mixed,
    and the take's other channels are silence over the inserted span.
  Duplicating and choosing channels are routing, written as the part's map;
  mixing is **rendered** into the working folder, because a part's picture
  is composed from its file's peaks one channel at a time and the peaks of a
  sum do not follow from its channels'. A rule is a function of the block's
  width, the take's width and the chosen channel that returns a map or a
  render, kept in one place, so another rule (a mix with a layout's
  coefficients) is one more function and the paste names which it applies.
  **The rules build one general structure** *(the user's, 2026-10-28)*: a
  channel matrix, a fixed gain for each input channel in each output
  channel, of which a part's map is the routing form (each output reads at
  most one input, at unity). The same matrix is what a track of one width
  playing into a master of another needs (the document plan's "A track is
  as wide as its widest region"), where it is applied as it plays and a mix
  costs nothing to draw. A pan or a balance stays the strip's control and
  out of the matrix: a mono track in a stereo mix is panned under a law,
  while a mono block pasted into a stereo take is duplicated at full level,
  and both are conventions of the field.
  Choosing the channel is a feature of its own ("Working on one channel of a
  take", Future directions); until it lands the paste starts at the first.
  *Fixed in part 2026-10-28:* the matrix is `clausters_core::channel_matrix`
  with the three rules and the implicit conversion, and a part's map is one
  function over it (`clausters_core::segments::channel_map`), which the
  host's drawing, the application's reader, the server's join, the list
  renders and `/buffer_mix` all call; a mono part under a stereo take is
  drawn on both channels now.
  *Fixed 2026-10-28:* a paste of another width goes by `ChannelMatrix::fit`.
  A block the host carried is fitted in memory, exactly, before its file is
  written; segments are routed by writing the composed map into each part,
  reading nothing, and mixed into a mono take by a render through the matrix
  (`renders::convert`), heard end to end as the average of the copied
  channels (`a_stereo_copy_pasted_into_a_mono_take_is_its_average`). Paste
  mixed fits a width the same way. Until a channel can be chosen the block
  lands from the first.

- ✅ **A window closing answers every window's pending action** *(found
  2026-10-25, reviewing the audio editor's code before `X34`)*. The host's
  application thread forgets a closed window by answering every action
  still waiting on a render with "the window closed", whichever window it
  was asked through (`crates/clausters-gui/src/host/audio_app.rs`, `forget`): with
  two editors open, closing one while the other saves raises in the other's
  client, and the save lands anyway. Only that window's actions should be
  answered.
  *Fixed 2026-10-25:* each action waiting on a render is held with the
  window it was asked through, and a closing window answers only its own.

- ✅ **Two new files in one window are one file** *(found 2026-10-25,
  the same review)*. A file the editor makes was named after the buffer
  it was played through, which the application's engine does not have, so
  every new file of a window was the same file of its working folder:
  writing the second overwrote the first, and closing either deleted what
  the other read. *Fixed 2026-10-25:* a made file is named after the
  number it is open under in its editor.

- ✅ **A closed audio window stays in the editing context** *(found
  2026-10-25, the same review)*. A window that closes detaches its view and
  stops its playback, but its member keeps its seat with its files, their
  histories and the takes those name, the last in a registry of the whole
  process (`audio/shared.rs`), so the application's memory grows with every
  window opened and closed over its life. A member whose window is gone
  should leave the context and let go of what only it named.
  *Fixed 2026-10-25:* a window's member leaves the context with it
  (`Editing::forget_audio`): every file it holds is closed with its
  history, the takes and renders only it named are let go, and its seat
  keeps an editor with no file.

- ✅ **A loop switched to another loop near its end plays silence on a
  machine of one or two cores** *(found 2026-10-27, by CI's synth-only
  job, and reproduced with `taskset -c 0,1` on the test binary of
  `crates/clausters-app-engine/tests/playback.rs`)*.
  `two_loop_switches_close_together_at_its_end_are_not_heard` fails the
  same way on every run held to two cores: on the third pass, where the
  switch turns on another loop, 37 blocks from block 640 are silence
  where the take's frames should play (over the fed stream; over the
  reader's own thread on an earlier CI run). It passes on every run with
  more cores, and it fails the same with the readers polling every 2 ms
  as with them woken, so it is not how a reader waits. What is open is
  the diagnosis: which turn the readers miss when their threads share two
  cores with the one that plays, and whether a person hears it on a
  small machine.
  *Fixed 2026-10-28:* the readers missed no turn; the transport took the
  wrong one at the loop's end. A loop change whose readers were ready on
  the block that ended on the loop's end landed a block late: wrapping
  first with the old loop (the readers held the new loop's start), or,
  put in place on the end itself, never wrapping at all. The engine now
  lands a ready change on the wrap before it, and wraps at once a loop put
  in place on its own end (`server::engine`, `cross_edge`, `edge_due`).
  Few cores only changed which block the readers were ready on.

- ✅ **A page decodes no compressed file when it opens** *(found
  2026-10-28, decoding a compressed file at open)*. The decode is an
  offline render whose `DiskIn` streams the file, and in a page `diskIn`
  streams WAV only (`clients/web/docs/src/platform.md`), so the same open
  that decodes natively fails its render in a page. The page's render
  worker should stream a compressed file through the decoder from its top,
  as a native render does; a span of one is still not a file, but a decode
  reads it once, in order.
  **Fixed 2026-10-30.** The decoder a native `DiskIn` streams through is
  one module (`dsp::decode`), and a page's offline `DiskIn` uses it for a
  file that is not read by arithmetic: opened through the render's files,
  decoded packet by packet from its top as the ring has room, the start
  thrown away as it is decoded, and opened again to loop.

- ✅ **`edit(SoundFile.empty(...))` is refused, though both books say it
  opens silence** *(found 2026-10-28, reading the client's check while
  decoding compressed files)*. The editor's constructor refuses a file with
  no path in both clients, and the window's open takes a path or none, so
  the silence the books describe (its first save asking for a path) is
  reached only by `new_file` on an open editor. Either `edit` opens an
  empty editor and adds a new file of that shape, or the books stop saying
  it.
  **Fixed 2026-10-30, as the books say** (decided by the user): a file
  with no path opens the editor with none and adds a new file of its
  shape (`new_file`), whose first save asks where in the Save as form.

- ✅ **The second audio editor window does not play** *(found 2026-10-28
  by the user, in the native eye pass of `G44c`)*. Two windows opened from
  a script, `edit(a)` then `edit(b)` over two ten-minute stereo files: the
  first plays, the second's play does nothing. Not looked into yet; the
  lifecycle example plays in both of its windows, so what differs (the
  length, the order of opening, the files) is the first thing to find.
  *Fixed 2026-10-28.* Not the second window: whichever played second was
  silent, its transport rolling over nothing. The engine an application
  holds keyed its readings by the member's name for a file, and every
  window numbers its files from one, so the second window's take was read
  into the first's buffers and its readers played buffers nobody fed. The
  readings are keyed by member and file now (`HeldEngine` takes the
  member), and a window's close lets go of all of them, which it never
  did. `a_second_window_plays_on_the_engine_the_first_holds`.

- ✅ **A save that cannot fit its file fails late, and its message is cut**
  *(found 2026-10-28 by the user, saving a take pasted to four and a half
  hours over the `.wav` it was opened from)*. Six gigabytes of stereo float
  do not fit a WAV, and the render found it at 40 %, after writing four:
  `pcm` refuses a WAV past 4 GB only at the write that crosses it. The
  field's answer is RF64, the WAV extension made for exactly this, kept
  under its `.wav` name: a save into a `.wav` that will not fit is written
  as RF64 (decided 2026-10-28), and the length is known before the render
  starts. The status line shows the refusal cut at the window's width, so
  the reason never reached the screen; it was read in the host's log.
  *Fixed 2026-10-28.* Every WAV written keeps an RF64's room, a `JUNK`
  chunk the size of a `ds64`, and its header is rewritten at the close as
  an RF64 in place if the body outgrew 32 bits (`pcm::header`): one rule
  for the native writer and a page's writer in pieces, so no length needs
  knowing before the render and nothing is refused. A render that fails
  says the file by its name and the reason after it, once.

- ✅ **A save cannot be cancelled** *(found 2026-10-28 by the user, the
  same save)*. The progress window's Cancel stays inactive and the status
  line reads `call_off refused`. A save renders into the working folder
  and moves its render over the file only when it is done, so a save
  cancelled before the moves leaves the file as it was and the partial
  render to delete: it is as cancellable as any other render, and a long
  take is when it is needed.
  *Fixed 2026-10-28.* The call-off reached the application, which checks
  it before each step, and a save is one render: a take of hours was
  written to its end before the call was heard. An offline render now
  checks it after each run, a second of the score
  (`render_to_wav_while`), and the save's render is let go there.

- ✅ **A lane's commands skip the queue a cue holds** *(found 2026-10-29,
  putting a box of notes on the application's engine)*. The engine an
  application holds keeps the commands that follow a cue until the disk
  readers are ready, so a play waits for its first blocks; `EngineMedium`
  hands a lane's `new`, `set` and `free` straight to the engine's lanes
  instead, so a lane set after a cue and before its play is fed ahead of
  the commands queued behind that cue. Nothing heard so far depends on the
  order, since a lane is fed from the transport's position; a play that
  starts a lane and a cue in one turn should keep the order it was asked
  in.
  **Fixed 2026-10-30.** What waits for a cue is a list of commands and
  lane changes (`Waiting`), and a lane made, set or freed while a cue is
  not over goes behind it, as a command pushed then does; the cue's end
  carries them out in the order they were asked. The medium also pushed
  the commands built before a lane's change past the queue (`send`), and
  now pushes them behind it.

- ✅ **A roll's notes member outlives its window** *(found 2026-10-29,
  giving the application's roll a transport)*. Closing a roll frees its
  playback and its transport, and the notes member stays in the editing
  context with the editor it held. Its edits are in the history shared with
  the multitrack, so the member cannot simply leave: an undo walked from
  the multitrack's window over one of them must still find what it edits.
  What it should become when its window goes (released once the history
  names it no more, or kept and reused by the next roll over the same
  sequence) is still to decide.
  **Fixed 2026-10-30: kept and reused.** The seat stays, as an audio
  member's does, and the context notes it as having no window
  (`forget_roll`); the next `open_notes` over the same sequence takes that
  seat with a new editor, so a box opened and closed any number of times
  holds one member. Releasing it once the history names it no more would
  add a walk over the history for the cost of one editor.

- ✅ **The multitrack's handler marks nothing** *(found 2026-10-29, moving
  the clients onto the application)*. The clients' own multitrack editor had
  `selected`, `select` and `unselect` over the boxes, read off the picture it
  held; the application's editor answers no verb for the boxes the hand holds,
  so the handler has none in either client. The audio editor's handler marks
  through its application, which is the shape to follow.
  **Fixed 2026-10-30.** The boxes held are the window's, as the old
  editor's were, and the application never sees them: the host answers a
  multitrack window's query `selected` (the regions' ids) and its action
  `select` (`regions`) from the window's widget, before the call would
  reach the application (`Host::multitrack_marks`). Both clients'
  `MultitrackEditor` take the marking mixin again over those two calls.

- ✅ **A box of samples is not the audio editor's view of its take** *(found
  2026-10-29 by the user, zooming into a box of the multitrack on its
  application: the peaks stay at the bucket they first came at, and the
  samples are never reached)*. The audio editor draws a take through the
  signal view's slot, which notes what of the span it shows its summary
  does not answer (`frame::owed`), and the host reads that finer -- a finer
  summary, then the samples. The multitrack draws its boxes another way:
  each take's signal body into the mesh, a join span by span from the
  takes it reads (`draw_spans`), and a spectral box through texture bodies
  of its own. None of the three owes anything, so nothing finer is ever
  asked; natively it never showed while a take was a mapped server buffer,
  read whole. **The fix is the view, not the read**: a box is the audio
  editor's view of its take, placed -- its slot keyed by the take, drawn
  over the box's rectangle on the box's window onto the take (its start,
  its rate, its loop's runs), owing and landing as the audio editor's
  does. A join is a list of parts like the audio editor's take, so it is
  drawn as one too, and the spectral view, the layers and the zoom to the
  samples come with the view rather than being written again. What the
  multitrack keeps is what is its own: where a box is, its frame, its
  fades' veil, its label and its grips.
  Fixed so: each box holds the signal element the audio editor draws with,
  made from its take's body and drawn through a slot keyed per box with
  the box's window onto the take (`SlotFrame::Signal`'s `placed`); a read
  lands on the view whose key asked for it (`Element::picture`), and a
  join is a list of its parts. The summary is shared, so a second box of a
  take costs what its zoom holds.

- ✅ **A spectral box over a list reads no columns** *(found 2026-10-29
  making the boxes the audio editor's view)*. A box's view draws a
  spectrogram as the audio editor's does, and over a server buffer's
  samples it analyzes them; over a list -- every take on the application --
  the columns of what is on screen are asked and placed by widget
  (`Owed::Columns`, `place_columns`, `place_column_texture`), never by the
  key of the view that owes them. Keying that path as the reads past a
  summary were keyed is what a spectral view of the multitrack waits on,
  with the verb that offers it.
  **Fixed 2026-10-29.** A column ask carries the picture's key and is one
  per picture, its analysis read off that picture's slot
  (`Element::picture_slot`), and its columns land in that picture's slot.
  The dark column a list's spectrogram starts from is one function
  (`frame::dark_columns`) that the window's view places when its list is
  known and a box's view hands over in its own fill. The multitrack
  application offers `view_wave` and `view_spectrogram`, the audio editor's
  verbs, which set the widget's `view`; the widget makes each take's body
  again in it, carrying the list's key as a redefine does.

- ✅ **The multitrack's cursors are not the audio editor's** *(found
  2026-10-29 by the user, the same pass)*. Placing the position cursor and
  what a play, a stop, a loop and a pass that ends do with it are written
  for the audio editor (`docs/app-behavior.md`, "The cursor moves the
  music" and the rules beside it) and carried out by its playback
  (`audio_playback::space`, `Pass`, `follow`, `cue`, `locate`); the
  multitrack's playback has its own versions of each, in seconds, and they
  differ. The rules are one: the policy moves where both playbacks read it,
  and the multitrack keeps only the unit it counts in.
  **The shape, decided 2026-10-29:** a module `clausters_editing::pass`
  holds `Pass` (`Until { end: Option<u64>, back }`, an end of `None` being
  a multitrack pass that runs on), `space(looping, selection, cursor,
  end: Option<u64>)`, and `Passes`, the rolling and looping state whose
  `play`, `set_pass`, `follow`, `resume`, `pause`, `stop`, `cue` and
  `locate` answer transport `Op`s through a frame-to-sample crossing the
  playback hands in. `AudioEditorPlayback` keeps its focus and viewed-file
  rules and carries `Passes` out on the file in focus. `MultitrackPlayback`
  drops `play_pass`, `set_loop` and its own `cue`, counts in samples at its
  rate, and keeps of its own only `End` (open, the contents, a marker:
  what it hands `space` as `end`) and re-marking a pass that runs to the
  contents when a sync moves them. The multitrack editor's `selection`
  let go puts the cursor at its start and follows the loop, and a click
  moves the music while it plays, as the audio editor's gestures do; the
  coordinator (`multitrack_runs`) computes the pass with `space` from the
  range, the cursor and the playback's end.
  **Fixed 2026-10-29** in that shape, with three changes found on the way.
  `Passes` also holds what both coordinators repeated: `place` (a cursor
  placed where it plays moves the music) and the pause a play resumes,
  which the audio editor's coordinator kept as a flag of its own.
  `MultitrackPlayback::space` reads the pass itself, handed a `View` in
  samples, because the end is its own: the `End` for a pass that stops,
  and the contents for a loop over no selection, which repeats them as it
  did before whatever the end says. It re-marks a pass over no selection
  only when the pass moves.

- ✅ **The roll's passes are not the shared rules** *(found 2026-10-29,
  fixing the entry above)*. The notes editor's playback
  (`notes_playback`: `play_pass`, `set_loop`, `cue`, `stop`) still has its
  own version of the space bar's pass, the loop switch and the cursor, in
  beats. It moves onto `clausters_editing::pass` as the multitrack's did:
  its unit is the beat, crossed to the transport's samples by the tempo,
  and what it keeps of its own is the sequence's end.
  **Fixed 2026-10-29.** `NotesPlayback` keeps its calls in beats and
  carries them out through `Passes`, the beat crossed to a sample at the
  edge; `View` moved to `pass` for both playbacks. Stopped, it marks
  nothing (the next play reads the end), and a pass over no range follows
  an edit of the notes. The roll's coordinator asks the engine whether it
  rolls before acting, and a cursor placed while it plays moves the music
  (`place`).

- ✅ **A box drawn as its page opens its roll** *(found 2026-10-29 by
  the user, the eye pass over the multitrack on its application)*. With the
  boxes of notes drawn as scores, a double click on one opened the score
  editor over its sequence while the multitrack was a client's; on the
  application it opens the roll, since the application's thread holds no
  score window. Waits on "The roll and the score editor as applications of
  their own" (Future directions). *2026-11-02:* that entry is done, so this
  one waits on nothing: the application holds score windows now, and a page
  opens `over` a sequence a roll shows. What is left is the multitrack's
  double click (`MultitrackEditor`, the `open` tag, which names only a
  sequence) asking for the view the box is drawn in, and the host opening a
  score member over the box's sequence as it opens a roll.
  *Fixed 2026-11-07*: the double click's `open` says whether the box is
  drawn as its page (`MultitrackEditor::draws_pages`: the view and a bound
  engraver, the condition the drawing reads), and the host opens a score
  window over the box's sequence beside the multitrack, in its order and
  gone with its session, as it opens a roll
  (`a_box_drawn_as_its_page_opens_its_page`).

- ✅ **A page's multitrack draws no box as its page** *(found 2026-10-29,
  the same pass)*. The application hands the multitrack an engraver the
  first time its boxes are drawn as scores, made from the engraver the
  native host links; the page's application build linked none, so there
  a box stayed a roll. *Fixed 2026-10-31: the application asks the page's
  engraver worker.*

- ✅ **An untitled multitrack's Save asks for nothing** *(found 2026-10-29,
  the same work)*. A multitrack handed over whole has no file, and Save from
  its window says "save it as" on the status line rather than opening the
  Save as dialog the audio editor opens; a script's `save(path)` names one.
  **Fixed 2026-10-30.** The window's Save and Save as are the multitrack
  editor's verbs (`save`, `save_as`): Save writes where the session saves
  (it did nothing from the window before, with a file or without), and
  with no file yet it opens the Save as form, the shared mechanism's
  (`multitrack::dialogs`), whose `OK` writes there and makes it where
  later saves go. A window saves unless it was opened read-only (`saves`),
  which is no longer read off whether it asks before closing.

- ✅ **A restored session stays dirty because its counters advanced**
  *(found 2026-10-30 while checking a retained session's undo)*. Comparing a
  saved session directly with its current snapshot counts `Multitrack.version`
  as contents. An edit followed by its inverse restores the contents but
  advances that counter, so the close/save reading stays dirty. Event-id
  watermarks have the same distinction. Compare contents independently of
  these counters while preserving them in the written snapshot; test an undo
  back to the checkpoint, including a sequence edit and an inactive session.
  **Fixed 2026-10-30.** Checkpoints compare contents independently of revision
  counters and event-id watermarks. Written snapshots retain the advancing
  values; restored sequence contents and an inactive session are covered.

- ✅ **The audio editor does not ask before unsaved work goes** (found
  2026-10-31, moving the rule for a window's file into `closing`). The multitrack
  and the score editor ask through `closing::close` and `closing::change_file`;
  `AudioEditor` implements neither `asks_to_close` nor `unsaved`, so a window the
  host holds alone closes a file or itself with its edits and says nothing,
  although X25 asks the last holder to follow the common rule and the Python
  docstring says it asks. It holds several files, so the form has to say which
  it asks about, and its Save is a render. Its history already keeps the rule:
  each file has its own, and New and Open add files, letting nothing go.
  **Fixed 2026-10-31**: a window no client holds asks, by `closing`'s rule, about
  the file it shows. Close file asks about that file alone; a close brings each
  file with changes into view in turn, and Don't save and Save go on to the
  next. A Save lets what waits go only once its store is confirmed, and over
  what it wrote.

- ✅ **A sound file opens in two audio editor windows of one host**
  *(found 2026-10-30, making a session file open once in a host)*. A
  multitrack's session is open in one window of a host and an open of it
  brings that window forward; the audio editor still opens a file another
  of its windows holds, so two windows edit one file with two histories
  and the second save writes over the first. The audio editor's window
  holds several files, so the open finds the window and the file in it
  (`Editing::window_holding_file` is the multitrack's half), focuses
  that file and brings the window forward, from File > Open, a client's
  `/gui_app_open` and `--audio`. **Acceptance:** the same file opened from
  each of the three, written two ways, is one window and one history.
  *Fixed 2026-11-02.* A file read into a window (`Task::Read`, which File >
  Open and a script's `openFile` share) is looked for first in every audio
  member's list (`Editing::audio_file_holding`, over the path a save writes
  and the compressed file a take was decoded from): one this window holds
  is focused, and one another holds is a `front` effect the host turns into
  that window's focus and raise. The host's open (`/gui_app_open`,
  `--audio`) answers the holding window the same way.

- ✅ **Three handlers hold the door's code** *(found 2026-10-31)*. In both
  clients `AudioEditor`, `MultitrackEditor` and `HostedEditor` each hold the
  opening, the closing and the history a window has. `HostedEditor` is the
  general one; the other two should take it, which `X27`'s audit of one owner
  per contract would otherwise find.
  *Fixed 2026-11-02 (`X27`)*: in both clients the audio editor and the
  multitrack are `HostedEditor`s, keeping only what is theirs (how each opens,
  what the multitrack hands back when its window goes, the audio editor's undo
  of the file in focus). `MultitrackTransport` moved beside `HostedEditor`, as
  in Python, so the web modules import no cycle. Both gained what the general
  handler has: `id` is the window's number in both, the multitrack has
  `dirty`, `on_change` and the history's readings, and its `undo` and `redo`
  carry the step to its end and answer whether anything moved, as a roll's do,
  rather than only asking the window.

- ✅ **`a_loop_moved_while_it_plays_is_crossfaded` fails now and then**
  *(found 2026-10-31, running the app engine's tests beside other builds)*.
  `crates/clausters-app-engine/tests/playback.rs` paces its blocks with real
  sleeps and failed in about one run of six under load, on the commit before
  the work at hand as well; alone it passed eight of eight. What is open is
  whether the test is sensitive to its pacing or a moved loop can step when the
  reader falls behind. **Acceptance:** the assertion's output from a failure
  says which, and the test holds under a loaded machine.
  *Closed 2026-11-12 (`crates/clausters-server/PLAN.md`, `R18`)* by taking the pacing out of
  the question: the test no longer sleeps between blocks but waits for the
  take's reader to fill (`AppEngine::fill_readers`), so its run is the same
  on an idle machine and a busy one. Fifty runs in a row passed under eight
  busy threads. No failure's output was ever caught, so which of the two it
  was stays unknown; a failure from now on is not the pacing's.

- ✅ **The clients' walk of steps keeps a move no save hands it** *(found
  2026-10-31, verifying the save's moves)*. `clients/python/clausters/_steps.py`
  and `clients/web/src/steps.ts` carry a `move` step with their own rename, which
  neither rolls back nor crosses filesystems as the application's walk does
  (`clausters_core::files::move_file`). A save is the host's now, and it is the
  only maker of a move, so the branch looks unreachable. `X27`'s removal of
  obsolete coordination decides: gone from both clients, with a test that nothing
  hands one out.
  *Fixed 2026-11-02 (`X27`)*: not only the move. Every step that works on files
  (a render, a store, a move, a measure) is the application's, and the clients'
  walk, which only a roll's playback still uses, carried all four. They are gone
  from both, with the options only they read (a render's progress, calling
  renders off, the per-endpoint lock); a file step that reaches a client is
  refused, naming it, before anything of its batch goes out
  (`tests/test_steps_walk.py`, `file-render.test.ts`).

- ✅ **A multitrack's window does not say whether its session is unsaved**
  *(found by the user, 2026-10-31, in the eye pass of `X25`)*. The editor knows
  (`MultitrackEditor::unsaved_at`, what its close form asks by), and nothing on
  screen shows it, so a save or an edit since the last one is seen only by what
  the close form does. Which mark, and whether every application's window wears
  the same one, is to be decided with the entry.
  *Fixed 2026-11-02.* The mark is a `*` leading the window's title, the
  desktop convention for a document window with unsaved changes, and every
  application wears it by one rule (`closing::shown_title`): only a window
  that is the work's only holder, since a client's loses nothing by a close.
  `Editing::window_title` reads it (the multitrack and the score through a
  reading cached by version, as the comparison is not a pointer's cost), the
  host sets it on the window after every run and publishes the tree with it,
  and a window's `title` prop is now set live (`HostEffect::Retitle`: a
  desktop window's title bar, a page's canvas label). Host tests over the
  audio editor and the multitrack: an edit marks, a save clears, an undo past
  the save marks again.

- ✅ **The roll's conversion asks nothing in its window** (2026-11-09, closing
  `X35`). Entering notation over a sequence written as no notation converts
  it with the reading the sequence stores (`notation.reading`) and the
  writing its parts say; a client gives a meter, a division and a writing to
  `NotesEditor.to_notation`. `X35.7` asked for a dialog in the window that
  gives them before the conversion, through `crate::forms`, which numbers its
  widgets by the caller -- and the notes editor's window is numbered by its
  one roll today. Until then a hand in the window converts with what the
  sequence says, and sets another reading from a script. *(Fixed
  2026-11-09: the roll's window names its dialog's widgets for its caller
  to number, as the score's does; `notation_mode` over a sequence written as
  no notation opens "Write as notation", holding the meter, the shortest
  value and the writing the sequence says, and converts at OK. The shared
  forms gained a field chosen among options, the host's `choice`. A
  script's `mode` converts with the stored reading before the verb, so it
  is never left waiting on a dialog.)*

- ✅ **The roll's two modes are not yet an editor a hand can trust**
  (2026-11-09, the user's first eye pass over `X35` and `X36`, on the score
  of `examples/notation/score_editor.py` rendered, and the same score read
  back from a Standard MIDI File). The user reordered notes, joined them and
  tried the new verbs by hand; what that showed, and what is still to do:
  1. **The gestures of editing are not defined per mode.** What a press, a
     drag, a resize, a join, a delete and each mark verb do has to be
     written down for every combination: MIDI or notation, monophonic or
     polyphonic. Today each rule was added where it was needed, and the four
     cases were never set side by side. The table goes in
     `docs/app-behavior.md`, and every rule in it gets a test.
  2. **The new verbs have to be reviewed one by one**: join and unjoin,
     monophonic, slur, slur joined, tie, staccato, pedal, the voice and
     staff verbs, the writing of the polyphony, and the conversion with its
     dialog. Which notes each takes, what it refuses and why, and what the
     roll shows afterwards.
  3. **A join is drawn between notes out of order.** After notes were
     reordered by hand, the join lines cross in an X between notes of one
     line (bars 2 and 3 of the rendered score). Whether the joins name the
     wrong notes or are only drawn wrong is to be found out. *(Fixed
     2026-11-09: both. A note moved to start at or before the note it went
     on from kept the join, so the link ran backwards; the document now
     takes such a join away on any edit, and the slurs of a written line
     follow. And a join between notes that overlap was drawn from the
     first's end back to the second's start; it is drawn down at the
     second's start.)*
  4. **What a note sounds inside what it is written reads as something
     else.** The sounding length is a thin light bar at the foot of the
     written box, so a staccato looks like a selection or a stray mark
     rather than the note itself, shorter. *(Fixed 2026-11-09: what sounds
     is the note, closed by a solid edge, and the rest of the written value
     is emptied and framed dotted -- legible whatever the velocity fill.)*
  5. **The voices are not told apart**: every note of the rendered score is
     drawn in one colour, where the voice tints were meant to show them.
     *(Not a drawing fault, 2026-11-09: a second voice is tinted; the notes
     the hand made overlap in the one voice the score has, which is item 1's
     question of what a polyphonic edit in notation mode writes.)*
  6. **The roll's labels collide and one is wrong.** The axis' corner
     label (`MIDI 1.0`) is drawn over the ruler's first bar number in both
     modes, and it says `MIDI 1.0` while the roll is in notation mode.
     *(Fixed 2026-11-09: the label is kept inside its cell, smaller and then
     cut short, and the notation mode sends none.)*
  7. **The status line shows a raw sample number** (`locate 132641`) after
     a click on the ruler, which is a diagnostic and not a reading.
     *(Fixed 2026-11-09: navigation -- a locate, a selection, a view --
     leaves no line, in every window; the picture shows it.)*
  8. **A note at the bottom of the pitch window is cut** by the lane under
     the plane (the last note of the rendered score, bar 4). *(Not a fault,
     2026-11-09: the window had been zoomed and panned by hand, and a note
     on its way out of the pitch window is cut where it leaves rather than
     pushed onto a row that is not its own; the window a roll opens on holds
     every note with air around it.)*
  9. **The MIDI roll numbers bars it does not draw.** Over a sequence that
     states no meter the ruler counts `bar:beat`, while the plane draws no
     barline; the two have to agree. *(Fixed 2026-11-09: the plane bars
     every bar the ruler counts.)*
  10. **The data of the events being edited cannot be seen.** The picture
      alone does not say what a note holds: its velocity, its channel, its
      keys past pitch and time, and in notation mode almost everything a
      page writes (the written value, the spelling, the staff and voice,
      the articulations, the dynamic, ties, slurs, pedal, the tuplet). A
      roll needs an inspector of the selected events, read from the
      sequence and edited through the same intents, in both modes.
  11. **The rest of the application has to be gone over again**: more
      faults are expected beyond these, and the two windows over one
      sequence, the box's roll in the multitrack and the page beside the
      roll are the first places to look.

  *Taken into milestones 2026-11-10*: items 1 and 2 are `X37`, item 10 is
  `X38`. *Item 10 done 2026-11-10 with `X38`*: the inspector and the list
  of events. *Items 1 and 2 done 2026-11-10 with `X37`*: the table of
  `docs/app-behavior.md`, "The roll's verbs, case by case", and its review. Item 11 stays open here, and is a pass over the application once
  `X37` to `X42` have changed it. *Item 11 begun 2026-11-10*: two rolls over one sequence (MIDI and
  hertz) checked by script -- the pitch line, the lanes and their tabs, a
  ramp, a curve added from the other window -- found Ctrl+Z in the second
  doing nothing after an edit in the first (its actions were not read again;
  fixed the same day, `docs/app-behavior.md`, "What one window does moves
  what every window offers"); the page beside a roll was checked with `X42`'s
  `phrase`. *Item 11 done 2026-11-10*: the box's roll in the multitrack,
  once its window could be captured headless (`crates/clausters-gui/PLAN.md`, Found by
  use, "A capture of the multitrack example's window never answers,
  headless"). The pass found that a client could not reach the roll a double
  click opens, which the decision of 2026-09-28 had given both clients
  (`open_roll` / `openRoll`, lost when the multitrack moved into the host;
  restored, answering the roll's notes editor); that the roll kept no
  selection for a client; that Ctrl+Z in the multitrack put the box's notes
  back and left the roll drawing the edit; and that a multitrack closed left
  its roll's member open -- all fixed the same day, the last two as rules of
  `docs/app-behavior.md`. What it found in the conversion is the entry
  below, "A performance's joins are lost when the roll writes it as
  notation"; the slurs' drawing stays with `X42`.

- ✅ **Entering notation mode moves a roll's view to its last note**
  (2026-11-10, the pass over the box's roll; the host of the day before does
  the same, so it is older than that day's work). A roll opened over a
  sequence written as notation shows every note in MIDI mode; switched to
  notation mode (`mode = "notation"`, the View menu's entry), the time view
  jumps so that it starts at the last note, and every note before it is off
  the screen -- a roll over a box's sequence does the same. What moves the
  window is to be found out: the mode changes no prop of the time view, so
  it is the host's group of the roll's time axis, rebuilt or refitted when
  the window's tree changes for the notation chrome. *(Fixed 2026-11-10: the
  notation mode draws the last note at its written value, which moved where
  the notes end just past the window's right edge, and the host pages a
  roll's axis forward when its content runs past that edge -- the rule for a
  roll written into live. An application's roll now says its window is the
  reader's (`autofit` off), as the multitrack's does, and the rule is in
  `docs/app-behavior.md`.)*

- ✅ **A performance's joins are lost when the roll writes it as notation**
  (2026-11-10, the pass over the box's roll; a roll over a client's sequence
  does the same). Two notes joined in MIDI mode are one line, and entering
  notation mode converts the sequence (`EventsIntent::ToNotation`): the page
  is rendered afresh, its events have new ids, and the performance's joins
  are not read into it, so the line is two attacks after the conversion and
  `pitch_line` answers one run more. In notation mode a join is what a
  joined slur writes, so what is open is whether the reading turns a join
  into a joined slur, or the conversion carries the joins over to the events
  that are the same notes. *2026-11-10, the user*: neither is taken yet; it
  waits on Future directions, "What a sequence is, as a thing that is
  edited, and what a change of representation keeps". *Decided there the
  same day*: a change of format is a conversion of the file into another,
  so this becomes what that conversion writes of a performance's joins.
  *2026-11-11, with `X44`*: the conversion makes a copy, so the performance
  keeps its joins; what is still open is whether the page the copy is
  writes them as joined slurs.
  *Decided 2026-11-11, the user*: **a join has no place in MIDI**; it is a
  thing of a page and of an event sequence for the server. No MIDI file
  brings one (a note-on there is always an attack), so every join a
  sequence holds was made in the document: by the roll's `join`, by a
  page's slur played, or by a pattern's voice that `continues` a node. What
  that asks:
  1. **A page writes a held note in two ways, and only one is a join.** A
     tie is metric writing: one note, one event. A slur over different
     notes is articulation, the one join a performance plays. Both are as
     the document already has them.
  2. **The roll's `join` over MIDI notes holds one pitch on**: two notes of
     the same `midinote` become one note, and notes of different pitches
     are refused. It writes no `joins` in a MIDI format.
  3. **Converting to notation**: a join into the same pitch is one long
     note, and where its ties go is the page's metric writing, which is
     `crates/clausters-notate`'s and is not built; a join between
     different notes is a slur that joins.
  4. **Converting to MIDI** leaves no `joins` in the copy: a join into the
     same pitch is one note, as in 3, and a join between different notes
     is a new attack.
  5. **An event sequence keeps its joins**, as the server plays them.
  The Standard MIDI File and Clip File writers drop `joins` today; once a
  MIDI sequence holds none, that drops nothing.

  **Done 2026-11-11.** The document says it once
  (`clausters_document::events::articulation`): `held_on` makes a join into
  the same pitch one note (the inverse of a split: the length, the curves
  after the first note's, what went on from the second), `said_in_midi`
  does that and attacks every other join, and `carry_joins` writes a
  performance's joins between different notes onto the page it is written
  as, as slurs that join. A sequence in a MIDI format holds no join after
  any edit, a spec given included; `join` there holds one pitch on, whether
  the notes touch or not, and refuses two; `as_midi` and the conversion to
  notation say joins as their format does. Where the ties of the long note
  go is the reader's today, and `crates/clausters-notate`'s once it is
  built.

- ✅ **`a_page_s_marks_are_drawn_with_the_engraver_s_symbols` fails with the
  notation features on** *(found 2026-11-10 running `clausters-apps` with
  `--features notation`)*. It expects `"\u{e4a0}-"` and gets
  `"\u{e4a0} -"`; it fails the same before the id work, and passes in the
  default feature set, which does not build it.

  **Fixed 2026-11-10, the same day.** The display panel made a note's marks
  travel one by one with a space between, for the roll to lay each out, and
  the test, built only with the notation layer, kept the joined form; the
  host's own reading test did too. Both state the wire as it is now.
  `check-features.sh`, which CI runs, builds that set, so the next push
  would have caught it.

- ✅ **The host opens no file in the roll** *(the user, 2026-11-10, the eye
  pass after `X43`)*. `clausters-gui` has a flag for each application but
  the notes editor: `--session`, `--score`, `--audio`. So a MIDI file or a
  score cannot be opened in the roll from the host, and the roll's window
  has no File menu that opens one either. It should open a Standard MIDI
  File, an MEI file and the other formats the engraver reads (MusicXML,
  ABC), as `--score` does. The flag's name is the next entry's.
  *Begun 2026-11-10*: `clausters-gui --roll [file]` opens the notes editor
  over a Standard MIDI File or a MIDI 2.0 Clip File (told apart by the
  header) or a score the engraver reads, read as the sequence it plays as
  (`events::score::played`, the score editor's own reading), or over an
  empty sequence. Open: the roll's File menu, and saving -- both are the
  file's, so they come with "What a sequence is, as a thing that is
  edited, and what a change of representation keeps" (Future directions).
  *Done 2026-11-11 with `X44`*: the roll's File menu opens, saves and
  closes its file, and `--roll` saves to `--save-to`.

- ✅ **The host's flags name the applications by an old word** *(the user,
  2026-11-10, the same pass)*. `--session` opens the multitrack because it
  was once the only application with a session, which no longer holds:
  every application has one now. The user proposes `--roll` for the notes
  editor (short and clear), and a name for the multitrack's flag that says
  what it opens; and asks whether its file could have an extension of its
  own (`.tks` was suggested) rather than `.json`
  (`clausters_apps::multitrack::dialogs::SESSIONS`). To decide: the flag's
  name (`--multitrack`, `--tracks`), the extension, whether `--session`
  stays as an alias for a release, and the two books' and the host's
  `--help` with them. *Done 2026-11-10, by the user's decision*
  (`docs/decisions.md`, "A file says which application it belongs to, and
  the format is the file's"): `--multitrack` and `--roll`, `--session`
  gone with no alias; the file stays `.json` and its `metadata` names the
  application (`session::APPLICATION`, `{"application": "multitrack"}`),
  written by the crate and both clients, defaulted when absent and refused
  when it names another.

- ✅ **The roll's examples do not explain the application** *(the user,
  2026-11-10, the same pass)*. Reading the Python client's examples of the
  notes editor, it is not clear how the roll works as an application, nor
  how the panels of its graphical elements (the events panel, the display
  panel, the lane tabs) relate to the menu that shows them. The examples
  are gone over, in both clients, so that each says what it shows and how
  a hand reaches it.

  Done 2026-11-11: the notes editor's example, in both clients, opens with
  the window part by part -- the menu bar and what each menu holds, the
  toolbar and its tools, the roll with its lane and the tabs over it, the
  events and display panels and the View entries that show them, the
  status band -- then what a press does under each tool, and `pencil`
  takes a tool from the script. What had gone stale went with it: the OSC
  markers are shown and not written, the toolbar holds the tools rather
  than Split, Join and Quantize.

- ✅ **The roll's default gestures are not defined, and its tools are not
  shown** *(the user, 2026-11-10, the same pass)*. How to create a note is
  not clear, and the most common ways of doing it are not there; a click on
  a note does not select it. The table of `docs/app-behavior.md` ("The
  roll's verbs, case by case") says what each verb does in each case, but
  not what a press, a drag or a double click does by default. Wanted: the
  default action of each gesture, the common ways of entering notes, a
  click that selects, and **an icon per editing mode** -- pencil, eraser,
  selection, stretch, split -- as the score editor's tools already are
  (`clausters_apps::score::tools`), with how the field's editors lay these
  out looked up before choosing.

  Done 2026-11-11, after the field's editors: a pointer is the default
  tool, beside a pencil, an eraser, scissors and a glue or join, each an
  icon in the editor's own toolbar and a key. Five tools, chosen by the keys
  1 to 5, the toolbar's icons and the Edit menu's Tool, and by a client's
  `tool`: the pointer (a click selects the note it is on, a drag moves or
  resizes it, a drag over empty grid sweeps, a double click there makes a
  note), the pencil (a press on empty grid makes a note, the drag its
  length), the eraser, the scissors (the note clicked cut there) and the
  stretch (the selection scaled in time from its first onset). The
  modifiers stay every tool's (Ctrl adds or erases, Alt or Shift and a
  click toggle a note in the selection, Shift and a vertical drag is the
  velocity), and the join stays `J`. `docs/app-behavior.md` holds the table
  of each gesture under each tool. Found on the way: a note made by a press
  was reported at the press, and the owner's correction, its notes in time
  order, left the drag resizing another note; it is reported at the
  release now.

- ✅ **The roll's lane tabs are not seen under the lane** *(the user,
  2026-11-10, the same pass)*. The tabs that choose which curve the lane
  shows (`X39`) sit at the foot of the window, where they are missed. They
  go above the lane.

  Done 2026-11-11: the roll draws them itself, a row right over the lane
  (`lane_tabs`, the `choice` element's tabs picture reused), and a press
  asks its owner for the lane by name; the window composes no tabs widget.

- ✅ **A curve added while the display panel is up has no switch until the
  panel is shown again** *(left by `X43`, 2026-11-10)*. A lane's switch is
  one of sixteen slots composed when the panel is shown, so a curve added
  with it open gets its switch at the next showing.

  Done 2026-11-11: the notes editor keeps the lanes the panel was composed
  for, and whenever they move while it is up (a hand, a step of the
  history, a script) the window is drawn again, so the switch comes with
  the curve (`NotesEditor::panel_stale`).

- ✅ **The roll shows the velocities of one channel only in notation mode**
  *(left by `X39`, 2026-11-10)*. In MIDI mode the velocity lane shows every
  channel's notes, since the roll chooses no MIDI channel yet: what is open
  is how the roll chooses one, and the lane follows it.

  The velocity lane is not a controller: it is each note-on's own velocity,
  a bar per note, so it is per note in MIDI 1.0 as in every format. What
  it lacks is the notation mode's filter, where a voice is edited and the
  others are drawn under a veil and not taken.

  **Decided 2026-11-11: the MIDI mode edits one channel or all of them, as
  the notation mode edits one voice.**
  - The choice is the window's, like the voice edited: recorded nowhere,
    flipping it is no entry of the history, and two rolls over one
    sequence may edit different channels. *All channels* is the default
    and is today's roll.
  - It is chosen from the Edit menu's channel entries (*All channels*,
    *Channel 1* to *Channel 16*), one verb each, refused with why where it
    does not apply.
  - With one channel edited, a note of another channel is drawn under the
    same veil as another voice's and a press does not take it; a
    selection, a selection by data and the velocity lane are among that
    channel's notes; a note drawn or pasted lands on it; a channel's curve
    added (CC 1, CC 11, bend, pressure) goes on it; and the lane tabs list
    that channel's curves and those of every channel.
  - With all channels, a channel's curve keeps going on the first selected
    note's channel (the first channel with none selected), and a note drawn
    lands on channel 1.
  - Under MPE a member channel is one note's, so a channel is not chosen:
    the choice is refused with why and the roll edits the zone whole. In
    the notation mode it is refused too, since there the voice is chosen.
  - The rule lives in `clausters-editing` beside `Editing::voice` (the
    same `dimmed` list the host already reads), so the host and both
    clients gain no new drawing, only the verbs.

  Done 2026-11-11: `Editing::channel` and `Editing::takes` in
  `clausters-editing`, the `channel_all`, `channel_1` ... `channel_16`
  choices under the Edit menu's Channel, `NotesEditor.channel` in both
  clients, and the host's veil drawn in either mode.
