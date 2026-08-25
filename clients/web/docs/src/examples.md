# Examples

Every example is a page, and the pages live in the **repository**, not in the
npm package: `clients/web/examples/`. Build the package once and serve the
directory, then open one:

```sh
cd clients/web
npm install && ./build.sh
python3 -m http.server        # then open http://localhost:8000/examples/…
```

Unless a page says otherwise it runs on the **in-page engine** — no server
process, no socket — and the line that would point it at a `clausters --ws`
server instead is marked in its source.

## The folders

One folder per subject, and the **same set in the Python client**
(`clients/python/examples/`), so a page and its script sit in the same place
under the same name:

| Folder | What is in it |
|---|---|
| `basics/` | the language: the engine in a page, a def built and played, control types, channels, graph maths, wavetables, the ambient verbs |
| `spectral/` | the frequency domain |
| `buffers/` | samples read, written and bounced |
| `transport/` | time: patterns on a clock, an automation lane, the shared transport |
| `io/` | the outside: responders |
| `panels/` | GUI: controls and layout, and the hosts that carry them |
| `views/` | GUI: reading something — meters, scopes, a waveform, a recording |
| `editors/` | GUI: writing something — a score, an arrangement, a patch, a document |
| `components/` | the page's own surface: `<clausters-bundle>` and the authored bundles |

**Each page documents itself**: the comment at the top of its source says what
it shows and what it needs, so the directory listing plus each page's header is
the catalog. Start with `basics/synth.html` — a def built, sent, played and
retuned from TypeScript, over either carrier — or with `basics/verbs.html`,
which opens a `Session` and then plays every kind of thing there is to play
against it.

Most pages are ports of the Python client's example of the **same name in the
same folder**, so the two can be read against each other: the same instrument,
the same point of interest, one written as a script and one as a page. The
`components/` folder is the exception — a page's custom elements have no script
counterpart — and a few pages elsewhere are still the platform's own
(`basics/engine.html`, `panels/two-hosts.html`).
