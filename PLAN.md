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
- ✅ **M4 — Buses y orden**: buses de audio/control, UGens `In`/`Out`, `/n_before`,
  `/n_after`, grupos anidados, add actions de `/s_new` (head/tail/before/after/replace).
  *(Completado 2026-06-10 — ver NOTAS.md. Incluye `/g_new`, `/g_freeAll`,
  `/g_deepFree`, `/c_set`/`/c_get` y notificaciones `/n_go`/`/n_end`. Cambio de
  formato: las defs ya no llevan campo `out`; la salida es vía UGens `Out`.)*
- ✅ **M5 — Buffers**: pool de buffers, hilo NRT, `/b_alloc`, `/b_read` (hound),
  `PlayBuf`/`BufRd`, replies asíncronos `/done`.
  *(Completado 2026-06-10 — ver NOTAS.md. Incluye `/b_allocRead`, `/b_write`,
  `/b_zero`, `/b_free` y `/b_query`. Buffers inmutables compartidos por
  `Arc`: el hilo NRT construye, el engine swapea, lo reemplazado sale por el
  garbage FIFO.)*
- ✅ **M6 — Scheduling sample-accurate**: cola de bundles ordenada por timetag en el
  hilo de audio (pre-alocada), conversión NTP→samples, ejecución con offset
  intra-bloque (partir el bloque en el sample del evento, como hace scsynth).
  *(Completado 2026-06-10 — ver NOTAS.md. `ProcessCtx` procesa por slices
  `offset`+`frames`; el engine publica su reloj de samples y la conversión
  NTP→samples vive en el hilo de red. Nota: scsynth real cuantiza al bloque
  — nosotros partimos el bloque de verdad, sin necesitar `OffsetOut`.)*
- ✅ **M7 — Modo NRT + tests dorados**: render offline a WAV (mismo motor, sin cpal),
  tests de regresión comparando contra archivos dorados, benchmarks del grafo.
  *(Completado 2026-06-11 — ver NOTAS.md. `clausters --nrt score.osc out.wav`
  con partituras en el formato binario de scsynth; los comandos async corren
  síncronos como en scsynth NRT; goldens en `tests/golden/` regenerables con
  `cargo run --example render_golden`; benchmark `cargo run --release
  --example bench`. Bonus: el bug de blobs de rosc también afectaba elementos
  de bundle — arreglado para ambos modos en `osc::decode_packet`.)*
- ✅ **M8 — Reloj de samples como timebase del cliente**: el reloj del SO y el
  cristal del DAC derivan entre sí (decenas de ppm ≈ ms por minuto), así que
  la conversión NTP→samples actual re-ancla cada bundle contra dos relojes
  que no coinciden. Extensión de protocolo para que el cliente use el reloj
  de samples como maestro: (1) exponer `current_samples()` por OSC (en
  `/status.reply` o un `/clock` nuevo); (2) aceptar bundles con target
  **en samples** (entero de 64 bits — `Cmd::Schedule` ya trabaja así, la
  conversión NTP es solo el front-end); (3) en el cliente, modelar
  `sample(t_local) = a + b·t` con pares (reloj monotónico local, sample
  consultado) y regresión con olvido — estilo DLL de JACK / Ableton Link —
  y agendar por adelantado directo en samples. La latencia de la consulta
  no importa (solo necesita incertidumbre acotada + scheduling ahead): el
  error del ancla desplaza todo el grid por una constante, y el timing
  *relativo* entre eventos queda sample-exacto por construcción.
  Demo/referencia en `examples/json_client.py`; documentar en
  `docs/schemas.md` la diferencia con scsynth (que no tiene esto). Ojo: el
  contador cuenta samples procesados, no escuchados (sumar latencia del
  dispositivo para alinear con el mundo exterior) y se pausa en xruns (el
  re-anclaje periódico lo absorbe). **Los dos relojes conviven, nada se
  descarta**: el camino NTP queda intacto (compatibilidad scsynth) y el
  target en samples es opt-in **por bundle** — clientes NTP y clientes
  sample-clock pueden hablarle al mismo servidor a la vez, porque ambos
  front-ends desembocan en la misma cola (`Cmd::Schedule`). Señalización:
  como el timetag OSC es formato NTP por especificación, no reinterpretarlo;
  la vía es un mensaje contenedor nuevo (p. ej. `/sched` con el target i64 +
  el bundle como blob), que además se anida/agenda igual que un bundle común.
  *(Completado 2026-06-12 — ver NOTAS.md. `/clock` → `/clock.reply h d` y
  `/sched <h target> <blob>` (atómico, timetags internos ignorados, target
  pasado = próximo bloque); cliente de referencia `examples/sample_clock.py`
  con el modelo de regresión; documentado en `docs/sample-clock.md` +
  schemas.md. El test agenda por `/sched` y asserta el sample **exacto**,
  sin la vecindad que necesita el camino NTP.)*

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

- ✅ **F0 — Toolchain y FFI mínimo**: instalar libfaust con backend LLVM; evaluar
  crates existentes vs. binding propio con bindgen sobre la C API
  (`libfaust-box-c.h`, `llvm-dsp-c.h`); feature flag `faust` (todo opcional, el
  core sigue compilando sin libfaust). Prueba de humo: compilar un box hardcodeado
  (sinusoide por recursión/phasor) y renderizar offline comparando contra nuestro
  `SinOsc`. Acá se mide el riesgo real: tamaño del link con LLVM, versión de
  libfaust, latencia de compilación.
  *(Completado 2026-06-10 — ver NOTAS.md. Mediciones: JIT ≈ 10 ms por def,
  libfaust.so 11 MB con libLLVM.so dinámica de sistema; binding propio a mano,
  sin bindgen por ahora.)*
- ✅ **F1 — Hilo compilador**: thread dedicado con cola `CompileRequest { nombre,
  json, cliente }`; tabla de factories con refcount; replies asíncronos
  `/done /d_faust <nombre>` o `/fail` con el error de compilación legible.
  *(Completado 2026-06-10 — ver NOTAS.md. F1 compila fuente Faust vía
  `/d_faust name source`; el mapeo JSON→Box llega en F2. Hallazgo: libfaust
  no tolera compilaciones concurrentes en un proceso — lock global además
  del hilo dedicado.)*
  Comando OSC nuevo: `/d_faust` (blob JSON) — `/d_recv` queda reservado para el
  formato UGen de M3.
- ✅ **F2 — Esquema JSON → Box API**: definir el schema (primitivas, composición
  `par`/`seq`/`split`/`merge`/`rec`, matemática, delays, y UI `hslider`/`button`
  como controles nombrados); intérprete JSON→llamadas a la C API con validación y
  errores con ruta del nodo JSON culpable. Acceso a la stdlib de Faust (`os.osc`,
  filtros de `fi.`) vía `DSPToBoxes` embebiendo fragmentos de fuente Faust dentro
  del JSON — lo mejor de ambos mundos.
  *(Completado 2026-06-10 — ver NOTAS.md. Schema documentado en
  `src/faust/boxes.rs`; `/d_faust` acepta JSON o fuente Faust cruda.
  Hallazgo: bug upstream en `boxFmod()`, rodeado vía fragmento.)*
- ✅ **F3 — FaustSynth en el árbol**: `FaustSynth: SynthNode` envolviendo la
  instancia JIT; `/s_new` con nombre de def Faust instancia en el hilo de red
  (`createDSPInstance` + `init(sr)` alocan) y enchufa por el cmd FIFO; mapeo
  buses↔`inputs`/`outputs` no intercalados de Faust; `/n_set` sobre parámetros
  por nombre (zonas `FAUSTFLOAT*` recolectadas con UIGlue al instanciar);
  liberación por garbage FIFO con refcount de factory (destruir una factory con
  instancias vivas es UB).
  *(Completado 2026-06-10 — ver NOTAS.md. Controles reservados `out`/`in`
  para el mapeo de buses; los params se sondean una vez en el hilo
  compilador y viven en `FaustDef`.)*
- ✅ **F4 — Paridad e interop**: synths Faust y UGen conviven en grupos/buses;
  tests dorados de grafos equivalentes (UGen `SinOsc` vs box `sin(phasor)`);
  cliente de ejemplo en Python que genera JSON; documentación del schema.
  *(Completado 2026-06-10 — ver NOTAS.md. `tests/faust_parity.rs` (sine con
  tolerancia float + ganancia bit-exacta + grupo compartido),
  `examples/json_client.py` (solo stdlib), `docs/schemas.md`.)*
- ✅ **F5 — Extensiones (opcional; revisado 2026-06-12, ver «Milestones
  futuros»)**: la lista original se revisó contra lo que el servidor ya
  resuelve. **Se mantiene**: `waveform` (tablas chicas embebidas en la propia
  def — wavetables, funciones de transferencia para waveshaping; son
  autocontenidas y no compiten con los buffers), el backend interpreter de
  Faust (sin LLVM) para plataformas sin JIT — cobra sentido real con el
  target wasm de M14 — y la Signal API como variante de bajo nivel (baja
  prioridad: la Box API cubrió todos los casos hasta ahora). **Se descarta**:
  `soundfile` — duplica el sistema de buffers: un `PlayBuf`/`BufRd`
  escribiendo a un bus alimenta a cualquier nodo Faust por su control `in`,
  sin copiar datos al mundo Faust (documentar el patrón en `docs/schemas.md`);
  y la polifonía nativa de Faust — el node tree ya es el alocador de voces
  (una voz = un `/s_new`, las instancias comparten factory) y el modo
  polifónico impone convenciones MIDI (`freq`/`gain`/`gate`) ajenas al modelo
  scsynth; el único caso de uso real sería portar DSP polifónico Faust
  existente sin tocarlo, marginal acá.
  *(Completado 2026-06-12 — ver NOTAS.md. Ops `waveform`/`rdtable`/`rwtable`
  en el schema, demo `wavetable` en el cliente Python, patrón
  buffers-como-señal documentado en `docs/schemas.md`; interpreter backend
  y Signal API quedan como parte del target wasm de M14.)*

### Previsiones de implementación

- **Dependencia libfaust (no LLVM directo)**: se enlaza contra libfaust; LLVM
  viene embebido adentro cuando la build trae el backend JIT. El costo se paga
  de una de dos formas según el modo de consumo: libfaust *del sistema*
  (dinámica) deja el binario liviano pero hereda la fragilidad de versiones —
  la C API (`libfaust-box-c.h`) cambió entre versiones de Faust, los headers
  de bindgen tienen que casar con la libfaust instalada, y esta a su vez está
  atada a una `libLLVM-XX.so` concreta; libfaust *vendoreada/estática* da un
  binario autocontenido a cambio de decenas de MB de LLVM adentro. F0 mide
  cuál conviene; el feature flag `faust` aísla todo del core.
- **Sample rate horneada en el init de cada instancia**: la factory compilada
  es independiente de la SR, pero `instanceInit(dsp, sr)` precalcula las
  constantes dependientes (coeficientes, incrementos de fase) una sola vez —
  a diferencia de nuestros UGens, que leen `ctx.sample_rate` por bloque. Con
  la SR fija por ejecución de `engine_pair` esto hoy no afecta; se vuelve
  relevante solo con cambio de dispositivo en caliente o render NRT (M7) a
  otra SR. Mitigación barata: re-`instanceInit` (resetea estado) o
  re-instanciar.
- **Ancho de float**: el JIT elige `FAUSTFLOAT` por flag al crear la factory
  (`-single`/`-double`, default single). Regla: crear factories con `-single`
  y assertear el tamaño de float de la factory antes de usarla, para casar
  con los buses `f32`. Si algún día se quisiera f64 (p. ej. mastering NRT),
  lo barato es un buffer de conversión en la frontera del nodo Faust — no
  buses f64 globales; queda abierta la opción de un alias `Sample`
  parametrizable al estilo del typedef FAUSTFLOAT.

Licencia: este proyecto es GPLv3-o-posterior, compatible con libfaust
(GPLv2-o-posterior); la combinación se distribuye como GPLv3+. Falta agregar
el archivo `COPYING` con el texto verbatim de la GPLv3.

Ver skill `faust-embedding` para los detalles de la C API y sus trampas.

## Milestones futuros (M9+) — características adicionales

Sección agregada el 2026-06-12 a partir de una lista de ideas a revisar (el
M8 salió de esa misma lista). El orden refleja dependencias y costo/valor,
no urgencia: M9–M11 son chicos e independientes entre sí, M12 habilita M13,
M14 es independiente de todos. Al final se anota qué ideas se descartaron y
por qué. Direcciones menores que no llegan a milestone (más UGens —
`Saw`/`Pulse`/filtros/`EnvGen` con done actions ya listados arriba —,
`/g_queryTree`, streaming de buffers) se toman sueltas cuando hagan falta.

- ✅ **M9 — Documentación de desarrollo**: hoy `docs/` solo tiene documentación
  de usuario (`schemas.md`). Agregar `docs/architecture.md` (en inglés, como
  todo `docs/`): mapa de hilos (red / audio / NRT / compilador Faust), mapa
  de módulos (qué vive en `src/server`, `src/node`, `src/dsp`, `src/osc`,
  `src/faust`, `src/synthdef`), ciclo de vida de la memoria (comandos
  pre-armados en el hilo de red, garbage FIFO, pools `Arc`) e invariantes
  que ningún cambio puede romper (RT-safety, identidad sample-exacta RT/NRT,
  decodificar siempre por `osc::decode_packet`). Y una guía «cómo agregar
  una UGen en Rust»: el trait, la aridad, `ProcessCtx` por slices, dónde se
  registra el `kind`, y qué tests exige (unitario de señal + no-alloc +
  golden si cambia el sonido). Dos decisiones quedan escritas acá:
  (a) el mapeo de UI de Faust a controles — usar los labels como nombres de
  control es deliberado: los nombres los pone el autor de la def, igual que
  en `controls` del JSON de UGens, con `out`/`in` reservados para buses; el
  *qué* ya está en `schemas.md`, falta el *porqué* en la doc de desarrollo;
  (b) plugins de UGens: Rust no tiene ABI estable, así que no hay plugins
  dinámicos en v1 — extender = compilar dentro del crate y la API interna
  documentada es el contrato; si algún día hacen falta plugins dinámicos, la
  vía es una C ABI o wasm **versionadas** (lección histórica de scsynth: su
  ABI de plugins se rompía con cada cambio de struct o de feature). Cierra
  con una pasada de rustdoc sobre los items públicos.
  *(Completado 2026-06-12 — ver NOTAS.md. `docs/architecture.md` con mapa de
  hilos/módulos, ciclo de vida de memoria, tabla de capacidades pre-alocadas,
  invariantes, guía «cómo agregar una UGen» y las dos decisiones; punteros
  desde CLAUDE.md y schemas.md; rustdoc sin warnings en ambas configs. La
  tabla de capacidades adelanta la mitad de auditoría de M10.)*

- ✅ **M10 — Memoria acotada y alineación**: la mitad «denormales» de la idea
  original ya está hecha (post-M7: `dsp::denormals`, `-ftz 2`, tests); queda
  la mitad de memoria. (1) Auditar y documentar en una tabla única (en
  `docs/architecture.md`) todas las capacidades pre-alocadas — FIFOs de
  comandos/basura/eventos, cola de schedule (1024), slab de nodos, pool de
  buffers (1024), buses (128 audio / 1024 control) — y el modo de fallo de
  cada una al llenarse: el FIFO de comandos ya responde `/fail … command
  FIFO full` en todos los caminos del servidor vivo; verificar el resto
  (¿qué hace el hilo de audio si el garbage FIFO está lleno?, ¿y el de
  eventos?) y emparejar comportamientos. (2) Alineación: wires y bloques de
  bus son `[f32; 64]` con alineación natural de 4 bytes; envolverlos en un
  tipo `#[repr(align(64))]` (un bloque = 256 bytes = 4 líneas de caché
  enteras, sin partir) para autovectorización estable — medir con
  `examples/bench` antes y después y conservarlo solo si no empeora.
  (3) Actualizar la skill `realtime-audio` con las tres cosas: memoria
  acotada con su tabla, nota de alineación, y referencia a la protección de
  denormales ya implementada.
  *(Completado 2026-06-12 — ver NOTAS.md. Tabla de M9 ahora clavada por
  `tests/capacity.rs` (desbordes de basura/eventos/slab/grupos + fila nueva
  de rings M14); `Block` `#[repr(C, align(64))]` para wires, buses y staging
  Faust — bench A/B intercalado: neutro dentro del ruido (±4%), se conserva
  por el argumento de estabilidad; skill `realtime-audio` actualizada
  (modos de fallo, alineación, denormales reales en vez del `_mm_setcsr`
  deprecado).)*

- ✅ **M11 — `/n_map`/`/n_mapa`: buses como fuente de parámetros** (derivado
  de la revisión de la UI de Faust): la concepción «los elementos de UI son
  señales que llegan por buses de control» hoy solo es cierta para defs
  UGen que incluyan `InCtl` en su grafo; los params Faust solo se mueven por
  `/n_set` discreto. `/n_map nodeID ctl bus` (scsynth) lo unifica para los
  dos mundos: el nodo lee el bus de control al inicio de cada bloque y lo
  escribe en su control/zona hasta que `/n_map ctl -1` o un `/n_set`
  posterior lo desactive. Implementación RT-safe: tabla de mapeos por nodo
  (índice de control → bus) resuelta en el hilo de audio leyendo los atomics
  de buses de control que ya existen — sin alocar. Agendable en bundles como
  `/n_set`.
  *(Completado 2026-06-13 — ver NOTAS.md. Se implementó también `/n_mapa`
  con buses de audio: como un control es un escalar por bloque (y las zonas
  Faust también), muestrea un sample del bus por bloque (control-rate, fiel a
  scsynth para controles `kr`; no hay controles audio-rate — para audio está
  `In`/`in`). El mirror suma el bus de un mapeo de audio a las lecturas del
  nodo y marca `dynamic` si el control mapeado es índice de bus, así M12/M13
  siguen correctos. `tests/mapping.rs`, +tests en rt_safety/auto_order/
  faust_synth; ejemplo `osc_ping map`. Quedan opcionales las variantes multi
  `/n_mapn`/`/n_mapan`.)*

- ✅ **M12 — Forma canónica del grafo por conexiones de buses**: inferir el DAG
  de dependencias entre nodos a partir de los buses: qué buses de audio lee
  (`In`, `in` de Faust) y escribe (`Out`/`ReplaceOut`, `out` de Faust) cada
  def. El análisis es estático solo cuando los índices de bus son constantes
  o controles — no señales calculadas: una def analizable aporta aristas, y
  un nodo con índice de bus dinámico actúa de barrera conservadora (depende
  de todo lo anterior y todo lo posterior depende de él). Sobre el DAG,
  **grupos auto-ordenados opt-in** (flag nuevo en `/g_new` o comando
  `/g_sortMode`): dentro de ese grupo el orden de ejecución se recalcula en
  el hilo de red ante cada cambio de topología o de def y se aplica
  reusando la maquinaria de moves existente (equivalentes a `/n_before`) —
  cero cambios en el hilo de audio. Los ciclos (feedback legítimo
  leer-antes-de-escribir) no se «resuelven»: conservan el orden explícito
  vigente = un bloque de delay, como los sends de retorno de un editor
  multipista; documentarlo. La pérdida de flexibilidad queda contenida por
  el opt-in: en un grupo auto-ordenado, `/n_before`/`/n_after` manuales
  responden `/fail`. Para que el cliente inspeccione lo inferido:
  `/g_queryTree` (pendiente del set scsynth) más un `/g_dumpGraph` de
  debug. Beneficio: los grupos pasan a ser «canales de multipista» y el
  cliente deja de micro-gestionar el orden de ejecución.
  *(Completado 2026-06-12 — ver NOTAS.md. `osc/graph.rs`: análisis de buses
  por def + `TreeMirror` en el hilo de red + sort topológico estable;
  `/g_sortMode` (agendable y válido en scores NRT), `/g_queryTree`
  compatible scsynth y `/g_dumpGraph`; los handlers inmediatos del server
  se unificaron vía `CmdTranslator::translate`. Ejemplo
  `examples/auto_order.py`, doc `docs/auto-order.md`. Cero cambios en el
  hilo de audio, como estaba previsto.)*

- ✅ **M13 — Procesamiento paralelo del árbol** (requiere M12): el DAG de M12
  es exactamente la estructura que habilita el paralelismo — etapas =
  conjuntos de nodos sin dependencias entre sí — análogo al `ParGroup` de
  supernova pero inferido en vez de declarado. Workers RT (N−1 hilos con
  prioridad de audio) sincronizados por etapa con spin acotado + backoff;
  nada de locks ni syscalls en el camino caliente. El riesgo central es el
  hazard de escritura: dos nodos de la misma etapa sumando al mismo bus.
  Como el análisis ya conoce las escrituras, la regla inicial es «misma
  etapa ⇒ buses de escritura disjuntos; si no, se serializan dentro de la
  etapa» (la alternativa — acumuladores por worker + pase de reducción —
  cuesta memoria y un recorrido extra; queda como plan B). `assert_no_alloc`
  en todos los workers; el modo NRT se beneficia igual (renders más
  rápidos). Encararlo recién cuando exista un grafo real que no entre en un
  core: hoy `examples/bench` da ~1800 voces sine en un core, y este es el
  milestone más caro en complejidad de toda la sección.
  *(Completado 2026-06-12 — ver NOTAS.md. Particionado en etapas en el
  propio engine a partir de máscaras `BusUsage` enviadas en `Cmd::AddSynth`
  — la seguridad nunca depende del espejo de red —; `server/workers.rs`
  (fork-join con robo de trabajo atómico, spin acotado, park en idle);
  `/g_parallel` + `--workers` en RT y NRT; **bit-idéntico al secuencial**
  por construcción y por test; speedup medido ~3.3x con 3 workers en el
  bench de 8 cadenas × 125 sines. Doc en `docs/parallel.md`.)*

- ✅ **M14 — Transportes enchufables, modo embebido y llamadas síncronas**
  (redefinido 2026-06-12; antes era solo «plano de datos por shm»): el
  objetivo es que un cliente local use el servidor como si la aplicación
  fuera monolítica — sin protocolo de red a la vista y sin asincronía
  obligatoria — sin perder el control remoto por UDP. Tres capas:

  1. **Separar codificación de transporte.** OSC queda como única
     codificación (mensajes, bundles, timetags de M8, replies: un solo
     camino de parseo/validación con `decode_packet`); el transporte pasa a
     ser un trait con tres implementaciones. **UDP**: lo actual, para
     clientes remotos — la modularidad no se pierde. **Ring de bytes OSC en
     memoria compartida** (dos procesos, misma máquina): par de rings
     ida/vuelta por cliente, índice de commit publicado al final de cada
     escritura (un cliente que muere a mitad de escritura no corrompe nada),
     contenido tratado como bytes no confiables (la validación OSC ya
     existe), despertar por semáforo nombrado/eventfd — bloquear ahí es
     legal porque quien drena es el hilo de **red**, no el de audio; a
     cambio de UDP local: backpressure real en vez de pérdida silenciosa de
     paquetes y ningún puerto abierto. **In-process**: el caso monolítico de
     verdad — el servidor como biblioteca, el cliente entrega los bytes OSC
     por llamada de función al hilo de red, estilo `World_SendPacket` de
     libscsynth. El navegador no tiene UDP, así que esta abstracción es
     además prerrequisito del target wasm (allí el «ring» es un
     `SharedArrayBuffer`; depende del backend interpreter de F5, sin JIT
     LLVM en wasm).

  2. **Plano de datos compartido** (el M14 original): segmento
     multiplataforma (`shm_open` en Unix, `CreateFileMapping` en Windows;
     `memmap2` o similar) con header magic + **versión de layout**, el reloj
     de samples (el `AtomicU64` que el engine ya publica — anclas de M8 sin
     jitter de UDP) y el array de buses de control (lectura/escritura, los
     mismos atomics). En modo in-process es acceso directo, sin segmento.

  3. **Modo de ejecución síncrono.** La asincronía estilo scsynth es tediosa
     en los clientes (Routines en sclang, callbacks/promesas en JS); para el
     uso interactivo/científico en Python (consultar un dato y plotearlo) el
     binding ofrece una fachada bloqueante: llamada que espera datos =
     enviar el request + bloquear con timeout hasta el reply correlacionado.
     No exige cambios en el servidor (funciona incluso sobre UDP hoy), pero
     sí resolver dos cosas. **Correlación**: los replies identifican por
     comando + bufnum/nodeID — alcanza si el binding serializa sus requests;
     para concurrencia real, extensión mínima de protocolo: token opcional
     en las queries que el reply ecoa. **Datos grandes**: leer un buffer
     entero por UDP exige trocear estilo `/b_getn` (límite de datagrama); en
     modo in-process es **zero-copy** — los buffers ya son `Arc<Buffer>`
     inmutables, el binding clona el `Arc` en el hilo de red y expone
     puntero + longitud de `f32` planos. **Principio de la frontera**: solo
     estructuras básicas (arrays `f32` contiguos, enteros, strings de
     error), nunca tipos de una librería — las científicas fueron ejemplo de
     uso, no dependencia: numpy puede *ver* ese puntero sin copiar (buffer
     protocol), pero eso es elección del cliente, no del binding. Bonus que
     cierra el ciclo: el render NRT como llamada síncrona
     (`render(score) → frames f32`; `render_to_vec` ya existe). Lo síncrono
     es siempre el **cliente**
     esperando: el hilo de audio nunca se entera y el servidor nunca
     bloquea. Nota por lenguaje: Python bloquea sin problema; en JS el modo
     síncrono solo existe en workers (`Atomics.wait` sobre
     `SharedArrayBuffer`) — en el hilo principal queda `await`, que ya es
     tolerable.

  Entregables: trait de transporte + ring shm + modo embebido (feature
  `embed` o crate aparte). La **cdylib con C ABI versionada** (acá aplica la
  lección de ABI de la idea de plugins) es obligatoria — es lo que permite
  conectar cualquier lenguaje: JavaScript vía Node/Deno FFI, y los que
  vengan. Los bindings son envoltorios finos que respetan el principio de la
  frontera; **cómo se construye cada binding es ortogonal a ese principio**:
  para Python (target principal) las dos vías son `ctypes` de la stdlib
  sobre la C ABI (Python puro, cero build propio, pero firmas declaradas a
  mano — frágil) o un módulo **PyO3** (extensión nativa: clases idiomáticas,
  errores → excepciones, buffer protocol zero-copy trivial; se distribuye
  como wheel). PyO3 no impone dependencias al cliente — expone tipos
  nativos y `memoryview` sobre `f32` planos, sin numpy — y para el modo
  embebido es la vía más natural: el módulo *es* el servidor enlazando el
  engine directo, sin pasar por la C ABI. Opciones a definir al encarar:
  ¿cliente Python por ctypes o PyO3? (PyO3 favorito para el modo embebido;
  ctypes alcanza para el caso dos-procesos); ¿token de correlación en el
  protocolo? (empezar sin él, serializando requests en el binding);
  ¿buffers grandes por el segmento shm en el caso dos-procesos? (empezar
  sin eso: copiar al segmento duplica memoria y complica el layout — datos
  grandes quedan para el modo embebido, que es el caso de uso científico
  real).
  *(Completado 2026-06-12 — ver NOTAS.md. Segmento versionado en
  `server/ipc.rs` (header ABI v1 + reloj de samples espejado por bloque +
  buses de control compartidos de verdad + par de rings SPSC de bytes OSC),
  respaldo archivo-mapeado (`--shm`, cliente Python stdlib `mmap`) o heap
  (in-process); `ClientId` reemplaza a `SocketAddr` en el server (replies
  enrutados por transporte); C ABI `embed` (cdylib): `clausters_render`
  síncrono + servidor vivo in-process; binding `clients/python/clausters.py`
  con la fachada síncrona. Diferidos explícitos: semáforo de wakeup (v1
  polling 2 ms), múltiples clientes de ring, token de correlación, buffers
  por shm, JS/wasm.)*

- **M15 — Documentación integral en inglés (README + libro mdBook + rustdoc)**:
  hoy la documentación en inglés está bien pero dispersa en `docs/`
  (`architecture.md` desarrollo; `schemas.md` referencia OSC/usuario;
  `auto-order.md`, `parallel.md`, `sample-clock.md`, `ipc.md` por feature) y
  falta una puerta de entrada y una estructura navegable que la unifique. Tres
  audiencias a cubrir: usuario por OSC, usuario como **librería**/embebido
  (`rlib`+`cdylib`: `engine_pair`, `render_to_wav`, el C ABI), y desarrollador.
  Plan:
  - **README.md** en la raíz, en inglés (obligatorio): overview, quickstart
    (build → correr servidor → un comando OSC; y un render NRT), matriz de
    features (`realtime`/`faust`/`embed`), links al libro y al rustdoc, licencia
    GPL-3.0. No duplica el libro, enlaza.
  - **Libro mdBook** como cuerpo navegable, **estándar de la comunidad Rust**
    (el fuente vive en el repo, el HTML generado se ignora en git). `book.toml`
    en la raíz con `src = "docs"` para **reusar los `docs/*.md` en su lugar**
    (cero churn en las referencias entrantes a `docs/x.md` que hay en rustdoc,
    tests y este archivo). `docs/SUMMARY.md` arma el índice; capítulos nuevos en
    `docs/`: `introduction.md`, `getting-started.md` (versión inglés de las
    partes ejecutables), `using-as-a-library.md`, `examples.md` (catálogo de
    `examples/` y `clients/python/`), `contributing.md` (setup de desarrollo,
    libfaust desde fuente, regla E2E de una sola invocación de Bash). Los
    capítulos existentes (`architecture.md`, `schemas.md`, los de feature) se
    reusan tal cual.
  - **rustdoc** como referencia de API: expandir el doc-comment de crate
    (`src/lib.rs`) para orientar (split engine/red, feature flags, entry
    points), enlazado desde y hacia el libro.
  - Los archivos en español (`PLAN.md`, `NOTAS.md`, `GUIA.md`) **se mantienen en
    español y en su lugar** — son del autor y se siguen actualizando; la doc de
    usuario en inglés es nueva/aparte (`GUIA.md` sigue siendo el checklist de QA
    por milestone).
  - Opcional (fuera del primer pase): workflow de CI para `mdbook build` +
    deploy a GitHub Pages y `mdbook test`; dividir `schemas.md` si queda largo.

  Criterio de cierre: `mdbook build` y `cargo doc` limpios y sin links rotos;
  README y libro con un camino claro desde la portada para cada una de las tres
  audiencias.

- ✅ **M16 — Persistencia de defs en disco + caché de bitcode**: hoy los defs
  (`/d_recv` y `/d_faust`) son volátiles, viven solo en memoria; un cliente que
  arma una biblioteca (incluso importando piezas de faustlib como faustdefs)
  tiene que reenviarla cada sesión. Guardar los defs en un directorio de datos y
  recargarlos al arrancar, en dos capas: **B** — la definición original (JSON
  del `SynthDefSpec` para UGens, source/JSON del def Faust) como fuente de verdad
  transparente, que se recompila al recargar; **A** — para Faust, una caché del
  bitcode LLVM (`writeCDSPFactoryToBitcodeFile`) **no autoritativa**, keyed por
  versión de libfaust + sha del payload, que salta el front-end de Faust en el
  arranque y hace fallback a recompilar ante cualquier miss/corrupción/upgrade.
  Dos subdirs `synthdefs/` y `faustdefs/`; dir resuelto por
  `--data-dir`/`$CLAUSTERS_DATA_DIR`/XDG, `--no-persist` para apagarlo, solo en
  el server RT (NRT no persiste). Recarga incremental en el hilo compilador para
  no bloquear el arranque con bibliotecas grandes. El `FaustDef` en sí no se
  serializa (factory JIT opaca): se persiste la definición, no el artefacto.
  *(Completado 2026-06-16 — ver NOTAS.md. FFI de bitcode + `getCLibFaustVersion`;
  módulos `faust::cache` y `server::defstore`; `CacheJob`/`client: Option` en el
  hilo compilador; wiring en `osc::server` + flags en `main`; dep `sha2`.
  `tests/persistence.rs` (3 core + 6 faust): round-trip de bitcode
  sample-idéntico, recarga end-to-end entre dos servidores, version mismatch,
  fallback ante corrupción, borrado por `/d_free`. Docs en `schemas.md`,
  `architecture.md`, `examples.md`, `GUIA.md` y `examples/persistence.sh`.)*

### Ideas revisadas: qué se descartó y por qué

- **Denormales** (de la idea de memoria/eficiencia): ya implementado post-M7
  (`dsp::denormals::flush_to_zero()` + `-ftz 2` + `tests/denormals.rs`);
  solo faltaba la parte de skill/documentación, absorbida por M10.
- **F5 original**: `soundfile` y polifonía nativa de Faust descartados con
  el racional en el propio F5 (arriba); `waveform`, interpreter backend y
  Signal API se mantienen, con el interpreter ligado al target wasm de M14.
- **UI de Faust**: la implementación se considera correcta — usar los labels
  de Faust como nombres de control es deliberado (los nombres los elige el
  autor de la def, como en el JSON de UGens) —; lo pendiente era documentar
  el racional (M9) y la generalización «params alimentados por buses de
  control» (M11).
- **API de plugins**: documentar la API interna sí (M9); plugins dinámicos
  no por ahora — Rust no tiene ABI estable y el problema histórico de
  scsynth confirma el costo de mantener esa frontera. La mitigación
  (versionar la frontera binaria) se aplica donde la frontera existe de
  verdad: la C ABI del modo embebido y el layout del segmento de M14.

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
- Cerrar un milestone incluye siempre, cuando corresponda: la documentación
  de desarrollo (`docs/architecture.md`, docs de módulos), la documentación
  de usuario en `docs/` para features nuevas, los pasos de prueba manual y
  conteos en `GUIA.md`, y un ejemplo explicado en `examples/` si la feature
  es de cara al usuario — no solo el código y NOTAS.md.