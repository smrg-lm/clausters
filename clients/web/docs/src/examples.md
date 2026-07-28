# Examples

Every example is a page. Build the package once and serve the directory, then open one:

```sh
cd clients/web
npm install && ./build.sh
python3 -m http.server        # then open http://localhost:8000/examples/…
```

Unless a page says otherwise it runs on the **in-page engine** — no server process, no socket — and the line that would point it at a `clausters --ws` server instead is marked in its source.

## The client

| Page | What it shows |
|---|---|
| [`examples/synth.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/synth.html) | A def built, sent, played and retuned from TypeScript, **over either carrier** — the choice is the one line of the page that names one. Start here. |
| [`examples/sequencing.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/sequencing.html) | The sequencing layer's two halves side by side: a `Pbind` playing generatively on the engine's own sample clock, and a `Timeline` bounced from a pattern, then seeked and looped by a `Playhead`. |
| [`examples/scope.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/scope.html) | The three data paths read by the script and drawn on its own canvas: a control bus as a meter, an audio tap as a triggered oscilloscope and a spectrum, and a buffer reduced by the peak pyramid into a waveform. |
| [`examples/gui-host.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/gui-host.html) | A GUI built and driven from TypeScript: the bound and the scripted control paths side by side, a metered bus, the linked waveform and spectrogram, and one button that swaps the in-page host for a native `clausters-gui --ws` one. |

## Ported from the Python client

Each of these is the web port of the example of the same name in `clients/python/examples/`, so the two can be read against each other — the same instrument, the same point of interest, one written as a script and one as a page.

| Page | Ported from | What it shows |
|---|---|---|
| [`examples/multichannel.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/multichannel.html) | `multichannel.py` | `dup` in both of its senses, operators broadcasting over a channel list, and `mix` folding twelve detuned sines back to one signal. |
| [`examples/typed-controls.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/typed-controls.html) | `typed_controls.py` | A control's type as part of the def: a lagged `freq` that glides, a `tr` gate that re-plucks, an `ir` scalar frozen for the synth's life. |
| [`examples/graph-maths.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/graph-maths.html) | `graph_maths.py` | The operator set past the four — `.midicps()`, `.distort()`, `.clip2()` — and the method form TypeScript composes them with. |
| [`examples/wavetables.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/wavetables.html) | `wavetables.py` | Buffers filled by the server (`/b_gen sine1`, `cheby`), morphed between with `vosc` and waveshaped with `shaper`, both driven live. |
| [`examples/pause-resume.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/pause-resume.html) | `pause_resume.py` | `/n_run`: a paused node stays in the tree with its state and resumes exactly where it left off. |

The rest of the Python examples are not ported yet; several of them exercise surfaces this client does not have (responders, MIDI, automation, the transport grid, an offline render).

## Components and the page runtime

| Page | What it shows |
|---|---|
| [`examples/document/`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/document/) | The shape the whole component format is for: an interactive text, with instruments interleaved with the prose that explains them. |
| [`examples/piano/`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/piano/) | A playable keyboard whose keys the GUI host maps to server voices itself, authored as a bundle by `make_bundle.py`. |
| [`examples/graph-controls/`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/graph-controls/) | A GraphDef's control surface as one component, likewise authored from Python. |
| [`examples/demo.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/demo.html) | The custom elements themselves: `<clausters-bundle>` and the power button. |
| [`examples/standalone.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/standalone.html) | The same boot by hand, through the raw page API rather than an element. |
| [`examples/engine.html`](https://github.com/smrg-lm/clausters/blob/main/clients/web/examples/engine.html) | The engine on its own: raw OSC in and out of the worklet, with no client above it. |
