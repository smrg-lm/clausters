# The editor in a page

The editor also runs as a page in a browser tab, for **JavaScript**: the same
editor, post window and documentation, with a JavaScript session where the
desktop application has its Python one. Its session plays on the
[clausters](https://clausters-web.readthedocs.io/) engine the page runs, and
draws on its GUI host.

## The session

The session is a worker of the page: every evaluation shares its global scope,
as in a console.

```javascript
const greeting = (name) => `Hello, ${name}`;
greeting("world")
```

- If the last statement is an expression, its value is shown after `->`, and
  is kept in `_`.
- What an evaluation declares at the top level -- `const`, `let`, `class`,
  `function` -- the next one sees, and **declaring it again redefines it**.
- `import` works at the top level, as in a module, and so does `await`.
  What is imported is the clausters package (`clausters/session`,
  `clausters/defs`, `clausters/seq`, `clausters/gui`...) and the page's own
  files (`./helper.mjs`, beside a saved document). The package's main entry,
  `clausters`, defines a page's elements and a worker has no page, so a
  session imports its subpaths.
- `console.log` and what a timer or a promise writes after its evaluation has
  ended go to the post window; so does an error nothing catches.
- **Restart** starts a new worker and clears every name.

The page keeps the engine and the GUI host, and the session reaches them
through `editor.engine` and `editor.gui`. The **startup code** (File →
Preferences) makes the session's clausters session with them, and is where
that changes:

```javascript
import { Session } from "clausters/session";
const session = (await Session.live(editor.engine)).activate();
const host = await session.gui({ port: editor.gui });
```

After it, `play`, a `Synth` or a pattern sound with nothing else named. The
engine's audio starts at the page's first key or click, as a browser requires.

A window the session opens goes in a frame of its own at the right of the
page, which can be resized; the window follows it.

## Interrupting

`Ctrl+.` stops an evaluation that awaits something, and then calls the
interrupt hook of the packages the session imported, which for clausters stops
everything its sessions play. The session keeps its names.

Code that never lets go -- `for (;;) {}` -- cannot be told anything, so the key
**terminates the worker and starts a new one**: what was playing is silenced
from the page, and what the session had defined is gone, as after a restart.
The post window says which of the two happened.

## Files

A page's files are its own storage, which the browser keeps for the page: File
→ Open and File → Save show it. In the same dialog, **Upload** copies files from
the disk into the folder shown, the download button beside a file hands it back
to the disk, and folders are made and deleted there.

## Completions and help

Completions, signatures, hover help and `Ctrl+D` come from the TypeScript
language service, in a worker of its own, which reads the document and the
type declarations of the clausters package. A helper file of the page's
storage it does not read.

## Serving the page

From the editor's folder, with the clausters web package built beside it
(`./build.sh` in `clients/web`):

```
npm run build:web
npm run serve:web
```

and open the address it prints. The page loads the clausters package from
`clausters/` beside it; `serve:web` serves it there from `CLAUSTERS_WEB` (the
repository's `clients/web` by default), and a page built with `VITE_CLAUSTERS`
loads it from that address instead.

[Back to the start](index.md#shortcuts)
