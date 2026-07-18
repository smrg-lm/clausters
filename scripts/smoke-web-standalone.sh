#!/usr/bin/env bash
# The standalone-in-a-tab acceptance: a native-format bundle (the exact files
# `clausters-gui --standalone` reads) boots entirely in a browser tab — the
# engine in an AudioWorklet, the GUI host on a canvas, no server process —
# with its meter live over /c_stream. web/standalone.html?smoke=1 does the
# asserting; the verdict is beaconed as a fetch of /smoke-verdict-… and read
# from the HTTP access log (real-time audio: no --virtual-time-budget, same
# posture as scripts/smoke-web.sh).
#
# The demo bundle is written here, by hand, in the persisted formats: a
# SynthDef spec whose drone also writes a 0.5 Hz LFO to control bus 0
# (OutCtl), and a GuiDef whose meter/scope read that bus and whose boot
# /s_new brings the drone up. bundle-manifest.py adds the one browser-only
# file (bundle.json). Requires wasm-bindgen-cli and Chrome/Chromium.
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8140}"

# Build both wasm bundles; build.sh stages the engine into web/engine/.
(cd clients/gui && ./web/build.sh release)

# The demo bundle, in the native persisted formats.
BUNDLE=clients/gui/web/bundle-demo
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/defs/synthdefs" "$BUNDLE/defs/guidefs"
cat > "$BUNDLE/defs/synthdefs/web_drone.json" << 'EOF'
{"name":"web_drone","controls":[{"name":"freq","default":220.0}],
 "ugens":[
  {"kind":"Sine","inputs":[{"control":0}]},
  {"kind":"Mul","inputs":[{"ugen":0},{"const":0.15}]},
  {"kind":"Out","inputs":[{"const":0.0},{"ugen":1}]},
  {"kind":"Out","inputs":[{"const":1.0},{"ugen":1}]},
  {"kind":"Sine","inputs":[{"const":0.5}]},
  {"kind":"OutCtl","inputs":[{"const":0.0},{"ugen":4}]}
 ]}
EOF
cat > "$BUNDLE/defs/guidefs/webdrone.json" << 'EOF'
{"id":1,"gui":{"type":"window","title":"Web standalone drone","w":480,"h":360,
 "layout":"col","name":"webdrone",
 "boot":[["/s_new","web_drone",1000,0,0]],
 "children":[
  {"id":10,"type":"knob","label":"freq","min":80.0,"max":600.0,"value":220.0,
   "bind":["/n_set",1000,"freq"]},
  {"id":11,"type":"meter","bus":0,"min":-1.0,"max":1.0,"label":"lfo"},
  {"id":12,"type":"scope","bus":0,"min":-1.0,"max":1.0,"label":"lfo"}
 ]}}
EOF
python3 clients/gui/web/bundle-manifest.py "$BUNDLE"

cd clients/gui/web
LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
trap 'kill $SERVER $CHROME_PID 2>/dev/null' EXIT
sleep 0.5

"$CHROME" --headless=new --disable-gpu --no-sandbox \
    --autoplay-policy=no-user-gesture-required \
    --user-data-dir="$(mktemp -d)" \
    "http://127.0.0.1:$PORT/standalone.html?smoke=1&bundle=bundle-demo" \
    >/dev/null 2>&1 &
CHROME_PID=$!

verdict=""
for _ in $(seq 1 180); do   # up to 90 s (two wasm bundles compile/instantiate)
    verdict=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    sleep 0.5
done

if [ -z "$verdict" ]; then
    echo "standalone smoke FAILED: no verdict within 90 s" >&2
    exit 1
fi

decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "standalone smoke FAILED" >&2; exit 1;; esac
