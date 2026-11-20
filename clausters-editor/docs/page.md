# The editor in a page

The editor also runs as a page in a browser tab, for **JavaScript**: the same
editor, post window and documentation, drawn by the same GUI host across the
whole tab, with a JavaScript session where the desktop application has its
Python one. Its session plays on the
[clausters](https://clausters-web.readthedocs.io/) engine the page runs, and
draws on the same host.

What the page has not, the menus do not offer: environments (its session is
the page's worker), a documentation folder of your own, the system's fonts,
and Quit (the browser closes the tab, and asks first when a file has changes
that are not saved).

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
  What is imported is the clausters package (`clausters`, and its subpaths
  `clausters/defs`, `clausters/seq`, `clausters/gui`...) and the page's own
  files (`./helper.mjs`, beside a saved document).
- `console.log` and what a timer or a promise writes after its evaluation has
  ended go to the post window; so does an error nothing catches.
- **Restart** starts a new worker and clears every name.

The page keeps the engine and the GUI host, and the session reaches them with
no address named, as a Python session reaches the local server. The **startup
code** (File → Preferences) makes the session's clausters session, and is where
that changes:

```javascript
import { Session } from "clausters";
const session = (await Session.live()).activate();
const host = await session.gui();
```

`await new Server().attach()` reaches the same engine, and makes it the default
session's server if none is; `boot()` refuses, since the engine is the page's
and already running, as a Python `boot()` refuses where a server answers.

After it, `play`, a `Synth` or a pattern sound with nothing else named, and so
does a routine:

```javascript
import { play, Routine } from "clausters";

function* test() {
  for (let i = 0; i < 10; i++) {
    play({ freq: 440 });
    yield 1;
  }
}

const r = new Routine(test);
r.play();
```

The engine's audio starts at the page's first key or click, as a browser
requires.

A window the session opens is a pane of the editor's window, as on the
desktop, with a tab: drag a tab onto another pane to put the two in one group
or side by side, or drag the line between two panes to resize them. An
application the session opens (`edit()` of a pattern opens the notes editor)
is a pane like any other window. The session's windows go with the session.

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
→ Open and File → Save show it, in the editor's own dialog. In the same
dialog, **Upload** copies files from the disk into the folder shown,
**Download** hands the file selected back to the disk, **New folder** makes the
folder whose name is typed, **Rename** gives the file selected the name typed,
and **Delete** deletes the file selected, or the folder shown when none is
(it asks first).

## Completions and help

Completions, signatures, hover help and `Ctrl+D` come from the TypeScript
language service, in a worker of its own, which reads the document and the
type declarations of the clausters package. A helper file of the page's
storage it does not read.

## Serving the page

From the page's folder in the repository (`crates/clausters-editor/page`),
with the clausters web package built beside it (`./build.sh` in
`clients/web`):

```
npm ci
./build.sh
npm run serve
```

and open the address it prints. The page loads the clausters package from
`clausters/` beside it; `npm run serve` serves it there from `CLAUSTERS_WEB`
(the repository's `clients/web` by default), and a page built with
`VITE_CLAUSTERS` loads it from that address instead. `npm test` drives the
page in headless Chrome (`npm test -- firefox` in Firefox).

[Back to the start](index.md#shortcuts)
