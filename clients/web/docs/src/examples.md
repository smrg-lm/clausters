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

**Each page documents itself**: the comment at the top of its source says what
it shows and what it needs, so the directory listing plus each page's header is
the catalog. Start with `synth.html` — a def built, sent, played and retuned
from TypeScript, over either carrier — or with `verbs.html`, which opens a
`Session` and then plays every kind of thing there is to play against it.

Some of the pages are ports of the Python client's examples of the same name, so
the two can be read against each other: the same instrument, the same point of
interest, one written as a script and one as a page. Others exist only here —
the components (`<clausters-bundle>` and the authored bundles), the page runtime
and the raw engine.
