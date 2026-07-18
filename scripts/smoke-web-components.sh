#!/usr/bin/env bash
# The web-components acceptance: a standalone bundle as one HTML element
# (<clausters-bundle>) over the per-page singletons — element up with the
# canvas adopted into its shadow DOM, and the raw server() surface sharing the
# element's namespace (/status counts the bundle's synth, the meter bus
# streams moving values). clients/web/demo.html?smoke=1 does the asserting;
# the verdict is beaconed as a fetch and read from the HTTP access log (the
# same real-time posture as the other web smokes).
set -euo pipefail
cd "$(dirname "$0")/.."

CHROME="${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser)}"
PORT="${PORT:-8141}"

(cd clients/web && ./build.sh release)

# The same demo bundle the standalone smoke uses, staged for this package.
BUNDLE=clients/web/bundle-demo
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

cd clients/web
LOG=$(mktemp)
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>"$LOG" &
SERVER=$!
CHROME_PID=""
trap 'kill $SERVER $CHROME_PID 2>/dev/null' EXIT
sleep 0.5

"$CHROME" --headless=new --disable-gpu --no-sandbox \
    --autoplay-policy=no-user-gesture-required \
    --user-data-dir="$(mktemp -d)" \
    "http://127.0.0.1:$PORT/demo.html?smoke=1" >/dev/null 2>&1 &
CHROME_PID=$!

verdict=""
for _ in $(seq 1 180); do   # up to 90 s
    verdict=$(grep -o 'smoke-verdict-[^ "]*' "$LOG" | head -1 || true)
    [ -n "$verdict" ] && break
    sleep 0.5
done

if [ -z "$verdict" ]; then
    echo "components smoke FAILED: no verdict within 90 s" >&2
    exit 1
fi

decoded=$(printf '%s' "${verdict#smoke-verdict-}" | python3 -c \
    'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read()))')
echo "$decoded"
case "$decoded" in PASS*) ;; *) echo "components smoke FAILED" >&2; exit 1;; esac
