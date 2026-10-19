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
  tree is published before preview corrections, and cursor/selection
  corrections address only the lanes included in that tree. Old content
  gestures are still refused by the context's version.
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
  locates the transport to the incoming file's cursor.
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
  a measure drawn inside one view, on that view's vertical (peak, RMS, a
  preview). An **edit layer** is a body a hand edits, superposed on a view in a
  pane (the take, the gain envelope); one is in hand at a time. Views are
  placed as **alternatives** in one lane, one shown at a time, or **stacked**
  in lanes of their own on one axis.
- **A preview is drawn as the view it previews**: over the waveform, as the
  processed waveform; in a spectrogram, as the spectrogram of the result, in
  place of the take's. Every view of the take gets it (`pictures` in the
  editor's `preview` answer), and a window drawn again is handed it again.
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

## The time axis

- **A take bounds its axis.** A group that holds a view of stored samples (a
  navigable, not live, `signal`: `OnAxis::bounds_axis`) navigates exactly the
  take's extent: zooming out stops at the take, and nothing else on the axis
  lengthens it. A curve over the take (the gain envelope), a preview of it, a
  marker list are read *on* that axis. A shortened take shortens the axis
  even where a curve's points still reach past it.
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

- **Each open audio file keeps its own state and history.** Switching files
  restores its cursor, selection, gain curve and view settings. Undo/redo
  steps only the focused file. The independent file list is on the left and
  can be hidden; hiding it changes neither focus nor playback.
- **A paste lands at the position cursor, and a cut or a paste leaves
  nothing selected.** The cursor is the line a click puts down; a selection
  swept elsewhere does not move where a paste goes. After a cut or a paste
  the frames a selection named have moved, so the selection is cleared and
  the cursor stands where the edit landed.
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
- **Clipboard storage and undo storage have separate lifetimes.** The remote
  publication holds one immutable buffer independently of the source document.
  Pasted samples enter the destination's ordinary resource/history mechanism:
  history-only samples still spill to float WAVs on disk or OPFS under the
  existing resident limit, and restore before use. Replacing the clipboard
  neither clears history nor forces spilled samples to stay resident. A
  file-owned loaded buffer stays rooted until that file closes, including
  while another file is focused; making a private copy cannot release the
  loaded buffer early and then release it again on close.

- **A choice in the file list is navigation.** It is never refused as stale,
  and the list keeps its widget across switches, so a hand clicking down it
  lands on the file it clicked last.
- **An edit is in the history; a view state is not.** Cuts, pastes, strokes,
  curve points and Apply are history entries; which layer is shown, which
  channels are folded, the preview switch, the scroll and the zoom are not.
- **An application is an API; a menu is a caller.** Every action is a verb
  of the application in the shared crate, which both clients call by the same
  name; a menu entry, a key and a tool only call it and hold no state.
- **A menu entry and its key are one verb, with or without a pointer.** A key
  addresses the view under the pointer; a menu entry has no pointer over the
  take, so it addresses the window's last selection or its one take, and a
  pane, its take and a curve over it are one take (`gestures::keys::sole_take`
  counts the views that hold samples themselves).
- **What the owner holds, the owner answers.** A press on a folded strip asks
  the owner to unfold it; a dialog's `OK` answers an outcome (`newFile`,
  `openFile`) the client acts on, since the client is what opens windows.
- **A dialog reads typed text, not notifications.** A field's report is its
  text; its focus (`"focus" 1|0`) is a notification and never a value
  (`forms::read`).

## Checking a change

A change to an application is checked, by test where the host allows it and
by eye on the example pair where it does not, against:

1. two windows open over two objects: time, cursor and meter stay apart;
2. zooming out as far as the axis allows, on a take and on an empty file;
3. zooming and panning the vertical, with one channel, two, and enough to
   overflow, in every view the View menu offers and with each body in hand;
4. the behavior the change did not touch: play, stop, loop, the selection,
   save, undo.
