---
name: gui-apps
description: How to design, build and check the Clausters applications (the audio editor, the multitrack editor, the notes, points and score editors, a panel window) as a whole rather than view by view. Covers how an application is composed of modular components written once and shared (chrome, forms, panels, closing, the editing context, services, coordinators, the window a take is drawn in), the layers and what each owns (clausters-document, clausters-editing, clausters-apps, clausters-app-engine, the GUI host, the clients' handlers), the global questions every change answers (whose state, which history, which verb, what two windows share, which identity, what a resource's lifetime is, which fronts and clients it reaches, what it costs at rest), the conversation between a gesture and its owner, and an optimized development process from the lowest layer to a headless check of the running application. Consult before adding or changing any application feature, verb, dialog, menu, transport or playback behavior, editing service, undo behavior or window composition, and before deciding where an application's logic lives. The views inside an application are gui-widgets; the host's crate and its check by script are clausters-gui.
---

# GUI applications: components, state, documents and how they work together

The three skills of the graphical track are complementary, and each answers one question:

- [[gui-widgets]]: **what one element is.** Its algorithm, its size, what feeds it, its gestures and the payload an edit of it sends back. A view inside an application is a widget, and that skill governs it there too.
- **gui-apps (this skill): how the pieces work together.** An application is a composition of views, state, a document, a history, verbs, a transport, resources and services, reached from two clients and run on several fronts. What matters here is rarely visible in one widget: who owns a piece of state, which history an edit enters, what two windows share, whether a verb exists for a client, what a window costs while nobody touches it, and above all **which component carries a behavior, so that every application gets it once**.
- [[clausters-gui]]: **the host's crate**, its seams and the procedure for checking the host by script, which this skill's process uses.

Owners outrank this skill. `crates/clausters-apps/PLAN.md` holds the milestones and their decisions; `docs/app-behavior.md` holds the behavior rules every application keeps, and a rule found by fixing a regression is added there in the same commit; `docs/architecture.md`, "Application state and services: implementation contract", holds the table of who owns each piece of state; `docs/verbs.md` holds the verbs and the client API each one is; `crates/clausters-document/PLAN.md` holds the document. This skill says how to think and work across them; it does not restate their rules.

## An application is a composition of modular components

**The rule** (`AGENTS.md`, "What is built is adapted, never written again"): a behavior that more than one application has is **one component**, written once, with a narrow interface, and every application composes it. An application keeps only what is its own: its structure, its composition and its answers in its structure's vocabulary. A feature built inside one application that another one will want is the defect, even when it works, because the second application then grows a copy and the two drift.

The components that exist (in `crates/clausters-apps/src` unless named), and what each is for. Look here before writing anything:

| Component | What it carries, for every application that composes it |
|---|---|
| `chrome` | The menu bar and the toolbar in one vocabulary: an entry or a tool names a verb and nothing else |
| `forms` | Dialogs: every form in the window from the start on a page of a `stack`, opened by a correction, never defined or freed |
| `panels` | A column of titled, folding sections beside the work, with a divider a drag moves |
| `closing` | Closing or replacing a window's document that may hold unsaved work: the save/discard/cancel rule |
| `editing::Editing`, `turn` | One undo order over every application open in a context; members (`Member::{Multitrack, Audio, Notes, Points, Score, External}`); what a turn is |
| `application::{Services, Runtime, Resources, Histories, clipboard::Exchange}` | Explicit services: histories, one resource graph with retention roots, the clipboard exchange, typed pending operations whose late results address their original object |
| `editing::coordinator`, `editing::held` | The audio member's playback and the transport facts a script and a hand share; the engine an editing context holds |
| `takes` | What an editor hands that engine (a take's parts, a track's placed regions) and a sound file's header: below every application, so no editor names the context that opened it |
| `samples` | The window a take is drawn in and the measures its picture stacks, which the multitrack's boxes reuse |
| `clausters_editing::verbs` | The one verb table: each verb's name, keys, label and class |
| `clausters_editing::conversation` | Messages, turns, answers and corrections between a host and an owner |
| `clausters_editing` projections | The structure as props, a gesture as payloads, the structure as what sounds, written once per structure |
| `clausters_document::view::catalogue` | The picture of a structure, which every application that shows that structure uses |

**A dialog asks; a panel shows.** A modal dialog (`forms`) is for asking before an act, such as unsaved work, a conversion's options or quitting, and for work in progress. Choosing what a view shows (which curve a lane draws, a spectrogram's settings, which parameter a note displays) and any view of the data as text (an event list, an inspector) is a section of `panels` beside the work, shown or hidden, never a dialog. A dialog that opens to change what is shown stops the work to adjust its picture.

What makes a component modular, and what to check when writing or extending one:

- **One owner, one implementation of the policy.** The contract table names one owner per piece of state; a component that needs a fact another owns takes it as an argument (the crate's rule: the facts of a running system, widget ids, buffers and buses, are the caller's).
- **It names no application.** A component asks what the structure holds (does it have an addressable time axis, does it have files) and never which application or class it is serving (`AGENTS.md`, "A view is configured by what it holds"). An application-kind switch inside a shared component is a component not yet factored.
- **It reaches nothing global.** Services are injected explicitly; nothing discovers applications through a registry, and a composition owns its transport and receives its editing and resource scopes.
- **It is extended where it does not fit.** A parameter, a window onto it, a hook: the old callers keep working and the new one calls it. A second version beside it, however small, is the defect, and a change says why a component could not be reused when it was not.
- **It is tested alone.** A component's behavior is a unit test in its own crate, with no host, no server and no window; an application's test is then only of its composition and its answers.
- **It is usable from every place an application runs**: compiled for the C ABI, wasm and the host, with no type of the host's renderer in it, since a client cannot link a renderer.

## The layers, and what each owns

| Layer | Owns | Never holds |
|---|---|---|
| `clausters-document` | The model: sources, regions, take lanes, tracks, automation, sessions; intents and their inverses; the history log; the clipboard document | The wire, a widget, a picture |
| `clausters-editing` | What a structure owes its endpoints (props, payloads, the instance, the applier); the verb table; the conversation; playback ops | An application's composition |
| `clausters-apps` | The applications: compositions, controls, the answer to each gesture, the editing context, services, closing, forms, chrome, panels | A drawing; a host type; a running system's facts |
| `clausters-app-engine` | The engine an editing application holds: takes, feeds, a playback's medium | OSC |
| `crates/clausters-gui` (the host) | Drawing, gestures, focus, navigation groups, the key table, action resolution, the host clipboard, activity, reconcile; it runs hosted applications on its application thread (`host::audio_app`) | The document's history; an application's policy |
| Clients (`clausters.gui.editing`, `clients/web/src/gui/editing`) | The window set, host resolution, widget ids, the event drain, `edit()`, editors as handles | A copy of a rule; state the host or the crate owns |
| `clausters-core` | Numeric, timing and drawing rules every layer reads | Anything above them |

Placement follows from the table: a rule that decides what an edit means is the document's; how a structure becomes props or sound is the editing crate's; what a window holds and how a gesture is answered is the application's; what only a renderer does is the host's; the clients bind and carry. When a change seems to need logic in a client, that is the signal it belongs one layer down.

## The global questions every change answers

Before writing, answer these for the change. Each one points at where the answer is already decided; a question with no decided answer is a decision to take and record, not to improvise in code.

1. **Whose state is it?** The document's (it has history), the window's or the view's (cursor, selection, zoom: no history), the host's alone (hover, focus, navigation), a session's, or a resource's. One owner per row of the contract table; never a copy kept in a client.
2. **Which verb is it, and which class?** Every feature is a verb of the one table, reachable as `editor.verb(name)` and as a named member in both clients (`docs/verbs.md`; `clients/python/tests/test_verbs.py` reads the three surfaces). A menu, a key and a tool are callers that hold no state, and an action's state is read when asked, never kept. A verb is one of three classes: an edit of the document (an entry of its history), a change of window state (none), or a change of which document the window holds (none, under `closing`).
3. **Which history does it enter?** One order per editing unit unless a caller composes a shared one; a view joins its document's order; a text field keeps its own; a step tells the window where it left the context once it has landed.
4. **What does the conversation look like?** A gesture leaves the host as an intent in the owner's terms, absolute and idempotent; the owner applies it, acknowledges with the effective value and the document's version, and corrects the window with each widget's own props, addressed to the components it composed. A gesture against a picture that is gone is refused and the picture handed back; a refused operation corrects its window and says why.
5. **Which identity names it?** A `FileId`, a widget id, a `ResourceId`, a `StructureId` or a `MemberId` are different things with different lifetimes. Widgets that draw part of a structure take ids named after it (`WidgetIds`), so the host's reconcile keeps its own state across a redefine; a binding carries a generation so an old gesture is refused.
6. **What do two windows share?** Two views of one object share its time axis, its play cursor and its history (a navigation group, by `link`); two windows over two objects share nothing that moves; a transport belongs to an object.
7. **What plays, and from where?** A transport per object; a playback time read from the playback, never computed; a pass that ends taken in at once; the multitrack and the roll play by the audio editor's rules, by calling what carries them out.
8. **How long does each resource live?** Retention roots in one graph (contents, history, clipboard, playback, pending work); clipboard storage and undo storage have separate lifetimes; a copy of a take is made on the server; successful I/O still waits for the application's confirmation.
9. **Where does it run?** Every front (a native window, the headless host, a page with the applications' build) and both clients, with the same calls in the same order; under `standalone` with no client at all. A behavior that differs between two of them is a defect.
10. **What does it cost at rest?** Demand follows visible consumers and playback; a settled window does nothing on a period; the chrome does not wait for a file's work; work is done at the pace of interaction.
11. **Does a component already do it?** Then the change is in that component, or is a call to it (the section above).

## The optimized development process

Ordered so that each step is the cheapest one that can still find the mistake, and so that nothing slow runs before something fast has said it is needed.

1. **Locate, in one pass.** Read the milestone in the apps plan, the rules of `docs/app-behavior.md` the change touches, the contract table's rows for the state involved and the verb table's rows. Find the component that already carries the behavior (`rg` over `crates/clausters-apps/src`, `crates/clausters-editing/src/verbs.rs`, `docs/verbs.md`). Answer the global questions above; where a record already decides one, follow it; where none does, decide and record it in `docs/decisions.md` when the choice is not obvious.
2. **Write the rule at the lowest layer that owns it, test-first there.** `cargo test -p clausters-document`, `-p clausters-editing` or `-p clausters-apps --lib <filter>` from the repository root: seconds to build, a fraction of a second to run, with no GPU, no server and no window. An application's turn is tested by opening the editor in an `Editing`, handing it calls and messages and asserting the answer, the corrections, the structure and the history (`crates/clausters-apps/src/editing/tests.rs` is the pattern: `call_json` over an editor opened in a context); a shared component is tested alone. This is where most application behavior is settled, and a behavior that can be a unit test is one.
3. **The host's half, if any**, with `cargo test --lib <filter>` in `crates/clausters-gui/` (a gesture or a key over a JSON-built host).
4. **The doors and both clients in the same change.** An FFI or wasm export declared in `docs/bindings.md` (checked by `crates/clausters-checks/tests/bindings.rs` and `clients/python/tests/test_native_parity.py`); a verb's row in `docs/verbs.md`; each client's handler a line over the door. Run the Python tests for the touched modules and `npx pyright` in `clients/python` after a signature change.
5. **The running application, headless, from one scratch script.** A hosted application runs inside the GUI host, so a change to the document, editing or apps crates reaches it by rebuilding **only the host binary in debug** (`cargo build --bin clausters-gui` in `crates/clausters-gui/`) and pointing the client at it with `CLAUSTERS_GUI_BIN`; what the client binds in its own process (the points editor, the client-side projections) is reached with `cargo build -p clausters-ffi` and `CLAUSTERS_FFI_LIB`. Nothing needs `scripts/refresh-bin.sh`'s release build until an example is run by hand. The script runs from a scratch directory whose `clausters.toml` says `[gui] headless = true`, so no window reaches the screen:

   ```python
   # <scratchpad>/run/clausters.toml holds: [gui] headless = true
   import os, sys, time
   os.chdir(sys.argv[1])
   from clausters import Session
   from clausters.gui import edit

   session = Session.live().activate()
   session.gui()
   ed = edit(structure, sample_rate=session.server.query_info().nominal_sample_rate,
             title="check", width=480, height=240)
   ed.select_all(); ed.quantize()            # verbs, as the hand's keys
   ed.drag((120, 90), (160, 90), mods="ctrl")  # a hand, in device pixels
   ed.press_key("z", "ctrl")                 # a key, through the key table
   print(ed.capture("1-after.png"))          # waits for the owner's answers
   time.sleep(1.0); a = ed.activity(); time.sleep(1.0); b = ed.activity()
   assert not b["demand"] and b["ticks"] == a["ticks"], "a settled window ticks"
   session.close()
   ```

   run as `CLAUSTERS_GUI_BIN=$PWD/target/debug/clausters-gui PYTHONPATH=$PWD/clients/python .venv/bin/python <script> <scratchpad>/run`, in one Bash invocation (a couple of seconds). **Assert on numbers first**: the structure read back (the edited data is read on the structure passed to `edit`), the editor's state, `host.query` of a widget, the activity. Then read the pictures for what a number cannot say.
6. **The behavior the change did not touch, in the same script.** `docs/app-behavior.md`, "Checking a change", is a list of steps a script takes: two windows over two objects (open two editors, play one, capture both: time, cursor and meter stay apart), zoom out as far as the axis allows on a take and on an empty file, the vertical with one, two and many channels, and play, stop, loop, the selection, save and undo. Keep these as functions of the scratch script and rerun the whole script after every rebuild; it is the regression net for the session's work. Test with long takes (minutes of stereo), since a picture or a read that is fine for a second of samples is not the case that breaks.
7. **Before and after, for a regression.** Build the previous commit in a worktree and run the same script against both host binaries; compare the pictures pixel by pixel (one adapter draws the same bytes run to run) and the numbers line by line, before guessing at a cause.
8. **The page, only when the web package changed.** `clients/web`: `./build.sh && ./test.sh`; the acceptance pages drive the applications' build of the host in a tab.
9. **The example pair, last.** The Python and web examples of the feature, the same calls in the same order; `scripts/look.py` over an example's steps leaves its pictures, and the user's eye pass on the example comes afterwards.
10. **Close in one pass over the packages**: the rule added to `docs/app-behavior.md` if the work found one, the plan's checkbox, the books where a surface changed (`scripts/check-docs.sh`), `cargo fmt` and clippy clean.

What keeps the loop fast:

- **The lowest rung that can answer, every time.** A turn tested in Rust is a second; the same check through a window is a build of the host and a session boot. Go up a rung only for what the lower one cannot see: the host's drawing, the clients' doors, the fronts.
- **Build only what the step needs**: one crate's tests, then one binary in debug, one build at a time (`-j 10`, `CARGO_INCREMENTAL=0`). The release bundle, the feature matrix and the cross-browser pass belong to a release, not to an iteration.
- **One scratch script for the whole piece of work**, its steps as functions and its assertions in it, rerun after each change rather than rewritten; its pictures in the scratchpad, never in the tree.
- **Never a window on the user's screen for a check**: the headless host runs while somebody works, and the eye pass is the user's, afterwards.

## Conventions

The track's rules apply ([[clausters-gui]]): fmt and clippy clean, English throughout, the non-divergence rule between the clients, closing work with the plan's checkbox, the docs and a commented example. The domain vocabulary is the project's (`AGENTS.md`): source, region, take lane, track, automation, session; "symbolic score" and "OSC score"; a node or widget is freed, an element is rendered. The applications' examples are written by [[examples]]; a widget an application composes is designed by [[gui-widgets]]; the host is mapped and checked by [[clausters-gui]].
