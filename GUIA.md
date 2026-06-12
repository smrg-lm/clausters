# Guía de compilación y prueba

Cómo compilar **Clausters**, correrlo y probar todo lo que hay hasta ahora
(milestones M0–M7, F0–F5 y M9). Pensada para Linux / Ubuntu 24.04 o más
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
cargo test                       # 82 tests, no necesita placa de audio
```

Los tests cubren: protocolo OSC con round-trips UDP reales
(`tests/osc.rs`), el motor offline con asserts de señal — frecuencia por
cruces por cero, RMS — (`tests/engine.rs`), el formato de SynthDefs
(`tests/synthdef.rs`), los buffers: hilo NRT, round-trips de WAV con
`hound`, `PlayBuf`/`BufRd` con igualdad exacta muestra a muestra y el ciclo
`/b_*` por OSC (`tests/buffers.rs`), el scheduling sample-accurate de
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
# clausters F5 — silent until /s_new | 44100 Hz, 2 channels | OSC on 127.0.0.1:57110 | ...
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

A mano con `oscsend` (los `/b_*` son asíncronos: responden `/done` o
`/fail`, visibles con `/dumpOSC` en la consola del servidor o desde el
cliente Python):

```sh
oscsend localhost 57110 /b_allocRead is 10 /ruta/a/un.wav
oscsend localhost 57110 /b_query i 10          # responde /b_info: frames, canales, sr
oscsend localhost 57110 /b_write isssii 10 /tmp/copia.wav wav float -1 0
oscsend localhost 57110 /b_zero i 10
oscsend localhost 57110 /b_free i 10
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
`/n_after`, `/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`; cualquier
otro responde `/fail`. Bundles con timetag pasado se ejecutan al llegar
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
cargo run --release --example bench --features faust   # + def Faust JIT
```

La columna `x real time` es el headroom: con N synths, cuántas veces más
rápido que 48 kHz procesa. En esta máquina ≈1800 voces sinusoidales.

### Qué probar a mano (núcleo)

Con el servidor corriendo y `oscsend` (los replies no se ven con oscsend;
para ver replies usar `osc_ping`):

```sh
# Eco de todo lo que llega, en la consola del servidor
oscsend localhost 57110 /dumpOSC i 1

# Un synth "default" dentro de un grupo, y orden de ejecución
oscsend localhost 57110 /g_new iii 1 1 0        # grupo 1 al final del root
oscsend localhost 57110 /s_new siii default 1000 1 1   # synth al final del grupo 1
oscsend localhost 57110 /n_set isf 1000 freq 330
oscsend localhost 57110 /n_set isf 1000 amp 0.3
oscsend localhost 57110 /g_freeAll i 1          # silencio: vació el grupo

# Buses de control (atómicos, sin pasar por el hilo de audio)
oscsend localhost 57110 /c_set if 7 220.0
cargo run --example osc_ping -- status          # y /c_get vía tests
```

Protocolo implementado hasta M6: `/status`, `/quit`, `/notify`, `/dumpOSC`,
`/s_new` (add actions 0–4), `/n_free`, `/n_set`, `/n_before`, `/n_after`,
`/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`, `/c_get`, `/d_recv`,
`/d_free`, los buffers `/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write`,
`/b_zero`, `/b_free` (asíncronos vía hilo NRT, responden `/done cmd
bufnum`) y `/b_query` (responde `/b_info`), más `/d_faust` con el feature.
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

git clone --depth 1 -b 2.81.10 https://github.com/grame-cncm/faust
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
cargo test --features faust      # 118 tests (los 82 del núcleo + 36 de Faust)
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
(p. ej. `at $.in[0].op: unknown op "zzz"`). Se ve con `/dumpOSC` o desde
los tests.

**Interop**: synths UGen y Faust conviven en el mismo árbol y mezclan en
los mismos buses — sonar `beep` (UGen) y `fsine` (Faust) a la vez.

Convención de controles de un synth Faust: los parámetros declarados por la
UI del def (sliders, botones) por su label, más dos nombres reservados:
`out` (primer bus de salida, default 0 = hardware izquierdo) e `in` (primer
bus de entrada, para defs que procesan señal).

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

Nota: **no hay op `soundfile`** a propósito — los archivos de audio van a
buffers (`/b_allocRead`) y se cruzan a un def Faust como señal:
`PlayBuf`/`BufRd` → bus de audio → control reservado `in` del synth Faust
(ver `docs/schemas.md`, «Tables and waveforms»).

## 4. Checklist de funcionalidades

| Funcionalidad | Automático | A mano |
|---|---|---|
| Servidor OSC, status/quit/notify | `tests/osc.rs` | `osc_ping status` |
| Node tree, grupos, orden, add actions | `tests/engine.rs` | secuencia `/g_new` de arriba |
| SynthDefs JSON de UGens, `/d_recv` | `tests/synthdef.rs` | `osc_ping vibrato` |
| Buses de audio/control, `In`/`Out` | `tests/engine.rs` | `/c_set`, `/n_set out` |
| RT-safety (cero allocs en audio) | `tests/rt_safety.rs` | — |
| JIT Faust (factory, paridad de señal) | `tests/faust_smoke.rs` | — |
| Hilo compilador, `/d_faust` asíncrono | `tests/faust_compiler.rs` | `/d_faust` + `/dumpOSC` |
| Schema JSON→Box, errores con ruta | `tests/faust_json.rs` | def `jsine` de arriba |
| FaustSynth en el árbol, zonas, buses | `tests/faust_synth.rs` | def `fsine` de arriba |
| Paridad UGen↔Faust (goldens), interop en grupos | `tests/faust_parity.rs` | `json_client.py ugen faust` (suenan juntos) |
| Cliente que genera JSON (ambos formatos) | — | `examples/json_client.py` |
| Buffers `/b_*`, hilo NRT, WAV (hound) | `tests/buffers.rs` | `json_client.py buffer`, `oscsend /b_*` de arriba |
| `PlayBuf`/`BufRd` (loop, interpolación, canales) | `tests/buffers.rs` | demo `buffer` (sine 330 Hz, luego quinta arriba) |
| Bundles con timetag, sample-accurate | `tests/scheduling.rs` | `json_client.py bundle` (arpegio agendado) |
| Modo NRT, partituras, tests dorados | `tests/golden.rs` | `json_client.py score` + `clausters --nrt` |
| Denormales (FTZ/DAZ por hilo + `-ftz 2` Faust) | `tests/denormals.rs`, tail en `tests/golden.rs` | — |
| Waveforms y tablas Faust (`waveform`/`rdtable`/`rwtable`) | `tests/faust_json.rs` | `json_client.py wavetable` |
| Benchmarks del grafo | — | `cargo run --release --example bench` |
| Documentación de desarrollo (M9) | `cargo doc --no-deps` sin warnings | leer `docs/architecture.md` |

Con esto el plan original (M0–M7), la bifurcación F (F0–F5) y M9 están
completos; lo que sigue está en «Milestones futuros» de PLAN.md (M8,
M10–M14).

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
