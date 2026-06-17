# Plan — Clientes de alto nivel para Clausters (Faust-first), con núcleo nativo Rust compartido

Este plan cubre el cliente **Python** (primer destinatario) pero está redactado para servir también a un futuro cliente **JavaScript**: ambos comparten el mismo núcleo nativo Rust y el mismo contrato C-ABI. La parte específica de cada lenguaje es solo el driver de coroutines y los wrappers finos del binding.

> **Nota — sc3 como modelo de referencia.** Ante cualquier duda de diseño o semántica (estructura de módulos, comportamiento de relojes/rutinas, eventos, patterns, interfaces OSC/MIDI, nombres, convenciones), recurrir a [sc3](https://github.com/smrg-lm/sc3) como modelo. Este cliente es una reescritura limpia y podada (Faust-first), pero sc3 es la fuente de verdad sobre cómo deben combinarse y comportarse estas piezas; desviarse de él solo con motivo explícito (lo específico de Clausters: FaustDefs, recursos del servidor, núcleo nativo Rust).

## Contexto

Clausters es el servidor de audio en Rust (estilo scsynth) controlado por OSC. Hoy el único cliente del repo es `clients/python/clausters.py`: la **capa de transporte de bajo nivel** (embed cdylib / shm / render), stdlib-only, con la regla de frontera "solo datos planos cruzan" (bytes in, `array('f')`/floats/ints out). No hay capa alta: armar defs, recursos, eventos y secuenciación queda hoy en mano del usuario.

El objetivo es un **cliente de alto nivel** portando selectivamente las features centrales de [sc3](https://github.com/smrg-lm/sc3) (port de SuperCollider a Python), pero **centrado en FaustDefs** en lugar de SynthDefs, y reusando los recursos del servidor (buses, buffers, unidades generadoras). En paralelo se extrae un **núcleo nativo en Rust** (TempoClock, builtins numéricos, armado OSC) compartido por el servidor y por todos los clientes futuros (Python ahora, JavaScript después), de modo que las operaciones del lado del cliente sean **numéricamente equivalentes** a las del servidor por construcción donde sea posible.

Decisiones acordadas:
- **Repo**: reescritura limpia en `clients/python/` (sc3 como referencia, sin arrastrar SynthDef ni el class-library completo).
- **Rust**: convertir `clausters` en un Cargo **workspace** y extraer un crate núcleo (`clausters-core`).
- **Binding**: un solo **C-ABI** sobre el núcleo, con wrappers finos por lenguaje (ctypes/cffi en Python; N-API o wasm en JS más adelante).
- **Costura**: el núcleo Rust posee builtins, TempoClock (cola + aritmética) **y** el armado de bundles/timetags OSC + conversión contra el sample-clock; frontera "solo datos planos, sin callbacks". El **driver de coroutines (`yield`) queda en cada lenguaje** — el control de flujo no se mueve a Rust.

## Principio rector de la costura

Lo que es transformación de valor o de tiempo (lenguaje-agnóstico) vive en Rust; lo que es control de flujo del lenguaje (las `Routine` que hacen `yield` en Python, los generators/async en JS, la ergonomía de patterns) vive en cada lenguaje. El loop que reanuda los `yield` lo maneja el lenguaje: consulta a la cola Rust "¿qué sigue y cuándo?", duerme, reanuda la rutina y devuelve el próximo tiempo a Rust. No hay callbacks de Rust hacia el lenguaje host — eso preserva la portabilidad multi-lenguaje y la regla "solo datos planos cruzan".

## Arquitectura objetivo

### Workspace Rust (raíz del repo)

Convertir el crate único actual en workspace. Layout propuesto:

```
Cargo.toml                  # [workspace] members
crates/
  clausters/                # crate servidor actual (bin + lib + features realtime/faust/embed)
  clausters-core/           # NUEVO: kernels puros, sin I/O, sin alloc en hot path
  clausters-ffi/            # NUEVO: cdylib C-ABI sobre clausters-core (la "lib para todos los clientes")
clients/
  python/                   # cliente alto Python
  PLAN.md                   # este plan (genérico, también para el futuro cliente JS)
```

`clausters-core` (biblioteca pura, candidata a `no_std` salvo donde necesite `alloc`):
- **builtins**: ops unarias/binarias sobre escalar y sobre slice `&[f32]` — las mismas fórmulas que el servidor. Set base: `add/sub/mul/div` (ya nativas en el servidor), y la matemática superior que hoy en el servidor solo existe vía Faust (`sin/cos/tan/exp/log/sqrt/abs/floor/ceil/min/max/pow/atan2/...`, ver `crates/clausters/src/faust/signals.rs`).
- **tempoclock**: cola de prioridad temporal + aritmética beat↔segundo↔sample, tempo/compás, conversión contra el sample-clock del servidor (que se lee por `/clock` o por el data-plane shm).
- **rng**: generador con semilla que **replica** el del servidor (`WhiteNoise` usa splitmix64/xorshift, `crates/clausters/src/dsp/noise.rs`) para reproducibilidad cliente/servidor.
- **osc**: armado de mensajes/bundles con timetag NTP, reusando `rosc` (ya dependencia del servidor). Conversión `timetag ↔ sample target` para `/sched` y bundles.

`clausters-ffi`: cdylib que exporta el C-ABI del núcleo (versión de ABI explícita, como el embed actual en `crates/clausters/src/embed.rs`). Distinto del `libclausters.so` del embed (ese es el servidor in-process; este es el núcleo de cliente). Dos cdylibs separados, ambos consumibles por ctypes/N-API/wasm.

### Equivalencia numérica — contrato realista

- Ops que el servidor calcula **nativo** (`add/sub/mul/div`, fase de `SinOsc`, RNG de `WhiteNoise`): refactorizar el servidor para que use `clausters-core` → **bit-exacto por construcción** (única fuente de verdad). Cuidar RT-safety: funciones `#[inline]`, sin alloc/lock/IO (CLAUDE.md, `tests/rt_safety.rs`).
- Matemática superior que en el servidor **solo existe vía Faust/LLVM** (`sin`, `log`, etc.): `clausters-core` implementa la **misma fórmula/semántica** (libm), pero la igualdad bit-a-bit con el codegen LLVM de Faust **no está garantizada**. Contrato: misma fórmula + tolerancia documentada; tests de paridad con tolerancia.

### Paquete del cliente (ejemplo Python; el cliente JS espeja la misma estructura)

Reescritura limpia, estructura espejo de sc3 pero podada y Faust-first:

```
clients/python/
  pyproject.toml
  clausters/                       # paquete alto
    __init__.py
    base/                          # port selectivo de sc3/base
      absobject.py  builtins.py  stream.py  clock.py  main.py
      netaddr.py    _oscinterface.py  _midiinterface.py
    seq/                           # port de sc3/seq: event, pattern, streampatterns
    defs/                          # port de sc3/synth, recortado a Clausters (Faust ahora, SynthDef después)
      faustdef.py  signals.py  (synthdef.py + ugens.py más adelante)
      node.py  bus.py  buffer.py  server.py
    _native.py                     # wrapper ctypes sobre clausters-ffi (núcleo Rust)
    transport.py                   # = el actual clausters.py (embed/shm/render), reubicado
```

- `transport.py`: el `clients/python/clausters.py` actual se conserva como capa de transporte (no reescribir; es ortogonal al núcleo). El paquete alto se apoya en él para hablar con el servidor.
- `_native.py`: ctypes sobre `clausters-ffi` (builtins, TempoClock, armado OSC). Frontera de datos planos, igual que `transport.py`.
- `base/builtins.py` + `base/absobject.py`: `AbstractObject`/operandos despachan las ops sobre escalar **o lista** a `_native` (equivalencia con el servidor). Donde el overhead FFI por escalar no compense, fallback puro-lenguaje idéntico en fórmula.
- `base/clock.py`: `TempoClock` envuelve la cola+aritmética nativa; el loop de scheduling (reanudar `yield`) queda en el lenguaje.
- `base/stream.py` / `seq/`: coroutines con `yield`, patterns y eventos — puro Python (en JS: generators/async).
- `defs/signals.py`: **la interfaz de usuario para construir FaustDefs**. Provee una librería de **callables en minúscula** (funciones u objetos invocables) que mapean, en principio, la **Signal API de Faust** (`sin`, `cos`, `add`, `mul`, `delay`, `select2`, `hslider`, `rdtable`, …). La **composición** de estos callables es lo que arma el grafo: una especificación que se serializa a **JSON signal tree** ahora (y a **box tree** más adelante) para enviar con `/d_faust` (ver `crates/clausters/src/faust/`). Convención de diseño firme: **nombres en minúscula incluso para objetos que actúan como funciones** — es una cualidad que facilita el trabajo de programación en Python (composición fluida estilo expresión). El **mismo patrón se reutiliza para las UGens** (`ugens.py`, constructores del grafo SynthDef en JSON).
- `defs/faustdef.py`: **centro del cliente**. Toma el grafo construido con `signals.py` (o Faust source directo) y produce la def para `/d_faust` en sus tres formas (source, JSON box tree, JSON signal tree); maneja controles (labels UI → nombres de control; reservados `out`/`in`). Persistencia/cache en disco la maneja el servidor (M16, bitcode cache). `synthdef.py` (después) hace lo análogo para el grafo de UGens.
- `defs/{node,bus,buffer,server}.py`: allocators client-side de IDs (estilo scsynth: nodos, buses audio 0..127 / control 0..1023, buffers 0..1023), manejo de `/done`/`/fail`, `/notify` → `/n_go`/`/n_end`. NRT: score → `render()` del transport.
- **A portar más adelante desde `sc3/synth`** (recordar): `synthdef.py` + `ugen.py` (representación cliente de las SynthDef de UGens), `synthdesc.py` + `spec.py` (specs de control; en Clausters solo existe `InCtl` como UGen de control), `_graphparam.py` (adapta tipos Python a los tipos que reciben los nodos; revisable, no tiene por qué ser igual, diferible).

### Separación de responsabilidades: cliente agnóstico vs representación del servidor

Hay **dos grupos de abstracciones** que deben quedar bien separados (ver memoria `separacion-cliente-servidor-clausters`):

1. **Agnóstico al servidor** (no sabe de transporte ni de la app servidor): el **timing** (`base/clock.TempoClock`), la **secuenciación** (`base/stream`, `seq`) y la **generación del grafo JSON** (`defs/signals`, `defs/faustdef`, `base/absobject`, `base/builtins`).
2. **Representación + configuración del servidor Clausters**: la clase **`Server`** (`defs/server`) = el servidor corriendo; los **handles+allocators de recursos** (`defs/node`, `defs/bus`, `defs/buffer`); y la **interfaz de comunicación** que la `Server` posee. Elegir comunicación por **memoria compartida** o **embed** = agregar una **interfaz de comunicación nueva** a la `Server`.

Correspondencia con el servidor Clausters:

| Python (cliente) | Representa | Contraparte en el servidor |
|---|---|---|
| `defs/server.Server` | el servidor corriendo + su comunicación | el proceso `clausters` (OSC/UDP; luego shm/embed) |
| `defs/node` (`Synth`/`Group`) | handles + allocator de ids | `src/node` (árbol de nodos) |
| `defs/bus` (`Bus`) | buses audio/control + allocator | buses de `src/dsp` |
| `defs/buffer` (`Buffer`) | buffers + allocator | `src/dsp/buffer` |
| `base/clock`, `base/stream`, `seq` | timing y secuenciación | — (agnóstico) |
| `defs/signals`, `defs/faustdef` | grafo JSON del lado del cliente | `/d_faust`, `src/faust` |

### Interfaces de destino y manejo del tiempo (RT / NRT / MIDI) — pieza central

El punto que hace que **una misma lógica de relojes y rutinas** sirva para tiempo real, render diferido y MIDI sin reescribirla. La división correcta es:

- El **reloj** (`base/clock.TempoClock`) solo agenda y provee tiempo (matemática beat↔segundo↔sample por `_native`, cola de scheduling, drives RT/NRT, reanudar `yield`). **No comunica con el servidor.**
- La **`Server`** posee la **interfaz de destino/comunicación** y **emite** los eventos, computando el timetag a partir del tiempo lógico del reloj de la rutina en curso (`main.current_tt`). Cambiar la interfaz cambia *a dónde* y *en qué modo* (vivo vs diferido) van los eventos; reloj y rutinas no cambian.
- `base/_oscinterface.py`: `OscUDPInterface`/`OscTCPInterface` (RT; TCP aún no en el servidor) y `OscNrtInterface` (acumula en `OscScore` → `render()`). `base/_midiinterface.py`: `MidiRtInterface`/`MidiNrtInterface`+`MidiScore`. shm/embed serían interfaces de comunicación adicionales de la `Server`.

> **Corrección post-C3:** en C2 la comunicación quedó **mal ubicada en `TempoClock`** (campos `target`/`interface`, métodos `send_bundle`/`send_msg`/`_emit`/`_when`). El milestone **C4** la mueve a `Server`. El reloj queda solo con timing.

## Milestones (track "C" del cliente, paralelo al track "M" del servidor)

> Marcadores: **✅ hecho** · **⏳ pendiente** · milestone **sin marca** = futuro, no empezado.

- ✅ **C0 — Workspace + núcleo + FFI**: convertir a workspace; crear `clausters-core` (builtins, tempoclock, rng, osc) y `clausters-ffi` (C-ABI + versión); refactorizar las ops nativas del servidor para consumir `clausters-core`. Tests de paridad numérica servidor↔core (bit-exacto en las nativas; tolerancia documentada vs Faust). Verificar RT-safety intacta. *(Completado 2026-06-17 — ver NOTAS.md.)*
- ✅ **C1 — Scaffold cliente + núcleo accesible**: `pyproject.toml`, paquete `clausters/`, reubicar transport, `_native.py` (ctypes sobre `clausters-ffi`). Smoke: importar, llamar un builtin escalar/lista, instanciar `TempoClock`, armar un bundle OSC, `render()`. *(Completado 2026-06-17 — ver NOTAS.md.)*
- ✅ **C2 — base**: `absobject`/`builtins` (despacho a nativo, escalar+lista), `stream` (Routine/Stream con `yield`), `main` (contexto global, semillas), `clock` (TempoClock sobre núcleo), `netaddr`. Interfaces de destino: `_oscinterface` (`OscUDPInterface` + `OscNrtInterface`/`OscScore`) y `_midiinterface` (`MidiRtInterface` + `MidiNrtInterface`/`MidiScore`), de modo que reloj y rutinas emitan contra una interfaz intercambiable. (`OscTCPInterface` queda como stub: TCP aún no implementado en el servidor.) *(Completado 2026-06-17 — ver NOTAS.md.)*
- ✅ **C3 — defs Faust-first**: `signals.py` (callables en minúscula que mapean la Signal API de Faust; su composición arma el JSON signal tree), `faustdef` (las tres formas para `/d_faust`, controles), `node`/`bus`/`buffer`/`server` (allocators, async `/done`-`/fail`, `/notify`). Vertical slice E2E: construir un grafo con `signals` → `faustdef` → `/d_faust` → `/s_new` → controlar por bus/clock. *(Completado 2026-06-17 — ver NOTAS.md.)*
- ✅ **C4 — Refactor: separación cliente/servidor** (corrección post-C3, quirúrgica — no reescribir lo que funciona) *(Completado 2026-06-17 — ver NOTAS.md)*: sacar la comunicación de `TempoClock` (campos `target`/`interface`, métodos `send_bundle`/`send_msg`/`_emit`/`_when`) y llevarla a `Server`. El reloj queda solo con timing (math + cola + drives + reanudar rutinas) y expone el tiempo lógico/wall; la `Server` posee la **interfaz de comunicación** y **emite**, leyendo el tiempo del reloj de la rutina en curso (`main.current_tt` lleva su `clock`). Reconciliar las dos capas de comunicación hoy duplicadas: `defs/server.UdpConnection` (RT bidireccional, replies) y `base/_oscinterface.Osc*Interface` (envío/acumulación), en una **interfaz de comunicación** coherente que la `Server` posee, con variantes RT (UDP; luego shm/embed) y NRT (score). La `Server` en modo NRT expone `render()`. Actualizar rutinas/tests/`GUIA.md`/ejemplos: el patrón pasa de `clock.send_bundle(...)` a `server.send_bundle(...)`. Sin tocar `signals`/`faustdef`/builtins/núcleo.
  - Criterio de aceptación: `TempoClock` no importa ni referencia ninguna interfaz/NetAddr; el slice E2E (NRT y vivo) y el seam RT/NRT siguen pasando con la nueva ubicación; un cambio de transporte (p. ej. shm) se hace agregando una interfaz de comunicación a la `Server`, sin tocar reloj/seq.
- ✅ **C5 — seq** *(Completado 2026-06-17 — ver NOTAS.md)*: `event`, `pattern`, stream-patterns; un mismo `Pbind`+`TempoClock`+`Server` corre en **RT** (interfaz UDP, servidor vivo) o **NRT** (interfaz score → `render()`) **solo cambiando la interfaz de comunicación de la `Server`**. **Suelta de C5 cerrada (2026-06-17)**: grafo UGen **instance-based** — `defs/ugens.py` (callables minúscula → `Ugen`/`Control`, operadores → `Add/Sub/Mul/Div`, sin contexto global de build) + `defs/synthdef.py` (`SynthDef` → JSON `SynthDefSpec` → `/d_recv`) + `Server.add_synthdef` (RT con `/done`, NRT scoreado en t=0). Paridad **byte-idéntica** con el `default` interno (`tests/test_synthdef.py`), E2E en vivo por UDP, ejemplo `examples/synthdef.py`. (El grafo del SynthDef ya no depende del port de `sc3/synth`; el resto de `sc3/synth` —más UGens del lado servidor— es independiente.)
  - ✅ **Semántica de `TempoClock`** (ver memoria `tempoclock-timebase-clausters`): el tiempo (beats) avanza **solo por los `yield`**; el reloj **monotónico** se usa **solo** para calcular los sleeps → timing **relacional exacto**. Al emitir, el timetag se computa desde el **beat lógico acumulado** (no desde el "ahora"); el timetag OSC usa un reloj **wall** separado (Unix), válido para el servidor. Test de exactitud: `/s_new` en `[0, 0.5, 1.0, 1.5]` exactos.
  - ✅ **Sin globales que se pisen entre hilos** (ver memoria `evitar-estados-globales-clausters`): `main.current_tt` es **thread-local**, así varios `TempoClock` (threads) y un reloj RT en vivo junto a un render NRT corren **en el mismo script** sin clobber. `Server`/`clock` explícitos por instancia; `default_clock` solo como azúcar opcional. Tests en `tests/test_concurrency.py` (thread-local, dos relojes NRT concurrentes, litmus RT+NRT en el mismo script).
  - ✅ **Timebase seleccionable** (`base/timebase.py`): `MonotonicTimebase` (default, eventos por bundle NTP) y `SampleClockTimebase` (anclado al reloj de muestras del servidor — `now = sample()/sr`); en modo sample-clock la `Server` emite por **`/sched <sample_absoluto>`** (sample-accurate, sin drift) en vez de timetag wall. Tests robustos con **ambas** opciones en `tests/test_timebase.py` (pacing, emisión NTP vs `/sched` con sample exacto, latency, y NRT idéntico independiente del timebase); `/sched` validado en vivo.
  - ✅ **Golden de paridad de score**: el render del camino seq (`Pbind`) es **byte-idéntico** al del OSC hand-rolled equivalente (mismo motor del servidor por el embed render) — `tests/test_golden.py` (`list(hi)==list(lo)`, frames 91200), prueba end-to-end de la capa event/pattern/timing.
  - ✅ **Ergonomía de defaults sin globales**: `clausters.Session` (contexto explícito que agrupa `Server`+`TempoClock`, con fábricas `Session.nrt()`/`Session.live()` y `play`/`render`/`run`); **varias sesiones coexisten** (NRT para plot + RT en vivo en el mismo script), sin estado global. `tests/test_session.py`.
  - ✅ **Grafo instance-based** (cerrado 2026-06-17): `defs/ugens.py` + `defs/synthdef.py` (`SynthDef` → `/d_recv`) construyen el grafo UGen **instance-based** (defs concurrentes), sin el estado global de armado de sclang. Contraparte UGen de `signals`/`FaustDef`; paridad byte-idéntica con el `default` interno. El resto de `sc3/synth` (más UGens) es del lado servidor e independiente.
- ✅ **C6 — Anclaje del sample-clock por UDP** *(Completado 2026-06-17 — ver NOTAS.md)*: `defs/clocksync.py` — `SampleClockModel` (least-squares `sample = a + b·t` sobre ventana deslizante de anchors `/clock`, midpoint del round-trip; mismo modelo que `examples/sample_clock.py`) y `UdpSampleClock` (socket propio; `anchor`/`warmup`/`track` en background; `.timebase()` → `SampleClockTimebase`). `Server.sample_clock()` lo construye. Así `SampleClockTimebase` funciona **en vivo por UDP** (sin shm/embed) y la `Server` emite por `/sched` anclado al reloj del servidor. Tests del modelo (recuperación de recta, drift ppm, fallback 1-anchor) + smoke del timebase; **validado en vivo** (query `/clock` → modelo → `/sched`, synths suenan).
- ✅ **C7 — Interfaces MIDI** *(replanificado 2026-06-17 → movido a Milestones futuros como **C11**)*: la primera parte quedó **mal planificada** (MIDI 1.0 en una librería de Python, solo cliente) y se rehízo. La decisión final —crate nativo reusable cliente+servidor con MIDI 2.0/UMP— se sacó del track secuencial y vive en **C11** (sección «Milestones futuros» abajo) y en el **M17** del `PLAN.md` raíz. El slot C7 queda cerrado acá para no frenar el avance secuencial: **no afecta lo que falta de C9 ni C10**.
- ✅ **C8 — Interfaz TCP** *(Completado 2026-06-17 — ver NOTAS.md)*: las dos puntas. **Servidor** (track M): `src/osc/tcp.rs` — `--tcp [port]` acepta OSC length-prefixed (prefijo 4 bytes BE + bytes, framing de scsynth) multiplexado en el loop single-thread sin runtime async ni dependencia nueva (hilo acceptor + un hilo lector por conexión → canal `mpsc` drenado cada iteración como el ring M14; wake por datagrama UDP de longitud 0 al propio socket → sin esperar el tick de GC; réplicas por el write-half que posee el hilo de red, `&TcpStream: Write`). `ClientId::Tcp(id)` rutea las réplicas a la conexión de origen. **Cliente**: `OscTCPInterface` real (drop-in de `OscUDPInterface`; framing + reensamblado de réplicas entre segmentos TCP). Tests: `tests/osc.rs::tcp_*` (round-trip `/status`+`/d_recv`, ruteo por conexión), `clients/python/tests/test_tcp.py` (framing/reensamblado con socket falso), E2E en vivo. Ejemplo `examples/tcp_client.py`. El timing sigue en timetags/`/sched`: la latencia de llegada no afecta cuándo dispara un comando agendado.
- ✅ **C9 — multi-lenguaje + cierre** *(Completado 2026-06-17 — ver NOTAS.md)*:
  - ✅ **Arquitectura cross-lenguaje documentada**: capítulo nuevo en el mdBook (`docs/clients.md`, en SUMMARY bajo "Library & Embedding") — el contrato C-ABI único (`clausters-core`/`clausters-ffi` + embed/shm), el cliente Python (capas base/seq/defs), el camino al cliente **JS** (mismo C-ABI vía N-API/wasm, generators/async en vez de `yield`) y el plan de **distribución** (wheels Python, npm/wasm JS, Faust en `third_party`). **Confirmación del reuso**: Python (lenguaje no-Rust) ya maneja todo el sistema por el C-ABI + OSC, prueba de que la frontera no es Python-específica.
  - ✅ **Ejemplo comentado del cliente** *(2026-06-17)*: `examples/sequencing.py` — tour de la capa de secuenciación de alto nivel (`Session` + `Pbind` + value patterns) con la **costura NRT/vivo** (el mismo pattern rinde offline o toca en vivo por UDP según la interfaz de la `Server`). Validado offline (render a samples/WAV) y en vivo (E2E misma invocación Bash). Catalogado en `docs/examples.md`.
- **C10 — Mantenimiento de documentación y ejemplos**: a medida que avanzan los milestones (C6–C9 y el port diferido de SynthDef), mantener al día el mdBook (`docs/`), `clients/python/README.md`, la `GUIA.md` del cliente (pasos + conteos) y los ejemplos. Revisar y actualizar lo que quede desfasado cuando sea necesario.

## Milestones futuros (track "C", paralelos al track "M")

Milestones del cliente **sin orden secuencial fijo**, encarables cuando corresponda (igual que los «Milestones futuros M9+» del servidor en el `PLAN.md` raíz). Se numeran a continuación del último de la sección secuencial (C10).

- **C11 — Interfaces MIDI** *(movido desde C7; diferido, sin fecha)*: completar `_midiinterface`. `MidiNrtInterface`/`MidiScore` con **escritura a archivo MIDI** para partituras; `MidiRtInterface` con un **backend real** para salida en vivo. Mapear `Event` → mensajes MIDI (note on/off por `sustain`, canal, velocity). Misma costura RT/NRT que OSC, vía la interfaz que posee la `Server` (o un destino MIDI análogo). MIDI no lleva timetags: el timing lo da el reloj al emitir. **Decisión revisada**: el MIDI no va en una librería de Python (python-rtmidi) ni solo en el cliente, sino en un **crate nativo reusable cliente+servidor** (`crates/clausters-midi`, C ABI versionada), con la capa de mensajes en **MIDI 2.0/UMP vía `midi2`** (alta resolución: velocity 16-bit, controllers 32-bit, `no_std`/no-alocante), persistencia con resolución plena en **MIDI 2.0 Clip File vía `midi2-clip`** y `.mid` (SMF, MIDI 1.0) vía `midly` como interop. **Ver M17 del `PLAN.md` raíz** para el alcance completo (protocolo MIDI del servidor + salida del cliente) y la evaluación pendiente de protocolo/crates.
- **C12 — Empaquetado del cliente Python (wheels)** *(diferido, sin fecha)*: distribuir el paquete `clausters` como **wheel** instalable por `pip`, empaquetando las bibliotecas nativas (`libclausters_ffi`, `libclausters` embed) para las plataformas objetivo. Incluye el **build reproducible de Faust en `third_party`** que las wheels necesitan (ya anotado como backlog del usuario). Es el empaquetado del lado **Python**; el del cliente JS (npm) va en el track **J** (ver abajo).

## Milestones futuros (track "J" del cliente JavaScript)

Track separado para el **cliente JavaScript**, todavía **sin planificar en detalle**: hay que planificarlo más adelante, **junto con el empaquetado npm** (el equivalente JS de las wheels de C12). El cliente JS se construye **sobre lo que se haga primero con Python**: espeja su estructura (capas base/seq/defs) y reusa el **mismo C-ABI** (`clausters-core`/`clausters-ffi` + embed) vía N-API o wasm, con generators/async en lugar del `yield` de Python. El plan concreto (milestones J1, J2, …) se define cuando el cliente Python esté lo bastante estable como para servir de modelo.

- **J — Cliente JS real + empaquetado npm** *(a planificar)*: binding del C-ABI (N-API/wasm), port de las capas del cliente espejando Python, y distribución por npm (incluido el build de Faust para wasm en `third_party`).

## Convenciones de organización

- Crates nativos bajo `crates/`; clientes por lenguaje bajo `clients/<lang>/`. El **C-ABI del núcleo es el único contrato** entre Rust y cada lenguaje, con versión de ABI explícita (como `embed.rs` / `clausters.py` ya hacen).
- Regla de frontera project-wide: "solo datos planos cruzan" (bytes/`array`/escalares/enteros), tanto en el transport como en el núcleo.
- Track de milestones del cliente con prefijo `C` para no colisionar con el track `M` del servidor; cerrar cada uno con el checklist de milestone del proyecto (código+tests, NOTAS/PLAN, doc de desarrollo, doc de usuario en `docs/`, `GUIA.md`, ejemplo en `examples/`).
- Código/comentarios/tests en inglés; `PLAN.md`/`NOTAS.md`/docs en español.

## Verificación

- **Workspace**: `cargo build` y `cargo test` (sin features y con `--features faust`) deben pasar; `tests/rt_safety.rs` y `tests/denormals.rs` siguen verdes tras el refactor.
- **Paridad numérica**: nuevo test en `clausters-core` (o `tests/`) que compara salida del builtin nativo contra la rama nativa del servidor (bit-exacto) y contra Faust (tolerancia).
- **Cliente**: `pytest` del paquete; smoke de `_native` (builtin, TempoClock, bundle, render).
- **E2E** (regla de CLAUDE.md: servidor y cliente en la **misma** invocación Bash): levantar `./target/debug/clausters &`, definir una FaustDef desde el cliente alto, `/s_new`, controlar por bus, verificar `/done`/replies, `kill`. NRT: score generado por `seq` → `render()` → comparar WAV/golden.

## A validar durante la ejecución (no bloqueante)

- Nivel de equivalencia aceptable para la matemática superior vs Faust (tolerancia concreta).
- Si conviene `cdylib` separado para `clausters-ffi` o exponer su C-ABI desde el mismo `libclausters` (preferencia inicial: separado, para no acoplar cliente y embed del servidor).
- Umbral de overhead FFI donde el builtin escalar use fallback puro-lenguaje en vez de cruzar la frontera.
