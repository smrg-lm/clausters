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

### Interfaces de destino y manejo del tiempo (RT / NRT / MIDI) — pieza central

Este es el punto que hace que **una misma lógica de relojes y rutinas** sirva para tiempo real, render diferido y MIDI sin reescribirla: el reloj (`TempoClock`) y las rutinas **no envían directamente**, sino que **emiten eventos contra una interfaz de destino intercambiable**. Cambiar la interfaz cambia *a dónde* y *en qué modo* (vivo vs diferido) van los eventos; la generación de eventos queda idéntica.

- `base/_oscinterface.py`: define interfaces OSC intercambiables — `OscUDPInterface` y `OscTCPInterface` (RT, distintos protocolos de transporte; UDP ya disponible en el servidor, **TCP aún no implementado**) y `OscNrtInterface`, que en lugar de enviar **acumula los eventos con timetag en un `OscScore`** (la partitura binaria que luego va a `render()` del transport para NRT).
- `base/_midiinterface.py`: análogamente, `MidiRtInterface` (envío MIDI en vivo) y `MidiNrtInterface`, que acumula en un `MidiScore`.
- Por eso el manejo del tiempo es responsabilidad compartida con clara división: la **aritmética y los timetags** (beat↔segundo↔sample, conversión contra el sample-clock, armado de bundles) viven en el núcleo Rust (`_native`); la **interfaz** solo decide destino (UDP/TCP/score/MIDI) y modo (RT vivo vs NRT diferido); el **driver de coroutines** (reanudar los `yield`) vive en Python. Un mismo `Routine` + `TempoClock` produce una sesión RT en vivo o un `OscScore`/`MidiScore` para render **solo cambiando la interfaz** — sin tocar relojes ni rutinas.

## Milestones (track "C" del cliente, paralelo al track "M" del servidor)

- **C0 — Workspace + núcleo + FFI**: convertir a workspace; crear `clausters-core` (builtins, tempoclock, rng, osc) y `clausters-ffi` (C-ABI + versión); refactorizar las ops nativas del servidor para consumir `clausters-core`. Tests de paridad numérica servidor↔core (bit-exacto en las nativas; tolerancia documentada vs Faust). Verificar RT-safety intacta.
- **C1 — Scaffold cliente + núcleo accesible**: `pyproject.toml`, paquete `clausters/`, reubicar transport, `_native.py` (ctypes sobre `clausters-ffi`). Smoke: importar, llamar un builtin escalar/lista, instanciar `TempoClock`, armar un bundle OSC, `render()`.
- **C2 — base**: `absobject`/`builtins` (despacho a nativo, escalar+lista), `stream` (Routine/Stream con `yield`), `main` (contexto global, semillas), `clock` (TempoClock sobre núcleo), `netaddr`. Interfaces de destino: `_oscinterface` (`OscUDPInterface` + `OscNrtInterface`/`OscScore`) y `_midiinterface` (`MidiRtInterface` + `MidiNrtInterface`/`MidiScore`), de modo que reloj y rutinas emitan contra una interfaz intercambiable. (`OscTCPInterface` queda como stub: TCP aún no implementado en el servidor.)
- **C3 — defs Faust-first**: `signals.py` (callables en minúscula que mapean la Signal API de Faust; su composición arma el JSON signal tree), `faustdef` (las tres formas para `/d_faust`, controles), `node`/`bus`/`buffer`/`server` (allocators, async `/done`-`/fail`, `/notify`). Vertical slice E2E: construir un grafo con `signals` → `faustdef` → `/d_faust` → `/s_new` → controlar por bus/clock.
- **C4 — seq**: `event`, `pattern`, stream-patterns; un mismo `Routine`+`TempoClock` corre en **RT** (interfaz UDP, servidor vivo) o **NRT** (interfaz score → `render()`) **solo cambiando la interfaz de destino**. Golden de paridad de score con el servidor.
- **C5 — multi-lenguaje + cierre**: confirmar reuso del C-ABI desde JS (nota N-API/wasm, sin implementarlo aún); docs (mdBook: capítulos nuevos en `docs/`), `GUIA.md` (pasos manuales + conteos), ejemplo comentado en `examples/`, `NOTAS.md`.

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
