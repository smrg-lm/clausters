// The editor as a page, driven as a hand drives it: a client of the page's
// GUI host -- the protocol's own, as a script checks a host it did not start
// -- presses the editor's keys, types into it and reads its post window,
// and the browser gives the page the one gesture a page waits for before its
// audio. What it checks is the JavaScript session's behaviour as the editor's
// documentation describes it (docs/page.md), against the real clausters
// engine and GUI host the page brings up.
//
//   ./build.sh && npm test -- [chrome|firefox]
//
// Chrome is driven over the DevTools protocol, Firefox over WebDriver BiDi.
// Needs a built clausters web package (clients/web, which ./build.sh builds;
// or CLAUSTERS_WEB) and the browser (CHROME or FIREFOX, else on the PATH).

import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL("..", import.meta.url));
const PORT = 8152;
const BROWSER = process.argv[2] ?? "chrome";
const PAGE = `http://127.0.0.1:${PORT}/`;

function which(names) {
  return names.find((b) => {
    try {
      execFileSync("which", [b], { stdio: "ignore" });
      return true;
    } catch {
      return false;
    }
  });
}
const bin =
  BROWSER === "firefox"
    ? (process.env.FIREFOX ?? which(["firefox"]))
    : (process.env.CHROME ?? which(["google-chrome", "chromium", "chromium-browser"]));
if (!bin || !["chrome", "firefox"].includes(BROWSER)) {
  console.error(`page.e2e: no ${BROWSER} to run the page in`);
  process.exit(2);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const server = spawn(process.execPath, [join(here, "scripts", "serve.mjs"), String(PORT)], { stdio: "inherit" });
// Firefox installed as a snap reads only profiles under its own folder.
const profile = mkdtempSync(
  BROWSER === "firefox" ? join(homedir(), "snap", "firefox", "common", "editor-page-") : join(tmpdir(), "editor-page-"),
);
const BIDI_PORT = 9372;
if (BROWSER === "firefox") {
  writeFileSync(
    join(profile, "user.js"),
    [
      // The browser's own autoplay policy, as a user's: the page's audio
      // starts at its first key, which the test presses.
      // Headless Firefox plays a page's audio aloud: silenced.
      'user_pref("media.volume_scale", "0.0");',
      'user_pref("browser.shell.checkDefaultBrowser", false);',
      'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
      'user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);',
      "",
    ].join("\n"),
  );
}
const browser = spawn(
  bin,
  BROWSER === "firefox"
    ? ["--headless", "--no-remote", "--profile", profile, "--remote-debugging-port", String(BIDI_PORT), "--width=1400", "--height=900"]
    : [
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        "--enable-unsafe-swiftshader",
        "--window-size=1400,900",
        "--remote-debugging-port=0",
        `--user-data-dir=${profile}`,
        "about:blank",
      ],
  { stdio: "ignore", detached: true },
);

/** A socket that sends commands and answers each by its id. */
async function rpc(url) {
  let socket;
  for (let i = 0; i < 100; i++) {
    try {
      socket = new WebSocket(url);
      await new Promise((resolve, reject) => {
        socket.addEventListener("open", resolve, { once: true });
        socket.addEventListener("error", reject, { once: true });
      });
      break;
    } catch {
      socket = null;
      await sleep(200);
    }
  }
  if (!socket) throw new Error(`could not reach ${url}`);
  let nextId = 1;
  const waiting = new Map();
  const listeners = new Map();
  socket.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && waiting.has(msg.id)) {
      waiting.get(msg.id)(msg);
      waiting.delete(msg.id);
    } else if (msg.method && listeners.has(msg.method)) {
      listeners.get(msg.method)(msg.params);
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      waiting.set(id, (msg) =>
        msg.error ? reject(new Error(`${method}: ${msg.error.message ?? msg.message ?? msg.error}`)) : resolve(msg.result),
      );
      socket.send(JSON.stringify({ id, method, params }));
    });
  /** What the other end says unasked: `listener` hears each `method`. */
  send.on = (method, listener) => listeners.set(method, listener);
  return send;
}

/** Chrome, over the DevTools protocol. */
async function chrome() {
  let port;
  for (let i = 0; i < 100 && !port; i++) {
    try {
      port = readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0];
    } catch {
      await sleep(100);
    }
  }
  const target = await (await fetch(`http://127.0.0.1:${port}/json/new?${PAGE}`, { method: "PUT" })).json();
  const send = await rpc(target.webSocketDebuggerUrl);
  return {
    evaluate: async (expression) => {
      const { result, exceptionDetails } = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
      return result.value;
    },
    /** A click of the browser's own at (x, y): the page's first gesture. */
    click: async (x, y) => {
      for (const type of ["mousePressed", "mouseReleased"]) {
        await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
      }
    },
    /** The next file picker the page opens answers `file`, a file on the
     *  disk, as a reader choosing it would. */
    chooseNext: async (file) => {
      await send("Page.enable");
      await send("Page.setInterceptFileChooserDialog", { enabled: true });
      send.on("Page.fileChooserOpened", ({ backendNodeId }) => {
        send("DOM.setFileInputFiles", { files: [file], backendNodeId }).catch(() => {});
      });
    },
    /** A key of the browser's own, which is a gesture of the reader's: what
     *  a page may open a file picker on. */
    press: async (key, code) => {
      await send("Input.dispatchKeyEvent", { type: "keyDown", key, code, text: key === " " ? " " : undefined });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key, code });
    },
    /** Lets the page's downloads land in `dir`. */
    downloadInto: (dir) => send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: dir }),
    /** Ctrl+V of the browser's own with `text` on its clipboard: Control
     *  pressed and held, then V, which runs the browser's paste. */
    paste: async (text) => {
      const origin = new URL(PAGE).origin;
      await send("Browser.grantPermissions", { origin, permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"] });
      const { exceptionDetails } = await send("Runtime.evaluate", {
        expression: `navigator.clipboard.writeText(${JSON.stringify(text)})`,
        awaitPromise: true,
        userGesture: true,
      });
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
      const control = { key: "Control", code: "ControlLeft", windowsVirtualKeyCode: 17 };
      const v = { key: "v", code: "KeyV", windowsVirtualKeyCode: 86, modifiers: 2 };
      await send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...control, modifiers: 2 });
      await send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...v, commands: ["paste"] });
      await send("Input.dispatchKeyEvent", { type: "keyUp", ...v });
      await send("Input.dispatchKeyEvent", { type: "keyUp", ...control });
    },
  };
}

/** Firefox, over WebDriver BiDi. */
async function firefox() {
  const send = await rpc(`ws://127.0.0.1:${BIDI_PORT}/session`);
  await send("session.new", { capabilities: {} });
  const { context } = await send("browsingContext.create", { type: "tab" });
  await send("browsingContext.navigate", { context, url: PAGE, wait: "complete" });
  /** A BiDi value as the plain value it describes. */
  const plain = (v) => {
    if (!v) return undefined;
    if (v.type === "array") return v.value.map(plain);
    if (v.type === "object") return Object.fromEntries(v.value.map(([k, x]) => [k, plain(x)]));
    return v.value;
  };
  const evaluate = async (expression) => {
    const answer = await send("script.evaluate", { expression, target: { context }, awaitPromise: true, resultOwnership: "none" });
    if (answer.type === "exception") throw new Error(answer.exceptionDetails?.text ?? "an exception");
    return plain(answer.result);
  };
  return {
    evaluate,
    /** Ctrl+V of the browser's own with `text` on its clipboard: Control
     *  pressed and held, then V, which runs the browser's paste. */
    paste: async (text) => {
      const answer = await send("script.evaluate", {
        expression: `navigator.clipboard.writeText(${JSON.stringify(text)})`,
        target: { context },
        awaitPromise: true,
        userActivation: true,
      });
      if (answer.type === "exception") throw new Error(answer.exceptionDetails?.text ?? "the clipboard was not written");
      await send("input.performActions", {
        context,
        actions: [
          {
            type: "key",
            id: "keyboard",
            actions: [
              { type: "keyDown", value: "\uE009" },
              { type: "keyDown", value: "v" },
              { type: "keyUp", value: "v" },
              { type: "keyUp", value: "\uE009" },
            ],
          },
        ],
      });
    },
    /** A click of the browser's own at (x, y): the page's first gesture. */
    click: (x, y) =>
      send("input.performActions", {
        context,
        actions: [
          {
            type: "pointer",
            id: "mouse",
            actions: [
              { type: "pointerMove", x, y },
              { type: "pointerDown", button: 0 },
              { type: "pointerUp", button: 0 },
            ],
          },
        ],
      }),
  };
}

let failures = 0;
function check(ok, what) {
  console.log(`${ok ? "ok" : "FAIL"} - ${what}`);
  if (!ok) failures++;
}

async function finish(code) {
  try {
    process.kill(-browser.pid, "SIGKILL");
  } catch {}
  server.kill();
  rmSync(profile, { recursive: true, force: true });
  process.exit(code);
}

// The editor's windows, by the ids it gives them (crates/clausters-editor,
// app.rs): its own start past every id a client allocates.
const IDS = 1 << 24;
const POST_WIN = IDS + 1;
const POST = IDS + 2;
const DOCS = IDS + 4;
const BAR = IDS + 5;
const FILE = IDS + 1000;
const CODE = FILE + 2;
/** The editor's file dialog, its field (filedialog.rs). */
const NAME = IDS - 100000 + 2;

try {
  const page = BROWSER === "firefox" ? await firefox() : await chrome();
  const evaluate = page.evaluate;
  // the page, loaded
  for (let i = 0; i < 300; i++) {
    const state = await evaluate("location.href + ' ' + document.readyState").catch(() => null);
    if (state === `${PAGE} complete`) break;
    await sleep(100);
  }
  // A client of the page's host, in the page: the protocol's GuiHost over a
  // link of its own, as a session's worker has one.
  await evaluate(`(async () => {
    const until = Date.now() + 30000;
    while (!document.querySelector("#dock canvas") && Date.now() < until) await new Promise((r) => setTimeout(r, 100));
    const c = await import("./clausters/dist/index.js");
    const p = await import("./clausters/dist/page.js");
    const gui = await p.guiHost();
    globalThis.check = await new c.GuiHost({ connection: await p.portConnection(gui.link().port) }).attach({ adoptAmbient: false });
    return true;
  })()`);
  /** Something the test's client does in the page, `host` its GuiHost. */
  const host = (body) => evaluate(`(async (host) => { ${body} })(globalThis.check)`);
  /** The post window's text: all of it selected, and the selection read. */
  const post = () =>
    host(`
      host.set(${POST}, { focus: 1 });
      host.verb(${POST_WIN}, "select_all");
      const text = (await host.query(${POST})).props.selected ?? "";
      host.set(${CODE}, { focus: 1 });
      return String(text);
    `);
  /** Waits until the post window has `text` after `from` characters. */
  const waitFor = async (text, from = 0, ms = 15000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      const now = await post();
      if (now.slice(from).includes(text)) return true;
      await sleep(200);
    }
    return false;
  };
  /** A key on the file's window, as a hand presses it. */
  const key = (name, mods = "") => host(`host.pressKey(${FILE}, ${JSON.stringify(name)}, ${JSON.stringify(mods)})`);
  /** Replaces the document with `code`, whole, as a paste puts it (typed a
   *  key at a time, the code's own indentation and closing brackets would
   *  be added to it), and evaluates all of it. */
  const run = async (code) => {
    await host(`host.set(${CODE}, { focus: 1, text: ${JSON.stringify(code)} })`);
    await key("a", "ctrl");
    const from = (await post()).length;
    await key("Enter", "shift");
    return from;
  };
  const interrupt = () => key(".", "ctrl");
  /** The panes of the page's dock that are not the editor's own. */
  const sessionPanes = () =>
    host(`return (await host.dockCall({ verb: "layout" })).members.filter((id) => id < ${IDS});`);

  // Until a key or a click the page may start no audio, so the session waits,
  // and an evaluation asked for meanwhile runs once it has started, after the
  // startup code.
  check(await waitFor("starts at the first key or click", 0, 30000), "the session waits for the page's first gesture");
  let at = await run("1 + 1");
  await page.click(300, 300);
  check(await waitFor("JavaScript worker ready", 0, 30000), "the session starts in a worker at the first click");
  check(await waitFor("-> 2", at, 15000), "the evaluation asked for meanwhile runs");
  const order = await post();
  check(order.indexOf("Startup code") >= 0 && order.indexOf("Startup code") < order.indexOf("-> 2"), "after the startup code");
  check(!order.includes("-> GuiHost"), "the default startup code shows no value");
  await sleep(1000);

  at = await run('const greeting = (name) => `Hello, ${name}`;\ngreeting("world")');
  check(await waitFor('-> "Hello, world"', at), "a value is shown after ->");

  at = await run("const greeting = 1;\ngreeting + 1");
  check(await waitFor("-> 2", at), "declaring a name again redefines it");

  at = await run('await new Promise((r) => setTimeout(r, 300));\n"half a second later"');
  check(await waitFor('-> "half a second later"', at), "await works at the top level");

  at = await run('import { Session } from "clausters";\ntypeof Session');
  check(await waitFor('-> "function"', at), "import works, from the clausters package");

  at = await run('setTimeout(() => console.log("from a timer"), 200);\n"scheduled"');
  check(await waitFor("from a timer", at), "output after the evaluation ended reaches the post window");

  at = await run("null.x");
  check(await waitFor("TypeError", at), "an error is shown");

  // Sound: the startup code's session, on the page's engine.
  at = await run('import { sine } from "clausters/defs";\nimport { play } from "clausters";\nawait play(sine(660).mul(0.05));\nawait new Promise((r) => setTimeout(r, 300));\n(await session.server.status()).synths');
  check(await waitFor("-> 1", at), "a synth plays on the page's engine");

  // A note played now sounds now: the engine's axis follows the wall clock
  // across the time its audio waited for the first gesture.
  at = await run('import { play } from "clausters";\nconst t0 = performance.now();\nplay({ freq: 440, amp: 0 });\nwhile ((await session.server.status()).synths === 0) await new Promise((r) => setTimeout(r, 5));\nperformance.now() - t0 < 200');
  check(await waitFor("-> true", at), "a note played now sounds within the latency");

  // A server handle as a Python script writes one: attach reaches the page's
  // engine, boot refuses since it is up.
  at = await run('import { Server } from "clausters";\nconst s = await new Server().attach({ adoptDefault: false });\nconst refused = await new Server().boot().then(() => "booted", (e) => e.message);\n[(await s.status()).synths >= 0, refused.includes("attach()")]');
  check(await waitFor("-> [ true, true ]", at), "new Server().attach() reaches the page's engine, and boot() refuses");

  // A routine, written as the Python client's.
  at = await run('import { Routine } from "clausters";\nvar notes = 0;\nfunction* test() {\n  for (i = 0; i < 4; i++) {\n    play({ freq: 440, amp: 0 });\n    notes++;\n    yield 0.25;\n  }\n}\nr = new Routine(test);\nr.play();\nawait new Promise((r) => setTimeout(r, 1500));\nnotes');
  check(await waitFor("-> 4", at), "a routine plays on the session's clock");

  // Ctrl+. while idle runs the interrupt hook: the session's sound stops.
  await interrupt();
  await sleep(600);
  at = await run("(await session.server.status()).synths");
  check(await waitFor("-> 0", at), "Ctrl+. stops what was sounding");

  // Ctrl+. while awaiting stops the evaluation; the names stay.
  at = await run("await new Promise(() => {})");
  await sleep(300);
  await interrupt();
  check(await waitFor("Interrupted", at), "Ctrl+. stops an evaluation that awaits");
  at = await run("greeting");
  check(await waitFor("-> 1", at), "and the session keeps its names");

  // A window from the session's code: a pane of the page's one dock.
  at = await run('import * as gui from "clausters/gui";\nconst host = await session.gui();\nconst level = host.open(gui.view({ title: "level" }, gui.slider({ name: "level" })));\nlevel.id');
  await waitFor("-> ", at);
  await sleep(500);
  check((await sessionPanes()).length === 1, "a window the session opens is a pane of the dock");

  // An application: the page is cross-origin isolated, so its host has them.
  at = await run('import { pbind, pseq } from "clausters/seq";\nimport { edit } from "clausters/gui";\nconst notes = await edit(await pbind({ midinote: pseq([60, 62, 64]), dur: 0.5 }).renderEvents());\nnotes.constructor.name');
  check(await waitFor('-> "NotesEditor"', at), "edit() of a rendered pattern opens the notes editor");
  await sleep(500);
  at = await run('const docked = await host.dockCall({ verb: "layout" });\ndocked.members.filter((id) => id < 2 ** 24).length');
  check(await waitFor("-> 2", at), "an application's window is a pane of the dock, and the session's dock calls are answered to it");


  // **One engine**: the page's engine is its host's applications' engine,
  // what the session's synths and the applications both play on.
  const one = await evaluate(`(async () => {
    const p = await import("./clausters/dist/page.js");
    const engine = await p.server();
    const gui = await p.guiHost();
    return engine === gui.engine && engine === await p.applicationsEngine(gui);
  })()`);
  check(one === true, "the page holds one engine: the host's applications' engine");

  // An infinite loop: the worker is terminated and the session started again.
  at = await run("for (;;) {}");
  await sleep(500);
  await interrupt();
  check(await waitFor("could not stop it, so it was started again", at), "Ctrl+. stops an infinite loop");
  check(await waitFor("JavaScript worker ready", at, 15000), "and a new session starts");
  await sleep(1500);
  check((await sessionPanes()).length === 0, "the old session's windows left the dock");
  at = await run("typeof greeting");
  check(await waitFor('-> "undefined"', at), "the stuck session's names are gone");

  // Restart clears every name.
  at = await run("const kept = 1;\nkept");
  await waitFor("-> 1", at);
  await host(`host.verb(${BAR}, "restart")`);
  check(await waitFor("JavaScript worker ready", at, 15000), "Restart starts a new session");
  await sleep(1500);
  at = await run("typeof kept");
  check(await waitFor('-> "undefined"', at), "Restart clears every name");

  // Completions from the TypeScript language service: the one that fits is
  // taken.
  await host(`host.set(${CODE}, { focus: 1 })`);
  await key("a", "ctrl");
  await host(`host.typeText(${FILE}, 'import { Session } from "clausters";\\nSession.li')`);
  let text = "";
  for (let i = 0; i < 30 && !text.includes("Session.live"); i++) {
    await key("space", "ctrl");
    await sleep(500);
    await key("Enter");
    text = await host(`return String((await host.query(${CODE})).props.text)`);
    // a list not up yet took no Enter: the line it broke is joined again
    if (!text.includes("Session.live")) await key("Backspace");
  }
  check(text.includes("Session.live"), "completions come from the package's types");

  // A paste is one edit of the browser's text: Ctrl+V in the editor puts
  // the clipboard in whole, its line breaks kept, and none of its
  // characters reaches the keys under the Control still held, where an `n`
  // was a new tab, an `o` the open dialog and a `+` a zoom.
  {
    const pasted = 'import { Session } from "clausters";\nconst one = 1 + 2;\nconsole.log("now", one);\n';
    const panes = () => host(`return (await host.dockCall({ verb: "layout" })).members.length`);
    const before = await panes();
    await host(`host.set(${CODE}, { focus: 1, text: "" })`);
    await page.click(300, 300);
    await page.paste(pasted);
    await sleep(800);
    text = await host(`return String((await host.query(${CODE})).props.text)`);
    check(text === pasted, "Ctrl+V pastes the browser's clipboard into the editor, whole");
    check((await panes()) === before, "and none of its characters is a chord: no tab or dialog opens");
  }

  // Saving into the page's storage, in the editor's own file dialog.
  await key("a", "ctrl");
  await host(`host.typeText(${FILE}, "// a sketch\\n1 + 1\\n")`);
  await key("s", "ctrl");
  await sleep(500);
  await host(`host.set(${NAME}, { focus: 1 }); host.pressKey(${BAR}, "a", "ctrl"); host.typeText(${BAR}, "/sketches/one.mjs");`);
  await sleep(300);
  // past Cancel to Save, and pressed
  await host(`host.pressKey(${BAR}, "Tab"); host.pressKey(${BAR}, "Tab"); host.pressKey(${BAR}, "space");`);
  await sleep(1000);
  const saved = await evaluate(`(async () => {
    try {
      const root = await navigator.storage.getDirectory();
      const dir = await root.getDirectoryHandle("sketches");
      return (await (await dir.getFileHandle("one.mjs")).getFile()).text();
    } catch (e) {
      return String(e);
    }
  })()`);
  check(saved === "// a sketch\n1 + 1\n", "a file is saved into the page's storage");
  const tab = await host(`return (await host.dockCall({ verb: "layout" })).panes.find((p) => p.id === ${FILE})?.title`);
  check(tab === "one.mjs", "and its tab takes the file's name");

  // **The file dialog manages the storage it shows**, as every
  // application's does: a new folder named in its field, the file selected
  // renamed and deleted (asked first), and in a page the copies to and from
  // the disk. The chooser carries each change out and lists its folder again,
  // and says how much of the page's storage is used.
  {
    const FILES = NAME - 1;
    const SAYS = NAME + 8;
    const press = (name, mods = "") => host(`host.pressKey(${BAR}, ${JSON.stringify(name)}, ${JSON.stringify(mods)})`);
    const says = async () => String((await host(`return await host.query(${SAYS})`)).props.text ?? "");
    const sketches = () =>
      evaluate(`(async () => {
        const dir = await (await navigator.storage.getDirectory()).getDirectoryHandle("sketches");
        const out = [];
        for await (const [name, handle] of dir.entries()) out.push(name + (handle.kind === "directory" ? "/" : ""));
        return out.sort().join(" ");
      })()`);
    /** Selects the row `n` rows down in the folder shown, by the keys. */
    const select = async (n) => {
      await host(`host.set(${FILES}, { focus: 1 })`);
      for (let i = 0; i < n; i++) await press("Down");
      await sleep(200);
    };
    /** Types `text` in the field and presses the tool `back` stops before
     *  it -- with the browser's own key where the tool needs the reader's
     *  gesture (a file picker). */
    const tool = async (text, back, gesture = false) => {
      await host(`host.set(${NAME}, { focus: 1 })`);
      if (text !== null) {
        await press("a", "ctrl");
        await host(`host.typeText(${BAR}, ${JSON.stringify(text)})`);
      }
      for (let i = 0; i < back; i++) await press("Tab", "shift");
      if (gesture) await page.press(" ", "Space");
      else await press("space");
      await sleep(800);
    };
    const disk = mkdtempSync(join(tmpdir(), "editor-page-disk-"));
    // it opens in the folder of the file in the tab, /sketches
    await key("o", "ctrl");
    await sleep(1000);
    const chooser = (await host(`return await host.query(${FILES})`)).props;
    check(chooser.quota > 0 && chooser.used > 0, "the dialog says how much of the page's storage is used");
    await tool("kept", 3);
    check((await sketches()) === "kept/ one.mjs", "New folder makes the folder named in the field");
    await select(3); // .., kept/, one.mjs
    await tool("two.mjs", 2);
    check((await sketches()) === "kept/ two.mjs", "Rename gives the file selected the name typed");
    if (page.downloadInto) {
      await page.downloadInto(disk);
      await select(3);
      await tool(null, 4);
      await sleep(1000);
      let got = "";
      try {
        got = readFileSync(join(disk, "two.mjs"), "utf8");
      } catch {}
      check(got === "// a sketch\n1 + 1\n", "Download hands the file selected to the browser");
    }
    if (page.chooseNext) {
      writeFileSync(join(disk, "brought.mjs"), "// from the disk\n");
      await page.chooseNext(join(disk, "brought.mjs"));
      await tool(null, 5, true);
      await sleep(1000);
      check((await sketches()) === "brought.mjs kept/ two.mjs", "Upload copies a file from the disk into the folder shown");
    }
    await select(page.chooseNext ? 4 : 3);
    await tool(null, 1);
    check((await says()).startsWith("Delete /sketches/two.mjs?"), "Delete asks first");
    await press("space");
    await sleep(800);
    check(!(await sketches()).includes("two.mjs"), "and deletes the file selected once asked again");
    await press("Escape");
    rmSync(disk, { recursive: true, force: true });
  }

  // The documentation: the editor's book and the package's.
  const documents = await host(`return (await host.query(${DOCS})).props.documents`);
  check(documents > 3, "the documentation holds the editor's book and the package's");

  // An application the bar opens is on a page of its own, where the bar
  // shows its menu: the session's tools, entries of the editor's menu, go
  // with the editor's menu, and are there again on the editor's page.
  {
    const interrupt = () => host(`return (await host.query(${BAR + 4})).props.shown`);
    const shownTool = async (want) => {
      for (let i = 0; i < 30; i++) {
        if (Boolean(await interrupt()) === want) return true;
        await sleep(100);
      }
      return false;
    };
    await host(`host.verb(${BAR}, "app:notes")`);
    check(await shownTool(false), "the session's tools are not on an application's page");
    await host(`await host.dockCall({ verb: "page", name: "Code" })`);
    check(await shownTool(true), "and are back on the editor's page");
  }

  console.log(failures ? `${failures} failed` : "all passed");
  await finish(failures ? 1 : 0);
} catch (error) {
  console.error(error);
  await finish(1);
}
