// Applications as pages of the editor's window, and views beside the code.
//
// Upload it in the editor's page (File > Open > Upload...) and evaluate it a
// cell at a time (Ctrl+Enter). The twin of
// clients/python/examples/panels/applications.py.
//
// `openApp(x)` opens the application that edits `x`; `openApp(null, { app })`
// opens one over nothing. `edit(x)` opens the view alone. The page's editor is
// not the host's dock yet, so here each opens in the windows panel and an
// application keeps its own menu bar.

// %%
import { Session, play, scope } from "clausters";
import { edit, openApp } from "clausters/gui";
import { INF, EventSequence, pbind, pseq } from "clausters/seq";

const s = (await Session.live()).activate();
const a = EventSequence.fromRows([[60, 1], [62, 0.5], [64, 0.5], [67, 1], [72, 1]]);

// %% the score editor over the sequence, on a page of its own
const q = await openApp(a, { view: "score", title: "a" });
console.log(q.title);

// %% an empty multitrack, on another page
const mt = await openApp(null, { app: "multitrack" });
console.log(mt.title);

// %% the roll over the same sequence, beside the code: no menu, no toolbar
const r = await edit(a, { title: "a" });

// %% what the roll and the page left in the sequence: drag a note first
a.toRows(["midinote", "dur"]);

// %% a scope of the session's server, beside the code
const p = play(pbind({ instrument: "default", degree: pseq([0, 2, 4, 7], INF), dur: 0.25 }));
const w = await scope(0);

// %%
p.stop();
w.close();
await r.close();

// %%
s.close();

/*
- Measures > Insert After on the score's window says "select a note first"
  with nothing selected: the pick is the score editor's.
- Restart takes the session's windows away with it.
*/
