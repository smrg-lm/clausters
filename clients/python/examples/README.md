# Examples (installed-package)

These examples import `clausters` from the **installed package** — no
`sys.path` shim, no `target/` directory, no separately built binary needed for
the offline ones. They are meant to be run after installing the wheel (see the
[client README](../README.md)):

```sh
python -m venv .venv && . .venv/bin/activate
pip install ./clients/python          # builds + bundles the native libs
python clients/python/examples/basics/hello_note.py
```

**Each example documents itself**: its module docstring says what it shows, what
it needs and how to run it. Two good entry points:

- `basics/hello_note.py` — the shortest path to sound: boot a server, play a
  note.
- `basics/verbs.py` — every playable kind through one `play`, and one `render`
  for the change of state.

## The folders

One folder per subject, and the **same set in the web client**
(`clients/web/examples/`), so a reader can pair an example with its page by
looking in the same place under the same name — they are one example in two
languages, and a name that only matched by accident is what made them hard to
read against each other.

| Folder | What is in it |
|---|---|
| `basics/` | the language: a note, an envelope, control types, channels, graph maths, wavetables, the ambient verbs |
| `spectral/` | the frequency domain: the FFT chain, cross-synthesis, bin expressions, convolution |
| `buffers/` | samples the client reads, writes and bounces |
| `transport/` | time: a timeline, an automation lane, and the shared transport across clients |
| `io/` | the outside: OSC in and out, MIDI, the wire, an embedded server, several servers |
| `faust/` | DSP written as Faust rather than as a UGen graph |
| `panels/` | GUI: controls, layout, style, and the hosts that carry them |
| `views/` | GUI: reading something — meters, scopes, spectra, waveforms, a node tree |
| `editors/` | GUI: writing something — a curve, a roll, a score, an arrangement, a patch |

The last three need a **display and a GPU adapter**; the rest render offline or
against a server. Every example is organized as `# %%` cells, so a window stays
open while you evaluate cell by cell in VS Code or Jupyter, and running the file
as a script drives it and tears down.

The ones that need a **running** server (`io/live_udp.py`, the transport and
responder demos) name it in their docstring; the wheel ships that server as the
`clausters` command.

**What a run leaves behind goes in `out/`**, here beside the folders — the
rendered WAVs, the MIDI files, the session an editor example hands to the host
and reads back. It is git-ignored, so there is one place to look in and one to
delete (`rm -rf clients/python/examples/out`), and a render lands there however
you ran the example rather than in whatever directory you happened to be in. A
path argument still overrides it: `python basics/envelope.py /tmp/mine.wav`.

The lower-level demos — the transports, the raw OSC helpers, the audible tours
of the UGen families — live in the repository-root
[`examples/`](../../../examples/); those use a `sys.path` shim so they run
straight from a source checkout without an install.
