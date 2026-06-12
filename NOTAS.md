# Notas de completado

Registro de lo implementado por Claude en cada milestone (ver PLAN.md).

## M0 — Esqueleto (completado 2026-06-10)

**Qué hay:** un binario que abre el dispositivo de audio por defecto con cpal y
suena una sinusoide de 440 Hz a amplitud 0.2. Verificado en esta máquina:
44100 Hz, 2 canales, sin errores de stream.

### Estructura

```
src/
├── lib.rs              # crate lib para que los tests usen el motor
├── main.rs             # arranca el backend y queda esperando (Ctrl-C)
├── server/
│   ├── engine.rs       # Engine: process_block() de 64 frames, no conoce cpal
│   └── backend.rs      # cpal + BlockAdapter (solo con feature `realtime`)
├── dsp/sinosc.rs       # SinOsc por acumulación de fase (fase en f64)
├── node/mod.rs         # stub — M2
└── osc/mod.rs          # stub — M1
tests/sine.rs           # tests offline del motor (2 tests, pasan)
```

### Decisiones tomadas

- **Motor desacoplado del backend**: `Engine::process_block(&mut [f32])` procesa
  bloques de `BLOCK_SIZE = 64` frames intercalados contra memoria. cpal vive solo
  en `backend.rs`. Esto habilita los tests sin dispositivo y el futuro modo NRT (M7).
- **Feature `realtime`** (default on): cpal es dependencia opcional;
  `cargo test --no-default-features` corre sin ALSA — es lo que debe usar CI.
- **`BlockAdapter`**: cpal entrega buffers intercalados de tamaño variable, no
  múltiplo de 64; el adapter pide bloques al motor y retiene el sobrante entre
  callbacks (`pos` arranca saturado para forzar el primer bloque).
- **Formatos de sample**: f32, i16, u16 vía `cpal::FromSample`; otros formatos
  devuelven error explícito.
- **Fase del oscilador en `f64`** para no degradar afinación en sesiones largas.
- El callback no aloca (todo pre-alocado en la construcción del adapter); aún sin
  guardián `assert_no_alloc` — entra en M2 junto con los FIFOs.

### Verificación

- `cargo test --no-default-features`: 2 tests pasan — frecuencia 440 Hz ±5 por
  cruces por cero, RMS ≈ 0.2/√2, sin NaN, canales coherentes.
- `cargo run --release` abre el stream y suena (probado 2026-06-10).

### Dependencias del sistema

- Linux: requiere `libasound2-dev` y `pkg-config` para compilar con la feature
  `realtime` (alsa-sys los necesita).

## M1 — Servidor OSC (completado 2026-06-10)

**Qué hay:** el binario ahora levanta, además del audio, un servidor OSC por UDP
en `127.0.0.1:57110` que implementa `/status`, `/quit`, `/notify` y `/dumpOSC`
con la semántica de scsynth. Verificado de punta a punta contra el binario real:
`/status` responde `/status.reply` con los sample rates del dispositivo y `/quit`
apaga el servidor limpiamente.

### Qué se agregó

```
src/osc/server.rs       # OscServer: socket UDP, dispatch de comandos, replies
src/main.rs             # arranca backend + OSC; el loop OSC corre en el main thread
src/lib.rs              # re-exporta rosc para tests y clientes
examples/osc_ping.rs    # cliente mínimo: /status (+ /quit) para pruebas a mano
tests/osc.rs            # 5 tests de integración por UDP real
```

### Comportamiento implementado

- **`/status`** → `/status.reply` con el formato scsynth de 9 argumentos:
  `(1, #UGens, #synths, #groups, #defs, avg_cpu, peak_cpu, sr_nominal, sr_real)`.
  Los contadores van en cero hasta que M2 conecte el node tree; los sample rates
  son los reales del dispositivo (Double).
- **`/notify 1|0`** → registra/desregistra la dirección del cliente y responde
  `/done /notify clientID` (IDs desde 1; registrar dos veces conserva el ID).
  La lista de clientes queda lista para las notificaciones `/n_go`/`/n_end` de M2.
- **`/quit`** → responde `/done /quit` y el loop retorna; main dropea el backend.
- **`/dumpOSC 0|1`** → activa/desactiva el log de mensajes parseados por stdout.
- **Comando desconocido / argumentos inválidos** → `/fail <cmd> <motivo>`, sin
  matar el servidor.
- **Bundles**: se ejecutan inmediatamente (recursivo); el scheduling por timetag
  es M6.

### Decisiones tomadas

- El servidor OSC corre **en el main thread** (bloqueante en `recv_from`); el
  audio vive en el hilo del callback de cpal. El hilo de red puede alocar y hacer
  I/O libremente — la frontera RT-safe (FIFOs) llega en M2.
- `rosc` se **re-exporta desde la lib** para que los tests de integración y los
  clientes usen exactamente la misma versión.
- Bind a `127.0.0.1` (no `0.0.0.0`) por defecto; exponerlo será opción de CLI.
- `ECONNREFUSED` en `recv_from` (rebote ICMP de un reply a un cliente ya cerrado,
  comportamiento de Linux) se ignora y se sigue sirviendo.
- Tests de integración con servidor en **puerto efímero** (`127.0.0.1:0`) y UDP
  real, hilo joineado tras `/quit` — corren en paralelo sin colisiones.

### Verificación

- `cargo test`: 7 tests pasan (5 de OSC + 2 del motor M0).
- E2E manual: `cargo run --release` + `cargo run --example osc_ping -- quit`
  (probado 2026-06-10; el servidor salió limpio tras `/quit`).

## M2 — FIFO RT-safe + node tree (completado 2026-06-10)

**Qué hay:** el servidor ahora arranca en silencio (como scsynth) y suena solo por
comandos: `/s_new` instancia el synth hardcodeado "default" (SinOsc con controles
`freq`/`amp`), `/n_set` lo modifica en vivo y `/n_free` lo libera. Toda la
comunicación red→audio va por ring buffers lock-free; el hilo de audio no aloca
nunca, verificado por el test guardián con `assert_no_alloc`.

### Qué se agregó

```
src/server/engine.rs    # reescrito: Cmd/Garbage FIFOs (rtrb), Counters, engine_pair()
src/node/mod.rs         # NodeTree: slab pre-alocado de 1024 slots, DFS con stack propio
src/node/default_synth.rs # DefaultSynth "default": SinOsc + controles freq(0)/amp(1)
src/osc/server.rs       # + /s_new, /n_free, /n_set; contadores reales en /status
tests/engine.rs         # 6 tests del motor (reemplaza tests/sine.rs)
tests/rt_safety.rs      # guardián assert_no_alloc sobre process_block
```

### Arquitectura implementada (el patrón scsynth)

- **`engine_pair()`** divide el servidor en dos mitades: `Engine` (hilo de audio)
  y `EngineHandle` (hilo de red), conectadas solo por dos FIFOs SPSC de `rtrb`
  (1024 entradas cada uno):
  - **Comandos** (red→audio): `Cmd::{AddSynth, FreeNode, SetControl}`. El synth
    viaja ya construido y boxeado — el hilo de audio solo lo enchufa, O(1).
  - **Basura** (audio→red): `Garbage::{Freed, Rejected}`. El hilo de audio nunca
    dropea un `Box`; lo devuelve entero y el hilo de red lo dropea en
    `collect_garbage()`, que corre tras cada paquete y cada 100 ms por timeout
    del socket (también ahí viajan los comandos rechazados: ID duplicado o tabla
    llena).
  - Si el FIFO de basura se llena: lista local pre-alocada de 64; si también se
    llena, `mem::forget` (leak deliberado — la única opción RT-safe).
- **`NodeTree`**: slab de 1024 slots pre-alocados (`MAX_NODES`), búsqueda lineal
  por ID (suficiente por ahora), hijos del grupo raíz en orden de ejecución, DFS
  iterativo con stack pre-alocado. `Group` existe estructuralmente; `/g_new`
  llega en M4.
- **Contadores**: el hilo de audio publica `synths`/`ugens` con stores atómicos
  relaxed; `/status.reply` los lee — los contadores ya son reales.

### Decisiones tomadas

- `/n_set` se adelantó de M3 porque `Cmd::SetControl` salía gratis y permite
  probar el motor en vivo (cambiar freq sin recrear el synth).
- IDs automáticos (`/s_new` con -1) desde 2_000_001, como el contador alto de
  scsynth. ID 0 (raíz) y negativos se rechazan con `/fail`.
- Add actions 0 (head) y 1 (tail) sobre la raíz; 2–4 responden `/fail` hasta M4.
- Controles por nombre (`freq`, `amp`) o por índice (0, 1); desconocidos se
  ignoran en silencio, como scsynth.
- `/n_free` de un ID inexistente se ignora (el `/fail` asíncrono necesita el
  reply FIFO de M4). Los rechazos del motor se loguean al recolectar basura.
- El `/status` enviado inmediatamente después de un comando puede mostrar el
  conteo viejo: los comandos se aplican al inicio del siguiente bloque (~1.45 ms
  a 44.1 kHz). Es la semántica asíncrona de scsynth, no un bug.

### Verificación

- `cargo test`: 14 tests pasan — 6 del motor (sinusoide, mezcla de dos synths,
  cambio de pitch en vivo, silencio tras free, rechazo de ID duplicado), 7 de
  OSC (incluye el round-trip red→FIFO→audio→/status con reloj manual) y el
  guardián RT: 400 bloques procesando, insertando y liberando 32 synths bajo
  `assert_no_alloc`, sin una sola alocación.
- E2E manual con el binario real (2026-06-10): `osc_ping status beep status quit`
  — beep audible a 440 Hz, re-afinado a 660 Hz en vivo, liberado, servidor
  apagado limpio. El example `osc_ping` ganó el modo `beep`.

## M3 — SynthDefs (completado 2026-06-10)

**Qué hay:** los clientes ya definen synths en vivo: `/d_recv` recibe una SynthDef
en JSON (formato propio, no el `.scsyndef` binario de SC), el intérprete la valida
y compila, y `/s_new` instancia grafos arbitrarios de UGens. El `DefaultSynth`
hardcodeado desapareció — "default" es ahora una SynthDef construida por el mismo
intérprete y registrada al arranque.

### Qué se agregó

```
src/synthdef/mod.rs      # SynthDefSpec (serde/JSON), compile() con validación, default_spec()
src/synthdef/instance.rs # UGenSynth: vector de UGens + wires, impl SynthNode
src/dsp/mod.rs           # trait UGen { process(ctx, inputs, output) }, helper at()
src/dsp/{sinosc,binop,noise,registry}.rs  # SinOsc refactorizado, Add/Sub/Mul/Div, WhiteNoise
src/node/mod.rs          # trait SynthNode; el árbol guarda Box<dyn SynthNode>
tests/synthdef.rs        # 12 tests: formato, validación, señal (FM/vibrato incluido)
```

### El formato (ejemplo completo en el doc de src/synthdef/mod.rs)

```json
{"name": "beep",
 "controls": [{"name": "freq", "default": 440.0}],
 "ugens": [{"kind": "SinOsc", "inputs": [{"control": 0}]},
           {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.2}]}],
 "out": 1}
```

Entradas: `{"const": x}`, `{"control": i}`, `{"ugen": j}` (solo UGens anteriores —
orden topológico validado). `compile()` rechaza con mensajes que nombran el nodo
culpable (`ugens[2].inputs[0]: references ugen 3; only earlier...`) y viajan al
cliente en `/fail`.

### Decisiones tomadas

- **Trait `SynthNode`** (prerequisito de la bifurcación F): el árbol y los FIFOs
  manejan `Box<dyn SynthNode>` — `UGenSynth` hoy, `FaustSynth` en F3, sin tocar
  motor ni árbol.
- **Las defs viven solo en el hilo de red** (`HashMap<String, Arc<SynthDef>>`):
  las instancias se construyen ahí y viajan ya listas; el hilo de audio nunca ve
  la tabla. `/d_free` solo saca la def del mapa — los synths vivos conservan su
  `Arc` (semántica scsynth exacta).
- **Resolución de nombres de controles en el hilo de red**: espejo
  `node_id → Arc<SynthDef>` mantenido desde `/s_new` y limpiado al recolectar
  `Garbage::Freed` — así `Cmd::SetControl` sigue siendo POD y el hilo de audio
  no compara strings.
- **Wiring sin alocar**: `UGenSynth::process` arma las entradas de cada UGen en
  un array fijo en el stack (`MAX_UGEN_INPUTS = 8`) con `split_at_mut` sobre los
  wires — el orden topológico garantiza que las entradas solo miran wires
  anteriores. Verificado por el guardián `assert_no_alloc`.
- UGens iniciales: `SinOsc` (freq modulable por señal — FM/vibrato funciona),
  `Add`/`Sub`/`Mul`/`Div`, `WhiteNoise` (xorshift con seed por instancia, sin
  `rand`). El resto del catálogo (filtros, EnvGen, PolyBLEP) queda para M4+.
- `/d_recv` acepta el JSON como Blob o String OSC.

### Bug encontrado: rosc 0.10.1 y blobs múltiplo de 4

El decoder de rosc sobre-lee el padding de blobs cuya longitud es múltiplo de 4 y
devuelve `Eof` en paquetes válidos (afectaría a clientes reales, no solo a los
tests). Workaround en `OscServer::run`: anexar 4 bytes cero al datagrama antes de
decodificar — inofensivo para paquetes bien formados (quedan como remainder sin
parsear). Considerar reportarlo upstream.

### Verificación

- `cargo test`: 31 tests pasan — 12 de synthdef (roundtrip JSON, validación de
  los 6 errores de compilación, señal: frecuencia/RMS de defs interpretadas,
  mezcla por `Add`, ruido, vibrato por FM), 12 de OSC (incluye `/d_recv` con
  def inválida → `/fail` con el error de compilación, `/d_free`, `/n_set` por
  nombre vía espejo), 6 del motor y el guardián RT (ahora procesando instancias
  interpretadas).
- E2E real (2026-06-10): `osc_ping status vibrato status quit` — def "vibrato"
  (5 UGens, FM) enviada por `/d_recv` como blob JSON, `/done` recibido, sonó
  1.2 s audible, `/status` durante la reproducción: 5 ugens / 1 synth / 2 defs.

## M4 — Buses y orden (completado 2026-06-10)

### Qué quedó hecho

- **Buses** (`src/dsp/mod.rs`): `Buses` con 128 buses de audio (`[f32; 64]`,
  propiedad del hilo de audio, limpiados cada bloque) y 1024 buses de control
  compartidos (`ControlBuses`: `Arc<Vec<AtomicU32>>` con bit-cast de f32,
  stores/loads relaxed — lock-free en ambos hilos). Los buses `0..channels`
  son las salidas de hardware. `ProcessCtx` ahora lleva `sample_rate` +
  `&mut Buses` y se pasa a todos los UGens.
- **UGens de E/S** (`src/dsp/io.rs`): `Out` (suma al bus — varios synths sobre
  el mismo bus se mezclan, semántica scsynth), `ReplaceOut` (sobreescribe),
  `In` (copia de un bus de audio), `InCtl` (lee un bus de control como
  constante de bloque). **Cambio de formato**: las SynthDefs ya no llevan el
  campo `out`; la salida es exclusivamente vía UGens `Out` (una def sin `Out`
  es silenciosa). La def "default" ahora termina en dos `Out` (buses 0 y 1).
- **Árbol de nodos** (`src/node/mod.rs`, reescrito): el grupo raíz (ID 0) vive
  en el slot 0 del slab y no se puede liberar/mover; cada nodo guarda su
  `parent`; grupos anidados con `children` pre-alocados
  (`MAX_GROUP_CHILDREN=256`, raíz `MAX_NODES`) y rechazo por capacidad antes
  de insertar (nunca crece un Vec en el hilo de audio). Add actions 0–4
  completas (`Replace` libera el subárbol del target). `move_node`
  (`/n_before`/`/n_after`) con chequeo de ciclo por ancestros y de capacidad
  al cruzar de grupo. `free` recursivo, `free_all` (vacía el grupo) y
  `deep_free` (libera solo synths, conserva subgrupos) — todos sin alocar,
  con un `free_stack` pre-alocado separado del `dfs_stack`. Los nodos salen
  por un *sink* (`&mut dyn FnMut(FreedNode)`) que reporta ID + parent.
- **Engine** (`src/server/engine.rs`): es dueño de los `Buses`; la salida
  interleaved se copia de los buses 0..channels (ya no hay `mix`/`scratch`).
  `Cmd` extendido: `AddSynth`/`AddGroup` con `target` + acción, `FreeNode`,
  `FreeAllInGroup`, `DeepFreeGroup`, `MoveNode`, `SetControl`. `Garbage` con 4
  variantes (Freed/Rejected × Synth/Group). FIFO nuevo de eventos
  (`NodeEvent { Go|End, id, parent_id, is_group }`, capacidad 2048, entrega
  best-effort) para `/n_go`/`/n_end`. `GarbageSink` interno presta los campos
  del engine por separado para evitar el doble préstamo con el árbol.
  `Counters` suma `groups` (inicializado en 1: la raíz existe antes del primer
  tick). `BLOCK_SIZE` se movió a `dsp` (re-export en `engine` para
  compatibilidad).
- **OSC** (`src/osc/server.rs`): `/s_new` con add actions 0–4 y target real;
  nuevos `/g_new` (tripletas id/acción/target), `/g_freeAll`, `/g_deepFree`,
  `/n_before`/`/n_after` (pares); `/c_set`/`/c_get` servidos directo en el
  hilo de red sobre los atomics (sin round-trip al engine; `/c_get` responde
  con un `/c_set` de pares índice/valor, como scsynth). `collect_garbage`
  drena además el FIFO de eventos y manda `/n_go`/`/n_end`
  (`[id, parent, -1, -1, isGroup]`) a los clientes de `/notify`. `/status`
  reporta el conteo real de grupos.

### Decisiones

- `/c_set`/`/c_get` no pasan por el FIFO de comandos: los buses de control son
  atomics compartidos y el hilo de red opera directo. Un synth los ve recién
  en su próximo bloque (mismo efecto que pasar por el FIFO).
- `Out` suma y `ReplaceOut` sobreescribe → el orden de ejecución es audible y
  testeable: los tests de orden usan un "silencer" (`ReplaceOut` de 0.0) que
  gana o pierde el bus según esté después o antes de la fuente.
- El reply FIFO genérico para `/fail` asíncrono quedó cubierto por el FIFO de
  eventos (`/n_go`/`/n_end`); los rechazos del engine siguen saliendo como
  `Garbage::Rejected*` con log en stderr (un `/fail` asíncrono real
  necesitaría guardar el remitente por comando — se evalúa en M5/M6).

### Verificación

- `cargo test`: 47 tests pasan — 15 del motor (mezcla por bus, orden
  audible + `MoveNode`, before/after/replace, grupos anidados con free
  recursivo, `free_all`/`deep_free`, eventos go/end, buses de control desde el
  hilo de red), 16 de OSC (nuevos: `/g_new` + conteo de grupos en `/status`,
  `/g_freeAll`, roundtrip `/c_set`/`/c_get`, notificaciones `/n_go`/`/n_end` a
  clientes `/notify`), 15 de synthdef (semántica Out/ReplaceOut/In/InCtl, def
  sin `Out` silenciosa) y el guardián RT (ahora con grupo + 32 synths, move y
  free recursivo bajo `assert_no_alloc`).
- E2E real (2026-06-10): `osc_ping status vibrato status quit` contra el
  binario M4 — la def "vibrato" reescrita con UGens `Out` (7 ugens) sonó
  audible; `/status` durante la reproducción: 7 ugens / 1 synth / 1 grupo.

## F0 — Toolchain Faust y FFI mínimo (completado 2026-06-10)

Primer milestone de la bifurcación F (SynthDefs vía Box API de Faust + JIT
LLVM). El objetivo era medir el riesgo real del toolchain; resultado: **mucho
más barato de lo previsto** (JIT ≈ 10 ms por def).

### Hallazgos del toolchain

- **El libfaust de Ubuntu no sirve para embeber**: `libfaust2t64` (2.81.10)
  se compila sin backend LLVM (no depende de libLLVM) y no existe paquete
  `-dev` con headers. Había que compilar desde fuente.
- **Crates evaluados y descartados**: `faust-build`/`faust-types` hacen
  codegen Faust→Rust en build-time (necesitan el compilador `faust` y el DSP
  como fuente estática) — no embeben el JIT. No hay binding mantenido de
  libfaust. Decisión: **binding propio escrito a mano** contra los headers
  reales (~30 funciones); bindgen queda para F1+ si la superficie crece
  (evita la dependencia de libclang por ahora).
- **Build desde fuente** (receta reproducible, sin sudo):
  `git clone --depth 1 -b 2.81.10 github.com/grame-cncm/faust` +
  `make most` + dos ajustes de caché cmake en `build/faustdir`:
  `-DINCLUDE_DYNAMIC=ON` (el target `most` no construye la .so) y
  `-DLINK_LLVM_STATIC=off -DLLVM_CONFIG=llvm-config-20`; después
  `make install PREFIX=$HOME/.local`. Deps de sistema: `cmake`,
  `llvm-20-dev`, `libzstd-dev`, `zlib1g-dev`.
- **Link estático de LLVM falla en Ubuntu sin `libpolly-20-dev`** (Polly va
  en paquete aparte) — con `LINK_LLVM_STATIC=off` no hace falta: se enlaza
  `libLLVM.so` monolítica.
- **Mediciones** (Faust 2.81.10 + LLVM 20.1.8): `libfaust.so` = 11 MB
  (dinámica contra `libLLVM.so.20.1`, 137 MB ya presente como lib de
  sistema); la alternativa estática (`libfaustwithllvm.a`) = 35 MB.
  Latencia JIT de la def del smoke test: **~10 ms**; instanciación + init:
  **~0.08 ms**. Compilación completa de Faust desde fuente: ~10 min en 8
  cores.

### Qué quedó hecho

- **Feature `faust`** en Cargo.toml (apagado por defecto; el core compila y
  testea sin libfaust en el sistema).
- **`build.rs`**: con el feature activo localiza libfaust vía `FAUST_PREFIX`
  (fallback `~/.local`, luego `/usr/local`), enlaza `-lfaust` y agrega rpath
  para que tests y binarios corran sin `LD_LIBRARY_PATH`.
- **`src/faust/ffi.rs`**: FFI mínimo verificado contra los headers de la
  build exacta — contexto (`createLibContext`/`destroyLibContext`), Box API
  (`CboxReal`, `CboxWire`, `CboxSeq/Par/Split/Rec`, los aplicados `Cbox*Aux`,
  `CboxHSlider`) y JIT (`createCDSPFactoryFromBoxes`, instancia, `compute`).
  Detalle de la C API: los operadores existen en dos formas — `CboxAdd()`
  (primitivo, caja de 2 entradas) y `CboxAddAux(b1, b2)` (aplicado) — porque
  C no tiene overloading.
- **`tests/faust_smoke.rs`** (gated): construye el equivalente de `SinOsc`
  desde primitivas — `sin(2π·phasor(freq))`, `phasor = (+(f/SR) : wrap) ~ _`,
  `wrap = _ <: _ - floor(_)`, `freq` como hslider en su default 440 — lo
  compila por JIT, renderiza 1 s offline y asserta frecuencia (±5 Hz) y RMS;
  segundo test: una box inválida llena el buffer de error (4096 bytes) en
  vez de crashear.

### Verificación

- `cargo test --features faust`: 49 tests (los 47 del core + 2 del smoke).
- `cargo test` sin feature: igual que antes, sin tocar libfaust.
- libfaust instalada en `~/.local` (lib + headers + stdlib de Faust).

## F1 — Hilo compilador Faust (completado 2026-06-10)

### Qué quedó hecho

- **`src/faust/compiler.rs`**: `CompilerThread` dedicado ("faust-compiler")
  con cola mpsc de `CompileRequest { name, source, client }` y canal de
  `CompileResult` de vuelta. El hilo de red drena resultados en su loop
  (tras cada paquete y en el tick de GC) y manda el reply asíncrono al
  cliente que pidió: `/done "/d_faust" <name>` o `/fail` con el error del
  compilador Faust verbatim. Shutdown limpio por Drop (cierra el canal y
  joinea).
- **`src/faust/factory.rs`**: `FaustFactory` (wrapper con ownership del
  puntero, `Drop` → `deleteCDSPFactory` en hilo no-RT). Refcount vía
  `Arc<FaustFactory>` en la tabla `faust_defs` del OscServer; las instancias
  de F3 retendrán clones para que la factory nunca muera antes que ellas.
- **OSC**: `/d_faust name source` (String o Blob UTF-8) encola la
  compilación; `/d_free` también limpia la tabla Faust; `/status` cuenta
  ambas tablas de defs. Sin el feature, `/d_faust` responde
  `/fail "server built without faust support"`.
- F1 compila **fuente Faust** (`createCDSPFactoryFromString`); el mapeo
  JSON→Box API entra en F2 reemplazando solo el cuerpo de `compile()`.

### Hallazgo: libfaust no tolera compilación concurrente

Dos `CompilerThread` compilando a la vez en el mismo proceso → SIGSEGV
(verificado: los tests en paralelo crasheaban, en serie pasaban). El estado
global del compilador Faust no es thread-safe ni siquiera para
`createCDSPFactoryFromString` (no solo el lib context de la Box API). Fix:
lock global de proceso (`compiler::ffi_lock()`) alrededor de toda llamada
FFI de compilación; el smoke test de F0 también lo toma. Un servidor tiene
un solo hilo compilador, pero los tests (y cualquier embedder con varios
servidores) necesitan el lock.

### Verificación

- `cargo test --features faust`: 55 tests (47 core + 2 smoke F0 + 6 de F1:
  hilo directo con orden FIFO y errores legibles, round-trip OSC asíncrono
  de `/d_faust` con `/done`/`/fail`, conteo en `/status`, `/d_free`).
  Estable en 3 corridas seguidas (sin razas).
- `cargo test` sin feature: intacto. Clippy limpio.

## F2 — Esquema JSON → Box API (completado 2026-06-10)

### Qué quedó hecho

- **`src/faust/boxes.rs`** (nuevo): intérprete JSON → llamadas a la Box API.
  El schema (documentado con tabla y ejemplo en el doc del módulo) refleja
  la C API uno-a-uno: atajos (número = constante, `"_"` = wire, `"!"` =
  cut), objetos `{"op": …}` para composición (`seq`/`par`/`split`/`merge`
  n-arios con fold a izquierda, `rec` binario), 18 binarios (aritmética,
  comparaciones, bitwise, `delay`), 19 unarios (trig, exp/log, redondeos,
  casts), `select2`/`select3`, UI (`hslider`/`vslider`/`nentry`/`button`/
  `checkbox`/`hgroup`/`vgroup`) y el escape hatch `{"op": "faust", "src":
  "…"}` que compila un programa Faust completo a box vía `CDSPToBoxes` —
  acceso a toda la stdlib (`os.osc`, `fi.`) componible con primitivas.
- **Errores con ruta**: la validación estructural se hace al construir y
  cada error lleva la ruta del nodo JSON culpable desde la raíz `$` (p. ej.
  `at $.in[0].in[1]: unknown op "zzz"`); los errores semánticos de Faust
  (aridades de composición, entradas colgantes) llegan verbatim del paso de
  factory.
- **`src/faust/compiler.rs`**: `CompileRequest` ahora lleva un
  `CompilePayload::Source` (F1) o `::Json` (F2); guard `LibContext` (lock +
  `createLibContext`/`destroyLibContext` en Drop); `FaustArgs::stdlib()`
  pasa `-I $PREFIX/share/faust` (búsqueda como build.rs: `FAUST_PREFIX` →
  `~/.local` → `/usr/local`) tanto a `createCDSPFactoryFromString` — los
  defs de fuente cruda ahora pueden `import("stdfaust.lib")` — como a los
  fragmentos.
- **`src/faust/ffi.rs`**: superficie Box API completada (~45 símbolos
  nuevos: binarios/unarios `Aux`, delays, selects, UI, `CDSPToBoxes`),
  verificados contra `nm -D libfaust.so` además del header.
- **OSC**: `/d_faust name def` distingue por sniffing — si el def empieza
  con `{` es JSON, si no es fuente Faust (la fuente Faust top-level nunca
  empieza con `{`, el sniff no es ambiguo).

### Hallazgo: bug upstream en `boxFmod()`

`CboxFmodAux(a, b)` construye `(a, b) : abs` — `boxFmod()` en
`compiler/box_signal_api.cpp` devuelve `gGlobal->gAbsPrim->box()` (bug de
copy-paste presente en 2.81.10 y todavía en master-dev). Lo detectó el test
"kitchen sink" que ejercita cada op del schema una vez (necesario porque el
linking dinámico es lazy: un símbolo mal tipeado en el FFI a mano recién
explota al llamarlo). Workaround: `fmod` no usa el binding sino un
fragmento `CDSPToBoxes("process = fmod;")` que devuelve el primitivo real
de 2 entradas; `CboxFmodAux` quedó sin bindear con nota en ffi.rs.

### Verificación

- `cargo test --features faust`: 64 tests (47 core + 2 smoke F0 + 8 F1/OSC
  + 7 F2: sine JSON con paridad de frecuencia y RMS contra el smoke de F0,
  fragmento stdlib componiendo `os.osc` con primitivas, import de stdlib
  desde fuente cruda, errores de validación con ruta del nodo, error de
  fragmento con ruta + mensaje del compilador, kitchen sink de todos los
  ops). Estable en 3 corridas seguidas.
- `cargo test` sin feature: 47 tests, intacto. Clippy limpio (solo los dos
  `Default` preexistentes de dsp).

## F3 — FaustSynth en el árbol (completado 2026-06-10)

### Qué quedó hecho

- **`src/faust/synth.rs`** (nuevo): `FaustDef` y `FaustSynth`.
  - **`FaustDef`** es lo que ahora guardan las tablas de defs: la factory
    compilada más los parámetros (nombre, init, min, max, step) y la aridad
    de E/S, descubiertos **una sola vez** sondeando una instancia
    descartable en el hilo compilador (`FaustDef::probe`, llamada por
    `compile()` después de crear la factory). Así `/s_new` y `/n_set`
    resuelven nombres de controles en el hilo de red sin tocar libfaust.
  - **`FaustSynth: SynthNode`**: se construye en el hilo de red
    (`createCDSPInstance` + `initCDSPInstance(sr)` alocan), recolecta las
    zonas `FAUSTFLOAT*` con `UIGlue` al instanciar, viaja ya armado por el
    cmd FIFO, y `process()` solo llama `computeCDSPInstance` (la única
    llamada RT-safe de libfaust) más copias de staging. `Drop` borra la
    instancia — siempre corre en el hilo de red porque los nodos liberados
    salen por el garbage FIFO; el `Arc<FaustDef>` del synth garantiza
    instancia-muere-antes-que-factory.
- **Convención de controles**: índices `0..n` = parámetros UI del def (orden
  de declaración, labels pelados — los grupos se aplanan); después dos
  nombres reservados: `out` (índice n) e `in` (n+1), el primer bus de audio
  al que mapean salidas/entradas. Defaults `out=0`, `in=0`. Clamp para que
  el span completo de canales quede dentro de los buses.
- **Mapeo de buses**: la E/S de Faust es `float**` no intercalada como
  nuestros buses, pero el synth pasa por buffers de staging propios: las
  salidas **suman** al bus (semántica de `Out`, los synths mezclan) y las
  entradas se copian antes de escribir salidas (una cadena in-place
  `in == out` queda correcta).
- **OSC**: `/s_new` instancia defs Faust como cualquier otro (helper
  `make_synth` que busca en ambas tablas); el espejo `node_defs` ahora
  guarda un enum `NodeDef::{UGen, Faust}` para resolver nombres en
  `/n_set`. `/d_free` con instancias vivas no rompe nada (refcount).
- **FFI**: `UIGlue` (struct repr(C) de 13 callbacks de CInterface.h) y
  `buildUserInterfaceCDSPInstance`.

### Decisiones

- Instanciación en el hilo de red **sin** `ffi_lock()`: crear instancias
  desde una factory ya compilada es independiente del estado global del
  compilador (es código JIT + malloc; FaustLive/faustgen~ lo hacen
  concurrente con compilaciones). El lock sigue siendo solo para compilar.
- `ugen_count() = 1` por instancia Faust en `/status.reply`.
- La SR queda congelada por `instanceInit` (ver previsión en PLAN.md);
  el probe usa 48 kHz fijo porque params y aridad no dependen de la SR.

### Verificación

- `cargo test --features faust`: 73 tests (64 de F2 + 8 de `faust_synth`:
  probe de params y controles reservados, sine en el árbol con
  frecuencia/RMS, `/n_set` por zona, ruteo por `out`, cadena por bus de
  entrada con `in`, mezcla UGen+Faust en el mismo bus (interop de F4
  adelantada), free con factory sobreviviendo al `/d_free`, y el ciclo
  completo por OSC con engine tickeado a mano + 1 en `rt_safety`: 8
  FaustSynths insertados, procesados, recontrolados y liberados bajo
  `assert_no_alloc`). Estable en 3 corridas.
- `cargo test` sin feature: 47 tests, intacto. Clippy limpio.

## F4 — Paridad e interop (completado 2026-06-10)

### Qué quedó hecho

- **`tests/faust_parity.rs`** (nuevo): tests dorados de grafos equivalentes,
  renderizados lado a lado **en el mismo engine** (UGen al canal 0, Faust al
  canal 1, mismos bloques):
  - **Sine**: `SinOsc(440)·0.2` contra el mismo grafo vía JSON→Box
    (`sin(2π·phasor)` con `delay 1` para alinear la fase: nuestro `SinOsc`
    arranca en fase 0, el phasor crudo `(+(f/SR) : wrap) ~ _` arranca en
    `f/SR`). Igualdad muestra a muestra con tolerancia `4e-3` — `SinOsc`
    acumula fase en f64 y Faust (-single) en f32, así que no puede ser
    exacta — y un assert de discriminación: las mismas señales corridas
    una muestra **deben** violar la tolerancia (un offset de fase de 1
    muestra pica en ≈ 0.0115, bien arriba).
  - **Ganancia bit-exacta**: una sine UGen alimenta el bus 4; una cadena
    UGen `In·0.5` y una Faust `_ * 0.5` lo leen en el mismo bloque hacia
    los canales 0 y 1. Misma multiplicación f32 sobre las mismas muestras:
    **cero** bits de diferencia (la aritmética sin estado es idéntica entre
    los dos mundos; solo los osciladores divergen por precisión).
  - **Grupo compartido**: synth UGen + synth Faust como hermanos en un
    grupo no-raíz, mezclan al mismo bus (RMS de la suma) y un solo
    `FreeAllInGroup` los libera juntos (2 en el garbage FIFO).
- **`examples/json_client.py`** (nuevo): cliente de ejemplo en Python, solo
  stdlib (encoder/decoder OSC a mano: i, f, s, b, d). **Genera** los dos
  formatos de def programáticamente — `SynthDefBuilder` para `/d_recv`
  (noise con AM) y helpers `box()`/`hslider()`/`faust()` para `/d_faust`
  (sine desde primitivas + def con stdlib vía escape hatch) — y maneja el
  ciclo completo: `/done`//`/fail`, `/s_new` con controles por nombre,
  `/n_set`, `/n_free`, `/status`, `/quit`. Demos: `status ugen faust quit`.
- **`docs/schemas.md`** (nuevo): documentación de referencia de ambos
  schemas (en inglés, como los docs de código): formato SynthDef JSON
  completo (tabla de UGens, formas de input, semántica `Out`/`ReplaceOut`,
  errores), defs Faust (fuente vs JSON, tabla de ops espejo de la Box API,
  controles reservados `out`/`in`, errores con ruta `$`), y el ciclo OSC
  común.

### Verificación

- `cargo test --features faust`: 76 tests (73 + 3 de paridad). `cargo test`
  sin feature: 47, intacto. Clippy limpio (solo las 2 warnings
  preexistentes de `Default`).
- E2E real del cliente Python contra el server release con feature, en una
  sola invocación: `status` (reply con doubles — el decoder necesitó el tag
  `d`), `/d_recv` amnoise `/done`, `/d_faust` jsine y jstdlib `/done`,
  synths sonando y `/quit`.
- Una corrida de la suite completa tuvo 1 fallo esporádico en
  `faust_synth` que no se reprodujo en 7 corridas posteriores (5 aisladas
  + 2 completas); sospecho del test OSC por UDP bajo carga. Vigilar si
  reaparece.

## M5 — Buffers (completado 2026-06-10)

### Qué quedó hecho

- **`src/dsp/buffer.rs`** (nuevo): `Buffer` — datos f32 intercalados +
  frames/canales/sample-rate — **inmutable una vez construido**, compartido
  como `Arc<Buffer>`. Pool de 1024 slots (`BufferPool`) en el engine;
  espejo en el hilo de red para `/b_query` y para darle a
  `/b_read`/`/b_write`/`/b_zero` el contenido/forma actual. La inmutabilidad
  es la decisión central: sin locks ni aliasing entre hilos (scsynth muta
  memoria compartida; nosotros pagamos una copia por reemplazo). UGens de
  grabación necesitarán otro esquema.
- **`src/server/nrt.rs`** (nuevo): hilo NRT con el mismo patrón que el
  compilador Faust (mpsc requests/results, drenado en el loop del servidor
  OSC). Jobs: `Alloc` (cero), `AllocRead`/`Read`/`Write` (WAV vía `hound`:
  int 1–32 bits escalado a ±1 y float32; `Read` superpone el archivo sobre
  una copia del contenido actual conservando la forma), `Free`. **Una sola
  cola = los comandos de buffer completan en orden de envío** (por eso
  hasta `/b_free` pasa por ahí: no puede sobrepasar a un alloc pendiente).
- **Engine**: `Cmd::SetBuffer { index, Option<Arc<Buffer>> }` swapea el
  slot; lo reemplazado sale como `Garbage::FreedBuffer` (el último `Arc`
  nunca se suelta en el hilo de audio). `ProcessCtx` ahora lleva
  `buffers: &[Option<Arc<Buffer>>]`.
- **UGens** (`src/dsp/buf.rs`): `PlayBuf` (bufnum, canal, rate, loop; rate
  en frames por sample de salida — 1.0 = sr del servidor, el cliente
  compensa con `sr_archivo / sr_servidor`; fase f64; silencio al final si
  no loopea) y `BufRd` (bufnum, canal, fase en frames, loop; la fase fuera
  de rango wrapea con loop y clampea sin él). Ambos **mono** con entrada
  `chan` (nuestros UGens tienen una salida): un archivo estéreo son dos
  lectores sample-locked. Interpolación lineal. Sin trigger ni done action
  todavía.
- **OSC**: `/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write` (solo WAV;
  int16/int24/float), `/b_zero` (reemplaza por uno en cero de la misma
  forma), `/b_free` — asíncronos, responden `/done cmd bufnum` o `/fail` —
  y `/b_query` → `/b_info` (síncrono desde el espejo). `leaveOpen` se
  acepta y se ignora (sin streaming).
- **Cliente Python**: demo `buffer` (escribe WAV con el módulo `wave`,
  `/b_allocRead`, rate correcta desde `/b_info` + `/status`, `/n_set` de
  rate, `/b_free`). Schema y comandos documentados en `docs/schemas.md`;
  pasos manuales en GUIA.md.

### Verificación

- `cargo test`: 61 tests (47 + 13 de `tests/buffers.rs` + 1 en
  `rt_safety`); con feature: 90. Los tests de buffers incluyen igualdad
  **exacta** muestra a muestra (playback rate 1, loop, canales,
  interpolación de `BufRd` con valores representables), round-trip WAV
  float sin pérdida, grilla de cuantización int16 verificada
  (escala 32767 al escribir, 1/32768 al leer), slicing de archivo,
  overlay de `/b_read`, errores (mismatch de canales, archivo inexistente)
  y el ciclo `/b_*` completo por OSC con engine tickeado a mano.
- `rt_safety`: instalar, reemplazar (incluso achicando con un `PlayBuf`
  leyendo), vaciar el slot y liberar el synth — cero allocs en el hilo de
  audio, 3 items por el garbage FIFO.
- E2E real: server release + `json_client.py buffer` (sine 330 Hz a
  22050 Hz tocada a rate 0.5 sobre servidor de 44100, quinta arriba con
  `/n_set`, `/b_free` con `/done`). Clippy limpio (solo las 2 warnings
  preexistentes de `Default`).

## M6 — Scheduling sample-accurate (completado 2026-06-10)

### Qué quedó hecho

- **Slices en `ProcessCtx`**: `offset` + `frames` — normalmente el bloque
  entero, pero un bundle agendado parte el bloque en el sample del evento y
  todos los nodos procesan solo el sub-rango. `UGenSynth` recorta wires e
  inputs a `frames`; `In`/`Out`/`ReplaceOut` indexan los buses en `offset`;
  `FaustSynth` copia staging parcial y llama `compute(frames)`. Esto va
  **más allá de scsynth real**, que cuantiza los bundles al bloque de 64 y
  necesita `OffsetOut` para compensar — acá el split es genuino y no hace
  falta.
- **Cola en el engine**: `Cmd::Schedule { time, cmds }` — tiempo absoluto
  en samples, comandos ya construidos (synths boxeados) en el hilo de red.
  `Vec<ScheduledBundle>` pre-alocada (1024), inserción ordenada estable
  (FIFO en empates, `partition_point` + `insert` sin alocar) y `remove(0)`
  al vencer; el shell `Vec` ejecutado vuelve como `Garbage::SpentBundle`
  (capacidad heap liberada en red); cola llena = bundle rechazado entero
  por el mismo camino. `process_block`: drena comandos inmediatos al inicio
  del bloque, luego loop de segmentos ejecutando los bundles vencidos en su
  offset exacto (tardíos en offset 0).
- **Reloj y conversión NTP**: el engine publica `now` (samples procesados,
  `AtomicU64`) cada bloque; el hilo de red convierte
  `delta = timetag − SystemTime::now()` y agenda en
  `current_samples() + delta·sr`. Timetag inmediato (`{0,1}`) o pasado =
  ejecución al llegar (los pasados loguean "late", como scsynth). Bundles
  anidados se agendan independientes por su propio timetag.
- **Traducción de mensajes** (`schedule_message`): `/s_new` (controles
  aplicados al boxear, espejo `node_defs` actualizado al agendar, IDs -1
  resueltos), `/n_set` (por nombre vía espejo), `/n_free`,
  `/n_before`/`/n_after`, `/g_new`, `/g_freeAll`/`/g_deepFree` y `/c_set`
  — este último como `Cmd::SetControlBus` nuevo: la forma inmediata escribe
  los atomics en red, pero la agendada debe caer en su sample exacto. Lo no
  agendable responde `/fail "… cannot be scheduled in a timed bundle"`.
- **Cliente Python**: `bundle(seconds_ahead, *packets)` (timetag NTP a
  mano) y demo `bundle`: un arpegio agendado entero por adelantado.

### Verificación

- `cargo test`: 72 (61 + 10 de `tests/scheduling.rs` + 1 en `rt_safety`);
  con feature: 102 (+1: FaustSynth partido a mitad de bloque con def
  constante, borde exacto). Los tests de scheduling son **sample-exactos**
  con señales DC: disparo a mitad de bloque (sample 100 = bloque 1 offset
  36), tres eventos partiendo un mismo bloque (10/30/50), atomicidad del
  bundle, empates en orden de llegada, tiempos fuera de orden, tardíos en
  offset 0, `/c_set` agendado (escalón en sample 32), cola llena (1025º
  rechazado y devuelto entero), y el round-trip OSC con timetag NTP real
  (ventana de tolerancia sobre el reloj publicado).
- `rt_safety`: 16 bundles en offsets impares (encolar ordenado, partir,
  ejecutar) — cero allocs, 16 shells de vuelta. Clippy limpio (solo las 2
  warnings preexistentes).
- E2E real: server release + `json_client.py bundle` (5 notas agendadas
  por adelantado, ritmo regular). Banner ahora dice `clausters M6`.

## M7 — Modo NRT + tests dorados (completado 2026-06-11)

### Qué quedó hecho

- **Refactor previo** (`src/osc/translate.rs`): la traducción mensaje→`Cmd`
  salió de `OscServer` a un `CmdTranslator` compartido — tablas de defs,
  espejo `node_defs`, auto-IDs, `translate()` (el viejo `schedule_message`),
  `d_recv`/`d_free`, `make_synth` — más `parse_buffer_msg` (los seis `/b_*`
  async → `NrtJob`, antes seis handlers casi iguales) y `parse_d_faust`.
  El server delega; `/s_new` inmediato ahora también pasa por `translate`.
- **Renderer** (`src/server/render.rs`): `Score` (eventos ordenados estables
  por tiempo) + `render`/`render_to_vec`/`render_to_wav`. Una `Score` se
  carga del formato binario de scsynth (`[i32 BE tamaño][paquete OSC]`…;
  el timetag cuenta **segundos desde el inicio del render**, tag inmediato
  = 0). El render es mono-hilo con las dos mitades de `engine_pair`: los
  comandos agendables viajan como `Cmd::Schedule` por la cola M6 (mismo
  split sub-bloque que en vivo → el render offline es idéntico sample a
  sample a una toma en vivo perfecta), y los async (`/d_recv`, `/d_faust`,
  `/d_free`, `/b_*`) corren **síncronos** antes de avanzar el tiempo
  (semántica scsynth NRT): `run_job` y `faust::compiler::compile` ahora son
  `pub` y se llaman en línea; los buffers se instalan con el resto del
  bundle (swap sample-accurate). El render termina en el tiempo del último
  bundle (sus comandos no suenan): cerrar la partitura con un bundle dummy.
  Errores estrictos: comando desconocido/fallido aborta con tiempo y
  mensaje; bundles dropeados por cola llena también (mejor que notas
  faltantes silenciosas en un golden).
- **CLI**: `clausters --nrt score.osc out.wav [--rate] [--channels]
  [--format float|int16|int24]` — disponible **sin** el feature `realtime`
  (sin cpal); `--help`. El cliente Python ganó `score_bundle` (timetag
  relativo) y la demo `score` que escribe `/tmp/clausters_score.osc`.
- **Bug nuevo de rosc, arreglado para ambos modos**: el bug de blobs
  múltiplo-de-4 también rompe blobs **dentro de un bundle** — el elemento
  se parsea de su propio slice con prefijo de tamaño (el padding externo no
  llega) y rosc devuelve el bundle con el contenido **silenciosamente
  vacío**. `osc::decode_packet` parte los bundles a mano (recursivo) y solo
  decodifica mensajes hoja con rosc + padding; lo usan el server UDP y el
  loader de scores. CLAUDE.md actualizado.
- **Goldens** (`tests/golden.rs` + `tests/golden/*.wav` float32, escenas en
  `tests/common/scenes.rs` compartidas con `cargo run --example
  render_golden`): `arpeggio` (def default, entradas a mitad de bloque,
  `/n_set`, frees escalonados) y `playbuf` (`/d_recv` + `/b_allocRead` a
  44100 con rate compensado, `/c_set` agendado, `/b_zero` a mitad de
  reproducción). Comparación por sample con tolerancia 1e-4 (el sin de
  libm puede variar entre plataformas; misma máquina es bit-exacto) **más**
  asserts de señal independientes (frecuencia por cruces por cero, RMS,
  silencios) para que un golden viejo no bendiga un render roto. Regenerar
  solo a mano y **escuchar antes de commitear**.
- **Benchmark** (`cargo run --release --example bench`): throughput de
  bloques offline → factor de tiempo real a 48 kHz, def default y def
  Faust (con feature). Medición acá: ~1790 synth·xRT estable de 32 a 1000
  synths default (≈1800 voces sinusoidales en tiempo real); 1 synth solo
  ~1000x (domina el overhead fijo por bloque).

### Verificación

- `cargo test`: 80 (72 + 8 de `tests/golden.rs`); `--features faust`: 111
  (+1 `/d_faust` síncrono en NRT). Sin default features también verde.
  Clippy limpio en ambas configs (solo las 2 warnings preexistentes).
- E2E: `json_client.py score` → `clausters --nrt` (release): 11 eventos,
  2.1 s; primer sample no-cero en el frame 4801 (el 4800 es sin(0)=0) y
  última nota liberada exactamente en el frame 96000 — sample-accurate de
  punta a punta. Server en vivo verificado con las demos
  `status ugen buffer bundle quit` tras el refactor.

## Post-M7 — Protección contra denormales (2026-06-11)

A pedido del usuario (la pregunta venía de antes; la técnica estaba
documentada en la skill `realtime-audio` pero sin implementar). Los
subnormales aparecen en estados recursivos que decaen a cero (colas de
filtros, envolventes, recursiones Faust) y en muchas CPUs se resuelven en
microcódigo 10–100x más lento — justo cuando un sonido se apaga. Tres
piezas:

- **`dsp::denormals::flush_to_zero()`**: pone el hilo llamador en modo
  flush-to-zero — MXCSR FTZ+DAZ (bits 15 y 6) en x86-64, FPCR.FZ (bit 24)
  en aarch64, ambos por asm inline (los intrínsecos `_mm_setcsr` están
  deprecados); no-op en otras arquitecturas. Se rearma en cada callback de
  cpal (barato, un par de accesos a registro) y se arma al inicio de
  `render()` — **en los dos modos**, porque FTZ cambia resultados (flushea
  a cero) y el render NRT debe seguir siendo sample-idéntico al vivo.
- **`-ftz 2` en las factories Faust** (`FaustArgs::defaults()`, antes
  `stdlib()`): el código generado flushea las variables recursivas por
  debajo del rango normal — independiente de la arquitectura y del modo
  FPU del hilo. Era la exposición real: los UGens propios actuales no
  tienen estado recursivo decayente (eso llega con LPF/EnvGen), pero un
  def Faust cualquiera sí.
- **Tests**: `tests/denormals.rs` (el switch FPU: resultado y operando
  subnormal colapsan a 0 tras armar; idempotencia; la matemática normal
  intacta — cada `#[test]` corre en su propio hilo, no contamina) y en
  `tests/golden.rs` el tail Faust `1-1' : fi.pole(0.9)` (y[n]=0.9ⁿ sale
  del rango normal cerca del sample 830): ningún sample subnormal y
  `out[1000] == 0.0` exacto. 82 tests núcleo / 114 con faust, goldens
  intactos (las escenas no generaban subnormales).

## F5 — Extensiones Faust: waveforms y tablas (completado 2026-06-12)

El alcance salió de la revisión del 2026-06-12 (ver «Milestones futuros» en
PLAN.md): de la lista original de F5 se implementó lo que aporta hoy —
`waveform` + primitivas de tabla — y se descartó lo que el servidor ya
resuelve por otro camino.

### Qué quedó hecho

- **Tres ops nuevos en el schema JSON→Box** (`src/faust/boxes.rs`):
  - `waveform` con `values` (array no vacío de números): tabla embebida en
    la def, calculada numéricamente por el cliente (wavetables, funciones de
    transferencia para waveshaping) sin formatear fuente Faust. Emite el par
    (tamaño, contenido) como en Faust. FFI: `CboxWaveform` recibe un array
    de boxes `CboxInt`/`CboxReal` **terminado en NULL** (verificado en la
    fuente de faust, `box_signal_api.cpp`).
  - `rdtable` (2 o 3 boxes en `in`) y `rwtable` (4 o 5): se componen como
    `seq(par(...), primitiva)` — exactamente como los helpers `Aux` de
    upstream, que esta vez **no** tienen el slip de `boxFmod` (revisado en
    la fuente). La forma corta es el idioma `wf, idx : rdtable` con un
    `waveform` ocupando (tamaño, init); Faust valida la aridad total al
    compilar.
  - Helper `number_box` compartido con el shorthand numérico (int si entra
    en `c_int`, real si no).
- **Descartes documentados** (en PLAN.md y `docs/schemas.md`): `soundfile`
  — los datos de audio viven en los buffers del servidor; `PlayBuf`/`BufRd`
  → bus → control `in` del def Faust cruza la señal sin copiar nada al
  mundo Faust — y la polifonía nativa de Faust — el node tree es el
  alocador de voces (una voz = un `/s_new`, las instancias comparten
  factory) y el modo polifónico impone convenciones MIDI ajenas al modelo.
  El backend interpreter (sin LLVM) y la Signal API quedan ligados al
  target wasm de M14.
- **Cliente Python**: demo `wavetable` — tabla de 256 puntos (4 armónicos
  de sierra) calculada en Python, normalizada y enviada como `waveform`;
  oscilador con `freq`/`amp` como sliders.
- **Docs**: filas nuevas en la tabla de ops y sección «Tables and
  waveforms» en `docs/schemas.md`, con el patrón buffers-como-señal.
  Banner del server a F5.

### Verificación

- `cargo test --features faust`: 118 (+4: ciclo exacto por la tabla con
  contador `& 3`; oscilador de wavetable de 64 puntos a 440 Hz con RMS
  1/√2; `rdtable` explícito con init constante; `rwtable` escribe-y-lee;
  más 4 casos de error de validación y los ops en el kitchen sink).
  `cargo test` core: 82, intacto. Clippy limpio en ambas configs (solo las
  2 warnings preexistentes).
- E2E: demo `wavetable` contra el server release con faust — `/done
  /d_faust jwavetable`, `/s_new` + `/n_set freq` audibles, `/quit` limpio.

## M9 — Documentación de desarrollo (completado 2026-06-12)

### Qué quedó hecho

- **`docs/architecture.md`** (en inglés, como todo `docs/`): mapa de hilos
  (red / audio / NRT / compilador Faust, más el modo offline mono-hilo y
  dónde se arma el flush-to-zero), mapa de módulos (tabla path → contenido),
  ciclo de vida de la memoria (la regla «se aloca en red/NRT/compilador, se
  usa en audio, se libera en red»; los dos cruces sin FIFO: buses de control
  atómicos y buffers `Arc` inmutables), **tabla de capacidades pre-alocadas
  con el modo de fallo de cada una al llenarse** (verificado caso por caso
  en el código: cmd FIFO → `/fail`, garbage FIFO → lista de retención de 64
  reintentada por bloque y `mem::forget` como último recurso, eventos
  best-effort, cola de schedule → `SpentBundle` no vacío, slab/grupos →
  `Rejected*`), relojes y scheduling (reloj de samples, conversión NTP en el
  hilo de red, split de bloque), y los **8 invariantes** que un cambio no
  puede romper (RT-safety, comandos pre-armados, `decode_packet`
  obligatorio, identidad RT/NRT, buffers inmutables, salida solo por
  `Out`/`ReplaceOut`, core sin features, determinismo en tests).
- **Guía «cómo agregar una UGen»**: ejemplo `Lag` completo (estado en el
  struct, `at()` para entradas, `output.len()` ≠ `BLOCK_SIZE` por los splits,
  `ctx.offset` solo para UGens de bus), registro en `registry.rs` (variante
  + `parse_kind`/`arity`/`build`), tests exigidos (señal + no-alloc +
  golden si corresponde, con la nota de determinismo de `WhiteNoise`) y qué
  documentación actualizar.
- **Decisión (a), UI de Faust**: los labels son los nombres de control a
  propósito (los nombres los pone el autor de la def, como `controls` en el
  JSON UGen); paths de grupos ignorados, primera declaración gana,
  `out`/`in` reservados al final y la def pisa lo reservado si los declara;
  los params NO están ligados a buses de control hoy (eso es M11/`/n_map`).
- **Decisión (b), plugins**: sin plugins dinámicos en v1 — Rust no tiene ABI
  estable; extender = compilar en el crate (la API interna documentada es el
  contrato) y la vía runtime para usuarios es `/d_faust`; si algún día hacen
  falta, C ABI o wasm **versionadas** (lección scsynth, misma política que
  el layout shm de M14).
- **Punteros**: CLAUDE.md ahora lista los dos docs de `docs/` (con «keep
  them current»); schemas.md abre remitiendo a architecture.md para
  internals.
- **Rustdoc**: las 2 warnings (links a `FaustArgs`, item privado, desde
  `denormals.rs` y `compiler.rs`) corregidas a texto plano; `cargo doc
  --no-deps` limpio con y sin feature.

### Verificación

- Afirmaciones del doc verificadas contra el código antes de escribirlas:
  capacidades y constantes (`engine.rs`, `node/mod.rs`, `dsp/mod.rs`,
  `buffer.rs`), timeout del socket (100 ms), reintento de `pending_garbage`
  por bloque, semilla global de `WhiteNoise`, conversión
  `current_samples() + delta·sr`.
- `cargo test` 82 / `--features faust` 118 — intactos (solo cambiaron
  comentarios de doc en `src/`); `cargo doc` sin warnings en ambas configs.

## M8 — Reloj de samples como timebase del cliente (completado 2026-06-12)

El servidor expone su reloj de samples y acepta agendar por sample absoluto;
el cliente puede usar el reloj de audio como maestro en vez del reloj del
SO (que deriva decenas de ppm respecto del cristal del DAC). Los dos
caminos conviven: NTP (M6) y samples (M8) desembocan en la misma cola
`Cmd::Schedule`, así que clientes de ambos tipos coexisten contra el mismo
servidor.

### Qué quedó hecho

- **`/clock`** → `/clock.reply h <samples> d <sampleRate>`: el contador de
  samples del engine (el `AtomicU64` que ya publicaba desde M6) y la sample
  rate real del dispositivo.
- **`/sched <h target> <b packet>`**: agenda un paquete OSC completo en un
  sample **absoluto**, atómico y sample-accurate (mismo split de bloque que
  M6). Decisiones: mensaje contenedor en vez de reinterpretar el timetag
  (que es formato NTP por especificación — no romper clientes estándar);
  los timetags internos del blob se **ignoran** (un `/sched` = un instante);
  target pasado = próximo bloque, como los bundles NTP tardíos; target
  `i` int32 tolerado (clientes a mano) pero se desborda en <13 h a 48 kHz;
  `/fail` por mensaje malo individual, el resto del paquete dispara igual
  (mismo criterio que `schedule_bundle`); no agendable dentro de un bundle
  NTP ni válido en partituras NRT (los timetags de score ya son exactos).
- **Cliente de referencia `examples/sample_clock.py`** (stdlib, importa los
  helpers OSC de json_client.py, que ganó el tag `h` int64 — encode con
  marker `Int64`, decode — y `reply(quiet=)`): clase `SampleClock` con
  anclas estilo NTP (t0/t1 alrededor de la consulta, par (punto medio,
  contador), incertidumbre = semiancho), ajuste por cuadrados mínimos sobre
  ventana deslizante de 64 anclas (olvido), `now()`/`local_time_of()`, y un
  patrón de 8 notas con espaciado **exacto en samples** agendado por
  adelantado (lead 0.3 s) re-anclando en cada beat. Honestidad del demo: el
  slope necesita minutos de línea de base para mostrar deriva real — en una
  corrida corta domina la cuantización por saltos de buffer del contador
  (ruido acotado: solo afecta cuándo se *envía* un /sched, jamás cuándo
  dispara) — y el reporte lo dice.
- **Docs**: `docs/sample-clock.md` nuevo (protocolo, receta del modelo, por
  qué la latencia no importa, caveats: samples procesados vs escuchados,
  pausa en xruns, saltos de buffer; diferencia con scsynth) + párrafo en
  schemas.md (Timed bundles) + architecture.md (los dos front-ends de la
  misma cola). Banner a M8.

### Verificación

- Tests nuevos: `/clock` reporta el contador y avanza con los bloques
  (tests/osc.rs); validación de argumentos de `/sched` (sin args, sin blob,
  target negativo, blob basura, query no agendable → `/fail` nombrando el
  mensaje); target `Int` + blob bundle con timetag NTP futuro ignorado;
  y el central en tests/scheduling.rs: `/sched` a mitad de bloque y assert
  del **sample exacto** (5026 = target+1 por sin(0)=0) — sin la vecindad
  que necesita el test NTP equivalente. 86 core / 122 con faust, clippy y
  rustdoc limpios.
- E2E con el servidor real: `sample_clock.py` completo (anclas, modelo,
  8 beats audibles regulares, slope reportado) y el one-liner `/clock` de
  GUIA.md (contador avanza ≈22050 por 0.5 s a 44.1 kHz).

## M12 — Grupos auto-ordenados por conexiones de buses (completado 2026-06-12)

El servidor infiere el DAG de dependencias entre nodos a partir de los
buses que cada def lee (`In`, `in` Faust) y escribe (`Out`/`ReplaceOut`,
`out` Faust), y mantiene **grupos auto-ordenados opt-in**: los grupos pasan
a ser canales de multipista y el cliente deja de micro-gestionar el orden.
Cero cambios en el hilo de audio: los re-ordenamientos llegan como
`Cmd::MoveNode` comunes.

### Qué quedó hecho

- **`src/osc/graph.rs`**: `BusUsage` (bitmasks `u128` de lectura/escritura
  + flag `dynamic`), análisis por def — `ugen_usage` (índices de bus
  constantes o por control = estáticos, registrando qué controles son
  índices de bus; índice por señal = `dynamic`) y `faust_usage`
  (`out..out+N` / `in..in+M` por los controles reservados, mismos clamps
  que `FaustSynth`) —, `stable_topo_sort` (Kahn estable: entre los listos
  gana el más temprano del orden actual; barrera = nodo dynamic con aristas
  contra todo según posición; deadlock = ciclo → se libera el más temprano:
  los ciclos conservan orden relativo = un bloque de delay, como un return
  de multipista; `ReplaceOut` cuenta lectura+escritura, así un insert fx
  cae entre las fuentes y los lectores; escritores puros al mismo bus no
  generan arista — mezclar conmuta), y **`TreeMirror`**: espejo del árbol
  en el hilo de red (topología, valores de controles por nodo, usage, flag
  auto por grupo) alimentado por el mismo stream de `Cmd` que recibe el
  engine, con rollback por la basura de rechazos (`remove` idempotente).
- **`CmdTranslator` integra el espejo**: cada brazo de `translate()`
  actualiza el espejo y, si cambia la topología o el usage, re-ordena la
  cadena de ancestros auto (`resort_from`) apéndice de moves al mismo batch
  — por eso funciona igual en inmediato, en bundles con timetag (el sort
  dispara atómico con el bundle) y en **scores NRT** (el renderer comparte
  el translator). `/n_set` sobre un control usado como índice de bus
  re-analiza y re-ordena. `/n_before`/`/n_after` con nodo o target dentro
  de un grupo auto → `Err`/`/fail`. Liberaciones no re-ordenan (quitar
  nodos nunca invalida un orden topológico).
- **Protocolo**: `/g_sortMode groupID mode` (1 = auto, 0 = manual; acepta
  pares; root permitido; agendable), `/g_queryTree [gid] [flag]` →
  `/g_queryTree.reply` **formato scsynth** (flag 1 incluye nombres y
  valores de controles desde el espejo) y `/g_dumpGraph [gid]` →
  `/g_dumpGraph.reply` con el grafo inferido legible (reads/writes/dynamic
  por hijo).
- **Refactor**: los handlers inmediatos duplicados del server (`/s_new`,
  `/n_set`, `/n_free`, `/n_before`, `/n_after`, `/g_new`, `/g_freeAll`,
  `/g_deepFree`) se unificaron en `handle_via_translate` — un solo camino
  de traducción para inmediato/bundle/score, que era prerequisito para que
  el espejo no se desincronice. De paso `/g_new` por bundle ganó la
  validación `id > 0` que solo tenía el camino inmediato.
- **Docs y ejemplo**: `docs/auto-order.md` (reglas del análisis, ciclos,
  barreras, caveat espejo-adelantado-del-engine), sección en schemas.md
  (+ `/g_sortMode` en la lista de agendables), architecture.md (fila de
  módulo + espejo en el hilo de red), `examples/auto_order.py` (cadena
  fuente→fx→master armada al revés: silencio en grupo manual, suena al
  activar `/g_sortMode`, grafo impreso antes/después, segunda voz en head
  que se ordena sola), sección en GUIA.md.

### Decisiones y caveats

- El espejo refleja comandos **al enviarse**: lo agendado en un bundle
  futuro se espeja ya (el queryTree puede mostrar brevemente el estado
  futuro); un re-sort que corre contra un bundle pendiente converge al
  siguiente cambio. Documentado.
- Barreras dinámicas: nada se ordena a través de ellas aunque el subgrafo
  estático lo pida (conservador a propósito).
- La capacidad del espejo es la del hilo de red (HashMaps): los límites
  reales los pone el engine y los rechazos ruedan atrás por el garbage.

### Verificación

- 10 tests nuevos en `tests/auto_order.rs` (+1 con faust): cadena invertida
  ordenada y **audible** (RMS exacto 0.1/√2) vs. silencio en grupo manual;
  `/g_sortMode` sobre hijos existentes y vuelta a manual; `/fail` de moves
  manuales en grupos auto y de `/g_sortMode` sobre grupos inexistentes o
  synths; formato completo de `/g_queryTree.reply` con flag 1; barrera
  dinámica reportada y respetada; ciclo de feedback conserva orden de
  inserción con la fuente ordenada antes; `/n_set` de control-índice
  re-ordena (silencio → sonido); score NRT con `/g_sortMode` renderiza la
  cadena invertida; def Faust ordenado por su control reservado `out`.
  **96 core / 133 con faust**, clippy y rustdoc limpios.
- E2E: `examples/auto_order.py` contra el server real — dump antes/después
  muestra el reorden (manual: master,fx,src → auto: src,fx,master) y la
  cadena suena.

## Próximo: features nuevas

El plan original (M0–M7), F0–F5, M8, M9 y M12 están completos. Direcciones
planificadas: M10, M11, M13 (ya tiene su prerequisito M12) y M14 en
«Milestones futuros» de PLAN.md. Sueltas: más UGens (filtros, EnvGen con
done actions, Line), streaming de buffers (`leaveOpen`), `/n_query`,
multi-cliente con notificaciones por ID.
