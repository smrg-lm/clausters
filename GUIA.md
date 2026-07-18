# Guía de smoke manual (servidor)

Checklist **estable** de las verificaciones que necesitan **oído u ojo humano**
y que la suite automática no puede hacer: escuchar que algo suene bien, oír una
pausa/reanudación, ver un contador cambiar en vivo. No es un registro por
milestone y no crece con cada feature nueva.

- **Compilar y correr el servidor**: ver `BUILD.md` (matriz de features, flags
  de arranque, dependencias del sistema). Esta guía asume un servidor ya
  compilado (`./target/release/clausters`).
- **Lo verificable con números** — RMS, picos, payloads de `/server_info.reply`,
  paridad bit-a-bit, no-alloc en el hilo de audio, conteos — lo cubren las
  suites automáticas y son la fuente de verdad: `cargo test`
  (que ya cubre las dos familias de defs; `--no-default-features` prueba el
  build sin libfaust), los golden de
  `tests/golden.rs`, y `clients/python/tests` (`pytest`). Si algo aquí falla al
  oído pero los tests pasan, el problema está en el audio del sistema, no en el
  servidor (ver "Problemas frecuentes").

Los ejemplos referidos están en `examples/` (catálogo en `docs/examples.md`).

## Smoke audible/visual

Arrancar el servidor en una terminal y correr el ejemplo en otra (mismo host).

- **Suena un synth y responde en vivo.** `examples/live_patch.py` crea un
  `default`, cambia `freq`/`amp` sobre la marcha → se oye el tono y su vibrato/
  glissando. Alternativa a mano con `examples/osc_ping` (`/s_new`, `/n_set`).
- **Buffers y PlayBuf.** Cargar un WAV y reproducirlo → se oye el sample; con
  velocidad distinta cambia el pitch/duración.
- **Streaming de disco (DiskIn/DiskOut).** Se oye la reproducción desde disco y,
  grabando, el WAV resultante reproduce lo emitido.
- **Reloj de samples en el tiempo.** `examples/sample_clock.py` /
  `examples/sequencing.py` → los synths caen **en el beat**, sin deriva audible
  en sesiones largas.
- **Grupos y orden de ejecución.** `examples/group_set.py` → un `/n_set` sobre el
  grupo alcanza a todas las voces; el orden `Out`/`ReplaceOut` es audible.
- **Polifonía por GraphDef.** `examples/graphdef_poly.py` → cuatro voces
  solapadas suenan a la vez.
- **Passthrough de entrada.** Servidor con una entrada de hardware
  (micro/loopback) y un synth `In(...) -> Out(0)` → se oye la entrada pasar
  directa unos segundos.
- **MIDI estándar.** `examples/midi_live.py` (o tocar un puerto ALSA-seq con
  `aconnect`) → las notas disparan voces; aftertouch/CC/bend modulan las voces
  vivas.
- **Cadena espectral (FFT/PV_*/IFFT).** Un def de FFT→PV_*→IFFT sobre ruido → se
  oye el ruido filtrado (p. ej. low-pass); `/u_cmd` cambia la ventana en vivo.
- **Síntesis cruzada y freeze espectral (dos cadenas).**
  `clients/python/examples/spectral_cross.py` → a la derecha se oye el ruido
  vistiendo la envolvente espectral de la melodía (izquierda, seca); en los
  últimos beats el `freeze` congela el acorde final como textura sostenida.
- **Op espectral escrita por el usuario (expresión de bins).**
  `clients/python/examples/spectral_kernel.py` → a la izquierda el ruido crudo,
  a la derecha el gate espectral inclinado (el umbral sube con la frecuencia):
  queda un residuo oscuro y ralo — una operación que no existe en el catálogo,
  escrita como expresión con `pv_kernel`.
- **Convolución particionada (reverb).**
  `clients/python/examples/convolution.py` → a la izquierda el pluck seco, a la
  derecha la cola de reverberación convolucionada (IR de ruido decayente
  preparada con `/b_gen prepare_partconv`); la cola sigue sonando tras cada
  nota.
- **Envolventes (EnvGen).** Un synth con `gate`/pausa → se oye el corte y la
  reanudación (la suite mide `beat RMS ~0.141` on / `0.000` en pausa).
- **Editor multipista (GUI).** `examples/gui_multitrack.py` abre una ventana con
  tres pistas de clips alineados sobre un eje de tiempo compartido → se ven los
  rectángulos de clip con su cuerpo dibujado (los dos carriles de audio mapean
  **archivos** de take: el cuerpo se decima a ancho de píxel por la pirámide de
  picos, no viaja por OSC); la pista de abajo trae **regla de tiempo** en beats y
  las tres muestran el **playhead** corriendo con el reloj del motor. Arrastrar un
  clip lo **mueve** y arrastrar un borde lo **redimensiona**, y la consola imprime
  el evento `"clip" offset dur` (edit-back). El eje es **navegable**: la rueda
  hace zoom y Shift+arrastrar panea — las tres pistas se mueven juntas (comparten
  el eje), y `r` resetea. En un clip con **notas** (cuerpo piano-roll), si la
  pista es lo bastante alta se leen los **nombres de nota** (cada C) en el borde
  izquierdo del clip — la regla de pitch compacta del roll sin teclado.
  Refrescar antes el binario bundleado
  (`clients/python/clausters/_bin/`) — ver `CLAUDE.md`.
- **Editor compositivo (el lazo completo).** `examples/gui_composer.py` compone
  con el modelo (un take bounceado y cargado de disco, una melodía, un patrón),
  lo abre como multipista y lo toca → se **oye** la composición y se **ve** el
  playhead barriendo los clips **mientras suena** (al terminar la pieza el
  transporte para y vuelve al inicio); los botones **play/pause/stop/rewind**
  manejan la reproducción, y arrastrar un clip mientras suena re-agenda la
  composición: suena donde lo soltaste. El carril **sweep** es una
  automatización: su cuerpo es la **curva**, y se edita ahí mismo (arrastrar un
  punto, Ctrl+click agrega/quita) → se **oye** el filtro seguir la curva dibujada.
- **Piano-roll editor (GUI).** `examples/gui_pianoroll.py` abre una ventana con
  la vista `pianoroll` dedicada → se ve el **teclado** a la izquierda (blancas/
  negras, la nota en cada C), la **grilla de notas** MIDI (coloreadas por
  velocity), el **carril de velocity** abajo y el **carril de eventos OSC** con
  sus banderas, más la **regla de tiempo** en beats. Arrastrar una nota la
  **mueve** en tiempo/pitch, arrastrar un borde la **redimensiona**, Ctrl+click
  **agrega/quita** una nota, arrastrar el carril de velocity fija la **velocity**,
  y Ctrl+click/arrastrar el carril OSC agrega/quita/mueve un evento — la consola
  imprime el edit-back (`"notes" …` / `"osc" …`). La rueda sobre la grilla hace
  **zoom** del eje de tiempo (Shift+arrastrar panea); la rueda sobre el **teclado**
  hace zoom del rango de **pitch** y arrastrarlo lo panea, y `r` resetea ambos
  ejes. Al mover el cursor sobre la grilla se lee el **nombre de nota + tiempo** en
  la esquina. El botón **play** toca las notas dibujadas (un `Pbind`) → se **oye**
  la melodía que dibujaste. **Selección múltiple:** arrastrar sobre grilla vacía
  marca el rectángulo tiempo × pitch → las notas adentro quedan **resaltadas**
  (la banda de selección de tiempo se sigue viendo igual que antes); Alt+click
  suma/quita una nota; arrastrar una nota seleccionada **mueve todo el bloque**
  (rígido — contra un borde se frena, no se deforma); Supr/Retroceso **borra**
  las seleccionadas; arrastrar el carril de velocity sobre una seleccionada
  mueve las velocities de todas **relativamente**; `q` **cuantiza** los starts
  de las seleccionadas (o todas) a la grilla `snap` → se ven saltar a la
  grilla y se **oye** la melodía cuadrada al tocarla; Ctrl+C/X **copia/corta**
  la selección y Ctrl+V la **pega** con la primera nota en el tiempo del
  cursor (queda seleccionada, lista para arrastrar — y viaja entre rolls y
  ventanas). Refrescar antes el binario bundleado
  (`clients/python/clausters/_bin/`) — ver `CLAUDE.md`.
- **MIDI en vivo pintando notas (GUI).** Con un roll `midi_in=True`, el host
  abre el puerto MIDI virtual **"clausters-gui"** → rutear un teclado (o
  `aconnect`/qpwgraph) y tocar: con el playhead corriendo las notas se
  **pintan donde suena** (soltar la tecla fija la duración real); con el
  transporte parado es **entrada por pasos** sobre la grilla `snap` (un acorde
  comparte el paso). Del lado cliente, la celda `record_midi()` de
  `examples/gui_pianoroll.py` hace lo mismo vía `MidiFunc` + `/gui_set` (puerto
  **"clausters-in"**). Cada nota pintada emite el edit-back `"notes"` normal.
  Refrescar antes el binario bundleado (`clients/python/clausters/_bin/`) —
  ver `CLAUDE.md`.
- **Vista dedicada del Editor (pianoroll ↔ datos).** En `gui_composer.py`, la
  celda "The melody, as a dedicated piano-roll" abre la melodía (un `Track`) en
  su propia ventana pianoroll junto al multipista → **arrastrar una nota** y
  presionar **play**: se **oye** la melodía cambiada — el roll reescribió el
  `Timeline` del arreglo, y el clip del multipista es otra vista del mismo
  elemento. Un elemento *generador* (el `Pbind` del bajo, abierto igual) se
  muestra **solo lectura**: arrastrar sus notas no cambia lo que suena (se
  bouncea a `Track` para editarlo). Refrescar antes el binario bundleado
  (`clients/python/clausters/_bin/`) — ver `CLAUDE.md`.
- **Patcher del grupo lógico (GUI).** Un `Group{logical}` se dibuja como *patch*
  (cajas de miembro, nodos de bus, un cable por `control ↔ bus`); arrastrar un
  puerto sobre otro bus lo **recablea** (sobre vacío, lo descablea) y la próxima
  render manda el `GraphDef` cableado como se ve → **cambia lo que suena**. Refrescar antes el
  binario bundleado (`clients/python/clausters/_bin/`) — ver `CLAUDE.md`.
- **Salud de tiempo real bajo carga.** `cargo run --release --example stress`
  ramea nodos: el medidor publica avg/peak y `late_blocks` en `/status.reply`;
  al sobrecargar a propósito el servidor **glitchea pero no muere** (el guard de
  SIGXCPU lo degrada a SCHED_OTHER, ver abajo).
- **El engine suena en el navegador (sin proceso servidor).**
  `crates/clausters-web/web/build.sh`, después
  `(cd crates/clausters-web/web && python3 -m http.server)` y abrir
  `http://localhost:8000/` → Power (el gesto), Sine → se oye el seno de 440 Hz
  del def `default` corriendo en el AudioWorklet; `/status` responde en el log
  de la página y el reloj de samples avanza. Versión scriptada:
  `scripts/smoke-web.sh` (Chrome headless).
- **Un bundle standalone bootea entero en una pestaña.**
  `scripts/smoke-web-standalone.sh` deja todo armado (builds + bundle demo en
  `clients/gui/web/bundle-demo`); para verlo a mano:
  `(cd clients/gui/web && python3 -m http.server)` y abrir
  `http://localhost:8000/standalone.html?bundle=bundle-demo` → Power → suena el
  drone del bundle (formato nativo de `--standalone`), el meter y el scope se
  mueven con el LFO por `/c_stream` sobre el engine in-page, y la perilla
  `freq` (bindeada `/n_set`) cambia el tono al arrastrarla. Sin ningún proceso
  servidor.

### Con la feature `faust`

- **Faust suena.** Compilar un `/d_faust` (fuente o Box API) → se oye la sierra
  suave a 220 Hz, y `/n_set freq 330` la sube.
- **`soundfile` lee un buffer del servidor.** `examples/faust_soundfile.py` → un
  def Faust lee un buffer cargado y lo reproduce.
- **Persistencia entre sesiones.** `examples/persistence.sh`: sesión 1 define un
  FaustDef y un SynthDef y sale; sesión 2 **no** reenvía el def, arranca, lo
  instancia y **suena** (recompila desde el JSON persistido / caché de bitcode).
- **MIDI-standalone.** `examples/midi_standalone.sh` → el servidor arranca con
  sus bindings persistidos y toca por MIDI sin cliente de lenguaje.

## Problemas frecuentes

- **No suena**: cpal abre el dispositivo default de ALSA; en escritorios con
  PipeWire/PulseAudio funciona vía el plugin ALSA. Verificar que algo más suene
  (`aplay -l`) y que el servidor imprima la línea de arranque.
- **stderr dice "RT CPU budget exceeded (SIGXCPU)" y el audio empeora**: es el
  watchdog de RTKit, no un bug. Al promover el thread de audio a tiempo real
  (feature `rtprio`, default) le impone `RLIMIT_RTTIME` (~200 ms de CPU
  *continua*); con carga sostenida > 100% del período (p. ej. una rampa de
  `stress` con `--limit` alto, agravado con quantum chico —
  `PIPEWIRE_QUANTUM=64/48000`) el kernel manda SIGXCPU. El guard lo captura y
  degrada el thread a SCHED_OTHER: **el servidor sigue vivo**, solo se pierde el
  scheduling RT hasta reiniciar. Mantener el corte del stress por debajo del
  100% (el default `--limit 90` existe por esto) o subir el quantum. Un build
  sin `rtprio` no corre este riesgo, a costa de la mitad de la capacidad. Si un
  servidor *muere* con "Rebasado el límite de tiempo de CPU" es un binario viejo
  (antes del guard): recompilar.
- **Glitchea con el CPU aparentemente bajo (avg < 50%)**: mirar el **peak** y
  `late_blocks` en `/status.reply` — la capacidad la fija el peor bloque, no el
  promedio (peak ≈ 2-3× avg). Si el peak revienta con avg moderado, verificar
  que el hilo de audio esté realmente en tiempo real
  (`ps -eLo comm,cls,rtprio | grep pw_out` → RR/FF): un binario sin `rtprio`,
  RTKit caído o un thread ya degradado por el guard corren como SCHED_OTHER y el
  jitter rompe el audio a ~la mitad de la capacidad. Una ráfaga grande de
  `/s_new` (cientos de nodos de una vez) también produce bloques tardíos
  puntuales — costo de inserción, no sobrecarga.
- **`cargo build` no enlaza (libfaust)**: `faust` es feature por defecto, así que
  el build normal necesita libfaust con backend LLVM. Verificar
  `ls ~/.local/lib/libfaust.so` o exportar `FAUST_PREFIX`; tras cambiarlo,
  `cargo clean -p clausters` para que build.rs lo relea. Para compilar sin
  libfaust: `cargo build --no-default-features --features synth,realtime,midi,pipewire,rtprio`.
- **`/d_faust` responde `/fail "server built without faust support"`**: ese
  servidor se compiló con `--no-default-features` (sin la familia Faust).
- **`import("stdfaust.lib")` falla**: falta la stdlib en `<prefijo>/share/faust`
  (la instala el `make install` de libfaust).
- **Puerto ocupado** (`Address already in use`): quedó otro servidor vivo;
  `osc_ping quit` o matar el proceso.
- **Los tests con feature crashean en paralelo**: hay un lock global de
  compilación precisamente por esto; si se llama a la FFI de libfaust desde
  código propio, toda compilación debe pasar por `faust::compiler::ffi_lock()`
  — libfaust no tolera compilaciones concurrentes en un proceso.
