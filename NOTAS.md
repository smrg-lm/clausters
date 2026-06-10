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

## Próximo: M5 — Buffers

Pool de buffers, hilo NRT para I/O de disco, `/b_alloc`, `/b_read` (hound),
`PlayBuf`/`BufRd`, replies asíncronos `/done` por comando.
