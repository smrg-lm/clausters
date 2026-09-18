# `form`: a frozen layer, and why it takes no work

The `form` namespace is a small client-side algebra for placing elements in
time: an `Element` (a thin adornment over an event, a timeline, a buffer or a
pattern — it adds an onset and a duration and nothing else), five primitives,
and an `Aggregate` that places elements by an offset, recursively, in one of two
kinds — **concrete** (the members relate in time) or **logical** (they relate by
processing, a chain wired through buses).

It is **relegated**, and this page is its whole documentation beside the
[API reference](api/Namespace.form.md). It takes no new work, nothing is
designed around it, and it has **no view**: do not extend it, do not project a
picture out of it, and do not read its shape as the model a multitrack is built
on. What is fundamental is the data — samples, notes, events, curves — and the
model an application edits is the **document**
([The document](composition.md)).

## What happened, and why it is structural

`form` had a multitrack editor projected out of it. `FormEditor` was removed on
2026-09-06 in both clients, with its examples, its tests and the tutorial pages
built on it.

The reason is not a defect count. A multitrack's own state — which track a thing
is on, its order there, its placement, its identity — is **authored, durable and
undoable**, and a projection has nowhere to keep it. It ended up in the widget
tree, which is drawn, and drawing frees: a clip changing track was a re-parent of
a UI object, a clip appearing was a change of shape on a wire, and a track's zoom
died because the only way to say *a clip arrived* was to rebuild the track.

What replaces it is a session in `crates/clausters-document` — source, region,
lane, track, automation, the vocabulary the field settled long ago — with three
classic applications over it: an audio editor, a multitrack editor and a score
editor, each programmable from the GUI host and driven identically from every
client. `crates/clausters-document/PLAN.md` carries that design.

## What it is still good for

Placing objects in time in a page, with no picture and no editor: the
structures are self-contained and the namespace still works exactly as it did.
It is the same layer the Python client has, in this language — the two write the
same document and flatten to the same timeline, and a parity suite holds them to
it. If you want a multitrack an application can open, edit and save, that is the
document, not this.
