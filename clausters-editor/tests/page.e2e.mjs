// The editor as a page, driven as a hand drives it: headless Chrome over the
// DevTools protocol types into the editor, presses its keys and reads the post
// window. What it checks is the JavaScript session's behaviour as the editor's
// documentation describes it (docs/page.md), against the real clausters
// engine and GUI host the page brings up.
//
//   npm run build:web && npm run test:page -- [chrome|firefox]
//
// Chrome is driven over the DevTools protocol, Firefox over WebDriver BiDi.
// Needs a built clausters web package (clients/web, ./build.sh; or
// CLAUSTERS_WEB) and the browser (CHROME or FIREFOX, else on the PATH).

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
const server = spawn(process.execPath, [join(here, "scripts", "serve-web.mjs"), String(PORT)], { stdio: "inherit" });
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
  socket.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && waiting.has(msg.id)) {
      waiting.get(msg.id)(msg);
      waiting.delete(msg.id);
    }
  });
  return (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      waiting.set(id, (msg) =>
        msg.error ? reject(new Error(`${method}: ${msg.error.message ?? msg.message ?? msg.error}`)) : resolve(msg.result),
      );
      socket.send(JSON.stringify({ id, method, params }));
    });
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
  const press = async (type, key, code, keyCode, modifiers) =>
    send("Input.dispatchKeyEvent", { type, key, code, windowsVirtualKeyCode: keyCode, modifiers });
  return {
    evaluate: async (expression) => {
      const { result, exceptionDetails } = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
      return result.value;
    },
    key: async ({ key, code, keyCode }, { shift = false, ctrl = false } = {}) => {
      const modifiers = (shift ? 8 : 0) | (ctrl ? 2 : 0);
      await press("rawKeyDown", key, code, keyCode, modifiers);
      await press("keyUp", key, code, keyCode, modifiers);
    },
    insertText: (text) => send("Input.insertText", { text }),
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
  const CODES = { Enter: "\uE007", Escape: "\uE00C", " ": " " };
  return {
    evaluate,
    key: async ({ key }, { shift = false, ctrl = false } = {}) => {
      const value = CODES[key] ?? key;
      const down = [...(ctrl ? ["\uE009"] : []), ...(shift ? ["\uE008"] : []), value];
      await send("input.performActions", {
        context,
        actions: [
          {
            type: "key",
            id: "keyboard",
            actions: [...down.map((v) => ({ type: "keyDown", value: v })), ...down.reverse().map((v) => ({ type: "keyUp", value: v }))],
          },
        ],
      });
    },
    // Text as the browser's own insertion, which the editor reads as typing.
    insertText: (text) => evaluate(`document.execCommand("insertText", false, ${JSON.stringify(text)})`),
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

try {
  const page = BROWSER === "firefox" ? await firefox() : await chrome();
  const evaluate = page.evaluate;
  const post = () => evaluate(`document.querySelector("pre")?.innerText ?? ""`);
  /** Waits until the post window has `text` after `from` characters. */
  const waitFor = async (text, from = 0, ms = 15000) => {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      const now = await post();
      if (now.slice(from).includes(text)) return true;
      await sleep(100);
    }
    return false;
  };
  const KEYS = {
    a: { key: "a", code: "KeyA", keyCode: 65 },
    s: { key: "s", code: "KeyS", keyCode: 83 },
    ".": { key: ".", code: "Period", keyCode: 190 },
    " ": { key: " ", code: "Space", keyCode: 32 },
    Enter: { key: "Enter", code: "Enter", keyCode: 13 },
    Escape: { key: "Escape", code: "Escape", keyCode: 27 },
  };
  const key = (name, modifiers) => page.key(KEYS[name], modifiers);
  /** Replaces the document with `code` and evaluates all of it. */
  const run = async (code) => {
    await evaluate(`document.querySelector(".cm-content").focus()`);
    await key("a", { ctrl: true });
    await page.insertText(code);
    await key("a", { ctrl: true });
    const from = (await post()).length;
    await key("Enter", { shift: true });
    return from;
  };
  const interrupt = async () => {
    await evaluate(`document.querySelector(".cm-content").focus()`);
    await key(".", { ctrl: true });
  };

  // Until a key or a click the page may start no audio, so the session waits,
  // and an evaluation asked for meanwhile runs once it has started, after the
  // startup code.
  check(await waitFor("starts at the first key or click", 0, 30000), "the session waits for the page's first gesture");
  let at = await run("1 + 1");
  check(await waitFor("JavaScript worker ready", 0, 30000), "the session starts in a worker at the first key");
  check(await waitFor("-> 2", at, 15000), "the evaluation asked for meanwhile runs");
  const order = await post();
  check(order.indexOf("Startup code") >= 0 && order.indexOf("Startup code") < order.indexOf("-> 2"), "after the startup code");
  check(!order.includes("-> GuiHost"), "the default startup code shows no value");
  await sleep(1000);

  at = await run('const greeting = (name) => `Hello, ${name}`;\ngreeting("world")');
  check(await waitFor('-> "Hello, world"', at), "a value is shown after ->");

  at = await run('const greeting = 1;\ngreeting + 1');
  check(await waitFor("-> 2", at), "declaring a name again redefines it");

  at = await run('await new Promise((r) => setTimeout(r, 300));\n"half a second later"');
  check(await waitFor('-> "half a second later"', at), "await works at the top level");

  at = await run('import { Session } from "clausters/session";\ntypeof Session');
  check(await waitFor('-> "function"', at), "import works, from the clausters package");

  at = await run('setTimeout(() => console.log("from a timer"), 200);\n"scheduled"');
  check(await waitFor("from a timer", at), "output after the evaluation ended reaches the post window");

  at = await run("null.x");
  check(await waitFor("TypeError", at), "an error is shown");

  // Sound: the startup code's session, on the page's engine.
  at = await run('import { sine } from "clausters/defs";\nimport { play } from "clausters/session";\nawait play(sine(660).mul(0.05));\nawait new Promise((r) => setTimeout(r, 300));\n(await session.server.status()).synths');
  check(await waitFor("-> 1", at), "a synth plays on the page's engine");

  // A note played now sounds now: the engine's axis follows the wall clock
  // across the time its audio waited for the first key.
  at = await run('import { play } from "clausters/session";\nconst t0 = performance.now();\nplay({ freq: 440, amp: 0 });\nwhile ((await session.server.status()).synths === 0) await new Promise((r) => setTimeout(r, 5));\nperformance.now() - t0 < 200');
  check(await waitFor("-> true", at), "a note played now sounds within the latency");

  // A routine, written as the Python client's.
  at = await run('import { Routine } from "clausters/session";\nvar notes = 0;\nfunction* test() {\n  for (i = 0; i < 4; i++) {\n    play({ freq: 440, amp: 0 });\n    notes++;\n    yield 0.25;\n  }\n}\nr = new Routine(test);\nr.play();\nawait new Promise((r) => setTimeout(r, 1500));\nnotes');
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

  // A window from the session's code, drawn by the page.
  at = await run('import * as gui from "clausters/gui";\nconst host = await session.gui({ port: editor.gui });\nhost.open(gui.view({ title: "level" }, gui.slider({ name: "level" })));\n"opened"');
  await waitFor('-> "opened"', at);
  await sleep(500);
  check(await evaluate(`!!document.querySelector(".session-window canvas")`), "a window the session opens is drawn on the page");

  // An infinite loop: the worker is terminated and the session started again.
  at = await run("for (;;) {}");
  await sleep(500);
  await interrupt();
  check(await waitFor("could not stop it, so it was started again", at), "Ctrl+. stops an infinite loop");
  check(await waitFor("JavaScript worker ready", at, 15000), "and a new session starts");
  check(await evaluate(`!document.querySelector(".session-window canvas")`), "the old session's windows leave the page");
  await sleep(1500);
  at = await run("typeof greeting");
  check(await waitFor('-> "undefined"', at), "the stuck session's names are gone");

  // Restart clears every name.
  at = await run("const kept = 1;\nkept");
  await waitFor("-> 1", at);
  await evaluate(`document.querySelector('[aria-label="Restart the session"]').click()`);
  check(await waitFor("JavaScript worker ready", at, 15000), "Restart starts a new session");
  await sleep(1500);
  at = await run("typeof kept");
  check(await waitFor('-> "undefined"', at), "Restart clears every name");

  // Completions from the TypeScript language service.
  await evaluate(`document.querySelector(".cm-content").focus()`);
  await key("a", { ctrl: true });
  await page.insertText('import { Session } from "clausters/session";\nSession.');
  await key(" ", { ctrl: true });
  let options = "";
  for (let i = 0; i < 100 && !options.includes("live"); i++) {
    await sleep(200);
    options = await evaluate(`document.querySelector(".cm-tooltip-autocomplete")?.innerText ?? ""`);
  }
  check(options.includes("live") && options.includes("nrt"), "completions come from the package's types");
  await key("Escape");

  // Saving into the page's storage, and opening it back.
  await evaluate(`document.querySelector(".cm-content").focus()`);
  await key("a", { ctrl: true });
  await page.insertText("// a sketch\n1 + 1\n");
  await key("s", { ctrl: true });
  await sleep(500);
  await evaluate(`(() => {
    const input = document.querySelector('[aria-label="File name"]');
    input.value = "/sketches/one.mjs";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await sleep(100);
  await evaluate(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Save").click()`);
  await sleep(800);
  const saved = await evaluate(`(async () => {
    const root = await navigator.storage.getDirectory();
    const dir = await root.getDirectoryHandle("sketches");
    return (await (await dir.getFileHandle("one.mjs")).getFile()).text();
  })()`);
  check(saved === "// a sketch\n1 + 1\n", "a file is saved into the page's storage");

  // The documentation: the editor's book and the package's.
  const books = await evaluate(`document.body.innerText`);
  check(/clausters web client/i.test(books) || /clausters-editor/i.test(books), "the documentation viewer lists the books");

  console.log(failures ? `${failures} failed` : "all passed");
  await finish(failures ? 1 : 0);
} catch (error) {
  console.error(error);
  await finish(1);
}
