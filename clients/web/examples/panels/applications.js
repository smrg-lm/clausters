// Applications as pages of the editor's window.
//
// Upload it in the editor's page (File > Open > Upload...) and evaluate it a
// cell at a time (Ctrl+Enter). The twin of
// clients/python/examples/panels/applications.py.
//
// The session draws in the page's own host: each application on a page of
// its own, in the windows panel. The page's editor is not the host's dock yet,
// so an application keeps its own menu bar there.

// %%
import { Session, play, scope } from "clausters";
import { edit } from "clausters/gui";
import { INF, EventSequence, pbind, pseq } from "clausters/seq";

const s = (await Session.live()).activate();
const g = await s.gui();   // the page's host: nothing is opened

(await g.dockCall({ verb: "layout" })).panes.map((p) => p.title);

// %% a sequence in the score editor, on a page of its own
const a = EventSequence.fromRows([[60, 1], [62, 0.5], [64, 0.5], [67, 1], [72, 1]]);
const q = await edit(a, { view: "score", title: "a" });
(await g.dockCall({ verb: "layout" })).page;

// %% the multitrack, on another page
const mt = await g.openApp("multitrack");
(await g.dockCall({ verb: "layout" })).pages.map((p) => p.name);

// %% back to the first page
(await g.dockCall({ verb: "page", name: "Main" })).page;

// %% a scope of the session's server
const p = play(pbind({ instrument: "default", degree: pseq([0, 2, 4, 7], INF), dur: 0.25 }));
const w = await scope(0);

// %%
p.stop();
w.close();

// %%
s.close();

/*
- The page tabs under the windows switch between the score and the
  multitrack.
- On the score's page, Measures > Insert After says "select a note first"
  under the score: the pick is the score editor's. Click a note and pick it
  again.
- Restart takes the session's windows away with it.
*/
