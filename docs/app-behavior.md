# Application behavior rules

The applications over the document (the audio editor, the multitrack editor,
the notes and score editors) share one host and one set of mechanisms: the
navigation groups, the transports, the layer stack, the dialogs. A feature
added to one of them is checked against these rules before it lands and
tested against them after, because a rule an addition breaks is a regression
even when the addition itself works. Each rule says what holds, and where the
code that keeps it lives.

## Objects, windows and what they share

- **A window has its own editing context.** An audio editor's window holds
  one or several files (`SoundFile`s or buffers) and shows the focused file;
  a multitrack editor's is one multitrack; a score editor's is one symbolic
  score. Two windows over two objects share nothing
  that moves: not a time window, not a cursor, not a meter.
- **A window owns its component bindings.** Switching files changes the
  observed contents and context version while keeping component ids stable.
  Two windows have separate component ids even over one object. A replacement
  tree is published before its corrections, and cursor/selection
  corrections address only the lanes included in that tree. Old content
  gestures are still refused by the context's version.
  Broadcast events are classified for operations only by the window or
  component that owns their target. Other editors neither reserve resources
  nor acknowledge or reject that gesture, including Save from a key or menu.
- **Views of one object share what the object has.** Two views of the same
  file (a waveform and a spectrogram, a roll and a score over one sequence)
  share its time axis, its play cursor and its history, because what they
  show is the same thing. Sharing is by **navigation group**: a widget joins
  its own group unless it declares a `link` (`host::timeline`), and a pane's
  bodies are members of the pane's group.
- **A transport belongs to an object.** Each audio editor plays on a
  transport of its own (the clients' `AudioPlayback`, one per editor), so
  playing one window moves the play cursor and the meter of that window only.
  The playback holds several files of one editor and one plays at a time.
  **A pass goes on while the window shows another file**: switching files
  does not touch it, the play cursor is not drawn over a file that is not
  the one playing, and the file list marks the one that plays. Stop stops
  it from any file; play is over the file shown. Stopped, switching files
  locates the transport to the incoming file's cursor. Rebuilding the view
  (layers, file-list visibility, channel folding or edited contents) keeps
  this association: a hidden play cursor stays hidden on the inactive file.
- **A file is not edited while it plays.** While a file of an editor plays,
  every verb that changes what it holds is refused with the reason ("the file
  is playing: stop it to edit") and reads refused in the menus and keys: cut,
  paste, mix, delete, a stroke, the gain envelope and its Apply, reverse, and
  an undo or a redo that would change that file. A copy, a selection, the
  loop, the views and a save are not edits and go on. Another file of the
  same editor, which is not the one playing, is edited as usual.
- **The meter belongs to the viewed file.** Its channel count always matches
  that file. It shows live levels only while that file plays; stopped or while
  another file plays, all its channels show zero. Returning to the playing
  file restores live metering without restarting playback. Mono, stereo and
  multichannel switches resize the existing meter in both directions
  (`AudioEditorPlayback::meters` and the audio application's `playing`
  corrections). Changing a meter's bus clears the previous source's levels,
  held peaks and clip lamps immediately.
- **Playback connects only negotiated hardware outputs.** A multichannel
  audio file keeps its channels separate for editing and metering. Its first
  channels feed the available device outputs; remaining channels go to a
  private unused bus, with no automatic downmix and no writes into other
  consumers' buses.
- **Undo/redo resolves its editing context explicitly.** Menus and shortcuts
  target the active document's history; an empty history disables the action
  and never falls back to another document or the application context. Text
  inputs resolve their own editing actions before document actions. Sharing
  an undo order across objects requires an explicit application composition;
  neither opening order nor a shared service instance establishes that policy.
  Every file an audio editor holds -- the one it was opened over included --
  has an order of its own; only an editor opened with `shared_history` /
  `sharedHistory` puts its first file in the context's shared order. So with
  edits in A and an empty B in focus, undo is disabled and A is untouched,
  whichever was opened first.
  A scripted history step with a malformed or unknown member is refused
  before selecting an order or preparing I/O. It cannot use the previously
  selected order or truncate an oversized id to address another member.
  Omitting the member, or passing null, still explicitly targets the context.
  A script can explicitly address such a shared context. Within that context,
  a step restores the objects recorded in the entry, including objects out of
  focus, and updates their playback graphs without changing the active
  document, starting playback or rebinding its views.
- **Services retain sources independently.** Contents, all undo orders,
  playback, clipboard and pending work are separate resource roots. Dropping
  one root never frees a source held by another. A source on a lost connection
  cannot be freed on its replacement, even when buffer numbers repeat.
- **A play cursor is drawn in its take's frames.** A transport counts the
  engine's samples, and every view of a take at another rate, the pane
  that draws its play cursor included, reads it by the take's rate over the
  engine's.

- **Successful I/O still requires application confirmation.** A sample edit changes contents/history only after its batches succeed and the original revision is accepted. A rejected commit retains its pending sources until repair settles. A save confirms its destination and saved content revision after I/O; undo to that revision is clean, and edits made during save remain dirty. A completion for a closed/recreated view cannot update its widgets or recreate playback.
- **Collection does not return allocator ids before acknowledgement.** Unreferenced owned sources enter retiring storage in the resource service. Failed cleanup keeps the reservation and its error for retry; an exact release ticket is consumed once. Closing a file follows this same cleanup path. Invalidating a server retires its operations, repairs, locks and cleanup tickets without sending frees to its replacement.

## Views and layers

- **Corrections address composed components.** A view's event and correction
  registry includes only widgets present in its current tree. Reserving a
  stable id for an optional component does not make it a correction target;
  removing a stacked lane stops its corrections, and showing it again reuses
  its id. Pending operations rebuild corrections from the current composed
  state when they finish; a recreated binding receives a fresh state push,
  rather than the old gesture receipt. Undo/redo keeps this rule when files or view layouts change.

- **Four words, each its own question.** A **view** is one picture of an
  object on a lane (the waveform, the spectrogram). A **visualization layer** is
  a measure drawn inside one view, on that view's vertical (peak, RMS). An **edit layer** is a body a hand edits, superposed on a view in a
  pane (the take, the gain envelope); one is in hand at a time. Views are
  placed as **alternatives** in one lane, one shown at a time, or **stacked**
  in lanes of their own on one axis.
- **The defaults, not the limits.** The audio editor's menu superposes the
  edit layers (the envelope over the take) and the visualization layers of
  one quantity (RMS inside the peaks), and places the waveform and the
  spectrogram as alternatives (`view_wave`, `view_spectrogram`) or stacked
  (`view_both`) (`clausters_apps::audio::editor::Views`). The mechanism keeps
  every combination: a script still superposes a spectrogram and the traces
  in one view through `editor.layers`, as long as one quantity claims its
  vertical. A change between one lane and two
  redefines the window (`restructure`), and the host keeps the zoom, the
  selection and the layer in hand.

- **An edit replaces a picture whole, never from the left.** A take edited
  under the same buffer (a paste, a cut, an undo: the owner's `reload`) goes
  on being drawn as it was until the summary of the new take has come whole,
  and is then replaced at once, extent included. A view asks for a take's
  summary at the level its width resolves, once it has been drawn, so what an
  edit costs follows the window's width and not the take's length; zoomed
  past that level it asks a finer grid over what it shows, handed over whole
  too. A view that is not drawn (a window closed, a canvas off screen) asks
  for nothing.

## The time axis

- **A take bounds its axis.** A group that holds a view of stored samples (a
  navigable, not live, `signal`: `OnAxis::bounds_axis`) navigates exactly the
  take's extent: zooming out stops at the take, and nothing else on the axis
  lengthens it. A curve over the take (the gain envelope), a
  marker list are read *on* that axis. A shortened take shortens the axis
  even where a curve's points still reach past it.
- **An edit does not zoom.** A take's first picture fits its view; a take
  that changes length under a view already drawn (a paste, a cut, an undo)
  keeps the window's length, as a drag does, and only re-clamps it
  (`Host::set_take_total`). A list's length is registered as the list is set,
  before its picture is read, so a view set in the same answer stands, and it
  is the list's length that the axis keeps: the picture of the take before an
  edit, drawn until the new one has come, never registers its own.
- **A paste lands at its end and is shown whole.** The position cursor, and
  the play cursor with it, goes to the end of what a paste put in; where that
  runs past what the view shows, the view opens out until the pasted span is
  inside, and a paste inside the view moves nothing of it. A paste asked past
  the end goes in at the end, and the view is opened from there.
- **A history step keeps the cursor on the take.** An undo or a redo that
  shortens the take under the position cursor or a selection puts them at its
  new end, and the window is told so with the restored list.
- **A pan or a zoom draws from what is held, never from nothing.** A view
  zoomed past its take's summary holds a finer summary, or the samples, over
  what it shows and half a view past either edge, and asks for the next span
  once it comes within a quarter of a view of what it holds; a grid on its way
  that still covers the view is let land rather than asked again. Zoomed in
  past the grid it holds, it draws from that grid until a finer one comes,
  never from the coarser summary under it. Samples are drawn one by one only
  where the run held covers the view; beyond a run there are no zeros to draw.
  A take of hours is addressed by frames past what an `int32` counts, as longs
  on the wire.
- **A time is never narrowed.** A group holds its times as NTP ticks
  (`clausters_core::time::Time`, an `i64` of 2^-32 s) at the rate its
  members speak; on the wire a whole frame crosses as an `int64` and a
  window's edge as a double, never as a float or an `int32`; and a time
  becomes a pixel only as a difference from the view's left edge, taken
  before anything is narrowed to the `f32` the GPU draws with. A float
  holds a whole frame only up to 2^24 of them, under six minutes at 48 kHz.
- **An authoring surface has headroom.** A group with no take and a surface
  written into (a multitrack's lanes, a roll, a pane of curves alone)
  navigates past its content (`Host::NAV_HEADROOM`) and, empty, over a grid
  (`EMPTY_BEATS` of its tempo), so there is somewhere to draw or drop into.
  The two never mix: a take's presence removes the headroom.
- **A live view slides.** A view of a bus (a retained scope, a recording)
  bounds nothing and refits only while nobody has navigated it.

## The vertical axis

- **The vertical is the view's, never the group's.** A take's amplitude
  window (`y_start`/`y_len`) is its own: a linked view shares time, not
  height. The wheel over a take's ruler zooms the amplitude about its centre,
  and a drag there pans it.
- **The vertical is in the units, and at the zoom, of the one layer that
  claims it.** Two quantities over one body are never ruled together: in a
  signal's stack the layer on the axis claims it (a spectrogram in hertz, the
  traces in their value), and in a pane the body in hand rules the gutter
  (`host::layers::child_rules_y`), the take's amplitude or the envelope's gain.
  Each quantity keeps its own window (`EditorProps::measure_y`): a frequency
  axis opens no further than Nyquist, an amplitude past full scale, and
  switching the view back finds each where it was left.
- **Every row-relative question asks one layout.** A stacked view's rows are
  `host::bands::Lanes`: the even split by default, folded channels as strips a
  control tall, a floor of three controls an open row is never thinned past,
  and a scroll when the floor does not fit. Drawing, the ruler, the
  spectrogram's viewports, the cursor readout, a value-restricted sweep, a held
  sample and a zoom's anchor all read it, so none of them can disagree.
- **One object, one colour.** The channels of a take stacked in rows are drawn
  in one colour, since colours of their own read as separate tracks; only
  channels overlaid on one row are told apart by colour.
- **Where the rows overflow, the plain wheel over the ruler scrolls them** and
  Ctrl zooms the amplitude, as a roll's keyboard gutter scrolls its octaves.
  Where they fit, the wheel zooms as it always did.

## State, history and verbs

- **Renders live on disk and go with their session.** An editor's working
  folder is under the user's cache (`clausters_core::workspace`), never the
  temporary directory, which is memory on many machines; it is removed when
  the window closes or the process ends, and by the next editor to open where
  the process was killed. A render that fails deletes what it had written
  there, so a disk filled by a long reverse is given back at once.
- **Work over a window can be called off without closing it.** The progress
  panel's Cancel button, or Escape while the panel is up, calls the owner's
  work off (`"call_off"`): a render stops before its next piece, the edit it
  was for does not happen, and its pieces are deleted. Closing the window
  calls it off the same way.
- **Each open audio file keeps its own state and history.** Switching files
  restores its cursor, selection, gain curve and view settings. Undo/redo
  steps only the focused file. The independent file list is on the left and
  can be hidden; hiding it changes neither focus nor playback.
- **A paste lands at the position cursor, and a cut or a paste leaves
  nothing selected.** The cursor is the line a click puts down, or the start
  of a selection once it is let go. After a cut or a paste the frames a
  selection named have moved, so the selection is cleared and the cursor
  stands where the edit landed.
- **The ends are before the first frame and past the last.** Going to the
  start puts the cursor before the first frame, which plays too; going to the
  end puts it past the last, where the take has played out: a play from there
  stops at once unless it loops, and a paste there goes after all of it.
- **The cursor moves the music.** A click placing the position cursor, or a
  selection let go (whose start the cursor goes to), moves the play cursor
  there too: a stopped transport stands on it, and a file playing goes on
  from it, its pass ending as the loop switch says from there. The two
  cursors move together everywhere but in play, where the position cursor
  stays where the pass began: a selection made whole at once (select all) is
  one let go, and the window is told the position cursor moved.
- **A copy of a take is made on the server.** Copy and cut in the audio
  editor copy the span into a server buffer the clipboard names, whatever
  its length; a paste copies it again on the server. No sample crosses the
  wire. Editors sharing the same live client connection can paste it across
  files and windows, including after closing the origin. Other connections,
  expired publications and legacy buffer numbers without an exchange refuse.
- **Copy publishes only when ready; cut removes only after publication.** A
  failed copy keeps both the previous clipboard and the file unchanged. A cut
  whose target changed during the copy refuses removal. Paste and Paste mixed
  create destination-owned samples; later clipboard replacement cannot change
  them or their undo history. An in-flight paste retains its snapshot until
  its sample-producing steps finish. Remote clipboard replacement is reconciled
  at the next clipboard action; no periodic clipboard polling is introduced.
- **Typed contents and system text have distinct authority.** A typed copy
  updates the host's system-text mirror. Unchanged mirrored text preserves the
  typed document and blobs. New external text replaces them as text, even when
  it looks like JSON; it cannot establish a buffer lease. Samples must match
  their declared shape exactly; malformed or truncated payloads refuse.
- **A block keeps its rate.** A block pasted from a file at another rate is
  a take at its own rate, read at the ratio; a mix needs one rate and is
  refused across two.
- **Clipboard storage and undo storage have separate lifetimes.** A copy names
  the spans of files it is of, and a file it names -- a render included -- is
  kept while the clipboard holds it, whatever the history let go of and after
  the editor it came from closed. Pasting the copy keeps it; replacing it
  neither clears history nor deletes a file a list still reads. A file the
  editor opened is never deleted: only renders are.

- **A file changed on disk is said when the window comes back.** The
  window asks to be told when it is activated; each open file read from the
  disk is then looked at, and one that changed since the editor read it -- or
  is gone -- is said on the status bar, once. The editor's own save over the
  file is no change. The take reads what is there now: nothing was kept of
  what it read before, and the notice is what lets a person know it.

- **A choice in the file list is navigation.** It is never refused as stale,
  and the list keeps its widget across switches, so a hand clicking down it
  lands on the file it clicked last.
- **An edit is in the history; a view state is not.** Cuts, pastes, strokes,
  curve points and Apply are history entries; which layer is shown, which
  channels are folded, the scroll and the zoom are not.
- **An application is an API; a menu is a caller.** Every action is a verb
  of the application in the shared crate, which both clients call by the same
  name; a menu entry, a key and a tool only call it and hold no state.
- **An action's state is read, never kept.** The owner publishes each
  action's reading -- what it addresses, whether it can be taken and why not,
  a switch's or a choice's state, pending work -- as the window's `actions`,
  recomputed from the file, the selection, the history and the pending
  operations whenever a turn or an operation moves them
  (`verbs::ActionState`, `AudioEditor::actions`). A menu entry, a tool and a
  key read that one object: an action that cannot be taken is a disabled
  entry, a tool that does not light, and a key or a `/gui_verb` that does
  nothing. A text field holding the focus answers its own editing first
  (`host::actions`): its copy and paste are the field's, and an undo while
  typing addresses the field, which keeps no history, never the document.
  A paste is offered only where the clipboard fits the destination: as many
  channels, the same rate for a mix, a server copy on the same connection --
  read against the platform's clipboard as it is at the paste, so text
  another program copied since is what does not fit. An action refused from
  its key or a script's verb is said on the window's status line with its
  reason; it never does nothing in silence.
  An empty selection is the one objection the host reads for itself: a sweep
  or a pick is drawn there before the owner hears of it, so a Ctrl+C right
  after one is never refused on a reading that predates it -- the host reads
  what the addressed view holds (a take's span, a roll's notes, a
  multitrack's clips) and the owner refuses it when nothing is selected on
  arrival.
- **A window that describes its actions offers nothing else of the host's
  table.** A verb the owner gives no reading (the audio editor has no
  `split`, `join`, `quantize` or `snap`) does nothing from its key and is not
  on the key sheet, so the sheet lists what the window does.
- **A dialog is modal for verbs as for the pointer.** While one is up, a key,
  a menu entry or a tool reaches only the element focused inside it -- a
  field's undo is the field's -- and never the object behind it.
- **A text field keeps its own history.** Ctrl+Z undoes its last change (a
  run of typing is one step), Ctrl+Shift+Z or Ctrl+Y redoes it, and the
  document behind it is not reached while it holds the focus.
- **A menu entry and its key are one verb, with or without a pointer.** A key
  addresses the view under the pointer; a menu entry has no pointer over the
  take, so it addresses the window's last selection or its one take, and a
  pane, its take and a curve over it are one take (`gestures::keys::sole_take`
  counts the views that hold samples themselves). A selection put on the
  pane -- by a script's `select`, or a sweep over its ruler -- addresses the
  take on that axis, since the pane has no samples to copy. A take whose owner
  copies is a take before any of its samples reach the host, as in a page
  whose window is out of view and fetches nothing, so a script's `cut()`,
  `copy()`, `delete()` and `paste()` and the Edit menu act on it the same
  (`WidgetKind::holds_samples`).
- **A step tells the window where it left the context once it has landed.**
  Undo or redo from one window of a shared order may restore a file another
  window holds; the asking window learns the new version only after that walk
  has committed, so its next gesture is never refused as stale. Check it with
  `editors/audio_lifecycle`: an undo from `first` of `second`'s cut, then a copy
  in `first`.
- **What the owner holds, the owner answers.** A press on a folded strip asks
  the owner to unfold it; a dialog's `OK` answers an outcome (`newFile`,
  `openFile`) the client acts on, since the client is what opens windows.
- **A dialog reads typed text, not notifications.** A field's report is its
  text; its focus (`"focus" 1|0`) is a notification and never a value
  (`forms::read`).
- **A file is chosen in the host's chooser, never in an application's
  own.** A form field that names files is the host's `files` element
  (`forms::Spec::chooser`): it lists the disk in a window and the page's
  storage in a tab, filtered to what the application opens, and takes the
  keyboard when its form is shown. A double click or Enter on a file is `OK`.
- **A chooser opens again where the hand left it, with nothing chosen.**
  Cancelled or accepted, the form keeps the directory last gone into and
  forgets the selection; going into a directory forgets the files chosen in
  the one left. A listing that comes back after the chooser moved on --
  climbed while a slow directory was read, opened again, drawn again --
  replaces nothing, even when it names the same directory.
- **An application that opens files opens without one, and outlives its
  last.** Where an application holds files or documents of its own -- opens,
  adds and closes them -- it opens with none, its window keeping the menu bar,
  the file list and the dialogs and saying where the contents would be that
  nothing is open; closing the last file leaves it in that state, and only
  closing the window closes it. With nothing open, the window's own verbs (New,
  Open, Close, the keys, the file list) answer and every verb that acts on a
  file is refused with `no file is open`. It does not apply to an editor over
  a structure a client handed it, which has nothing to open: the notes and
  points editors, a panel. The audio editor keeps it; the others that open
  files are pending (`crates/clausters-apps/PLAN.md`, "Found by use").
- **A window drawn again shows the dialog that was up**, its fields holding
  what was typed and its chooser where it was, with nothing selected; a
  window closed and opened again has none up (`forms::Dialogs`).
- **A closed window leaves the page, as it leaves the screen.** The canvas a
  page's client made for a window -- in the element it was given, or of its
  own -- is removed when the window closes, by a script's `close()` or the
  host's `/gui_closed`; a canvas the page handed over is the page's and stays.
  Kept on the page, it went on showing the last frame of a window that no
  longer existed.
- **A correction addresses only what the window drew.** A prop set after a
  turn -- the play cursor, the level meter -- names a widget of the window as
  it was last drawn; a widget the window was asked for and did not draw (the
  meter before a playback says where it writes, anything of an empty window)
  is not corrected until a redraw carries it. And a window drawn after the
  focused file changed reads the meter of the file it now shows: the playback
  is told which file is viewed before the window asks where its level is.
- **A settled window does nothing on a period.** Stopped, an editor's meter
  reads no bus and its playhead stands on a transport the server says is
  stopped, so its window is neither ticked nor drawn until something changes
  (`/gui_activity` answers an empty `demand`). Playing ticks only what reads
  the playing file; hiding a window stops its drawing and queries but not the
  playback; closing it releases what it ticked for (`docs/architecture.md`,
  "Activity: what an open application keeps doing").
- **A playhead that moves is drawn every refresh of the display**, not on
  the frame tick: a cursor moved by a timer the display does not follow
  steps unevenly. Only the live layers are drawn, and only while the window
  can be seen and its transport rolls (`docs/gui-protocol.md`, "What a
  window ticks for"). Only a frame that drew asks for the next: one that
  found nothing to draw waits for no refresh, and asking again after it spins
  a core (`Host::asks_next_frame`). Check it with `editors/audio_lifecycle`:
  loop `a.wav`, then `copy_first()`, and `activity()` stays near the tick.
  It is drawn **where the transport is at that refresh**: the engine
  publishes its counters once per audio block, which is the device's period
  (47 times a second at 1024 frames), so a rolling counter is carried between
  blocks at the engine's rate and never steps back (`clocks::Glide`). Read
  only as published, two frames in five on a 60 Hz display found the cursor
  unmoved, and each such frame asked for no next one.


- **A failed repair remains reachable.** After resolving an I/O failure, an audio editor's `retry_pending()` / `retryPending()` retries its pending repairs without repeating the edits. Until repair succeeds, the affected objects remain locked and their sources retained. Concurrent retries cannot dispatch the same repair twice; a successful retry does not repeat the original gesture receipt or change history.
- **Content confirmation preserves current presentation.** Showing or hiding an envelope while an edit or history restore waits for I/O remains in effect after confirmation and redraw. Curve visibility is matched by identity against the current file, not restored from the prepared content snapshot.

## Checking a change

A change to an application is checked, by test where the host allows it and
by eye on the example pair where it does not, against:

1. two windows open over two objects: time, cursor and meter stay apart;
2. zooming out as far as the axis allows, on a take and on an empty file;
3. zooming and panning the vertical, with one channel, two, and enough to
   overflow, in every view the View menu offers and with each body in hand;
4. the behavior the change did not touch: play, stop, loop, the selection,
   save, undo.
