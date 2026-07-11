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
  (con y sin `--features faust`, este último `--test-threads=1`), los golden de
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
  el eje), y `r` resetea. Refrescar antes el binario bundleado
  (`clients/python/clausters/_bin/`) — ver `CLAUDE.md`.
- **Editor compositivo (el lazo completo).** `examples/gui_composer.py` compone
  con el modelo (un take bounceado y cargado de disco, una melodía, un patrón),
  lo abre como multipista y lo toca → se **oye** la composición y se **ve** el
  playhead barriendo los clips; arrastrar un clip lo mueve, y con `follow=True`
  la composición se re-agenda: suena donde lo soltaste. El carril **sweep** es una
  automatización: su cuerpo es la **curva**, y se edita ahí mismo (arrastrar un
  punto, Ctrl+click agrega/quita) → se **oye** el filtro seguir la curva dibujada. Refrescar antes el
  binario bundleado (`clients/python/clausters/_bin/`) — ver `CLAUDE.md`.
- **Salud de tiempo real bajo carga.** `cargo run --release --example stress`
  ramea nodos: el medidor publica avg/peak y `late_blocks` en `/status.reply`;
  al sobrecargar a propósito el servidor **glitchea pero no muere** (el guard de
  SIGXCPU lo degrada a SCHED_OTHER, ver abajo).

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
- **`cargo build --features faust` no enlaza**: libfaust no está donde se espera.
  Verificar `ls ~/.local/lib/libfaust.so` o exportar `FAUST_PREFIX`. Tras
  cambiarlo, `cargo clean -p clausters` para que build.rs lo relea.
- **`/d_faust` responde `/fail "server built without faust support"`**: el
  servidor se compiló sin `--features faust`.
- **`import("stdfaust.lib")` falla**: falta la stdlib en `<prefijo>/share/faust`
  (la instala el `make install` de libfaust).
- **Puerto ocupado** (`Address already in use`): quedó otro servidor vivo;
  `osc_ping quit` o matar el proceso.
- **Los tests con feature crashean en paralelo**: hay un lock global de
  compilación precisamente por esto; si se llama a la FFI de libfaust desde
  código propio, toda compilación debe pasar por `faust::compiler::ffi_lock()`
  — libfaust no tolera compilaciones concurrentes en un proceso.
