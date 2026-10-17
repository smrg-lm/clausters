# Application behavior rules

The applications over the document (the audio editor, the multitrack editor,
the notes and score editors) share one host and one set of mechanisms: the
navigation groups, the transports, the layer stack, the dialogs. A feature
added to one of them is checked against these rules before it lands and
tested against them after, because a rule an addition breaks is a regression
even when the addition itself works. Each rule says what holds, and where the
code that keeps it lives.

## Objects, windows and what they share

- **A window shows one object.** An audio editor's window is one file (a
  `SoundFile` or a buffer); a multitrack editor's is one multitrack; a score
  editor's is one symbolic score. Two windows over two objects share nothing
  that moves: not a time window, not a cursor, not a meter.
- **Views of one object share what the object has.** Two views of the same
  file (a waveform and a spectrogram, a roll and a score over one sequence)
  share its time axis, its play cursor and its history, because what they
  show is the same thing. Sharing is by **navigation group**: a widget joins
  its own group unless it declares a `link` (`host::timeline`), and a pane's
  bodies are members of the pane's group.
- **A transport belongs to an object.** Each audio editor plays on a
  transport of its own (the clients' `AudioPlayback`, one per editor), so
  playing one window moves the play cursor and the meter of that window only.
  The playback can hold several files of one editor, all but the one played
  last paused, which is how one editor will hold several files.

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
- **Where the rows overflow, the plain wheel over the ruler scrolls them** and
  Ctrl zooms the amplitude, as a roll's keyboard gutter scrolls its octaves.
  Where they fit, the wheel zooms as it always did.

## State, history and verbs

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
