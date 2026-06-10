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
- **M1 — Servidor OSC**: socket UDP (puerto 57110 por defecto), `rosc`, responder
  `/status.reply`, `/quit`, `/notify`. Logging con `/dumpOSC`.
- **M2 — FIFO RT-safe + node tree**: ring buffers de comandos y basura, `NodeTree`
  con grupos, un synth hardcodeado instanciable vía `/s_new` y liberable con
  `/n_free`. Test con `assert_no_alloc` activo en el callback.
- **M3 — SynthDefs**: formato de definición (sugerido: estructura serializada con
  `serde` — JSON/binario propio), intérprete que construye el vector de UGens y
  asigna wires, `/d_recv`, `/n_set` sobre controles nombrados e indexados.
- **M4 — Buses y orden**: buses de audio/control, UGens `In`/`Out`, `/n_before`,
  `/n_after`, grupos anidados, add actions de `/s_new` (head/tail/before/after/replace).
- **M5 — Buffers**: pool de buffers, hilo NRT, `/b_alloc`, `/b_read` (hound),
  `PlayBuf`/`BufRd`, replies asíncronos `/done`.
- **M6 — Scheduling sample-accurate**: cola de bundles ordenada por timetag en el
  hilo de audio (pre-alocada), conversión NTP→samples, ejecución con offset
  intra-bloque (partir el bloque en el sample del evento, como hace scsynth).
- **M7 — Modo NRT + tests dorados**: render offline a WAV (mismo motor, sin cpal),
  tests de regresión comparando contra archivos dorados, benchmarks del grafo.

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

## Notas del proyecto

- Los avances realizados en cada milestone se van a gregando en las notas del proyecto.