
## Reading a document back: what is content, and what the emitter made up

The reader (`clausters_core::notation::read`) is the other return path, and
naming it that way is the point: the interpreter turns a model into sound, this
turns a *document* into a model. Until it existed, every verb in the algebra was
unavailable on exactly the scores a user is most likely to open — a phrase typed
in ABC, an imported MusicXML, a hand-written MEI — because those are documents
and nothing else.

**There is one input format, not four.** The engraver normalizes whatever it
loaded to MEI (`Engraver::mei`), so a caller reads that and every importer
verovio has is covered by parsing one encoding. No parser per format, and no
decision about which of them is canonical.

**An element with no id was invented by the emitter.** This is the rule the
round trip turned on, and it was not obvious until the first read-back grew the
score. A voice is written into whole measures, so the emitter completes the last
bar with a rest and pads a voice shorter than its neighbour until the staves are
in step — neither of which is in the model. Reading them back as content means a
score **gains a bar of silence for having been saved**, every time. They are
known by having no `xml:id`, since every element written from an item carries the
item's own; and the rule is applied only to a document this layer wrote, because
a document from anywhere else has ids of another shape and every rest in it was
written by somebody.

**Split pieces rejoin.** A note that overruns a barline is written as two tied
elements sharing one model id, so the reader folds a run of same-id elements back
into the one item they came from. Without that, a note across a barline becomes
two notes on every trip, and the model drifts from the score by being saved.

**Verovio respells a tie and reading only our own spelling would lose it.**
We write `@tie="i"`/`"t"`; a document that has been through the engraver comes
back with those attributes gone and a `<tie startid endid/>` hanging off the
measure instead — the same shape as a slur. Both spellings are read. This is the
sort of difference that is invisible until a score is saved twice, and it was
found by round-tripping through the engraver rather than by reading the schema.

**What the model grew, and where the line is.** The reader must not lose what it
cannot hold, so the model grew where a document is musical: a **header** (title,
subtitle, composer, lyricist — the fields a score editor offers, named rather
than a map, so each has one home in MEI and one spelling in every client), the
**right barline** of a measure and a **break** before one (both sparse on the
grid, beside the irregular bars), and **beams**.

A beam is a `Spanner`, which looks like a category error and is not: it has two
ends and joins items exactly as a slur does. Putting beams and breaks in the
model at all is the same line `Marks::stem` already drew — **what the engraver
decides when nobody decided is the engraver's; what a writer chose is the
model's**. A beam that crosses a beat groups the rhythm a particular way and is a
statement about the music; a break in a published score is a statement about the
page. That verovio can compute a default for both does not make a chosen one
recomputable. What stays outside, and is therefore not loss: automatic beaming,
the line breaks that merely fit, the staff geometry.

**A repeat barline is notation and not performance.** It is drawn, and it is not
what makes a passage play twice — repetition is written out by the `repeat`
operation, which is why the interpreter has nothing to expand. The two live at
different layers on purpose, and a document that arrives with repeat barlines
keeps them as the marks they are.
