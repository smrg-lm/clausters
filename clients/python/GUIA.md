# Guía de smoke manual (cliente Python + GUI)

Checklist **estable** de las verificaciones que necesitan **oído u ojo humano**:
oír que el cliente suene (en vivo, embebido u offline reproducido) y **ver** que
las ventanas del host GUI dibujen sus widgets. No es un registro por milestone y
no crece con cada feature nueva.

- **Instalar y correr**: ver `clients/python/README.md` (venv limpio, `pip
  install -e`, que compila los cdylibs + el binario del servidor + el de
  `clausters-gui` y los empaqueta). Esta guía asume el cliente instalado.
- **Lo verificable con números** — RMS, picos, `rendered N frames | peak …`,
  paridad bit-a-bit con el servidor, reproducibilidad de semillas, conteos — lo
  cubren las suites automáticas y son la fuente de verdad: `pytest` en
  `clients/python/tests`, los tests de `clients/gui` (incl. E2E headless del
  protocolo `/gui_*`), y los del workspace Rust. Si algo aquí falla al oído/ojo
  pero los tests pasan, el problema está en el audio o el display del sistema.

Ejemplos en `clients/python/examples/` (catálogo en el mdBook del cliente,
`examples.md`).

## Smoke audible (cliente)

- **Suena embebido, sin arrancar nada.** `Session.embed(...)` toca en proceso →
  se oye el patrón (`examples/embedded.py`).
- **Suena en vivo por UDP.** `Session.live(...)` / `examples/live_patch.py` /
  `examples/sequencing.py` contra un servidor corriendo → se oyen los eventos en
  el beat, sin deriva.
- **Offline reproducido.** Un `render()` NRT escribe un WAV que al reproducirlo
  suena como el patrón (los golden aseguran las muestras; el oído confirma el
  archivo).
- **Faust en vivo.** `examples/faust_soundfile.py` → un def Faust lee un buffer y
  se oye.
- **MIDI.** `examples/midi_file.py` escribe un `.mid` que reproduce en cualquier
  player; `examples/midi_live.py` toca el servidor por un puerto ALSA-seq
  (`aconnect`).
- **Responders (hub OSC/MIDI).** `examples/osc_responder.py` /
  `examples/midi_responder.py` → tocar un teclado dispara voces en el servidor.
- **Transporte estilo DAW.** `examples/timeline_transport.py` y
  `examples/transport_conductor.py` → un director mueve play/stop/locate y varios
  playheads siguen en lockstep (audible juntos).

## Smoke visual (host GUI)

Arrancar el host en modo ventana (`clausters-gui`, o el propio ejemplo lo
lanza) y correr el script driver en otra terminal, mismo host. Ver que la
ventana **dibuje y responda**:

- **Ventana y widgets base.** `examples/gui_window.py` / `gui_panel.py` → abre
  una ventana; los controles responden a `/gui_set` en vivo y emiten eventos.
- **Bindings.** `examples/gui_bind.py` → mover una perilla cambia la `freq` de un
  synth sin pasar por el script.
- **Meters y scope por memoria compartida.** `examples/gui_meters.py` /
  `gui_scope.py` → el meter/osciloscopio sigue el bus/tap en vivo (~30 fps), con
  trigger estable.
- **Phasescope + espectro.** `examples/gui_analyzer.py` → el goniómetro barre
  mono→wide→antifase y el espectro sigue la fuente.
- **Árbol de nodos en vivo.** `examples/gui_nodetree.py` → crear/mover nodos se
  refleja en la vista del árbol.
- **Canvas con shader WGSL.** `examples/gui_canvas.py` → la canvas anima barriendo
  un parámetro (OSC) y leyendo un bus (shm).
- **Waveform/espectrograma de nivel editor.** `examples/gui_editor.py` →
  multicanal, selección/playhead, crossfade de LOD al hacer zoom.
- **Vistas enlazadas.** `examples/gui_linked.py` → waveform y espectrograma con
  `link=1` navegan en bloque: zoom/pan/selección en cualquiera mueve ambos, el
  script ve un solo stream de eventos, y cambiar `link` en vivo saca/reincorpora
  una vista (al desvincular conserva su vista y diverge).
- **Reglas configurables.** `examples/gui_rulers.py` → unidades por eje en franjas
  laterales (tiempo, dB, escalas de frecuencia). Zoom vertical: la rueda sobre la
  franja del eje y acerca (las etiquetas se refinan sin chocar), arrastrar la
  franja panea, `R` resetea; el script también lo mueve con `y_start`/`y_len`.
- **Bulk data.** `examples/gui_bulk.py` → un buffer grande se transfiere por
  recurso compartido / fetch y se muestra.
- **Bundle standalone.** `examples/gui_standalone.py` autorea un bundle (GuiDef +
  GraphDefs); lanzarlo con `--standalone <name>` abre un instrumento
  autocontenido, sin cliente de lenguaje, que suena y responde.

### En el browser (WebGPU / WebGL2)

Generar el bundle wasm (`clients/gui`, `wasm-pack` + `wasm-bindgen` a `web/`),
servirlo con un `http.server` (WebGPU necesita contexto seguro; localhost vale)
y abrirlo en un navegador con WebGPU (fallback WebGL2 donde esté deshabilitado):

- La página dibuja los mismos widgets que el nativo ("connect", "meters",
  "bulk").
- Con un servidor `--ws` y `?server=ws://127.0.0.1:57120`, los bindings y los
  meters/scopes siguen los buses en vivo; `parity.html` loguea la paridad
  nativo/browser por consola.
