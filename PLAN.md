# Plan de implementación: servidor de audio en tiempo real estilo scsynth

Servidor de síntesis en Rust controlado por OSC, inspirado en la arquitectura de
scsynth (SuperCollider): un proceso que abre el dispositivo de audio, mantiene un
árbol de nodos (synths y grupos), y recibe comandos OSC por UDP para crear/destruir
synths, fijar parámetros, gestionar buses y buffers, todo con scheduling
sample-accurate.

## Principios de diseño (no negociables)

1. **El hilo de audio nunca bloquea**: sin `malloc`/`free`, sin locks, sin syscalls,
   sin I/O dentro del callback de audio. Ver skill `realtime-audio`.
2. **Toda comunicación con el hilo de audio es lock-free**: ring buffers SPSC para
   comandos entrantes y para devolver "basura" (memoria a liberar) al hilo no-RT.
3. **Procesamiento por bloques**: bloques de 64 samples (como scsynth), no
   sample-a-sample, para amortizar el dispatch de UGens.
4. **Compatibilidad conceptual con scsynth, no binaria**: mismo modelo (node tree,
   buses, buffers, SynthDefs, comandos `/s_new`, `/n_set`, etc.) pero formato de
   SynthDef propio (no el formato binario `.scsyndef` — al menos no en v1).

## Arquitectura de hilos

```
┌─────────────┐  OSC/UDP   ┌──────────────┐  SPSC cmd FIFO  ┌──────────────┐
│ Cliente OSC │ ─────────> │ Hilo de red  │ ──────────────> │ Hilo de audio│
│ (sclang,    │ <───────── │ (parse OSC,  │ <────────────── │ (callback    │
│  TouchOSC…) │  replies   │  aloca memoria│  SPSC garbage/  │  cpal, DSP)  │
└─────────────┘            │  pre-armada) │  reply FIFO     └──────────────┘
                           └──────┬───────┘
                                  │ tareas lentas (disco, decode)
                           ┌──────▼───────┐
                           │ Hilo NRT     │  (carga de archivos a buffers, etc.)
                           └──────────────┘
```

- **Hilo de red**: socket UDP, parsea OSC (`rosc`), construye comandos *ya
  completamente alocados* (p. ej. el nodo Synth ya instanciado) y los empuja al FIFO.
  El hilo de audio solo los "enchufa" — O(1), sin alocar.
- **Hilo de audio**: callback de cpal. En cada bloque: (1) drena el FIFO de comandos,
  (2) ejecuta bundles agendados cuyo timestamp cae en este bloque, (3) recorre el
  árbol de nodos en orden y ejecuta el DSP, (4) empuja memoria muerta al FIFO de
  basura.
- **Hilo NRT**: lectura/escritura de archivos de audio para buffers (`/b_read`,
  `/b_write`), igual que el "NRT thread" de scsynth.

## Crates

| Crate | Uso |
|---|---|
| `cpal` | I/O de audio multiplataforma (ALSA/JACK en Linux) |
| `rosc` | Encode/decode de OSC 1.0 (mensajes y bundles con timetag) |
| `rtrb` | Ring buffer SPSC lock-free y realtime-safe |
| `basedrop` | Punteros compartidos con deallocación diferida fuera del hilo RT |
| `hound` | Lectura/escritura WAV para buffers |
| `assert_no_alloc` | En tests/debug: panic si el hilo de audio aloca |

## Estructuras de datos centrales

- **`NodeTree`**: árbol de `Group` y `Synth` con IDs enteros (mapa ID→nodo
  pre-alocado o slab). Orden de ejecución = recorrido en profundidad, como scsynth.
- **`Synth`**: instancia de una `SynthDef`: vector de UGens construidos + buffers de
  cableado ("wires") + valores de controles.
- **`SynthDef`**: grafo de UGens topológicamente ordenado, con constantes, controles
  nombrados y asignación de wires. Se define en un formato propio (ver M3).
- **`Bus`**: arrays globales de buses de audio (por bloque) y de control (un valor);
  los primeros N buses de audio se mapean a las salidas/entradas de hardware.
- **`Buffer`**: pool pre-alocado de buffers de samples con canal/frames/samplerate,
  llenados por el hilo NRT.
- **UGen**: trait con `fn process(&mut self, ctx: &ProcessCtx, inputs, outputs)`
  sobre bloques de 64 samples; dispatch dinámico (`Box<dyn UGen>`) está bien en v1
  (la construcción ocurre fuera del hilo RT; la llamada virtual por bloque es barata).

## Protocolo OSC (subconjunto de scsynth)

Implementar en este orden: `/status`, `/quit`, `/notify`, `/dumpOSC` — `/s_new`,
`/n_free`, `/n_set`, `/n_run` — `/g_new`, `/g_freeAll`, `/g_deepFree`, `/n_before`,
`/n_after` — `/b_alloc`, `/b_free`, `/b_read`, `/b_write`, `/b_zero` — `/d_recv`
(con nuestro formato), `/d_free` — `/c_set`, `/c_get`. Bundles con timetag NTP →
scheduling sample-accurate dentro del bloque. Ver skill `scsynth-osc`.

## UGens iniciales

Osciladores: `SinOsc`, `Saw` (PolyBLEP), `Pulse`, `WhiteNoise`, `Phasor`.
Filtros: `LPF`/`HPF` (biquad), `OnePole`, `Lag`.
Envolventes/control: `EnvGen` (con done actions: free self, como scsynth), `Line`.
E/S: `Out`, `In`, `ReplaceOut`. Buffers: `PlayBuf`, `BufRd`. Matemática: operadores
binarios/unarios entre señales. Ver skill `ugen-dsp` para los algoritmos.

## Milestones

- ✅ **M0 — Esqueleto**: `cargo init`, cpal abre el dispositivo y suena una sinusoide
  hardcodeada. Estructura de módulos: `server/`, `dsp/`, `osc/`, `node/`.
  *(Completado 2026-06-10 — ver NOTAS.md.)*
- ✅ **M1 — Servidor OSC**: socket UDP (puerto 57110 por defecto), `rosc`, responder
  `/status.reply`, `/quit`, `/notify`. Logging con `/dumpOSC`.
  *(Completado 2026-06-10 — ver NOTAS.md.)*
- ✅ **M2 — FIFO RT-safe + node tree**: ring buffers de comandos y basura, `NodeTree`
  con grupos, un synth hardcodeado instanciable vía `/s_new` y liberable con
  `/n_free`. Test con `assert_no_alloc` activo en el callback.
  *(Completado 2026-06-10 — ver NOTAS.md. Bonus: `/n_set` adelantado de M3.)*
- ✅ **M3 — SynthDefs**: formato de definición (sugerido: estructura serializada con
  `serde` — JSON/binario propio), intérprete que construye el vector de UGens y
  asigna wires, `/d_recv`, `/n_set` sobre controles nombrados e indexados.
  *(Completado 2026-06-10 — ver NOTAS.md. Incluye el trait `SynthNode`,
  prerequisito de la bifurcación F.)*
- **M4 — Buses y orden**: buses de audio/control, UGens `In`/`Out`, `/n_before`,
  `/n_after`, grupos anidados, add actions de `/s_new` (head/tail/before/after/replace).
- **M5 — Buffers**: pool de buffers, hilo NRT, `/b_alloc`, `/b_read` (hound),
  `PlayBuf`/`BufRd`, replies asíncronos `/done`.
- **M6 — Scheduling sample-accurate**: cola de bundles ordenada por timetag en el
  hilo de audio (pre-alocada), conversión NTP→samples, ejecución con offset
  intra-bloque (partir el bloque en el sample del evento, como hace scsynth).
- **M7 — Modo NRT + tests dorados**: render offline a WAV (mismo motor, sin cpal),
  tests de regresión comparando contra archivos dorados, benchmarks del grafo.

## Bifurcación F — SynthDefs vía Faust (Box/Signal API + JIT)

Camino alternativo (no reemplaza M3–M7: conviven) para construir nodos de síntesis:
en lugar de interpretar un grafo de UGens propios, el servidor recibe **JSON que se
mapea a llamadas de la Box API (o Signal API) de libfaust**, compila a código nativo
con el backend LLVM (como FaustLive) y cuelga el resultado en el mismo node tree.
La ventaja: el "instruction set" del cliente es la Box API completa de Faust —
clientes en cualquier lenguaje solo generan JSON, sin depender de nuestro set de
UGens.

### Cambios al diseño base que esto exige

- **Prerequisito en M3**: el nodo synth del árbol debe ser `Box<dyn SynthNode>`
  (trait con `process`, `set_control`, `done`), no un tipo concreto — así
  `UGenSynth` (M3) y `FaustSynth` (F3) son intercambiables en el mismo árbol.
  M3 debe implementarse ya con este trait.
- **Hilo compilador** (nuevo, además del NRT): recibe pedidos de compilación,
  serializa el acceso a libfaust (su contexto global no es thread-safe) y publica
  factories en una tabla compartida (`basedrop::Shared`). La compilación JIT tarda
  decenas-cientos de ms: siempre asíncrona, nunca bloquea ni la red ni el audio.
- **Frontera RT intacta**: `compute()` de un dsp Faust ya inicializado es RT-safe
  (sin alocaciones); crear/inicializar/destruir instancias y factories NO lo es —
  instanciación en el hilo de red/compilador, destrucción vía garbage FIFO, igual
  que los synths actuales.

### Milestones F (después de M4 recomendado; F0 puede hacerse antes como spike)

- **F0 — Toolchain y FFI mínimo**: instalar libfaust con backend LLVM; evaluar
  crates existentes vs. binding propio con bindgen sobre la C API
  (`libfaust-box-c.h`, `llvm-dsp-c.h`); feature flag `faust` (todo opcional, el
  core sigue compilando sin libfaust). Prueba de humo: compilar un box hardcodeado
  (sinusoide por recursión/phasor) y renderizar offline comparando contra nuestro
  `SinOsc`. Acá se mide el riesgo real: tamaño del link con LLVM, versión de
  libfaust, latencia de compilación.
- **F1 — Hilo compilador**: thread dedicado con cola `CompileRequest { nombre,
  json, cliente }`; tabla de factories con refcount; replies asíncronos
  `/done /d_faust <nombre>` o `/fail` con el error de compilación legible.
  Comando OSC nuevo: `/d_faust` (blob JSON) — `/d_recv` queda reservado para el
  formato UGen de M3.
- **F2 — Esquema JSON → Box API**: definir el schema (primitivas, composición
  `par`/`seq`/`split`/`merge`/`rec`, matemática, delays, y UI `hslider`/`button`
  como controles nombrados); intérprete JSON→llamadas a la C API con validación y
  errores con ruta del nodo JSON culpable. Acceso a la stdlib de Faust (`os.osc`,
  filtros de `fi.`) vía `DSPToBoxes` embebiendo fragmentos de fuente Faust dentro
  del JSON — lo mejor de ambos mundos.
- **F3 — FaustSynth en el árbol**: `FaustSynth: SynthNode` envolviendo la
  instancia JIT; `/s_new` con nombre de def Faust instancia en el hilo de red
  (`createDSPInstance` + `init(sr)` alocan) y enchufa por el cmd FIFO; mapeo
  buses↔`inputs`/`outputs` no intercalados de Faust; `/n_set` sobre parámetros
  por nombre (zonas `FAUSTFLOAT*` recolectadas con UIGlue al instanciar);
  liberación por garbage FIFO con refcount de factory (destruir una factory con
  instancias vivas es UB).
- **F4 — Paridad e interop**: synths Faust y UGen conviven en grupos/buses;
  tests dorados de grafos equivalentes (UGen `SinOsc` vs box `sin(phasor)`);
  cliente de ejemplo en Python que genera JSON; documentación del schema.
- **F5 — Extensiones (opcional)**: Signal API como variante de bajo nivel,
  waveforms/soundfiles, polifonía nativa de Faust, backend interpreter de Faust
  (sin LLVM) para plataformas sin JIT.

### Riesgos conocidos

- **Licencia**: sin riesgo — este proyecto es GPLv3-o-posterior, compatible con
  libfaust (GPLv2-o-posterior); la combinación se distribuye como GPLv3+.
- **LLVM**: link pesado (decenas de MB) y sensible a versiones; el feature flag
  aísla el costo.
- **Sample rate fijo por instancia**: `init(sr)` congela el SR; re-instanciar si
  cambia el dispositivo.
- `FAUSTFLOAT` debe compilarse como `f32` para casar con nuestros buses.

Ver skill `faust-embedding` para los detalles de la C API y sus trampas.

## Estrategia de pruebas

- **Unitarias por UGen**: render offline de N bloques, asserts sobre la señal
  (frecuencia vía cruces por cero, RMS, respuesta a impulso para filtros).
- **Golden files**: el modo NRT (M7) renderiza escenas a WAV y se compara con
  tolerancia contra archivos de referencia versionados.
- **RT-safety**: `assert_no_alloc` envuelve el callback en builds de test; CI corre
  el grafo más pesado bajo esa condición.
- **Integración OSC**: tests que levantan el servidor en un puerto efímero y le
  hablan con `rosc` desde el test; verificable a mano con `oscsend` o con sclang
  apuntando `Server` a nuestro puerto. Ver skill `audio-testing`.

## Skills del proyecto

- `.claude/skills/realtime-audio` — reglas del hilo RT, patrones lock-free, cpal.
- `.claude/skills/scsynth-osc` — referencia del protocolo OSC de scsynth y semántica
  del node tree.
- `.claude/skills/ugen-dsp` — algoritmos DSP de los UGens (osciladores, filtros,
  envolventes) con sus fórmulas.
- `.claude/skills/audio-testing` — cómo testear audio sin oídos: NRT, golden files,
  asserts de señal, no-alloc.
- `.claude/skills/faust-embedding` — embeber libfaust: C API (box/signal/LLVM),
  ciclo de vida factory/instancia, fronteras RT, mapeo JSON→Box API.

## Notas del proyecto

- Los avances realizados en cada milestone se van a gregando en las notas del proyecto.