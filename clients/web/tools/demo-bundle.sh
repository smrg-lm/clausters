#!/usr/bin/env bash
# Writes the demo bundle (clients/web/bundle-demo/) in the native persisted
# formats — the exact files `clausters-gui --standalone` reads, plus the
# generated bundle.json manifest: a SynthDef spec whose drone also writes a
# 0.5 Hz LFO to control bus 0 (OutCtl), and a GuiDef whose meter/scope read
# that bus and whose boot /synth_new brings the drone up. Shared by the
# standalone and web-components smokes, and by the manual demo pages
# (examples/components/demo.html, examples/panels/standalone.html).
set -euo pipefail
cd "$(dirname "$0")/.."

BUNDLE=bundle-demo
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
 "boot":[["/synth_new","web_drone",1000,0,0]],
 "children":[
  {"id":10,"type":"knob","label":"freq","min":80.0,"max":600.0,"value":220.0,
   "bind":["/node_set",1000,"freq"]},
  {"id":11,"type":"meter","bus":0,"rate":"control","min":-1.0,"max":1.0,"label":"lfo"},
  {"id":12,"type":"scope","bus":0,"rate":"control","min":-1.0,"max":1.0,"label":"lfo"}
 ]}}
EOF
python3 tools/bundle-manifest.py "$BUNDLE"
