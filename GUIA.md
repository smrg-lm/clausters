# Guía de compilación y prueba

Cómo compilar **Clausters**, correrlo y probar todo lo que hay hasta ahora
(milestones M0–M14 y F0–F5). Pensada para Linux / Ubuntu 24.04 o más
nuevo.

Documentación de referencia: `docs/schemas.md` (formatos de defs y comandos
OSC, para usuarios) y `docs/architecture.md` (internals: hilos, memoria,
invariantes y cómo agregar UGens, para desarrollo). Ambas en inglés.

## Qué es

Un servidor de síntesis de audio en tiempo real estilo **scsynth**
(SuperCollider), escrito en Rust. Un proceso abre el dispositivo de audio y
queda en silencio escuchando comandos **OSC por UDP** (puerto 57110): crear
synths, fijar parámetros, armar grupos, definir instrumentos nuevos en
caliente. Hay dos maneras de definir instrumentos:

- **SynthDefs propias** (JSON de UGens: `SinOsc`, `Out`, `Mul`…), siempre
  disponibles.
- **Defs Faust** (feature opcional `faust`): el servidor embebe el
  compilador de Faust con JIT LLVM y compila DSP en caliente, ya sea fuente
  Faust o un JSON que se mapea a la Box API de Faust.

## 1. Requisitos base

```sh
# Rust (edition 2024: necesita rustc 1.85+; rustup instala stable actual)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Toolchain C y ALSA (backend de audio vía cpal)
sudo apt install build-essential pkg-config libasound2-dev

# Opcional, para mandar OSC a mano desde la terminal
sudo apt install liblo-tools   # da el comando `oscsend`
```

## 2. Núcleo (sin Faust): compilar, testear, escuchar

```sh
git clone <este-repo> clausters && cd clausters

cargo build --release
cargo test                       # 181 tests, no necesita placa de audio
```

Los tests cubren: protocolo OSC con round-trips UDP reales
(`tests/osc.rs`), el motor offline con asserts de señal — frecuencia por
cruces por cero, RMS — (`tests/engine.rs`), el formato de SynthDefs
(`tests/synthdef.rs`), los buffers: hilo NRT, round-trips de WAV con
`hound`, lectura multi-formato vía `symphonia` (detecta el contenedor por
contenido, no por extensión), `PlayBuf`/`BufRd` con igualdad exacta muestra
a muestra y el ciclo `/b_*` por OSC (`tests/buffers.rs`), el scheduling
sample-accurate de
bundles con timetag — cortes exactos a mitad de bloque, orden estable,
bundles tardíos, conversión NTP→samples por OSC — (`tests/scheduling.rs`),
el modo NRT con sus tests dorados — render offline comparado por sample
contra los WAV de referencia de `tests/golden/` — (`tests/golden.rs`), y
el guardián de RT-safety: el hilo de audio no debe alocar jamás, y
`tests/rt_safety.rs` lo verifica con `assert_no_alloc` (también al
instalar/reemplazar/liberar buffers y al encolar/ejecutar bundles).

### Correr el servidor y escucharlo

Terminal 1:

```sh
cargo run --release
# clausters — silent until /s_new | 44100 Hz, 2 channels | 0 DSP worker(s) | OSC on 127.0.0.1:57110 | ...
# (la sample rate es la del dispositivo de audio default)
```

Terminal 2 — el cliente de ejemplo:

```sh
cargo run --example osc_ping -- status    # /status.reply con contadores
cargo run --example osc_ping -- beep      # synth "default": la 440, re-afinado a 660 con /n_set, liberado
cargo run --example osc_ping -- vibrato   # define una SynthDef JSON por /d_recv y la toca (sinusoide con vibrato)
cargo run --example osc_ping -- quit      # apaga el servidor
```

Se pueden encadenar: `cargo run --example osc_ping -- status beep vibrato quit`.

También hay un cliente en Python (sin dependencias, solo stdlib) que
**genera** las defs como JSON en vez de tenerlas a mano — sirve de
referencia para escribir clientes propios:

```sh
python3 examples/json_client.py status ugen   # ruido con AM definido por /d_recv
```

### Probar buffers y PlayBuf (M5)

La demo `buffer` del cliente Python hace el ciclo completo: escribe un WAV
de prueba (sine de 330 Hz a 22050 Hz), lo carga con `/b_allocRead`, lee
frames/canales/sample-rate con `/b_query`, lo toca en loop con un def
`PlayBuf` a la altura correcta (rate = sr del archivo / sr del servidor,
se escucha la sine), lo re-afina una quinta arriba con `/n_set` y libera
todo:

```sh
python3 examples/json_client.py buffer
```

Ese `rate = sr_archivo / sr_servidor` también lo calcula la UGen
`BufRateScale` dentro del propio SynthDef: con
`PlayBuf(buf, rate: BufRateScale(buf) * pitch)` la corrección de tono es
transparente y el cliente no necesita conocer ninguna de las dos
frecuencias. La familia completa es `BufSampleRate`, `BufRateScale`,
`BufFrames`, `BufChannels` y `BufDur` (todas toman el bufnum y devuelven un
valor constante por bloque).

### Streaming de disco: DiskIn / DiskOut

A diferencia de `PlayBuf`/`BufRd` (que cargan el archivo entero a memoria),
`DiskIn` y `DiskOut` hacen streaming desde/hacia disco en tiempo real, así
que un archivo arbitrariamente largo nunca toca el pool de buffers. Cada una
es **auto-contenida**: un thread de I/O de fondo por instancia más un ring
lock-free compartido con el hilo de audio; el hilo de audio nunca hace I/O.
Llevan campos estáticos en el spec del UGen: `path` (obligatorio), `loop`
(DiskIn) y `format` (DiskOut: `int16`/`int24`/`float`). Son **mono por
UGen** (un `DiskIn` por canal; un `DiskOut` escribe WAV mono). `DiskIn`
streamea un frame de archivo por sample del servidor (sin remuestreo, como
en scsynth).

La demo `disk` del cliente Python graba una sine a disco con `DiskOut` y
después la reproduce con `DiskIn`:

```sh
python3 examples/json_client.py disk
```

A mano con `oscsend` (los `/b_*` son asíncronos: responden `/done` o
`/fail` al cliente; el tráfico OSC también se puede ver en el log del
servidor activando `/dumpOSC` o con `RUST_LOG=clausters::osc=trace`):

```sh
oscsend localhost 57110 /b_allocRead is 10 /ruta/a/un.wav
oscsend localhost 57110 /b_query i 10          # responde /b_info: frames, canales, sr
oscsend localhost 57110 /b_write isssii 10 /tmp/copia.wav wav float -1 0
oscsend localhost 57110 /b_zero i 10
oscsend localhost 57110 /b_free i 10
```

`/b_allocRead` y `/b_read` aceptan, además de WAV, formatos comprimidos y
otros contenedores (FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF, CAF): el
WAV pasa por `hound` (exacto, soporta int24) y el resto se decodifica con
`symphonia`. La detección es por **contenido**, no por extensión. `/b_write`
sigue escribiendo solo WAV. Para probarlo con archivos reales, generá unos
con `ffmpeg` y cargalos:

```sh
ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 /tmp/t.wav
ffmpeg -i /tmp/t.wav /tmp/t.flac        # tambien /tmp/t.ogg, /tmp/t.mp3
(cargo run --release & PID=$!; sleep 1.5; \
 oscsend localhost 57110 /b_allocRead is 10 /tmp/t.flac; \
 oscsend localhost 57110 /b_query i 10; sleep 0.5; kill $PID)
# /b_info debe reportar 44100 frames, 2 canales, sr 44100 (igual que el WAV)
```

Para *escuchar* un buffer hace falta un def con `PlayBuf` (4 entradas:
bufnum, canal, rate, loop) — `oscsend` no puede mandar blobs de `/d_recv`,
por eso la demo Python. `BufRd` (bufnum, canal, fase, loop) lee el buffer
con una señal de fase arbitraria: scrubbing, wavetables.

### Probar bundles con timetag (M6)

Los bundles OSC con timetag NTP futuro se agendan sobre el reloj de samples
del servidor y disparan **sample-accurate** (el engine parte el bloque de
64 en el sample exacto del evento). La demo `bundle` agenda un arpegio
entero por adelantado — todos los `/s_new`/`/n_free` viajan juntos al
principio — y se escucha el ritmo perfectamente regular, inmune al jitter
de la red y del cliente:

```sh
python3 examples/json_client.py bundle
```

`oscsend` no arma bundles con timetag; a mano se puede probar desde
`sclang` (`s.sendBundle(0.5, ["/s_new", "default", 4000, 1, 0])`) o
copiando la función `bundle()` del cliente Python. Comandos agendables
dentro de un bundle: `/s_new`, `/n_set`, `/n_free`, `/n_before`,
`/n_after`, `/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`,
`/g_sortMode`; cualquier otro responde `/fail`. Bundles con timetag pasado se ejecutan al llegar
(y el servidor loguea "late").

### Probar el modo NRT / render offline (M7)

No necesita servidor corriendo ni placa de audio: `clausters --nrt` lee
una **partitura** (formato binario de scsynth: paquetes OSC con prefijo de
tamaño; el timetag cuenta segundos desde el inicio) y renderiza a WAV con
el mismo motor, sample por sample idéntico a una toma en vivo perfecta.
La demo `score` del cliente Python escribe el mismo arpegio de la demo
`bundle` como partitura:

```sh
python3 examples/json_client.py score      # escribe /tmp/clausters_score.osc
./target/release/clausters --nrt /tmp/clausters_score.osc /tmp/out.wav
ffplay -autoexit /tmp/out.wav              # o aplay, o abrirlo en Audacity
```

Debe escucharse el arpegio de 5 notas con ritmo perfecto y el reporte
decir `rendered 11 events … 100800 frames (2.100 s)`. Opciones: `--rate`,
`--channels`, `--format float|int16|int24` (`--help` las lista). Detalles
finos: el render termina en el tiempo del último bundle (por eso la
partitura cierra con un bundle dummy), y a diferencia del modo en vivo
las partituras sí pueden llevar `/d_recv`, `/d_faust` y los `/b_*`
(corren síncronos, semántica NRT de scsynth).

Los **tests dorados** comparan renders contra los WAV de referencia de
`tests/golden/` (se pueden escuchar: son float32 de 0.3 s y 0.25 s). Si
un cambio intencional altera el sonido, regenerarlos y **escucharlos**
antes de commitear:

```sh
cargo run --example render_golden
ffplay -autoexit tests/golden/arpeggio.wav   # arpegio de 3 voces solapadas
ffplay -autoexit tests/golden/playbuf.wav    # sine 220 Hz que baja de volumen y se corta
```

El **benchmark del grafo** mide cuántos synths sostiene el motor en tiempo
real (corre offline, a fondo, en release):

```sh
cargo run --release --example bench                    # def default
cargo run --release --example bench --features faust   # + comparación UGen vs Faust
```

Con `--features faust` agrega dos secciones **head-to-head**: el *mismo* DSP
(pares de paridad de `tests/faust_parity.rs`, que ambos motores calculan
muestra a muestra) corrido por las dos variantes, así el tiempo aísla el
overhead por synth dentro del bucle de audio (grafo de UGens con dispatch
`dyn` y buffers intermedios vs una llamada `compute` de Faust LLVM). La
construcción y el JIT quedan fuera del bucle medido. Las dos secciones: una
**sine** (`sin(2π·phasor)·0.2`, realista pero con `SinOsc` en f64 y Faust en
f32) y una **gain** (`·0.5` bit-exacto sobre un bus compartido, sin
transcendental ni asimetría de precisión → overhead de motor puro). En ambas,
en igualdad de DSP, Faust resulta **más rápido**, no más lento (`slowdown` <
1, ~0.7×): una sospecha de lentitud de Faust no se sostiene en condiciones
equivalentes.

### Probar el reloj de samples como timebase (M8)

El reloj del SO y el cristal de la placa derivan entre sí; M8 agrega dos
comandos para que el cliente use el **reloj de samples del servidor** como
maestro: `/clock` (consulta el contador, responde `/clock.reply` con int64 +
sample rate) y `/sched <target int64> <blob>` (agenda un paquete OSC en un
**sample absoluto**, atómico y sample-accurate). Conviven con los bundles
NTP de M6 — misma cola interna. El ejemplo de referencia modela
`sample(t) = a + b·t` con regresión sobre anclas `/clock` y agenda 8 notas
con espaciado exacto en samples:

```sh
cargo run --release                      # terminal 1
python3 examples/sample_clock.py         # terminal 2
```

Debe escucharse un patrón de 8 notas perfectamente regular y verse el
reporte por beat (sample objetivo, slope del modelo). Notas: la
incertidumbre del ancla solo corre el grid entero por una constante (no se
acumula) y el espaciado *relativo* es sample-exacto por construcción; el
slope necesita minutos de línea de base para mostrar la deriva real (en una
corrida corta domina la cuantización del buffer del dispositivo). Detalles
en `docs/sample-clock.md`.

A mano con `oscsend` no se puede (necesita int64 + blob), pero se ve el
reloj avanzar con dos consultas espaciadas usando el decoder del cliente
Python, o directamente:

```sh
python3 -c "
import sys; sys.path.insert(0, 'examples')
import json_client as osc, time
c = osc.Client()
for _ in range(3):
    c.send('/clock'); c.reply(); time.sleep(0.5)
"
```

### Verificar el ancla de tiempo OSC en /clock.reply (M21)

M21 agrega un **tercer campo** a `/clock.reply`: el **tiempo OSC** del servidor
tomado junto con el contador de samples. El par `(tiempo OSC, sample)` es el
**ancla** del reloj maestro — deja que varios clientes mapeen su tiempo OSC
logico al eje de samples del servidor (`sample = S0 + (T - T0) * rate`).
Compatible hacia atras: los clientes que solo leen `(sample, rate)` ignoran el
timetag final.

A mano, confirmar que la respuesta ahora trae 3 argumentos y que el tercero cae
cerca de la hora actual del sistema:

```sh
cargo run --release                      # terminal 1
python3 -c "
import sys, time; sys.path.insert(0, 'examples')
import json_client as osc
c = osc.Client()
c.send('/clock'); addr, args = c.reply(quiet=True)
print('args:', args)                     # [samples, rate, tiempo_OSC_en_segundos_unix]
print('campos:', len(args))              # 3
print('delta vs reloj local:', args[2] - time.time(), 's')  # cerca de 0
"                                        # terminal 2
```

Debe imprimir 3 campos; el tercero (tiempo OSC, ya convertido a segundos Unix
por el decoder del ejemplo) debe diferir de `time.time()` en una fraccion de
segundo.

### Cliente: enganchar el reloj al master con lock_to (C14)

C14 agrega `clock.lock_to(server)` / `Session.lock_to_server()`: el reloj del
cliente, que por defecto usa tiempo OSC (anda sin servidor), se engancha al
**reloj de samples** del servidor (timebase `SampleClockTimebase`, agenda por
`/sched`). Si no hay master alcanzable, queda en tiempo OSC sin fallar.

Verificar el cambio de timebase y que toca sonido, con un servidor vivo:

```sh
cargo run --release                      # terminal 1
PYTHONPATH=clients/python python3 -c "
from clausters import Session
from clausters.base import MonotonicTimebase, SampleClockTimebase
from clausters.seq import Pbind, Pseq
s = Session.live(tempo=2.0)
print('antes:', type(s.clock.timebase).__name__)   # MonotonicTimebase
s.lock_to_server()
print('despues:', type(s.clock.timebase).__name__)  # SampleClockTimebase
s.play(Pbind(instrument='default', freq=Pseq([440.0, 550.0, 660.0]), dur=0.25, amp=0.1))
s.run(1.5)                                # se escuchan las notas, agendadas por /sched
s.close()
"                                        # terminal 2
```

Y el fallback sin servidor (no debe fallar, queda en tiempo OSC):

```sh
PYTHONPATH=clients/python python3 -c "
from clausters.base import TempoClock, MonotonicTimebase
from clausters.defs import Server
clock = TempoClock(tempo=1.0)
clock.lock_to(Server('127.0.0.1', 59999), timeout=0.2)  # nada escuchando
print('timebase:', type(clock.timebase).__name__)        # MonotonicTimebase
"
```

### Servidor: transport compartido para alinear fase (M22)

M22 agrega `/transport`: una grilla de beats `(origin_sample, tempo)` que el
servidor guarda y sirve para que varios clientes arranquen en el mismo beat (la
parte cliente, `quant`/`join_transport`, es C15). Sin args consulta; con
`(int64 origin, double tempo)` la fija. El servidor no agenda desde ella, solo
la almacena (en memoria, se resetea al reiniciar).

A mano con el decoder del ejemplo:

```sh
cargo run --release                      # terminal 1
python3 -c "
import sys; sys.path.insert(0,'examples')
import json_client as osc
c = osc.Client()
c.send('/transport'); print('inicial:', c.reply(quiet=True)[1])           # [0, 0.0, 0]  sin definir
c.send('/transport', osc.Int64(96000), 2.0); print('set:', c.reply(quiet=True)[0])  # /done
c.send('/transport'); print('despues:', c.reply(quiet=True)[1])           # [96000, 2.0, 1]
c.send('/transport', osc.Int64(0), 0.0); print('malo:', c.reply(quiet=True)[0])     # /fail (tempo<=0)
"                                        # terminal 2
```

Debe verse `[0, 0.0, 0]` antes de fijar, `[96000, 2.0, 1]` despues, y `/fail`
ante un tempo invalido (la grilla previa queda intacta).

### Cliente: alineamiento de fase con quant + join_transport (C15)

C15 honra `quant` (antes se ignoraba) y agrega `clock.join_transport(server)`:
varios clientes unidos a la misma transport del servidor arrancan una rutina
`quant`-eada en el **mismo beat** (a la muestra si ademas hacen `lock_to`).

El ejemplo lo demuestra con dos clientes independientes en un proceso:

```sh
cargo run --release                                                          # terminal 1
PYTHONPATH=clients/python python3 clients/python/examples/transport_sync.py  # terminal 2
```

Debe imprimir el mismo `next bar -> sample N` para A y B (`aligned to the
sample`) y tocar las dos notas juntas.

Y el `quant` solo (un cliente, sin transport), determinista:

```sh
PYTHONPATH=clients/python python3 -c "
from clausters.base import TempoClock
c = TempoClock(tempo=2.0); c._logical_beat = 3.5
print('delay al proximo bar (quant=4):', c._quant_delay(4))   # 0.5
"
```

La columna `x real time` es el headroom: con N synths, cuántas veces más
rápido que 48 kHz procesa. En esta máquina ≈1800 voces sinusoidales.

### Probar los grupos auto-ordenados (M12)

El servidor puede inferir el orden de ejecución a partir de los buses que
cada def lee y escribe, y mantener un grupo ordenado solo (`/g_sortMode`):
los grupos pasan a ser como canales de un multipista y el cliente deja de
pelear con `/n_before`. La demo arma la cadena fuente → fx → master **al
revés a propósito** (silencio en un grupo manual) y la repara con un solo
comando:

```sh
cargo run --release                  # terminal 1
python3 examples/auto_order.py      # terminal 2
```

Debe verse el grafo inferido antes y después (`/g_dumpGraph`), oírse
silencio 2 s, luego la cadena a 330 Hz al activar `/g_sortMode 100 1`, y
una segunda voz (495 Hz) agregada en la cabeza del grupo que suena igual
(cada cambio re-ordena). A mano:

```sh
oscsend localhost 57110 /g_sortMode ii 100 1   # activa; 0 vuelve a manual
oscsend localhost 57110 /g_queryTree ii 0 1    # árbol estilo scsynth, con controles
oscsend localhost 57110 /g_dumpGraph i 100     # buses leídos/escritos por nodo
```

Reglas finas (detalle en `docs/auto-order.md`): índices de bus constantes
o por control se analizan (un `/n_set` al control re-ordena); un índice
calculado por señal marca el nodo `dynamic` = barrera que nada cruza; los
ciclos (feedback) conservan su orden relativo = un bloque de delay; y
dentro de un grupo auto los `/n_before`/`/n_after` manuales responden
`/fail`.

### Probar el mapeo de buses a parámetros (M11)

`/n_set` escribe un control una vez; `/n_map` lo **liga a un bus de control**
y `/n_mapa` a un **bus de audio**: el nodo re-lee el bus al inicio de cada
bloque, así el parámetro sigue al bus en vivo (sirve igual para defs UGen y
zonas Faust). El subcomando `map` de `osc_ping` lo demuestra entero —
`/n_map freq` a un bus de control retuneado con `/c_set`, y luego un LFO que
escribe un bus de audio del que un segundo synth toma su `freq` (vibrato):

```sh
cargo run --release                         # terminal 1 (o ver consola)
cargo run --example osc_ping -- map quit    # terminal 2
```

A mano:

```sh
oscsend localhost 57110 /s_new siii default 1000 1 0
oscsend localhost 57110 /n_map isi 1000 freq 5      # freq <- bus de control 5
oscsend localhost 57110 /c_set if 5 330             # …y 5 lo retunea en vivo
oscsend localhost 57110 /c_set if 5 660
oscsend localhost 57110 /n_map isi 1000 freq -1     # -1 desliga (queda en 660)
oscsend localhost 57110 /n_set isf 1000 freq 440    # un /n_set también desliga
```

`/n_mapa` toma un bus de audio muestreando **un sample por bloque** (a
control-rate: un control es un escalar por bloque, y las zonas Faust también;
para señal de audio real está `In`/bus de entrada). Un control mapeado que se
usa como índice de bus vuelve al nodo barrera `dynamic`, y un mapeo de audio
suma ese bus a las lecturas del nodo, así el análisis de M12/M13 sigue
correcto. Detalle en `docs/schemas.md` y `docs/architecture.md`.

### Probar el protocolo MIDI estándar (M17)

El servidor puede accionarse con **MIDI estándar de canal-voz** (note on/off,
velocity, aftertouch, pitch-bend, control change, program change), no solo OSC:
una nota crea un nodo de síntesis y un mensaje expresivo escribe un control
nombrado. El transporte es **MIDI estándar del SO**: con `--midi [nombre]` el
servidor abre un **puerto de entrada virtual ALSA** (nombre por defecto
`clausters`) al que se enruta cualquier dispositivo/app (un teclado por el
kernel, `aconnect`, un DAW). La entrada es MIDI 1.0 (7 bits, ensanchados
internamente a alta resolución). Usa `midir` (la librería pautada en el plan;
ALSA seq por debajo en Linux), con su hilo de entrada decodificando los
mensajes y pasándolos al loop de red.

Primero hay que **ligar** un canal a un instrumento (por OSC), y luego tocar
MIDI por el puerto. Todo en la **misma** invocación de Bash:

```sh
# Un SMF tipo-0 mínimo: note on canal 0, nota 69, velocity 100 (nota sostenida).
printf '\x4d\x54\x68\x64\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60\x4d\x54\x72\x6b\x00\x00\x00\x08\x00\x90\x45\x64\x00\xff\x2f\x00' > /tmp/note.mid

( ./target/release/clausters --no-persist --midi clausters & PID=$!; sleep 1.5; \
  aconnect -l | grep -i clausters; \
  oscsend localhost 57110 /midi_bind is 0 default; \
  cargo run --release --example osc_ping -- status; \
  aplaymidi -p clausters /tmp/note.mid; \
  sleep 0.4; \
  cargo run --release --example osc_ping -- status; \
  oscsend localhost 57110 /quit; wait $PID )
```

El `/status.reply` después de la nota debe mostrar **synths = 1** (el 3er entero)
y ugens > 0: la nota MIDI creó el nodo `default` (`note -> midi2freq`,
`velocity -> velocity2amp`). Para escucharlo, mandá un `.mid` real con notas
sostenidas desde un secuenciador/teclado conectado con `aconnect`. La actuación
y la **paridad byte a byte** con el `/s_new`/`/n_set`/`/n_free` equivalente
están además cubiertas por `cargo test --test midi`. Detalle en
`docs/schemas.md`.

### Probar el procesamiento paralelo (M13)

Con `--workers N` el servidor levanta N hilos DSP y los grupos marcados con
`/g_parallel` procesan sus hijos independientes en paralelo, por **etapas**
derivadas del mismo análisis de buses de M12. Garantía central: el resultado
es **bit-idéntico** al secuencial (las etapas solo agrupan hijos con buses
disjuntos), así que activar workers solo cambia el tiempo de pared, jamás el
audio. Dos escritores al mismo bus se serializan solos; un índice de bus
dinámico corre aislado.

La prueba que importa es el **benchmark** (offline, a fondo, en release):

```sh
cargo run --release --example bench
# ... al final:
# parallel group (/g_parallel): 8 subgroups x 125 sines, disjoint buses:
#   0 workers: ... (speedup 1.00x)
#   3 workers: ... (speedup ~3x en una máquina de 4+ cores)
```

En vivo y en NRT:

```sh
cargo run --release -- --workers 3                       # servidor RT
./target/release/clausters --nrt score.osc out.wav --workers 3   # render más rápido
oscsend localhost 57110 /g_parallel ii 100 1   # marca el grupo 100
oscsend localhost 57110 /g_dumpGraph i 100     # muestra "(…, parallel)"
```

Sin `--workers` el flag se acepta y se recuerda pero todo sigue secuencial.
La identidad bit a bit está clavada por `tests/parallel.rs` (en vivo, en
NRT, y tras un `/n_set` que re-apunta un bus); la RT-safety del conductor
por `tests/rt_safety.rs`. Detalle en `docs/parallel.md`.

### Probar la configuración del servidor (`--sample-rate`, `--audio-buses`, `--control-buses`, `/server_info`)

El servidor impone su sample rate (default 48000; con `0` sigue al device) y
fija los conteos de buses al bootear. PipeWire honra el rate por aplicación,
así que se puede pedir uno distinto al del device sin tocar el sistema:

```sh
cargo run --release -- --sample-rate 44100 --audio-buses 64 --control-buses 2048 &
# el banner muestra "44100 Hz"; consultá la config con la query:
./target/debug/examples/osc_ping info
#   /server_info.reply [Int(64), Int(2048), Int(2), Int(64), Double(44100.0), Double(44100.0)]
#   = [audio_buses, control_buses, channels, block_size, nominal_sr, actual_sr]
```

`nominal != actual` solo si el host no pudo honrar el rate (cae al device). El
cliente Python define la config y la consulta: `ServerOptions(audio_buses=64,
control_buses=2048).args()` da los flags de lanzamiento, y `Server.query_info()`
lee `/server_info` de vuelta. El segmento `--shm` dimensiona su región de
control buses según `--control-buses` (ABI v2; el array va al final, los rings
quedan en offsets fijos):

```sh
cargo run --release -- --shm /dev/shm/clausters --control-buses 2048 &
# ls -l muestra el tamaño = prefijo_fijo + 2048*4; el ShmClient lo mapea entero
# y lee control_buses del header.
```

### Probar la memoria acotada y la alineación (M10)

Toda estructura pre-alocada tiene un comportamiento definido y no-fatal al
llenarse — la tabla completa (capacidad + modo de fallo) está en
`docs/architecture.md`. Los tests la clavan desbordando cada una a
propósito (FIFO de basura → leak acotado en vez de bloquear; eventos →
drop silencioso; slab/grupos → rechazo con rollback):

```sh
cargo test --test capacity
```

Los bloques de señal (wires, buses, staging Faust) están alineados a línea
de caché (`Block`, `#[repr(align(64))]`): un bloque = 4 líneas exactas, sin
straddling. Medido neutro en el bench (dentro del ruido de la máquina);
verificable con A/B intercalado:

```sh
cargo run --release --example bench | grep "1000 synths"
```

### Probar los transportes locales: shm y embebido (M14)

OSC sigue siendo la única codificación, pero ya no hace falta UDP para
clientes locales. **Memoria compartida** (dos procesos):

```sh
cargo run --release -- --shm /dev/shm/clausters   # terminal 1
python3 examples/shm_client.py                    # terminal 2
```

Debe verse el reloj de samples avanzando leído directo del segmento, un
`/status` ida y vuelta por el ring (sin sockets), y oírse una sinusoide que
hace fade **escribiendo el bus de control 7 en memoria compartida** — sin
mandar ningún comando: el `InCtl` del engine lee esos mismos atomics al
bloque siguiente.

**Modo embebido** (el servidor como biblioteca, C ABI):

```sh
cargo build --release --features embed,realtime   # produce libclausters.so
python3 examples/embed_render.py /tmp/arp.wav     # render síncrono, sin servidor
ffplay -autoexit /tmp/arp.wav                     # el arpegio de 5 notas
```

`clausters.render(score)` bloquea al *llamador* (nunca al servidor: no hay
servidor) y devuelve los floats planos — el flujo científico de consultar y
graficar. El binding (`clients/python/clausters/ipc.py`, stdlib pura) también
trae `Clausters(workers=N)` para el servidor vivo in-process: comandos por
llamada de función, replies por `poll()`/`request()` (la fachada síncrona),
y `clock`/`ctl_set` directos al data plane.

Tests del segmento y la C ABI: `cargo test --test ipc` (núcleo) y
`cargo test --features embed --test ipc` (+1 del render embebido). El
layout está versionado (ABI v1): un cliente con versión distinta es
rechazado al conectar. Detalles y referencia C en `docs/ipc.md`.

### Probar el transporte WebSocket (navegador y Python)

WebSocket es el cuarto portador de la **misma** codificación OSC (además
de UDP, TCP y el ring de memoria compartida): el único que alcanza un
navegador, que no puede abrir UDP crudo ni mapear memoria compartida.
Siempre está disponible, como TCP y shm (no detrás de una feature). Cada
paquete OSC viaja como **un** mensaje binario de WebSocket — el frame *es*
el límite del paquete, sin prefijo de longitud (a diferencia de TCP).

El WS del **cliente** Python vive en el core nativo (`clausters-ffi`,
vía ctypes, igual que shm/embed) — una sola implementación del protocolo,
no una copia en Python — así que hay que tener ese cdylib compilado:

```sh
cargo build -p clausters-ffi               # el WS del cliente (una vez)
cargo run -- --ws                          # terminal 1 (OSC sobre WebSocket 57120)
python3 examples/ws_ping.py                # terminal 2: el facade Server sobre WS
```

Debe verse el `/status` ida y vuelta, el `add_synthdef`
(`/d_recv` -> `/done`), la nota sonando ~1 s y el `free` — todo sobre
frames binarios. Desde el **navegador**, abrir `examples/ws_ping.html`
(sin dependencias ni cdylib): hace el mismo `/status` con la API nativa
`WebSocket` e imprime `/status.reply`. Tests del framing y round-trip:
`cargo test --lib osc::ws` (hub) y
`cargo test -p clausters-ffi` (cliente C ABI).

### Verificar el reloj de samples grabando la salida (M8 + M14)

`clock_recorder.py` cierra el lazo: lee el reloj **directo del segmento**
(`ShmClient.clock`, sin ida y vuelta), agenda un **impulso prístino de un
solo sample cada N samples** con `/sched` y graba la salida real para medir
si los impulsos cayeron parejos. Cada marca es la UGen `Impulse` a
frecuencia 0: emite un único `1.0` en el primer sample del synth, y como un
`/s_new` por `/sched` parte el bloque en el sample objetivo, ese primer
sample *es* el objetivo — un impulso limpio en un frame exacto, sin envolvente
ni rampa de ataque que difumine dónde cayó. La duración es libre: de unos
segundos a varias horas.

```sh
cargo run --release -- --shm /dev/shm/clausters     # terminal 1
python3 examples/clock_recorder.py --seconds 20      # terminal 2
```

La grabación usa `pw-record` (PipeWire). Por defecto captura el **nodo de
salida del propio servidor**: lo encuentra con `pw-dump` (aparece como
`alsa_playback.clausters`, clase `Stream/Output/Audio`) y engancha sus
puertos de salida, así graba exactamente lo que emite el servidor sin
importar a qué sink esté ruteado. Esto es más robusto que el monitor del
sink, que solo ve al servidor si su salida se mezcla en ese sink (cpal/ALSA
suele rutearla por otro lado, y entonces el monitor queda casi vacío aunque
los impulsos se oigan). Si no encuentra el nodo, cae al monitor del sink por
defecto (detectado con `wpctl inspect @DEFAULT_AUDIO_SINK@`, o `pactl`).
Podés forzar el origen con `--target <node.name>` (mirá `pw-dump`/`wpctl
status`), reemplazar todo el comando con `--record-cmd "..."` (`{out}` se
sustituye por `--out`), o usar `--no-record` para solo agendar. Si los
impulsos se oyen pero la grabación sale casi vacía, el análisis lo detecta y
avisa que se capturó el nodo equivocado. El sample rate que verás es el real
del dispositivo (p. ej. 44100 Hz): lo reporta el servidor por el segmento y
la grabación se hace a ese mismo rate. Al terminar, el script escanea el WAV y reporta
el espaciado medido vs el esperado, el **jitter** (rms alrededor de la
recta ajustada) y la **deriva** en ppm, con veredicto PASS/FAIL.

Lo que prueba: dos impulsos separados N samples en la agenda salen separados
N samples en la grabación — el espaciado nunca pasa por el reloj de pared de
esta máquina, solo por el de audio. Nota sobre el "testigo": al capturar el
nodo del propio servidor, el grabador comparte el mismo reloj de PipeWire, así
que el jitter cae casi a cero (verifica que `/sched` es sample-exacto, pero no
mide deriva de cristal independiente); para ver deriva real en ppm hay que
grabar por el **monitor del sink** (reloj independiente, `--target <sink>`).
Para corridas largas el agendado se mantiene a distancia fija del
reloj (`--lead`), así la memoria queda acotada aunque corra horas. Un WAV ya
capturado se re-analiza sin servidor con `--analyze archivo.wav` (pasale el
`--period` y `--server-rate` usados). Requiere hardware de audio real (el
sandbox no tiene dispositivo de salida).

### Probar el feedback intra-synth (LocalIn/LocalOut)

El grafo de UGens es un DAG: no se puede wirear un ciclo. `LocalIn`/`LocalOut`
dan feedback **privado del synth** con **1 bloque de control (64 muestras) de
retardo** (estilo scsynth): `LocalOut` escribe en un buffer que persiste entre
bloques y `LocalIn` lo lee — como `LocalIn` va antes que `LocalOut` (lo exige
el compilador), lee el valor del bloque anterior. Un lazo de un canal resuena
en `sampleRate/64` (≈ 750 Hz a 48 kHz).

```sh
cargo run --release                              # terminal 1
python3 examples/json_client.py feedback         # terminal 2
```

Debe oírse un comb resonante (pluck metálico repetido ~3 veces/seg, afinado
en sampleRate/64) que decae según la ganancia de realimentación (0.98). Es
feedback a **tasa de bloque**, no sample-accurate: para un IIR sub-bloque
(one-pole, biquad) hay que fusionar el lazo en un nodo — una UGen recursiva o
un def de Faust (`~`/`CboxRec`), que es la razón de ser de `FaustSynth`.
Tests: `cargo test --test feedback` (retardo exacto de 1 bloque, acumulador
por bloques, dos canales, split de bloque, validaciones de compilación) y la
escena no-alloc en `tests/rt_safety.rs`.

### Probar envolventes con EnvGen (done actions)

`EnvGen` toca una envolvente por segmentos (estilo scsynth): 5 entradas fijas
(`gate, levelScale, levelBias, timeScale, doneAction`) y luego el array de
envolvente (`initLevel, numSegments, releaseNode, loopNode` y por segmento
`target, duration, shape, curve`). El `gate` la dispara: mientras está en alto
**sostiene** en el `releaseNode` (o, con `loopNode < releaseNode`, **cicla** los
segmentos `[loopNode, releaseNode)`); al bajar toca los segmentos de release y,
al terminar, aplica el `doneAction`: 1 = pausa el synth (se saltea pero queda en
el árbol; no hay `/n_run` para reanudar aún), 2 = libera el nodo, 14 = libera el
grupo contenedor (el synth incluido) — todo por el garbage FIFO, sin `free` en el
hilo de audio. Formas: 0 step, 1 lineal, 2 exponencial, 3 seno, 4 welch,
5 curvatura custom (valor `curve`), 6 squared, 7 cubed, 8 hold.

El cliente Python arma el array con el helper `Env` (`Env.adsr`, `Env.perc`,
`Env.asr`) y el callable `env_gen`. Render offline de un pad ADSR que se
autolibera al soltar la nota:

```sh
python3 clients/python/examples/envelope.py /tmp/env.wav
ffplay -autoexit /tmp/env.wav        # 8 notas, cada una con ataque/decay y
                                     # cola de release; el synth se va solo
```

Debe oírse cada nota con su ataque suave, sostén y una cola de release audible
antes de la siguiente (el `sustain` del `Pbind` es menor que el `dur`). Tests:
`cargo test --test envgen` (rampa lineal + hold, ratio exponencial constante,
sostén que sólo avanza al soltar el gate, loop que cicla y sale al release,
`pauseSelf` que corta la salida sin liberar, `freeGroup` que libera el grupo, y
`doneAction=2` que libera el nodo) y la escena no-alloc `envgen_free_self_...` en
`tests/rt_safety.rs`; del lado cliente, los `test_env_*` en
`clients/python/tests/test_synthdef.py` (layout, formas, release/loop nodes,
constantes de done action). Nota: si corrés el ejemplo desde un checkout, el cdylib embebido debe
tener EnvGen — reconstruilo con `cargo build --release --features embed -p clausters`
y refrescá `clients/python/clausters/_libs/libclausters.so` (o apuntá
`CLAUSTERS_LIB` al `.so` recién compilado).

### Probar las tasas de cálculo (`ir`/`kr`/`ar`/`dr`, S1)

Cada salida de UGen tiene una **tasa** explícita, elegida con el campo opcional
`"rate"` en el def (`"rate": "kr"`) o por defecto según el kind (`ar` para las
UGens de señal). Son las cuatro de scsynth:

- **`ar`** (audio): un valor por sample — el cable de señal normal, el default.
- **`kr`** (control): un valor por bloque, recalculado cada bloque; aguas abajo
  se lee como constante sobre el bloque.
- **`ir`** (inicial/escalar): se computa **una vez al arrancar el synth** y se
  sostiene toda la vida del nodo (`SampleRate`, una semilla `Rand`, un
  `BufFrames.ir`); sus entradas deben ser constantes/`ir`.
- **`dr`** (demanda): se **tira** (pull), no corre por bloque — la fuente
  (`Dseq`) entrega el próximo valor cada vez que su driver (`Demand`) la tira en
  un flanco de `trig`.

El compilador valida la coerción: una tasa más lenta alimenta gratis una entrada
más rápida (una constante difundida sobre el bloque), pero una señal `ar` no
puede alimentar una entrada `ir`, y una salida `dr` sólo puede ir al slot fuente
de un `Demand`. Un rechazo llega en `/fail` nombrando el nodo ofensor.

Como es infraestructura, la verificación principal es por tests (el sandbox
aísla la red de todos modos):

```sh
cargo test --no-default-features --test rates
# ar varía por sample; kr es constante por bloque pero sigue a su entrada entre
# bloques; SampleRate.ir da la tasa del motor; Rand.ir queda congelado en rango;
# Demand/Dseq recorre y cicla una secuencia y resetea/agota; + 5 rechazos del
# compilador (rate desconocida, rate no permitida para el kind, entrada no-ir a
# una UGen ir, cable dr a una entrada normal, fuente no-dr en el slot de Demand)
cargo test --no-default-features --test rt_safety rate_substrate
# el ir init pass y el camino de pull demand no allocan en el hilo de audio
```

`Demand`/`Dseq` es un driver **mínimo** por ahora (una fuente por driver, fin de
stream = valor sostenido): prueba el protocolo de pull sobre el que se construye
después el resto de la familia demand (`Dseries`/`Dwhite`/`Duty`). El cliente
Python todavía no arma `rate`/demand de forma idiomática — se usan por JSON
crudo vía `/d_recv` (eso lo espeja el track de clientes más adelante).

### Qué probar a mano (núcleo)

Con el servidor corriendo y `oscsend` (los replies no se ven con oscsend;
para ver replies usar `osc_ping`):

```sh
# Eco del trafico OSC en el LOG del servidor (target clausters::osc en trace).
# Equivale a arrancar con RUST_LOG=clausters::osc=trace; /verbosity ajusta el
# nivel general en vivo (entero -1..3 o un directivo EnvFilter).
oscsend localhost 57110 /dumpOSC i 1
oscsend localhost 57110 /verbosity i 2          # debug en vivo (o "info", etc.)

# Un synth "default" dentro de un grupo, y orden de ejecución
oscsend localhost 57110 /g_new iii 1 1 0        # grupo 1 al final del root
oscsend localhost 57110 /s_new siii default 1000 1 1   # synth al final del grupo 1
oscsend localhost 57110 /n_set isf 1000 freq 330
oscsend localhost 57110 /n_set isf 1000 amp 0.3
oscsend localhost 57110 /n_map isi 1000 freq 5  # freq sigue al bus de control 5
oscsend localhost 57110 /c_set if 5 660         # …retunear en vivo, sin /n_set
oscsend localhost 57110 /g_freeAll i 1          # silencio: vació el grupo

# Buses de control (atómicos, sin pasar por el hilo de audio)
oscsend localhost 57110 /c_set if 7 220.0
cargo run --example osc_ping -- status          # y /c_get vía tests
```

Protocolo implementado hasta M6: `/status`, `/quit`, `/notify`, `/dumpOSC`,
`/verbosity`, `/s_new` (add actions 0–4), `/n_free`, `/n_set`, `/n_map`, `/n_mapa`,
`/n_before`, `/n_after`,
`/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`, `/c_get`, `/d_recv`,
`/d_free`, los buffers `/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write`,
`/b_zero`, `/b_free` (asíncronos vía hilo NRT, responden `/done cmd
bufnum`) y `/b_query` (responde `/b_info`), más `/d_faust` con el feature.
Introspección del árbol: `/g_queryTree` (responde `/g_queryTree.reply`),
`/n_query` (responde `/n_info` por nodo: padre, hermanos, def, controles,
maps y buses leídos/escritos) y `/g_dumpGraph` (texto legible).
Notificaciones `/n_go`/`/n_end` para clientes registrados con `/notify 1`.
Bundles con timetag NTP futuro se agendan y disparan sample-accurate
(sección M6 más arriba); con timetag inmediato o pasado se ejecutan al
llegar.

## 3. Feature `faust`: DSP compilado en caliente

### Por qué hay que compilar libfaust

El paquete de Ubuntu (`libfaust2t64`) viene **sin el backend LLVM** y sin
headers: no sirve para embeber el JIT. Hay que compilar libfaust desde
fuente una vez (~10 min, sin sudo, se instala en `~/.local`):

```sh
# Dependencias (usar la versión de LLVM más nueva que tenga la distro;
# probado con LLVM 20. Ajustar el «20» en lo que sigue si es otra.)
sudo apt install cmake llvm-20-dev libzstd-dev zlib1g-dev

git clone --depth 1 -b 2.85.5 https://github.com/grame-cncm/faust
cd faust
make most CMAKEOPT="-DCMAKE_BUILD_TYPE=Release -DINCLUDE_DYNAMIC=ON \
    -DLINK_LLVM_STATIC=off -DLLVM_CONFIG=llvm-config-20"
make install PREFIX=$HOME/.local
```

Notas:

- `INCLUDE_DYNAMIC=ON` construye `libfaust.so` (el target `most` solo, no).
- `LINK_LLVM_STATIC=off` enlaza la `libLLVM.so` del sistema; el link
  estático falla en Ubuntu salvo que se instale además `libpolly-XX-dev`.
- Si `make most` ya se había configurado antes, los flags se pueden aplicar
  re-corriendo cmake sobre el caché: `cmake build/faustdir -D...` y repetir
  `make most`.
- Queda instalado: `~/.local/lib/libfaust.so`, headers en
  `~/.local/include/faust/` y la stdlib de Faust en `~/.local/share/faust/`.

El `build.rs` del proyecto busca libfaust en `FAUST_PREFIX`, con fallback a
`~/.local` y luego `/usr/local`. Si se instaló en otro lado:
`export FAUST_PREFIX=/ruta/al/prefijo`. El binario lleva rpath, no hace
falta `LD_LIBRARY_PATH`.

### Compilar y testear con el feature

```sh
cargo test --features faust      # 148 tests (los 111 del núcleo + 37 de Faust)
```

Los tests de Faust cubren: humo del JIT con paridad de señal contra nuestro
`SinOsc` (`tests/faust_smoke.rs`), el hilo compilador y el round-trip
asíncrono de `/d_faust` (`tests/faust_compiler.rs`), el intérprete JSON→Box
con errores que señalan el nodo JSON culpable (`tests/faust_json.rs`), el
synth Faust en el árbol de nodos: control por nombre, ruteo de buses,
mezcla con synths UGen y no-alocación en el hilo de audio
(`tests/faust_synth.rs`, `tests/rt_safety.rs`), y los tests dorados de
paridad (`tests/faust_parity.rs`): el mismo grafo como def UGen y como def
Faust rinde lado a lado en un engine — una etapa de ganancia sobre el mismo
bus da **idéntico bit a bit**, y los osciladores igualan muestra a muestra
dentro de una tolerancia float (fase f64 vs f32).

### Probar Faust a mano

Servidor con el feature:

```sh
cargo run --release --features faust
```

**Def desde fuente Faust** (con acceso a la stdlib):

```sh
oscsend localhost 57110 /d_faust ss fsine \
  'import("stdfaust.lib"); freq = hslider("freq", 440, 20, 20000, 0.01); process = os.osc(freq) * 0.2;'
sleep 0.5   # la compilación es asíncrona (~10 ms) y responde /done o /fail

oscsend localhost 57110 /s_new siiisf fsine 2000 1 0 freq 330
sleep 1
oscsend localhost 57110 /n_set isf 2000 freq 660    # parámetro por nombre (zona FAUSTFLOAT)
sleep 1
oscsend localhost 57110 /n_set isf 2000 out 1       # control reservado: mover la salida al canal derecho
sleep 1
oscsend localhost 57110 /n_free i 2000
```

**Def como JSON→Box API** (el schema completo está documentado en
`docs/schemas.md`, junto con el formato SynthDef; `/d_faust` distingue JSON
porque empieza con `{`).
El op `faust` embebe fuente Faust como caja componible — acá un oscilador
de la stdlib seguido de un atenuador hecho con primitivas:

```sh
oscsend localhost 57110 /d_faust ss jsine \
  '{"op":"seq","in":[{"op":"faust","src":"import(\"stdfaust.lib\"); process = os.osc(330);"},{"op":"mul","in":["_",0.2]}]}'
sleep 0.5
oscsend localhost 57110 /s_new siii jsine 2001 1 0
sleep 1.5
oscsend localhost 57110 /n_free i 2001
```

**Con el cliente Python** — genera esos mismos JSON programáticamente
(funciones `box()`/`hslider()`/`faust()`) y maneja todo el ciclo: define una
sine desde primitivas y otra def que importa la stdlib, las toca juntas,
mueve `freq` con `/n_set` y las libera:

```sh
python3 examples/json_client.py faust
python3 examples/json_client.py quit    # apaga el servidor al terminar
```

**Errores legibles**: un def roto responde `/fail` con el mensaje del
compilador Faust verbatim, o con la ruta del nodo JSON inválido
(p. ej. `at $.in[0].op: unknown op "zzz"`). El `/fail` llega al cliente
(`osc_ping`/cliente Python) y desde los tests.

**Interop**: synths UGen y Faust conviven en el mismo árbol y mezclan en
los mismos buses — sonar `beep` (UGen) y `fsine` (Faust) a la vez.

Convención de controles de un synth Faust: los parámetros declarados por la
UI del def (sliders, botones) por su label, más dos nombres reservados:
`out` (primer bus de salida, default 0 = hardware izquierdo) e `in` (primer
bus de entrada, para defs que procesan señal).

**Def como JSON→Signal API** (la capa más baja de Faust). Mismo `/d_faust`,
pero el JSON tiene raíz `{"signals":[...]}` — así se distingue del box tree
(`{"op":...}`) y de la fuente. Es la API de entradas/delays/recursión
**explícitos**: el `self()` de realimentación que la box API envuelve en `~`.
El cliente Python lo demuestra con un seno por `recursion`/`self` y un
one-pole sobre ruido (feedback sample-accurate, lo que el `LocalIn`/`LocalOut`
del grafo no puede dar):

```sh
python3 examples/json_client.py signal
```

Debe oírse el seno (330→440 Hz) y luego un ruido pasado por el lowpass de un
polo, que se cierra al subir `a` a 0.99. A mano, un one-pole leyendo el bus 4:

```sh
oscsend localhost 57110 /d_faust ss siglp \
  '{"signals":[{"op":"recursion","in":[{"op":"add","in":[{"op":"mul","in":[{"op":"sub","in":[1.0,{"op":"hslider","label":"a","init":0.9,"min":0.0,"max":0.999,"step":0.001}]},{"op":"input","index":0}]},{"op":"mul","in":[{"op":"hslider","label":"a","init":0.9,"min":0.0,"max":0.999,"step":0.001},{"op":"self"}]}]}]}]}'
```

El schema completo (tabla de ops, discriminador, límites) está en
`docs/schemas.md`. No expone `seq`/`par`/wires implícitos ni el escape `faust`
(son conceptos del box tree), ni recursión N-aria (igual que la box solo tiene
`~`).

El **sample rate** se obtiene con el op `fconst` (la constante `fSamplingFreq`
detrás de `ma.SR`), no como número horneado: el servidor la resuelve al
instanciar la def, así queda afinada a cualquier tasa. A mano, una def cuya
única salida es el SR (debe dar 48000 si el motor corre a 48 kHz):

```sh
oscsend localhost 57110 /d_faust ss srcheck \
  '{"signals":[{"op":"floatcast","in":[{"op":"fconst","ctype":"int","name":"fSamplingFreq","file":"<math.h>"}]}]}'
```

### Probar waveforms y tablas (F5)

El op `waveform` embebe una tabla calculada por el cliente dentro de la def
(wavetables, waveshaping); `rdtable`/`rwtable` la leen/escriben. La demo
calcula 256 puntos (4 armónicos de sierra) en Python y los manda como JSON:

```sh
cargo run --release --features faust            # en una terminal
python3 examples/json_client.py wavetable quit  # en otra
# se oye la sierra suave a 220 Hz y después /n_set freq 330
```

A mano con `oscsend` (tabla de 4 valores leída cíclicamente con un contador
`& 3` — la versión mínima del idioma `wf, idx : rdtable`; a 48 kHz es un
tono agudo de 12 kHz, bajito):

```sh
oscsend localhost 57110 /d_faust ss jtab \
  '{"op":"mul","in":[{"op":"rdtable","in":[{"op":"waveform","values":[0.0,0.5,-0.5,0.25]},{"op":"and","in":[{"op":"intcast","in":[{"op":"rec","in":[{"op":"add","in":["_",{"op":"int","value":1}]},"_"]}]},{"op":"int","value":3}]}]},0.1]}'
sleep 0.5
oscsend localhost 57110 /s_new siii jtab 2002 1 0
sleep 1
oscsend localhost 57110 /n_free i 2002
```

### Probar `soundfile` leyendo un buffer del servidor (F6)

El op `soundfile("<bufnum>", n)` de Faust se enlaza al **buffer del servidor**
cuyo índice es la etiqueta: `soundfile("5", 1)` lee el buffer 5. Al hacer
`/s_new`, la instancia se llena con el contenido actual de ese buffer
(des-interleaveado al layout planar de Faust); sus salidas son
`[length, sampleRate, canal0 … canalN-1]` y el índice de lectura satura en el
largo de la parte. Es una **foto** tomada al instanciar (re-`/s_new` para
tomar un buffer que cambió); no remuestrea (un frame de archivo por sample del
servidor). La demo carga un WAV en el buffer 5 y lo loopea desde adentro del
DSP Faust:

```sh
cargo run --release --features faust              # en una terminal
python3 examples/json_client.py soundfile quit    # en otra (220 Hz en loop)
```

Para *streaming* de un bus en vez de un buffer estático sigue estando el otro
camino: `PlayBuf`/`BufRd` → bus de audio → control reservado `in` del synth
Faust (ver `docs/schemas.md`).

La versión idiomática desde el cliente Python (allocator de buffers, barrera
`/done`, `/n_set` en vivo y la semántica de foto con una segunda voz) está en
`examples/faust_soundfile.py` — ver la sección de ejemplos en vivo de
`clients/python/GUIA.md`. `soundfile` anda igual en el servidor RT y en el
renderer NRT offline (el espejo de buffers se llena en ambos; regresión en
`tests/golden.rs::soundfile_reads_a_score_buffer_in_nrt`).

### Probar la persistencia de defs entre sesiones

El servidor guarda los defs cargados (`/d_recv` y `/d_faust`) en un directorio
de datos y los recarga solo al arrancar, así un cliente no tiene que reenviar
su biblioteca cada sesión. Está activo por defecto; se controla con
`--data-dir <dir>` y `--no-persist` (o `$CLAUSTERS_DATA_DIR`). Las dos sesiones
van en la **misma** invocación de Bash (el primer servidor termina con `/quit`
antes de arrancar el segundo):

```sh
D=/tmp/clausters-defs; rm -rf "$D"
cargo build --release --features faust
# Sesión 1: definir un Faust def y un SynthDef de UGen, luego salir.
( ./target/release/clausters --data-dir "$D" & PID=$!; sleep 1.0; \
  oscsend localhost 57110 /d_faust ss psine \
    'process = sin(6.283185307179586 * ((+(440.0/48000.0) : _-floor(_)) ~ _)) * 0.2;'; \
  sleep 0.5; oscsend localhost 57110 /quit; wait $PID )

ls "$D/defs/faustdefs"        # psine.json + psine.<sha>.bc
cat "$D/defs/faustdefs/psine.json"   # el source original + version de libfaust + sha

# Sesión 2: NO se reenvía el def; arranca, se instancia y suena.
( ./target/release/clausters --data-dir "$D" & PID=$!; sleep 1.0; \
  oscsend localhost 57110 /s_new siii psine 3001 1 0; sleep 1; \
  oscsend localhost 57110 /n_free i 3001; \
  oscsend localhost 57110 /quit; wait $PID )
```

Debe oírse el seno a 440 Hz en la **segunda** sesión sin haber reenviado
`psine`. Borrar el `.bc` o cambiar de versión de libfaust no rompe nada:
recompila desde el `.json` (la caché de bitcode es no autoritativa). `/d_free
psine` borra ambos archivos. Con `--no-persist` no se escribe ni se recarga
nada.

### Probar la propagación por grupo y GraphDef (M18)

Ambos renderizan offline (NRT), así que basta la librería embed
(`cargo build --release --features embed,realtime`) y no hace falta placa de
audio ni servidor corriendo:

```sh
# /n_set sobre un grupo llega a las tres voces (semántica scsynth):
PYTHONPATH=clients/python python3 examples/group_set.py
#   -> rendered 76800 frames | peak ~0.52  ("one /n_set ... drove all three voices")

# GraphDef: un programa de grafo con superficie nombrada (un puerto "freq"
# que maneja dos osciladores, el segundo escalado a una quinta):
PYTHONPATH=clients/python python3 examples/graphdef.py
#   imprime el JSON de /d_graph y luego: rendered 96000 frames | peak ~0.11

# GraphDef polifónico: parte compartida (mixer, una vez) + miembro per-voz
# (oscilador, voice=True) spawneado por nota con /graph_voice:
PYTHONPATH=clients/python python3 examples/graphdef_poly.py
#   -> rendered 43200 frames | peak ~0.12  (cuatro voces solapadas)
```

A nivel servidor lo cubren `tests/group_nset.rs` (un `/n_set`/`/n_map` sobre un
grupo se propaga al subárbol y para en cada synth) y `tests/graphdef.rs`
(`/d_graph` valida + persiste, `/graph_new` instancia los miembros **compartidos**
con buses privados y superficie, `/graph_voice` spawnea un subgrafo **per-voz**,
`/n_set` sobre instancia o voz resuelve contra su superficie, `/n_free` libera y
recupera buses, y `/midi_bind` a una GraphDef toca voces por nota). Para probarlo
en vivo por OSC: cargar las defs miembro con `/d_recv`, enviar el GraphDef con
`/d_graph`, instanciar con `/graph_new nombre id 0 0`, spawnear una voz con
`/graph_voice id voz freq 440` y moverla con `/n_set voz puerto valor`. Por MIDI:
`/midi_bind canal nombreGraph` (con `clausters --midi` y `aconnect`) y cada nota
spawnea una voz.

### Probar la operación MIDI-standalone (M19)

El payoff de M16+M17+M18: tocar el server desde un controlador **sin programar
nada por OSC**. Los `/midi_bind` se persisten en `midi.json` y se restauran al
arranque (después de las defs/graphdefs). Recipe completo (necesita `oscsend`):

```sh
cargo build --release
examples/midi_standalone.sh
```

Sesión 1 define un SynthDef + un GraphDef y bindea el canal 0; sesión 2 reinicia
con `--midi` y el binding vuelve solo (la instancia compartida de la GraphDef se
re-crea — verificable con `/g_queryTree i 0`). Después se enruta un controlador
(o `aplaymidi` de un `.mid`) al puerto virtual `clausters` con `aconnect` y cada
nota suena. Un `boot.json` opcional (`[{"graph":"nombre","ports":{...}}]`)
instancia grafos standalone al arranque. Cubierto por `tests/midi_standalone.rs`
(dos servers reales sobre un mismo data dir: el binding GraphDef y el preset
boot reviven) y `tests/persistence.rs`/`tests/midi.rs` (round-trip de `midi.json`
y restore + nota tocable).

## 4. Checklist de funcionalidades

| Funcionalidad | Automático | A mano |
|---|---|---|
| Servidor OSC, status/quit/notify | `tests/osc.rs` | `osc_ping status` |
| Node tree, grupos, orden, add actions | `tests/engine.rs` | secuencia `/g_new` de arriba |
| SynthDefs JSON de UGens, `/d_recv` | `tests/synthdef.rs` | `osc_ping vibrato` |
| Buses de audio/control, `In`/`Out` | `tests/engine.rs` | `/c_set`, `/n_set out` |
| Feedback intra-synth, `LocalIn`/`LocalOut` (1 bloque) | `tests/feedback.rs`, `tests/rt_safety.rs` | `json_client.py feedback` (comb resonante) |
| RT-safety (cero allocs en audio) | `tests/rt_safety.rs` | — |
| Envolventes `EnvGen` (formas, sostén por gate, done actions) | `tests/envgen.rs`, `tests/rt_safety.rs`, `test_synthdef.py` | `python3 clients/python/examples/envelope.py` |
| Tasas de cálculo `ir`/`kr`/`ar`/`dr` + init pass + driver demand (S1) | `tests/rates.rs`, `tests/rt_safety.rs` (`rate_substrate`) | por JSON crudo `/d_recv` (ver sección S1) |
| JIT Faust (factory, paridad de señal) | `tests/faust_smoke.rs` | — |
| Hilo compilador, `/d_faust` asíncrono | `tests/faust_compiler.rs` | `/d_faust` + `/dumpOSC` |
| Schema JSON→Box, errores con ruta | `tests/faust_json.rs` | def `jsine` de arriba |
| Schema JSON→Signal API (input/delay/recursion/self) | `tests/faust_signal.rs` | `json_client.py signal` |
| FaustSynth en el árbol, zonas, buses | `tests/faust_synth.rs` | def `fsine` de arriba |
| Paridad UGen↔Faust (goldens), interop en grupos | `tests/faust_parity.rs` | `json_client.py ugen faust` (suenan juntos) |
| MIDI estándar: actuación + transporte ALSA/midir (M17) | `tests/midi.rs` | `--midi` + `aplaymidi` -> `osc_ping status` (synths=1) |
| Cliente que genera JSON (ambos formatos) | — | `examples/json_client.py` |
| Buffers `/b_*`, hilo NRT, WAV (hound) | `tests/buffers.rs` | `json_client.py buffer`, `oscsend /b_*` de arriba |
| `PlayBuf`/`BufRd` (loop, interpolación, canales) | `tests/buffers.rs` | demo `buffer` (sine 330 Hz, luego quinta arriba) |
| Bundles con timetag, sample-accurate | `tests/scheduling.rs` | `json_client.py bundle` (arpegio agendado) |
| Reloj de samples, `/clock` + `/sched` (M8) | `tests/osc.rs`, `tests/scheduling.rs` | `python3 examples/sample_clock.py` |
| Grupos auto-ordenados, `/g_sortMode` + `/g_queryTree` (M12) | `tests/auto_order.rs` | `python3 examples/auto_order.py` |
| Grupos paralelos, `/g_parallel` + `--workers` (M13) | `tests/parallel.rs`, `tests/rt_safety.rs` | `cargo run --release --example bench` (sección parallel) |
| Transporte shm + data plane (M14) | `tests/ipc.rs` | `--shm` + `python3 examples/shm_client.py` |
| C ABI embebida, render síncrono (M14) | `tests/ipc.rs` (feature embed) | `python3 examples/embed_render.py` |
| Transporte WebSocket, navegador-alcanzable (`--ws`, siempre activo) | `osc::ws` (`cargo test --lib osc::ws`) | `--ws` + `python3 examples/ws_ping.py` (y `examples/ws_ping.html`) |
| Memoria acotada + alineación (M10) | `tests/capacity.rs` | leer la tabla en `docs/architecture.md` |
| Modo NRT, partituras, tests dorados | `tests/golden.rs` | `json_client.py score` + `clausters --nrt` |
| Denormales (FTZ/DAZ por hilo + `-ftz 2` Faust) | `tests/denormals.rs`, tail en `tests/golden.rs` | — |
| Waveforms y tablas Faust (`waveform`/`rdtable`/`rwtable`) | `tests/faust_json.rs` | `json_client.py wavetable` |
| Persistencia de defs en disco + caché de bitcode (M16) | `tests/persistence.rs` | `--data-dir`, dos sesiones de arriba |
| `/n_set`/`/n_map` sobre un grupo: propagación scsynth (M18) | `tests/group_nset.rs` | `python3 examples/group_set.py` |
| GraphDef: programa de grafo + superficie nombrada (M18) | `tests/graphdef.rs` | `python3 examples/graphdef.py` |
| GraphDef per-voz `/graph_voice` + `/midi_bind` a GraphDef (M18) | `tests/graphdef.rs` | `python3 examples/graphdef_poly.py` |
| MIDI-standalone: bindings persistidos + boot preset (M19) | `tests/midi_standalone.rs`, `tests/persistence.rs`, `tests/midi.rs` | `examples/midi_standalone.sh` |
| Config TOML compartida (usuario + proyecto) | `cargo test -p clausters-core config` | ver "Config" abajo (`examples/config.toml`) |
| Benchmarks del grafo | — | `cargo run --release --example bench` |
| Documentación de desarrollo (M9) | `cargo doc --no-deps` sin warnings | leer `docs/architecture.md` |
| Documentación integral: README + libro mdBook + rustdoc (M15) | `mdbook build` y `cargo doc` limpios | leer `README.md` y el libro |

Con esto el plan original (M0–M7), la bifurcación F (F0–F5) y M8–M14
(salvo M11) están completos; de los «Milestones futuros» de PLAN.md queda
solo M11 (`/n_map`).

## Config: archivo TOML compartido

Servidor y clientes leen un único archivo TOML de configuración (solo lectura
para los programas; el esquema completo está en `docs/configuration.md` y un
ejemplo comentado en `examples/config.toml`). Dos capas: la de **usuario**
(`$CLAUSTERS_CONFIG`, o `~/.config/clausters/config.toml`) y la de **proyecto**
(el `clausters.toml` más cercano subiendo desde el directorio actual), que pisa
a la de usuario. Precedencia total: flag de CLI > proyecto > usuario > default.

Prueba de precedencia (servidor), en una sola invocación de shell:

```sh
mkdir -p /tmp/cfgtest && cd /tmp/cfgtest
printf '[server]\nsample_rate = 44100\n' > user.toml
printf '[server]\nsample_rate = 48000\n' > clausters.toml   # proyecto pisa usuario
export CLAUSTERS_CONFIG=/tmp/cfgtest/user.toml
# sin flag: gana el proyecto (48000)
( clausters & PID=$!; sleep 1.5; python3 -c "from clausters import Session; print(Session.live().server)"; \
  kill $PID 2>/dev/null )
# con flag: gana la CLI (96000)
clausters --sample-rate 96000   # ver la linea de arranque "... 96000 Hz ..."
```

(La línea de arranque del servidor imprime el sample rate efectivo; con
`--sample-rate 96000` debe decir 96000 aunque el archivo diga 48000.)

App standalone sin intérprete: con un bundle guardado en un data-dir (GuiDef +
SynthDefs/GraphDefs/FaustDefs + `boot.json`) y `[standalone].gui` en la config,
`clausters-gui --standalone --data-dir <dir>` levanta servidor embebido + GUI y
abre la ventana, cargando todo desde disco. El cliente Python escribe ese bundle
(`clients/python/examples/gui_standalone.py`).

## 5. Problemas frecuentes

- **No suena**: cpal abre el dispositivo default de ALSA; en escritorios
  con PipeWire/PulseAudio funciona vía el plugin ALSA. Verificar que algo
  más suene (`aplay -l`) y que el servidor imprima la línea de arranque.
- **`cargo build --features faust` no enlaza**: libfaust no está donde se
  espera. Verificar `ls ~/.local/lib/libfaust.so` o exportar
  `FAUST_PREFIX`. Tras cambiarlo, `cargo clean -p clausters` para que
  build.rs lo relea.
- **`/d_faust` responde `/fail "server built without faust support"`**: el
  servidor se compiló sin `--features faust`.
- **`import("stdfaust.lib")` falla**: falta la stdlib en
  `<prefijo>/share/faust` (la instala el `make install` de libfaust).
- **Puerto ocupado** (`Address already in use`): quedó otro servidor vivo;
  `osc_ping quit` o matar el proceso.
- **Los tests con feature crashean en paralelo**: no debería pasar (hay un
  lock global de compilación precisamente por esto), pero si se llama a la
  FFI de libfaust desde código propio, toda compilación debe pasar por
  `faust::compiler::ffi_lock()` — libfaust no tolera compilaciones
  concurrentes en un proceso.
