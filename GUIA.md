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
- **Piano virtual (GUI).** `examples/gui_piano.py` abre una ventana con el
  widget `piano` → se ve un **teclado ejecutable** con las proporciones de un
  piano real (blancas iguales; negras más angostas y cortas, no centradas en
  los límites), que se **redimensiona** con la ventana sin deformarse, con la
  **franja overview** arriba (las 128 notas MIDI con la ventana visible
  marcada) y las teclas fuera del rango 21–108 **grisadas** (inertes al click).
  Click en una tecla → se **oye** una voz del servidor (más fuerte cuanto más
  cerca del borde frontal se toca); arrastrar por varias teclas hace
  **glissando** (la consola imprime los eventos `"note" pitch vel state ch`).
  **Arrastrar la franja** panea el rango visible, la **rueda sobre la franja**
  hace zoom (anclado al cursor), la **rueda sobre las teclas** panea de a
  blancas — la consola imprime `"range" min max`. Con `voice=` en el builder
  (la alternativa comentada) el host toca las voces solo, sin script en el
  medio — lo mismo que el bundle web `clients/web/examples/piano`.
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
  `clients/web/build.sh`, después
  `(cd clients/web && python3 -m http.server)` y abrir
  `http://localhost:8000/examples/engine.html` → Power (el gesto), Sine → se
  oye el seno de 440 Hz del def `default` corriendo en el AudioWorklet;
  `/status` responde en el log de la página y el reloj de samples avanza.
  Versión scriptada: `scripts/smoke-web.sh` (Chrome headless).
- **El bundle como componente web (paquete `clausters`).**
  `scripts/smoke-web-components.sh` deja `clients/web` staged con su
  `bundle-demo`; a mano: `(cd clients/web && python3 -m http.server)` y abrir
  `http://localhost:8000/examples/demo.html` → el botón power del elemento
  `<clausters-bundle>` es el gesto → suena el drone y el canvas del GUI queda
  dentro del elemento; los botones `/status` y `freq +50` hablan por el
  singleton crudo con el mismo engine (el `/n_set` sube el tono del synth que
  booteó el elemento — un solo namespace).
- **Un bundle standalone bootea entero en una pestaña.**
  `scripts/smoke-web-standalone.sh` deja todo armado (builds + bundle demo en
  `clients/web/bundle-demo`); para verlo a mano:
  `(cd clients/web && python3 -m http.server)` y abrir
  `http://localhost:8000/examples/standalone.html` → Power → suena el
  drone del bundle (formato nativo de `--standalone`), el meter y el scope se
  mueven con el LFO por `/c_stream` sobre el engine in-page, y la perilla
  `freq` (bindeada `/n_set`) cambia el tono al arrastrarla. Sin ningún proceso
  servidor.
- **El shell de aplicación (props de layout).** `examples/gui_shell.py` abre
  una "aplicación": barra de menú de altura fija arriba (menú + play/stop),
  área de trabajo elástica (sidebar de ancho fijo con knob/slider + scope que
  se estira), barra de estado abajo. Redimensionar la ventana → las barras
  conservan su altura y solo el área central crece; play → suena la voz suave
  y el osciloscopio dibuja la salida estéreo real (taps); mover el knob/slider
  → se oye el cambio, la barra de estado dice `freq ... Hz` / `amp ...` en
  cada gesto y la onda del scope cambia con él.
- **El workspace 2D (`scroll`) y sus formas acotadas.** `examples/gui_workspace.py`
  abre una ventana con tres paneles del *mismo* widget configurado distinto: el
  **plano libre** arriba (9 cajas dispersas en un área virtual de 1600x1200) →
  arrastrar el fondo vacío desplaza en **ambos ejes**, la rueda hace zoom
  **anclado en el cursor** (el punto bajo el puntero no se mueve) y las cajas
  que salen del panel se **recortan** en el borde, no invaden al vecino; el
  **scroll vertical** en el medio (`axis="y"`, `zoom=0`) → la rueda desplaza
  hacia abajo y la x nunca se mueve ni cambia la escala; la **tira horizontal**
  abajo (`axis="x"`) → la rueda recorre los compases y frena al final del
  contenido. Cada gesto imprime `view x= y= zoom=` en la terminal, y el botón
  **reset view** junto al plano lo devuelve al origen con zoom 1 — el evento
  del botón llega al script y este responde por `/gui_set` (el round trip que
  deja la navegación en manos del script). Girar una perilla dentro de una caja del plano sigue funcionando
  (el widget gana el press; el plano solo toma el fondo vacío).
- **Texto: tamaño, wrap y alineación.** `examples/gui_text.py` abre una
  ventana con el mismo label a `text_size` crecientes (1.0 a 4.0 — el 2.0 es
  idéntico a como se veía todo antes), un párrafo envuelto (`wrap`) tres veces
  con `align` `start`/`center`/`end`, y una fila de controles con etiquetas
  demasiado largas → las etiquetas que no entran terminan en `…` (una elipsis)
  en vez de pisar al vecino, y la perilla/toggle/menu a `text_size=3.0` se
  leen al triple. A los ~3 s el título crece en vivo por `/gui_set` y el
  párrafo del medio cambia de alineación dos veces.
- **El patcher, nivel 1 (defs enteras cableadas por buses).**
  `examples/gui_patch1.py` construye un `GraphPatch` en código (`tone → dac`, la
  terminal que llega a los parlantes sola) y abre su **vista dirigida**: cajas
  con **inlets** arriba y **outlets** abajo, un **cable** por `outlet → inlet`.
  Arrastrar una caja la mueve (los cables la siguen); arrastrar un pin de outlet
  hasta un inlet los **cablea** (una mezcla de rate se rechaza en el gesto, la
  terminal imprime `wired ...`); arrastrar el canvas vacío barre una marquesina;
  **Shift+arrastre** panea y la rueda hace zoom anclado al cursor. **render**
  compila el patch dibujado y lo suena; **stop** lo libera. Un cable *es* un bus
  que nunca numerás, y ningún bus se dibuja.
- **El patcher, nivel 2 (el grafo interno de una def).**
  `examples/gui_patch2.py` no necesita servidor de audio: abre la **estructura**
  de una def como su grafo de UGens (`some_def.plot_def()`), una ventana por
  llamada. Primero un `SynthDef` (`voice`): el panel se titula **`synthdef`** (el
  tipo, no el nombre) y **contiene** todas las cajas; el host las acomoda con un
  **layout por niveles tipo Sugiyama** (cada caja rankeada por su camino más largo
  hasta un sink, así las entradas caen justo encima de donde se usan y los `Out`
  quedan abajo; la señal baja de arriba hacia abajo y los nodos del mismo nivel se
  alinean bajo sus padres). Cada UGen una caja (inlets nombrados por la firma del
  constructor: `out` → `bus`/`signal`), cada constante su propia **caja de valor**
  (con relleno distinto), y los cables **coloreados por tipo**: audio verde,
  control azul, e **init `ir`** ámbar y **punteado** (el `detune` escalar). Paneá
  con arrastre en el vacío, zoom con la rueda. Cerrá la ventana y abre la segunda:
  un `FaustDef` de árbol de señal (panel **`faustdef`**), decodificado nodo por
  nodo. Es de **solo lectura**: la vista de la def, fiel a lo que la def es. La
  terminal imprime el decode (con el rol de cada caja) y confirma que el round
  trip reproduce el spec.
- **Grupos de tema y acentos por widget.** `examples/gui_style.py` arranca el
  host con un archivo de tema cálido (naranja) y abre una ventana con cuatro
  filas: la primera toma el tema del host, la segunda es un **grupo de tema**
  frío (azul, prop `theme` del panel), la tercera un grupo anidado más oscuro
  *dentro* del frío (hereda el azul y oscurece el panel y el texto), y la
  cuarta tres sliders con acentos propios (`color`: rojo, verde, amarillo) →
  a los ~4 s el grupo frío vira a violeta en vivo, a los ~8 s el slider "a"
  vira a cian, y a los ~12 s el grupo se limpia (`theme=""`) y vuelve al
  naranja del host. Todo por `/gui_set`, sin redefinir la ventana.
- **Partitura grabada y editable (GUI).** `examples/gui_score.py` abre una
  ventana con la frase grabada por verovio; suena una vez y el cursor la sigue
  → hacer clic en una nota la vuelve a sonar y la resalta; **arrastrarla hacia
  arriba o abajo** la mueve por grados de la escala (la cabeza **con su plica y
  su corchete**, encastrando en cada grado; las líneas adicionales no siguen al
  arrastre — cuántas lleva una nota lo decide el grabado), y al soltar el
  script la transpone, la re-graba, la manda de vuelta y **suena en la altura
  nueva** — la nota queda
  seleccionada y la página no parpadea. Necesita un verovio con el editor vivo
  (`third_party/BUILD-VEROVIO.md`); con el wheel publicado el arrastre se
  dibuja pero el script avisa que la edición fue rechazada.
- **Un archivo de tema recolorea el host entero.** Escribir un `tema.toml` con
  `accent = "#ff8c40"` y `text = "#f0e8dc"`, lanzar
  `clausters-gui --theme tema.toml` y abrir cualquier ejemplo GUI → sliders,
  knobs, meters y marcos pasan del verde al naranja en todas las ventanas (un
  tema por host); un rol desconocido en el archivo solo deja un warning en el
  log y el resto aplica. Lo mismo por config con la tabla `[gui.theme]`.

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
