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
  bodies are members of the pane's group. **Takes linked on one axis play
  together**: the host's monitor sounds every server buffer of the group
  from the group's cursor, on one transport, and every lane draws the one
  playhead (`host::play`). Played one at a time, each lane kept a playhead
  of its own and the others stood scattered over the axis.
- **A transport belongs to an object.** Each audio editor plays on a
  transport of its own (the clients' `AudioPlayback`, one per editor), so
  playing one window moves the play cursor and the meter of that window only.
  The playback holds several files of one editor and one plays at a time.
  The engine every window of an application shares reads each window's
  takes as that window's (`HeldEngine` is keyed by member and file): keyed
  by the file alone, the second window to play read its take into the
  first's buffers and played silence.
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
  **Each editor opened on its own is a unit of editing with an order of its
  own**, however many share an application: two multitracks' sessions in
  two windows of one host, two rolls over different sequences, each
  window's undo walks its own and never the other's. A view opened from
  inside an editor is a view of that unit, not another one: a roll opened
  from a multitrack's box takes its entries into that session's order, so
  the multitrack's undo walks them, and the same sequence under another
  multitrack is another structure in another order.
  **A window that holds unsaved work says so**: where the window is the
  work's only holder (it asks before a close), its title is led by `*`
  while the work differs from its file, in every application, and loses
  the mark when a save lands or an undo walks back to what was saved
  (`closing::shown_title`, read after every run). A window a client holds
  wears no mark, since a close loses nothing there.
  **A document is open once in a host.** Opening a session file a window
  already holds brings that window forward and opens nothing: from the
  window's File > Open, from a client (`/gui_app_open`), or from the host's
  own start. A sound file is the same, held in an audio editor's list: the
  window that holds it comes forward with that file in focus, and an open
  of it in its own window focuses it. A client that asked for the file alone is answered that window
  and is its holder from then on when no client held it; one held by
  another client is refused, with the window named.
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
- **An application answers on every front.** A host with no window and no
  adapter draws nothing, and its applications still open and answer their
  verbs: an application's answers wake the loop that serves the protocol, as
  they wake a windowed one.

## Views and layers

- **Corrections address composed components.** A view's event and correction
  registry includes only widgets present in its current tree. Reserving a
  stable id for an optional component does not make it a correction target;
  removing a stacked lane stops its corrections, and showing it again reuses
  its id. Pending operations rebuild corrections from the current composed
  state when they finish; a recreated binding receives a fresh state push,
  rather than the old gesture receipt. Undo/redo keeps this rule when files or view layouts change.

- **A widget is corrected with its own props, and only its own.** When a
  history step draws every widget of a window again, each one is answered
  with what it draws: the multitrack with the multitrack, its ruler with its
  cursor, a button of the transport row or of the close form with nothing.
  A view's props set on a control are not ignored -- `weight` is a place prop
  every widget takes -- so a toolbar answered as the multitrack grew to fill
  the window.

- **A box of samples is the audio editor's view of its take.** Each box in
  the multitrack draws its take through the view the audio editor draws a
  file with, placed in the box at the box's window onto the take (its
  start, its rate, its laps), so it zooms down to the samples and reads
  past its summary as that window does. A join is a take made of parts,
  drawn the same way. The multitrack draws only what is its own around the
  picture: the box's frame, fades, label and grips, the curves, the
  selection and the cursor, over the picture.

- **Four words, each its own question.** A **view** is one picture of an
  object on a lane (the waveform, the spectrogram). A **visualization layer** is
  a measure drawn inside one view, on that view's vertical (peak, RMS). An **edit layer** is a body a hand edits, superposed on a view in a
  pane (the take, the gain envelope); one is in hand at a time. Views are
  placed as **alternatives** in one lane, one shown at a time, or **stacked**
  in lanes of their own on one axis.
- **A view changes the picture, never the verbs.** The waveform, the
  spectrogram alone and the two stacked are pictures of one take, and every
  verb, key, tool and gesture does the same in each: select all, the ends, a
  delete, a sweep, a click. A view whose picture is not a summary (the
  spectrogram's columns) is still the take for whatever asks for one.
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
- **A spectrogram of a list holds the whole take under the span shown.**
  Its columns are asked of the list as two pictures: the whole take, about a
  column to the pixel, and the span shown once the view is narrower than
  that resolves. A view draws the span where its columns reach and the whole
  take everywhere else, so a zoom out, or a pan past what is held, shows the
  whole take coarser at once and never stretches the span's first or last
  column over the rest. Where nothing held draws the view, the view alone is
  asked first and then the span that reaches half a view past either edge;
  the next is asked within a quarter of a view of what is held, as a
  waveform's detail is. The whole take, analyzed as the file's spectral summary, is read
  from the summary. After an edit the whole take is asked again first,
  at the length the take has now, and the picture before the edit is drawn
  only where its columns reach. After a change of analysis (a window, an
  overlap, a shape) the span shown is asked first, and the whole take only
  when a view shows it, unless the summary answers it; an ask of the
  analysis before is let go at its next answer, so changing a setting many
  times analyzes only the last. Going back to one of the last two analyses
  left draws its picture of the whole take at once and asks nothing for it;
  an edit keeps none of them.
- **A play is drawn from its first block.** What a run sends the engine (a
  play, a stop, a locate) is taken at the engine's next block, after the
  window has heard the run end, so a window keeps its playheads' tick for a
  settling after each answer of its application rather than read the
  transport once and sleep (`Host::settle_window`).
- **A playing window draws what moves, not the window.** A frame asked for
  by a playhead, a meter or a live reading copies the picture kept under
  the live layers and draws them over it, antialiased as a whole frame
  draws them, so the work of a playing window does not grow with its size or
  with what its views show. The picture is drawn again only when a whole
  frame changes it.
- **A redefine keeps what has not changed.** A window redefined in place (a
  panel shown or hidden beside a view) keeps the pictures of every view that
  reads the buffer it read: the view does not empty and is not asked for
  again, and the fetch the new tree makes lands in the picture on screen,
  without counting as an edit of the take.
- **A spectrogram shows only magnitudes the analysis measured.** A pixel
  inside one bin or one column shows that value, so a bin taller than a pixel
  is a block as tall as its band and a column wider than a pixel a block as
  wide as its hop; a pixel over several shows the largest. Nothing is
  interpolated between two values unless the view asks to be drawn smooth
  (`interpolate`), a look the settings offer beside the colours and the
  range of levels they span. No column is finer than the hop between
  the analysis's windows, so a zoom past it draws the same windows wider and
  analyzes nothing; and a column is drawn centred on the windows it was
  measured from, so a click lights the block it is in. Nothing lies before
  a take's start or past its end, so the first and last columns reach the
  edges rather than leaving half a window dark there.
- **A list's spectrogram is its columns, wherever the host runs.** A file is
  a list whether or not its segments are mapped (in a page they are not), so
  its texture is asked of the list as columns and never analyzed from
  samples the view read back for its trace. Reading a run back to analyze it
  is for a server buffer drawn from its summary, which has no columns to
  ask.
- **A view switch draws what the other view left.** Moving between the
  waveform and the spectrogram keeps both pictures of a take: the summary
  with the finer grid a zoom grew in it, and the columns, whole take and
  span. Switching back draws what was held at once, at the zoom the window
  has, and only a picture never resolved is asked for; an edit made while
  the other view is shown forgets the hidden summary, which is of the take
  as it was. A hidden waveform that let its samples go counts no channels,
  so the spectrogram shown instead is ruled one row a channel.
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
  Each row is ruled on its own, and **an edge two rows share is ruled once**:
  a label that cannot keep half the clear space between labels from it is
  dropped and its tick stays, so the labels of two rows keep the space the
  labels of one row keep (`ruler::label_tops`).
- **A row's height is a length, so it is logical.** A track's or an
  automation row's `height`, the gap between rows, the floor and ceiling a hand
  zooms one between and the stack's scroll are kept as the wire states them and
  resolved at the window's scale only where a picture or a hit test reads them
  (`Multitrack::stack`, the roll's automation rows), so a track is as tall
  beside its header at every density, and a hand's drag is made logical before
  it changes one.
- **A box's rows are its view's rows, with no scroll.** A box of samples lays
  its channels out by the audio editor's own layout (`Lanes::placed`): the
  view's folds and strips, an open row never thinner than one control, and no
  scroll, since the stack the box sits on is what scrolls. A take whose
  channels do not fit at that floor is drawn overlaid on one row, every
  channel still in it, and pulling the track's edge gives the rows back; a
  spectrogram, which cannot be overlaid, shares the box evenly instead.
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
- **What one window does moves what every window offers.** Two windows over
  one structure walk one history, so after an edit in one, the other's menu
  and its keys offer the undo that takes it back: every window's actions are
  read again after a run, and the ones that moved are said (found 2026-11-10,
  when Ctrl+Z in a second roll did nothing after an edit in the first).
- **An editor's time window is the reader's.** A roll, like a multitrack,
  does not re-frame or page its view when what it holds changes, since a
  change there is mostly the reader's own edit (found 2026-11-10, when
  writing a take as notation drew the last note at its written value and the
  view jumped to it, the notes before it off the screen).
- **A step redraws every window it moved, whichever window took it.** An
  undo or a redo walks one history, so every window over what it changed is
  corrected with it, whether the step came from a key, an entry or a
  client's call (found 2026-11-10, when Ctrl+Z in the multitrack put a box's
  notes back and left its roll drawing the edit).
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
- **A pass that ends on its own is taken in at once.** The transport stops at
  the end of the take or the selection and goes back to the position cursor,
  and the editor hears it on its next turn, with nothing else asked: it is no
  longer playing, the window is told, and an edit refused while playing is
  available again (`Editing::stopped`).
- **The cursor moves the music.** A click placing the position cursor, or a
  selection let go (whose start the cursor goes to), moves the play cursor
  there too: a stopped transport stands on it, and a file playing goes on
  from it, its pass ending as the loop switch says from there. What sounded
  is cut, and what starts on the place it lands on sounds, as a stop, a
  rewind and a play would have it. The two
  cursors move together everywhere but in play, where the position cursor
  stays where the pass began: a selection made whole at once (select all) is
  one let go, and the window is told the position cursor moved.
- **A control and the automation of its parameter are one parameter.** A track
  automation moves a parameter of the track's graph, and a control of that
  parameter (a header's level) shows it: it reads the curve at the play cursor,
  playing or stopped. A hand on the control takes over for as long as it holds,
  and is heard; it writes nothing, neither the curve nor the track's own value,
  and letting go gives the parameter back to the curve. Any parameter is asked
  by its port (`automated`), never by a list of the ones known today.
- **A page over a sequence plays that sequence.** Its tempo and its curves are
  what the roll plays and what the page's play cursor is timed on, so the sound
  and the line agree in every window over it; a page of its own plays its score,
  at the engraver's tempo.
- **A rewind is where the next pass starts.** It lets go of one item picked on a
  page, which would start the pass there; a stretch is what a pass plays, as a
  time range is, so it stays and the rewind goes to its start. Home and End on
  a page are the same placing. A press selecting on a page places the cursor
  there as a click on a time axis does, so a pass in progress moves to it.
- **A click lets go of a time range.** Anywhere on the view, on a box or a note
  too: a range is swept on purpose, and any place clicked after it ends it. A
  drag of something else keeps it, and so does a press on a header's control.
- **The multitrack and the roll play by the audio editor's rules.** What
  the space bar plays, how a pass ends, what the loop switch does to a pass
  in progress, what a placed cursor and a selection let go do to the music,
  and what a play after a pause resumes are one policy
  (`clausters_editing::pass`), which every playback carries out. Each keeps
  only its unit (the multitrack the transport's samples, the roll its
  beats, crossed by the tempo) and its end: a pass with no selection stops
  where the end switch says, or runs on, and a loop with no selection
  repeats the contents (every note, in a roll).
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
  channels are folded, the scroll and the zoom are not, nor the
  spectrogram's settings, which are the window's whichever file is in
  focus.
- **What is played is not an edit; what the hand does is, once per
  gesture.** A value moved by live input (a controller, an automation
  playing, a bus, another client) makes no history entry, in any
  application. A hand's gesture is one entry, from press to release; a
  recording pass is one entry of the application that records, and each
  later edit of what it recorded is its own. Where a value has several
  writers, which is a live node parameter in a panel and never a document
  with its one owner, an undo writes the value the entry stored, whatever
  the parameter holds by then (`docs/decisions.md`, "A live parameter's
  undo writes the value it had").
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
- **A tool takes no focus.** A button of a toolbar (a verb's, or any `flat`
  one) leaves the keys where they were, so the space bar after Rewind plays
  rather than pressing Rewind again. Tab reaches the controls of a form, whose
  Space presses them.
  A combobox keeps the keys off a press the same way: after a hand picks from
  its list the space bar plays. Tab reaches it, and then Space opens its list.
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
- **An ellipsis says the verb asks before it acts.** A menu entry whose verb
  opens a form that has to be filled before anything happens (New file...,
  Open file..., Save as...) ends in `...`; one that acts at once does not,
  whatever it shows -- a switch that shows a panel (Spectrogram settings), a
  view chosen, a sheet to read. It is the convention of the platforms' menu
  guidelines, the same in every application; a switch or a choice never ends
  in one (`verbs` tests), and the audio editor's forms are the verbs that do.
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
- **A verb is one of three classes, in every application.** An edit of the
  document is an entry of its history. The window's own state (selection,
  cursor, zoom) is none. **Changing which document the window holds (New, Open,
  Close file) is none either**: a document has a history of its own, the one let
  go of takes its order with it, and nothing walks back across the change. A
  window that replaces its document asks first by the rule a close asks by, when
  it is the work's only holder and has changes (`clausters_apps::closing`), and
  hands the document back to the client that opened it otherwise. An audio
  window adds files rather than replacing one, so its New and Open let nothing
  go; it asks about the file it shows, Close file about that one, and a close
  brings each file with changes into view in turn.
- **An application that opens files opens without one, and outlives its
  last.** Where an application holds files or documents of its own -- opens,
  adds and closes them -- it opens with none, its window keeping the menu bar,
  the file list and the dialogs and saying where the contents would be that
  nothing is open; closing the last file leaves it in that state, and only
  closing the window closes it. With nothing open, the window's own verbs (New,
  Open, Close, the keys, the file list) answer and every verb that acts on a
  file is refused with `no file is open`. It does not apply to an editor over
  a structure a client handed it, which has nothing to open: the notes and
  points editors, a panel. The audio editor, the multitrack and the score
  editor keep it.
- **A window drawn again shows the dialog that was up**, its fields holding
  what was typed and its chooser where it was, with nothing selected; a
  window closed and opened again has none up (`forms::Dialogs`).
- **A closed window leaves the page, as it leaves the screen.** The canvas a
  page's client made for a window -- in the element it was given, or of its
  own -- is removed when the window closes, by a script's `close()` or the
  host's `/gui_closed`; a canvas the page handed over is the page's and stays.
  Kept on the page, it went on showing the last frame of a window that no
  longer existed.
- **An application's window in a page is a window, as on the screen.** The
  canvas the host appends for it opens at the size the window declares and
  is laid out at the page's density, read by the host since nobody else
  reports it (`host::web::canvas`); at a fixed 480 x 420 laid out at a scale
  of 1, the editor was crushed and a press at a fractional density landed
  off its mark.
- **An edit shows in one step.** A window whose picture is of another
  length than the take its axis spans (a list just named, its summary on
  its way) keeps its last frame on screen, up to `PICTURE_HOLD_MS`, rather
  than draw the old picture on the new axis (`replies::PictureHold`).
- **What moves is drawn over the picture, never through it.** A frame drawn
  because a playhead or a meter moved copies the kept picture and draws the
  live layers in their boxes (`frame::underlay`); each pass ends with its
  scissor over the whole target, since WebGL resolves and presents by a blit
  the scissor clips, and a pass that ended on a box left that box
  transparent in a page.
- **A correction addresses only what the window drew.** A prop set after a
  turn -- the play cursor, the level meter -- names a widget of the window as
  it was last drawn; a widget the window was asked for and did not draw
  (anything of an empty window) is not corrected until a redraw carries it.
- **The chrome does not wait for a file's work.** A window over a file is
  drawn whole as it opens -- menu bar, toolbar, file list and level meter --
  while the file is still being read and summarized; what that work yields
  reaches the window as corrections. The meter reads no bus until the
  playback says where it writes, and the answer that says so points it
  there, so nothing about the window's layout moves when the file is ready.
  And a window drawn after the
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


- **A failed repair remains reachable.** After resolving an I/O failure, an audio editor's `retry_pending()` / `retryPending()` retries its pending repairs without repeating the edits or measurements. Until repair succeeds, the affected objects remain locked and their sources retained. Concurrent retries cannot dispatch the same repair twice; a successful retry does not repeat the original gesture receipt or change history. A multitrack's session work has nothing to restore, so its refusals are repaired at once and leave nothing to retry.
- **A session summary belongs to the session that asked for it.** Source measurements publish together after their operation succeeds. A duplicate completion cannot advance a later batch, and a response from a replaced or closed session cannot attach peaks to its successor, even when the source ids are the same.
- **A session save confirms its captured contents.** The multitrack's destination and checkpoint change only after storage confirms the prepared document. Later edits remain unsaved; conversation and navigation alone do not make it dirty. A save-and-close waits for confirmation, and a failed save keeps the window and the previous destination and checkpoint.
- **A close form goes down with its answer, in every application.** Its Save runs with the window usable: what the form was up for waits for the confirmed write, stays undone if the write fails, and stays undone if the contents moved meanwhile, the status line saying why. Only a save over the file a take reads refuses edits while it runs, since the file is moved under the list that reads it.
- **A save leaves a whole file or the one it found.** A save over a file a take reads moves that file aside and the render into its place; when the second does not land, or the steps are called off between them, the first is moved back. A move across filesystems (the working folder is rarely where the file is) copies beside the destination, names it when whole, and only then removes the source. A session or an MEI file is written beside its name and renamed.
- **A refused operation corrects its window.** What the abort read back reaches the gesture's answer, beside why, so a picture a gesture moved ahead returns to what the take holds.
- **A view of a session goes with the session.** A roll opened from a multitrack's box edits what that session holds, in that session's order. When the window lets the session go (File > New session, Open session..., Close session, or its own close) the rolls opened from it close and the order goes with them, so nothing walks or draws a session no window holds; the multitrack goes on. A roll that goes with its multitrack's window is closed as its own close closes it: its application forgets it and whoever asked for it hears it go (found 2026-11-10, when a closed multitrack left its roll's member open).
- **What a window does not do is refused with why.** An application publishes a reading for every verb of the window's table, so a key, an entry or a script's `/gui_verb` that asks for something the structure cannot do says so on the status line. A points editor is the plain case: a curve plays nothing here, has no grid, and its points have no clipboard, so a Ctrl+V over one explains itself and turns nothing into points. Such a verb is read as `absent`: refused with why, and left off the window's key sheet, which lists what the window does. A page reads the verbs of a grid and a mix the same way. A curve's holder may play it: with `on_play` (`onPlay`) set, the window's play is available and calls it, from the space bar or a widget that names the verb. A points editor needs only what it does: no engine, no device, no playback. An editor publishes its readings from the moment its window opens.
- **A change of a window's session asks as a close asks.** File > New session, Open session... and Close session put another session in the window's place, or none; where the window is the work's one holder and its session has unsaved work, the close form asks first. Its Save changes the session only once the file is written, and not at all if the session moved while it was: what waited for a save is done only over what the save wrote, and a close waits the same way. A window a client holds hands the session back to it instead, and asks nothing. The menu's file verbs name no data a gesture was drawn against, so they are never refused as overtaken, and the window is told the version it draws after the change. A session opened read-only is so alone: the one the window makes or opens in its place saves where its own file is.
- **Content confirmation preserves current presentation.** Showing or hiding an envelope while an edit or history restore waits for I/O remains in effect after confirmation and redraw. Curve visibility is matched by identity against the current file, not restored from the prepared content snapshot.
- **An applied curve is put away.** Apply renders the gain curve into the take and leaves it at unity, which shows nothing: once the render lands the curve is hidden and the hand is back on the take. An undo brings the points back and leaves the curve hidden, since which body is shown is the view's. What a confirmation tells the window is the whole pane as the editor holds it (its label, the body in hand, whether the curve is shown), never part of it.

- **A roll edits monophonically or not, and the switch is the window's.**
  Monophonic is an option of whoever edits, like snap: the sequence holds
  none of it and flipping it records nothing. On, a gesture never leaves two
  notes of one line (a staff and voice, else a channel) overlapping: a note
  lengthened or moved over the next shortens it from its start (its onset
  moves to where the edited note ends, its end stays), removes it where it
  covers it whole, and shortens the note it starts inside to end there, all
  in the gesture's one entry, so one undo gives every note back. A length
  that reaches the next note does not join them. Off, notes overlap as the
  hand leaves them, exactly as before the switch existed.
- **A join on a roll is a link, and `J` over notes of one pitch that touch
  still makes them one note.** Over any other selection of a line, `J` joins
  each note into the next without a new attack and Shift+J takes the joins
  away; each is one entry of the sequence's history, and a join is drawn as a
  line from where a note ends to where the next starts.
- **MIDI has no join.** A note-on there is always an attack, so a sequence
  written in a MIDI format holds no join: `J` over its notes holds one pitch
  on, making the notes of one pitch one note whether they touch or not, and
  refuses notes of different pitches. Only a page (a slur that joins) and a
  sequence for the server hold joins. A conversion says them in the format
  it writes: to MIDI, a join into the same pitch is one note and any other an
  attack; to notation, a join into the same pitch is one note, tied where
  the metre cuts it, and one between different notes a slur that joins.
- **A roll's mode is its sequence's format's, and nothing the roll does
  changes the format.** A sequence is written as a page (notation) or as a
  performance (MIDI 1.0, MPE, MIDI 2.0, or none stated, for the server), and
  the roll edits a page in notation mode and every other in MIDI mode, so
  two rolls over one sequence are always in one mode. Going to another
  format is a **conversion into a copy**, opened in a roll of its own and
  untitled, the sequence the roll edits left as it was, since each format
  holds what the others do not and a switch in place would lose it (a
  performance's joins, a page's written values). The File menu's *Convert
  to notation...* asks how first: the dialog holds the meter, the shortest
  value and the writing the sequence's reading and parts say, nothing is
  made until OK, and Cancel makes nothing; a script's `to_notation` says
  how itself. *Convert to MIDI 1.0*, *to MPE* and *to MIDI 2.0* leave a
  page's notation out and keep what sounds, refused with why where a curve
  has no spelling in the spec. A conversion to the format the sequence has
  is refused. What the notation mode shows (the system, a part, a staff) and
  the voice it edits are the window's.
- **A roll the host holds alone has a file; a client's roll and a box's do
  not.** A roll opened by `clausters-gui --roll` or over a conversion's
  copy is its sequence's only holder, so its File menu has New, Open...,
  Save, Save as... and Close file, the title wears the unsaved mark, and a
  close or a change of file with changes the file does not hold asks first,
  as every application's window does (`crate::file`, the score editor's
  rule written once). Save writes the file of the format: MEI for a page, a
  Standard MIDI File for MIDI 1.0, MPE and a sequence for the server, a Clip
  File for MIDI 2.0. A roll over a client's sequence or a multitrack box's
  edits what its holder keeps, and refuses the file verbs with why.
- **In notation mode a gesture writes what a page writes.** An onset lands on
  the reading's grid and a length on a written value; a note drawn is in the
  voice edited; a velocity moved is written as the dynamic it is nearest; and
  what each note sounds is derived from the page in the same entry, so it is
  heard at once. Where a part writes its polyphony in voices, two notes of one
  voice overlap only as a chord, and a gesture that leaves any other overlap
  is refused with why. A note of a voice other than the one edited is drawn
  faint and is not taken by a press or a marquee.
- **The roll's verbs, case by case.** A verb means one thing in each of the
  roll's four cases: MIDI or notation mode, and monophonic or polyphonic
  editing. The option only decides what a gesture that places a note does to
  the notes of its line it covers, so it has its own column; a row with
  nothing there is the same either way. A key in the table is the default
  the key table gives, which a user changes. Each row names its tests.

  | Verb or gesture | MIDI mode | Notation mode | Monophonic | Refused | Tests |
  |---|---|---|---|---|---|
  | Draw: Ctrl on empty grid, then the drag sets the length | a note as long as drawn, its `dur` and `sustain` | onset on the reading's grid, length a written value, in the voice edited, its pitch spelled by the key, its sound derived | the notes it covers in its line are shortened or taken away | in notation, an overlap in a voice of a part that writes voices, but as a chord | `ctrl_adds_and_removes_a_note`, `a_new_note_and_a_new_velocity`, `a_note_in_notation_mode_lands_on_the_grid_and_a_written_value`, `notes_of_one_voice_overlap_only_as_a_chord_unless_written_with_ties`, `a_monophonic_gesture_shortens_what_it_covers_in_its_line` |
  | Move: drag a body, a selected one moving the selection | its time and its pitch, the pitch family kept coherent | onset on the grid, the pitch spelled again | as Draw | as Draw | `a_moved_note_writes_its_key_with_coherence`, `a_block_moves_together_and_an_unselected_note_alone`, `a_note_in_notation_mode_is_written_and_its_sound_derived` |
  | Resize: drag an edge, a selected one's resizing the selection | what it sounds | the written value | as Draw | as Draw | `a_length_is_written_as_its_value`, `an_edge_of_a_selected_note_resizes_the_selection` |
  | Level: Shift and a vertical drag | the velocity | the dynamic nearest it | | | `a_new_note_and_a_new_velocity`, `a_note_in_notation_mode_lands_on_the_grid_and_a_written_value` |
  | Select: a click, Alt+click, a marquee, Ctrl+A | every note | the voice edited; another voice is drawn faint and not taken | | | `a_note_of_another_voice_is_drawn_faint_and_not_taken` |
  | Select by pitch, channel, voice or staff | every note sharing it with a selected one | among the voice edited | | nothing selected | `a_selection_by_data_marks_every_note_that_shares_it`, `a_selection_by_pitch_is_the_window_s_and_records_nothing` |
  | Delete, cut (Delete, Ctrl+X); erase (Ctrl on a note, and the drag on) | the notes go, and the joins naming them | the same, and the slurs of their lines follow | | | `removing_a_note_leaves_its_neighbours_theirs`, `a_ctrl_drag_from_a_note_erases_what_it_crosses`, `a_note_moved_before_the_one_it_went_on_from_is_joined_no_more` |
  | Paste (Ctrl+V) | at the cursor, as new notes | as new notes in the voice edited | as Draw | as Draw | `the_block_keys_quantize_delete_and_travel_through_the_clipboard` |
  | Split (E) | each selected note across the cursor cut in two, with its keys and curves on both halves | the same, cut on its written length, both values written again | | the cursor inside no selected note | `a_split_note_keeps_its_keys_and_its_curves_on_both_halves`, `a_written_note_is_split_on_its_value_and_both_halves_are_written_again`, `a_split_is_the_documents_in_either_mode`, `a_roll_that_speaks_joins_asks_its_owner_to_split_the_selection` |
  | Join (J) | notes of one pitch that touch become one; any other selection of a line goes on, each note into the next, with no new attack; in a MIDI format, notes of one pitch become one | the same, the line's slurs written again from its joins | | no line holds two of them one after the other; in a MIDI format, notes of different pitches | `joins_are_drawn_and_edited_by_the_notes_named`, `a_roll_that_speaks_joins_asks_its_owner_to_join_the_selection`, `a_join_from_the_roll_in_midi_holds_one_pitch_on` |
  | Unjoin (Shift+J) | the joins among the selected notes go | the same, and the slurs that joined them | | nothing selected | `a_join_from_the_roll_is_an_edit_of_the_sequence` |
  | Transpose: Up, Down; Ctrl+Up, Ctrl+Down | a semitone; an octave | a step of the key; an octave | | no note to move | `a_transposition_in_midi_mode_moves_the_selection_or_every_note`, `a_step_in_notation_mode_is_a_step_of_the_key` |
  | Quantize (Q) | the selected onsets, or every one, onto the grid | | | no grid, or nothing off it | `a_verb_that_acts_on_nothing_says_why_rather_than_nothing` |
  | Slur, slur joined or attacked, tie, staccato, pedal (S, Shift+S, T, `.`, P) | refused: a mark is written on a page | written over the selection and heard at once; a tie makes two touching notes of one pitch one event | a slur is drawn joined, else attacked | in MIDI mode; nothing selected | `the_pages_marks_are_written_over_the_selection`, `a_page_s_mark_waits_for_the_notation_mode` |
  | Tenuto, accent, marcato, breath (`-`, `>`, `^`, `,`) | refused: a mark is written on a page | on where a selected note lacks it, off where all have it, heard at once (a breath lets the note go before the next attack); drawn over the box, the breath at its end, with the engraver's symbols where the host holds one | | in MIDI mode; nothing selected | `the_pages_marks_are_written_over_the_selection`, `a_page_s_marks_are_drawn_with_the_engraver_s_symbols` |
  | A dynamic, pp to ff, or none (the Notation menu's Dynamic) | refused | written on each selected note, or taken away; the level derived from it; drawn under the box | | in MIDI mode; nothing selected | `the_pages_marks_are_written_over_the_selection` |
  | Display settings (View) | the panel beside the roll: how the slurs, a note's marks, the pitch line, the notes' curves and the lanes offered are drawn, the window's, recorded nowhere | the same; the slurs and the marks are drawn here alone | | a value none of the settings takes | `the_display_panel_sets_how_the_roll_draws_and_records_nothing` |
  | Color by voice, staff or part | refused: switch to notation first | the tint each note is drawn with, the window's, recorded nowhere | | | `a_notes_tint_is_its_voice_its_staff_or_its_part` |
  | The lanes: the tabs under the roll | the velocities, a bar per note, or a curve of the sequence, one at a time with its value ruler; the window's, recorded nowhere | the same, the velocities of the voice edited | | | `the_lanes_are_chosen_by_their_tabs_and_a_curve_added_is_one_entry` |
  | Velocity bars: a bar dragged, or a drag across the lane | that note's velocity, or the selection's together when it is selected; across the lane a ramp over the notes it crosses; one entry | the same, over the voice edited | | | `a_velocity_bar_is_dragged_and_a_ramp_drawn_across_the_lane` |
  | Add a curve: a channel's (CC 1, CC 11, bend, pressure) or the selected notes' (bend, pressure, timbre); remove the curve shown | one entry, one point at the parameter's resting value | the same | | what the format cannot say, with why (under MPE a channel's bend); a note that has one of the kind; no curve shown | `under_mpe_a_notes_timbre_is_added_and_a_channel_bend_refused`, `a_curve_is_removed_by_its_name_and_not_added_twice` |
  | Show the system, a part, a staff; the staff above or below; voices 1 to 4 | refused: the sequence is not a page | what is shown and the voice edited, the window's, recorded nowhere | | past the top or the bottom staff | `the_notation_mode_shows_a_staff_and_edits_a_voice_of_it` |
  | Write the polyphony in voices or with ties | refused | every part written so, one entry; nothing heard changes | | | `a_note_in_notation_mode_is_written_and_its_sound_derived` |
  | The panel: the inspector and the list of events (View menu) | the selected notes' time, length, pitch, velocity and channel; every event, its time as bar and beat | the same, a row with its written value, staff and voice; a value typed lands as the mode writes it | as Draw, for a value typed | no note selected, for a value typed | `the_panel_follows_the_roll_and_edits_through_it`, `the_inspector_shows_what_the_selection_shares_and_says_what_varies`, `a_value_typed_is_reported_on_every_selected_note`, `a_roll_that_tells_its_marks_says_each_change_once` |
  | Convert to notation..., to MIDI 1.0, to MPE, to MIDI 2.0 (the File menu) | a copy in the format, in a roll of its own; the sequence unchanged | the same | | the format the sequence has; a meter that is not one; a curve the spec cannot say | `converting_a_take_to_notation_opens_a_copy_and_leaves_the_take`, `converting_a_page_to_midi_opens_a_copy_and_leaves_the_page`, `entering_notation_in_a_window_with_the_dialog_asks_how_first`, `a_meter_that_is_not_one_is_refused_in_the_dialog` |
- **A freed list leaves no pending picture request.** Closing or rebinding
  the last view of a file list retires its summaries and reads. A new window's
  clip waveforms can load immediately; requests to lists that no longer exist
  cannot occupy its fetch queue.

## Checking a change

A change to an application is checked, by test where the host allows it and
by eye on the example pair where it does not, against:

1. two windows open over two objects: time, cursor and meter stay apart;
2. zooming out as far as the axis allows, on a take and on an empty file;
3. zooming and panning the vertical, with one channel, two, and enough to
   overflow, in every view the View menu offers and with each body in hand;
4. the behavior the change did not touch: play, stop, loop, the selection,
   save, undo.
