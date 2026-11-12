# clausters-notate: from a performance to a page

*Opened 2026-11-08 by the user, beside the roll's notation mode
(`crates/clausters-apps/PLAN.md`, `X35`; `docs/history/plans/document.md`,
"The notation mode: a sequence that is written, not only played"). The `Qx`
labels are this file's coordinates and appear nowhere else. The crate does
not exist yet: `Q1` makes it, and until then this directory holds only its
plan.*

## What the module is

A sequence as MIDI holds it is a performance: notes at the times they
were played, in seconds or in beats nobody counted. A sequence in notation
mode is written: its notes have written values, a meter bars them, each
belongs to a staff and a voice, and each pitch is spelled. Going from the
first to the second is **analysis**, and it is several problems, each with a
literature of its own:

1. **Beats**: where the beats fall in a performance that speeds up and slows
   down, and so the tempo map that turns its seconds into beats.
2. **Quantization**: which written value each played onset and length is.
3. **Meter**: where the bars fall, which beats are strong, the time
   signatures and their changes, an anacrusis.
4. **Voices**: which notes form one line, which staff (and which hand) each
   goes on.
5. **Spelling**: which name each pitch is written with, and in which key.

The module does each of these as **an operation of its own**, so a script or
the GUI can run one step, look at what it gave, correct it and run the next,
or run the whole chain at once. Each step writes its result into the sequence
as the notation keys the document already has (`value`, `staff`, `voice`,
`pitches`, the section's `grid` and `key`), so the last step is the reading
that exists today (`clausters_document::events::transcription::read`), which
becomes the module's **assembler**.

## Why a module of its own

- **Its complexity is its own**: each step is a research problem, compared
  against the others by measurement, with methods that will be replaced as
  better ones are written. It needs its own plan, its own evaluation and its
  own documentation, which is what the user asked for.
- **What is built is adapted**: today's reading already holds a first method
  for several steps (a grid snap, a voice-finding rule, a key found by counting
  scale notes, a spelling on the line of fifths). They move here as each
  step's baseline, unchanged, and the reading calls them.
- **The core stays small**: a step may later want an optional dependency (a
  trained model), which belongs behind a feature of this crate and not of
  `clausters-core`.

## Decisions taken before opening the track

- **The steps are separate operations, and the reading assembles.** No step
  writes a page; each writes notation keys on the events, and
  `transcription::read` reads them exactly once they are all there (the
  exactness `O37` proves).
- **Today's heuristics are the baselines.** `Q1` extracts them unchanged and
  proves it on the existing tests; together they are the trivial pipeline the
  roll's conversion (`X35.7`) uses before any new method lands.
- **Every new method must beat its baseline** on the evaluation of `Q3`
  before it becomes a step's default.
- **The logic is in Rust, once.** Both clients and the GUI call the same
  steps.

## Open decisions

- ⬜ **The crate's name and label.** Proposed: `clausters-notate`, with the
  `Q` prefix (unused by any plan); the name avoids
  `events::transcription`, which stays the assembler.
- ⬜ **Where the crate sits.** Proposed: pure Rust over plain types
  (`Ratio`, `TempoMap`, its own note lists) depending only on
  `clausters-core`; the document crate's `notation` feature depends on it
  through an adapter (`events/notate.rs`); wasm reaches it through
  `clausters-core-web`, the C ABI through a JSON door per step, the way the
  core's `articulation` is bound.
  *The neighbours it sits beside, since 2026-11-12 (`crates/clausters-server/PLAN.md`, `R16`):*
  the notation model is the crate `clausters-notation` (it was a module of
  the core), and the engraving library's binding is `clausters-verovio` (it
  was `clausters-notation`). `clausters-notate` reads a performance and
  yields what `clausters-notation` models, so by the levels of
  `docs/architecture.md` it is at level 1 if it depends on the core alone and
  at level 2 if it builds the model's types.
- ⬜ **A sixth step: reading articulation.** Staccato from short played
  lengths, slurs from legato overlaps (through the core's `articulation`),
  pedal spanners from a CC64 lane. Proposed as a step of its own.
- ⬜ **Trained models.** Whether to ship any, and if so only behind a feature
  that is off by default and off in wasm.
- ⬜ **A performance with no tempo map.** Today a MIDI file read with none is
  one beat a second; proposed: the beats step rewrites `at` into the beats it
  found and writes the tempo map it found.

## The pipeline

| Step | From | To | Baseline (today's, extracted) |
|---|---|---|---|
| Performance | a sequence as played (through its tempo map), or a MIDI file | notes `{id, on, off, pitch, velocity, channel}` in seconds, and the pedal's down and up times | |
| Beats | the performance | beat times, and a tempo map | the tempo map the sequence has |
| Quantization | the performance and the beats, a division and the tuplets admitted | each note's written onset and value, exact | the nearest point of the grid, with a tuplet only where it fits clearly better |
| Meter | the quantized notes (and the beats) | the beat unit, the meters by measure, an anacrusis: the section's `grid` | given |
| Voices | the quantized notes and the parts (their staves and writing) | each note's `staff` and `voice` | the first free voice, up to the part's voices |
| Spelling | pitches and onsets | the key, and each note's `pitches` | the key whose scale holds the most note time; the spelling nearest the tonic on the line of fifths |
| Assembly | the events with every key written | a page | `transcription::read` |

## Design reference: what the field does, and where we differ

A survey per step, with the work each method comes from. It is the draft of
the module's documentation (`Q0`), which cites the same works.

**Beats.** A performance has no clock: a beat tracker proposes a tempo and a
phase and follows them as the player moves. Dixon's tracker induces tempo
hypotheses from the intervals between salient onsets at several metrical
levels and keeps a set of competing agents, each following one hypothesis,
choosing the best-supported one; it assumes no tempo, meter or style. Ellis
takes a global tempo first and finds by dynamic programming the beat times
that best balance falling on strong onsets against keeping that tempo.
Cemgil and Kappen treat tempo tracking and quantization together as the
filtering of a probabilistic model of expressive timing, and find that
sequential Monte Carlo (particle filters) does better than the MCMC methods
they compare. *Where we differ*: the input is notes, not audio, so onset
detection is not ours; the strength of an onset is its velocity, its length
and how many notes start with it.

**Quantization.** Which written values a played rhythm is. Desain and Honing
compare the traditional grid methods with a connectionist network of
interacting cells that settles a performed rhythm towards a metrical one.
Cemgil, Desain and Kappen derive the quantizer from a probabilistic model of
performance by Bayes' rule, and show that several known quantizers are
choices of its prior and likelihood. Nakamura, Yoshii and Sagayama model a
piano performance as several voices, each an HMM, of which one moves at a
time (a merged-output HMM), which handles polyrhythm and voices that are only
loosely together. *Where we differ*: today's baseline is the grid; a method
replaces it only by beating it on the evaluation.

**Meter.** Where the bars and the strong beats are. Longuet-Higgins and Lee
give an incremental rule set that proposes a first beat and revises it as the
rhythm goes on. Temperley and Sleator model meter (and harmony) with
preference rules, scored together, and Temperley's book develops them for
meter, phrase, counterpoint, harmony and key. *Where we differ*: a meter that
changes along the piece is the page's grid by measure, which the document
already holds.

**Voices.** Which notes form one line. Chew and Wu cut the music into contigs,
stretches where the number of sounding notes is constant, and connect the
fragments of adjacent contigs by the smallest pitch distance. McLeod and
Steedman separate a MIDI performance into monophonic voices with an HMM, live
as well as offline. *Where we differ*: a part written in two voices per staff
or with ties (`O35`) is what the voices are separated into, and the hands of a
piano are its two staves.

**Spelling.** Which name a pitch is written with. Chew and Chen spell in real
time from the Spiral Array: a centre of effect of the recent notes stands for
the key, and each pitch takes the name nearest to it. Meredith's ps13 takes,
for each note, the name nearest the tonics the context implies on the line of
fifths, and in its evaluation spells over 99% of a corpus of nearly two
hundred thousand notes correctly. *Where we differ*: today's spelling is
ps13's first stage with one key, which is why it is the baseline.

**The whole chain.** Nakamura, Benetos, Yoshii and Dixon join multi-pitch
detection to rhythm quantization to go from audio to a score; Shibata,
Nakamura and Yoshii add statistics over the whole piece (self-similarity,
how the hands are voiced) and report the error rates of a full transcription
of piano music. Liu, Kong, Morfi and Benetos predict with a neural network
which notes fall on beats, then the quantized times, the key and time
signatures and the hands, from performance MIDI. *Where we differ*: audio is
outside the module; its input is notes.

**Evaluation.** Foscarin, McLeod, Rigaux, Jacquemard and Sakai's ASAP aligns
scores with performances of piano music, with beats, downbeats, time and key
signatures annotated. McLeod and Steedman's MV2H scores a transcription
jointly on multi-pitch, voices, meter, note values and harmony. Davies,
Degara and Plumbley describe the measures used for beat tracking.

### Bibliography

- Cemgil, A. T., Desain, P., Kappen, B. (2000). Rhythm quantization for
  transcription. *Computer Music Journal* 24(2), 60-76.
- Cemgil, A. T., Kappen, B. (2003). Monte Carlo methods for tempo tracking and
  rhythm quantization. *Journal of Artificial Intelligence Research* 18,
  45-81. doi:10.1613/jair.1121
- Chew, E., Chen, Y.-C. (2005). Real-time pitch spelling using the Spiral
  Array. *Computer Music Journal* 29(2), 61-76. doi:10.1162/0148926054094378
- Chew, E., Wu, X. (2004). Separating voices in polyphonic music: a contig
  mapping approach. *Computer Music Modeling and Retrieval* (CMMR 2004),
  Springer LNCS (the volume is given as 2771 and as 3310 by different
  sources; to be checked on the publisher's page).
- Davies, M. E. P., Degara, N., Plumbley, M. D. (2009). *Evaluation methods
  for musical audio beat tracking algorithms*. Technical report
  C4DM-TR-09-06, Queen Mary University of London.
- Desain, P., Honing, H. (1989). The quantization of musical time: a
  connectionist approach. *Computer Music Journal* 13(3), 56-66.
- Dixon, S. (2001). Automatic extraction of tempo and beat from expressive
  performances. *Journal of New Music Research* 30(1), 39-58.
  doi:10.1076/jnmr.30.1.39.7119
- Ellis, D. P. W. (2007). Beat tracking by dynamic programming. *Journal of
  New Music Research* 36(1), 51-60. doi:10.1080/09298210701653344
- Foscarin, F., McLeod, A., Rigaux, P., Jacquemard, F., Sakai, M. (2020).
  ASAP: a dataset of aligned scores and performances for piano
  transcription. *ISMIR 2020*, 534-541.
- Liu, L., Kong, Q., Morfi, V., Benetos, E. (2022). Performance MIDI-to-score
  conversion by neural beat tracking. *ISMIR 2022*.
- Longuet-Higgins, H. C., Lee, C. S. (1982). The perception of musical
  rhythms. *Perception* 11, 115-128.
- McLeod, A., Steedman, M. (2016). HMM-based voice separation of MIDI
  performance. *Journal of New Music Research* 45(1), 17-26.
- McLeod, A., Steedman, M. (2018). Evaluating automatic polyphonic music
  transcription. *ISMIR 2018*, 42-49.
- Meredith, D. (2006). The ps13 pitch spelling algorithm. *Journal of New
  Music Research* 35(2), 121-159. doi:10.1080/09298210600834961
- Nakamura, E., Benetos, E., Yoshii, K., Dixon, S. (2018). Towards complete
  polyphonic music transcription: integrating multi-pitch detection and
  rhythm quantization. *ICASSP 2018*, 101-105.
- Nakamura, E., Yoshii, K., Sagayama, S. (2017). Rhythm transcription of
  polyphonic piano music based on merged-output HMM for multiple voices.
  *IEEE/ACM Transactions on Audio, Speech, and Language Processing* 25(4),
  794-806.
- Shibata, K., Nakamura, E., Yoshii, K. (2021). Non-local musical statistics
  as guides for audio-to-score piano transcription. *Information Sciences*
  566, 262-280. doi:10.1016/j.ins.2021.03.014
- Temperley, D. (2001). *The Cognition of Basic Musical Structures*. MIT
  Press.
- Temperley, D., Sleator, D. (1999). Modeling meter and harmony: a
  preference-rule approach. *Computer Music Journal* 23(1), 10-27.
  doi:10.1162/014892699559616

## The milestones

- ⬜ **Q0 - The module's documentation, before its code.** The design
  reference above becomes the module's page in the server book's developer
  guide: the steps, what each takes and gives, the methods per step with the
  works they come from, and how they are evaluated. No milestone labels in it.
  It is kept current by each later milestone, and the rustdoc of each method
  cites its work.
- ⬜ **Q1 - The crate, its types and the baselines.** The crate with the
  pipeline's types; today's grid snap, voice finding, key finding and
  spelling moved out of `transcription.rs` into the steps, unchanged, with
  the reading calling them; the JSON doors (C ABI and wasm). **Acceptance:**
  every existing reading test passes unchanged; a `docs/decisions.md` entry
  records the step boundaries.
- ⬜ **Q2 - Each step, from a script.** One function per step in Rust, and the
  same in both clients (a `notate` module in Python, its port on the web):
  run a step and get its result as data, or write it into a sequence as one
  undoable intent; the whole chain with the steps named. Both clients' books
  gain the section that shows it.
- ⬜ **Q3 - The evaluation.** Small fixtures of our own, licensed with the
  project, run in CI; a runner for the external datasets, outside CI, reading
  them from where the user keeps them and never copying them into the
  repository (some forbid it). Per step: beat F-measure and the continuity
  measures; onset and value accuracy; voice F-measure; spelling accuracy;
  and MV2H for the whole chain. The baselines' scores are recorded first.
- ⬜ **Q4 - Beats.** Dixon's multiple agents and Ellis's dynamic programming
  over note onsets weighted by velocity, length and density.
- ⬜ **Q5 - Quantization.** A probabilistic quantizer (Cemgil and Kappen),
  then the merged-output HMM for piano (Nakamura, Yoshii and Sagayama).
- ⬜ **Q6 - Meter.** Preference rules (Temperley and Sleator) over the
  quantized notes; meters that change along the piece, and an anacrusis. It
  may correct the beat level, which feeds back into `Q4` and `Q5`.
- ⬜ **Q7 - Voices.** Contigs (Chew and Wu) and the voice-separation HMM
  (McLeod and Steedman), into the part's writing (`O35`): voices per staff,
  or one voice with ties; a piano's two hands as its two staves.
- ⬜ **Q8 - Spelling.** ps13 in full (Meredith) and the Spiral Array (Chew and
  Chen), which also gives the key and its changes.
- ⬜ **Q9 - The whole chain.** The steps run together and compared with the
  joint methods (Nakamura and others; Shibata and others; Liu and others),
  any trained one only behind its feature.
- ⬜ **Q10 - From the GUI.** One verb per step in the roll and the score
  editor (find the beats, quantize, find the meter, separate the voices,
  spell), each with a preview of what it would write; the roll's conversion
  dialog (`X35.7`) offers the chain.

## Definition of done (per milestone)

Code and tests; a clear commit message; this file's checkbox; the module's
page and the books where the milestone touches them; both clients in the same
commit (the Python client is the reference, the web client follows); an
example of each user-facing step, in both languages; the evaluation's numbers
for a new method recorded here beside its baseline's.

## Future directions (to fold into milestones as they firm up)

- ⬜ **Audio in, notes out.** Multi-pitch detection from a recording, which
  would feed the performance step; out of this module's scope until the steps
  over notes are done.

## Found by use: the running list of fixes

Every entry is a checkbox. A fixed one is ticked with the record of what was
wrong and moves whole to this plan's file under `docs/history/plans/`.
