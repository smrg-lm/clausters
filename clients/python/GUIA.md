# Guía de prueba del cliente Python

Cómo probar a mano la librería cliente de **Clausters** (paquete
`clausters`, port Faust-first de sc3) milestone a milestone (C0–C9). Pensada
para Linux / Ubuntu 24.04+. Es el equivalente de la `GUIA.md` de la raíz, pero
para la librería en vez del servidor.

Referencia: `clients/PLAN.md` (plan y milestones), `clients/python/README.md`
(estructura del paquete), y la `GUIA.md` de la raíz para compilar el servidor.

## Qué es

Un cliente de alto nivel que arma definiciones Faust, maneja recursos del
servidor (nodos, buses, buffers) y secuencia eventos con relojes y rutinas. Lo
numérico (builtins, white noise, conversiones de tiempo) se computa en un
**núcleo nativo en Rust compartido con el servidor** (`clausters-core` vía el
cdylib `clausters-ffi`), así los resultados del cliente coinciden con el
servidor. La frontera es de **datos planos**: números/bytes entran, floats /
`array('f')` salen.

## 1. Requisitos

```sh
# Python 3.10+ (usa anotaciones modernas)
python3 --version

# Las dos bibliotecas nativas las compila cargo (no pip). Desde la raíz del repo:
cargo build -p clausters-ffi                    # libclausters_ffi  (el núcleo: _native)
cargo build --features embed,realtime,faust     # libclausters      (transport: render/Clausters, con Faust)
```

Quedan en `target/debug/` (agregá `--release` para `target/release/`). El
cliente las ubica solo; si hace falta forzar una ruta:
`CLAUSTERS_FFI_LIB=…/libclausters_ffi.so` y `CLAUSTERS_LIB=…/libclausters.so`.

> Nota: `transport._find_library` prefiere `target/release/`. Si ahí quedó un
> `libclausters.so` viejo **sin** la feature `embed` (o sin `faust`), `render()`
> fallará; rebuildeá ese release o exportá `CLAUSTERS_LIB` al de `debug`.

Para correr los snippets, parate en `clients/python` o exportá el path:

```sh
cd clients/python            # o: export PYTHONPATH=$PWD/clients/python
```

## 2. Núcleo nativo accesible (C0 + C1)

El binding ctypes sobre el núcleo. Comprueba equivalencia f32 con el servidor.

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters import _native as n
print("ABI", n.abi_version())                       # 1
print("add escalar (f32):", n.binary(n.BinaryOp.ADD, 1.5, 2.0))  # 3.5
print("mul broadcast:", list(n.binary(n.BinaryOp.MUL, [1.0,2.0,3.0], 2.0)))          # [2,4,6]
print("white noise determinista:", list(n.white_noise(42,4)) == list(n.white_noise(42,4)))
print("beats->secs:", n.beats_to_secs(2.0,0.0,0.0,2.0))   # 1.0 (120 bpm)
print("secs->samples:", n.secs_to_samples(1.0,48000.0))   # 48000
PY
```

`render()` offline (el "flujo científico", sin servidor) — necesita
`libclausters` con `embed`:

```sh
PYTHONPATH=. python3 - <<'PY'
import clausters
from clausters.base import _osclib as osc
sc = osc.score(
    osc.score_bundle(0.0, osc.message("/s_new","default",1000,1,0,"freq",440.0,"amp",0.2)),
    osc.score_bundle(0.3, osc.message("/n_free",0)),   # cierra el render
)
samples, frames = clausters.render(sc, sample_rate=48000.0, channels=2)
print(f"render {frames} frames, peak {max(abs(s) for s in samples):.3f}")
PY
```

## 3. Capa base: builtins, rutinas, reloj, la costura (C2)

Builtins escalar/lista en f32 (igualan al servidor), operadores, rutinas:

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.base import builtins as B
from clausters.base import AbstractObject, Routine
print("f32 != f64:", B.add(0.1,0.2) != 0.1+0.2)        # True
print("lista cíclica:", B.add([10.,20.,30.,40.],[1.,2.]))  # [11,22,31,42]
print("midicps(69):", B.midicps(69))                    # ~440

def cuenta(_):
    for i in range(3):
        yield i
r = Routine(cuenta)
print("routine:", [r.next(), r.next(), r.next()])       # [0,1,2]
PY
```

**La costura** (lo central): el **reloj solo agenda y mide el tiempo**; la
**`Server` posee la interfaz de comunicación y emite** (C4). Una misma rutina
produce un score NRT si la `Server` tiene una `OscNrtInterface`; cambiando la
interfaz por `OscUdpInterface` la misma rutina se manda en vivo, sin tocarla.

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.base import Routine, TempoClock, OscNrtInterface
from clausters.defs import Server
server = Server(interface=OscNrtInterface())             # modo NRT (sin servidor)
clock = TempoClock(tempo=2.0)                            # el reloj solo mide el tiempo
def arpegio():                                          # halla su reloj vía main.current_tt
    for i, freq in enumerate([262.,330.,392.,523.,659.]):
        nid = 1000+i
        server.send_bundle(("/s_new","default",nid,1,0,"freq",freq,"amp",0.2))
        server.send_bundle(("/n_free",nid), delay_beats=0.9)
        yield 1.0
    server.send_bundle(("/n_free",0))                    # cierra
clock.play(Routine(arpegio)); clock.render()             # drena la cola (NRT)
samples, frames = server.render(sample_rate=48000.0, channels=2)
print(f"seam NRT: {frames} frames, peak {max(abs(s) for s in samples):.3f}")
PY
```

## 4. Defs Faust-first y recursos (C3)

Construir un grafo Faust con `signals` (callables en minúscula que mapean la
Signal API; se componen con operadores), envolverlo en un `FaustDef`, y los
allocators de recursos:

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.defs import signals as S, FaustDef, NodeIdAllocator, AudioBusAllocator
freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
phasor = S.rec(lambda s: (s + freq/S.sr()) % 1.0)        # feedback explícito; S.sr() = ma.SR
sine = S.sin(phasor * S.TAU) * 0.2
fdef = FaustDef.from_signals("sine", sine)
print("controles:", fdef.control_names(), "+ reservados", fdef.reserved)
print("dump_def (recorte):", fdef.dump_def()[:70], "...")

ids = NodeIdAllocator(1000); print("ids:", ids.alloc(), ids.alloc())
buses = AudioBusAllocator(size=128, reserved=2); b = buses.alloc(2)  # size = conteo del servidor
print("bus audio:", b.index, b.channels)                 # 2 2 (arriba de las salidas)
PY
```

### Vertical slice **offline** (NRT, sin servidor) — necesita `embed,faust`

Grafo → `/d_faust` → `/s_new` → control por reloj → `render()`:

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.base import Routine, TempoClock, OscNrtInterface
from clausters.defs import signals as S, FaustDef, Server
freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
phasor = S.rec(lambda s: (s + freq/S.sr()) % 1.0)        # S.sr() lo da el servidor
fdef = FaustDef.from_signals("c3sine", S.sin(phasor*S.TAU)*0.2)
server = Server(interface=OscNrtInterface()); clock = TempoClock(tempo=1.0)
def play():
    server.send_bundle(("/d_faust", fdef.name, fdef.dump_def()))   # def primero
    server.send_bundle(("/s_new", fdef.name, 1000, 1, 0))
    yield 0.5
    server.send_bundle(("/n_set", 1000, "freq", 660.0))           # control por reloj
    yield 0.5
    server.send_bundle(("/n_free", 1000)); server.send_bundle(("/n_free", 0))
clock.play(Routine(play)); clock.render()
samples, frames = server.render(sample_rate=48000.0, channels=2)
print(f"E2E NRT: {frames} frames, peak {max(abs(s) for s in samples):.3f}")
PY
```

### Vertical slice **en vivo** (servidor UDP) — facade `Server`

Regla E2E (igual que la `GUIA.md` raíz): servidor y cliente en la **misma**
invocación Bash (el sandbox aísla la red entre invocaciones). El servidor usa
el puerto fijo 57110.

```sh
# desde la raíz del repo, con el binario compilado con faust
(./target/debug/clausters & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 - <<'PY'
from clausters.defs import Server, FaustDef
from clausters.defs import signals as S
srv = Server()                                # 127.0.0.1:57110
print("status:", srv.status()[:5])
freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
phasor = S.rec(lambda s: (s + freq/S.sr()) % 1.0)        # S.sr() = ma.SR del servidor
fdef = FaustDef.from_signals("livesine", S.sin(phasor*S.TAU)*0.2)
print("add ->", srv.add_faustdef(fdef))       # bloquea hasta /done (compila Faust)
syn = srv.synth("livesine", target=0)         # /s_new, id asignado por el cliente
srv.set(syn, {"freq": 440.0}); srv.sync(); srv.free(syn)
print("LIVE OK"); srv.close()
PY
 kill $SRV 2>/dev/null)
```

Con altavoces deberías oír un tono que cambia de 330 a 440 Hz antes de
liberarse. (Sin placa de audio, el server igual responde por OSC pero no suena.)

#### Envío async de def + barrera `/sync`

`add_faustdef`/`add_synthdef` bloquean por defecto (`wait=True`, esperan
`/done`). Para no bloquear, `wait=False` (fire-and-forget) y luego `srv.sync()`
como barrera real (`/sync`→`/synced`): el server responde recién cuando
terminaron todas las compilaciones/jobs async previos. (En la **misma**
invocación Bash con un server compilado con `faust`.)

```sh
(./target/debug/clausters & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 - <<'PY'
from clausters.defs import Server, FaustDef
from clausters.defs import signals as S
srv = Server()
fdef = FaustDef.from_signals("asyncsine",
    S.sin(S.rec(lambda s:(s+330.0/S.sr())%1.0)*S.TAU)*0.2)
print("fire-and-forget ->", srv.add_faustdef(fdef, wait=False))  # no bloquea
print("synced id ->", srv.sync())               # barrera: espera la compilacion
syn = srv.synth("asyncsine", target=0)          # la def existe seguro tras /synced
srv.sync(); srv.free(syn)
print("ASYNC OK"); srv.close()
PY
 kill $SRV 2>/dev/null)
```

**Regla**: `sync()` (y `wait=True`) bloquean el thread; **nunca** llamarlos
dentro del generador de una rutina (congelarían el reloj). Para crear defs
desde una rutina, `wait=False` y resolver la dependencia sin bloquear (la
barrera no-bloqueante que se pueda `yield` llega con `OSCFunc`).

#### Sample rate desde el servidor (`S.sr()`)

`S.sr()` es la versión de `ma.SR` de Faust: una constante foránea (`fconst`)
que el servidor resuelve al compilar la def, **no** un número horneado. Así una
def queda afinada a cualquier tasa. `S.PI`/`S.TAU` son literales (igual que
`ma.PI`), floats de Python. El ejemplo `examples/biquad_signal.py` arma un
biquad RBJ con coeficientes calculados sobre `S.sr()`; debe sonar afinado a
cualquier `sample_rate` del render:

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.base import Routine, TempoClock, OscNrtInterface
from clausters.defs import Server
import sys; sys.path.insert(0, "../../examples"); import biquad_signal as ex
for rate in (44100, 48000, 96000):
    srv = Server(interface=OscNrtInterface()); clk = TempoClock(tempo=ex.TEMPO)
    srv.add_faustdef(ex.build_def()); clk.play(Routine(lambda: ex.voice(srv))); clk.render()
    s, f = srv.render(sample_rate=rate, channels=2)
    print(f"rate={rate}: {f} frames, peak {max(abs(x) for x in s):.3f}")  # peak ~0.74 en todas
PY
```

### Def UGen propia con `SynthDef` (C5 leftover, `/d_recv`)

La contraparte UGen de `FaustDef`: el grafo se arma con callables minúscula
(`sin_osc`, `control`, `out`, …) y `SynthDef` lo serializa al JSON
`SynthDefSpec` que el servidor compila por `/d_recv`. **Instance-based, sin
contexto global de build** (varias defs en paralelo). El grafo equivalente al
`default` interno (`SinOsc(freq)*amp` a los buses 0 y 1) rinde **byte-idéntico**.

Detalles de la clase tal como quedó: el recorrido es post-orden (los `ugens`
salen topológicamente ordenados y los subgrafos compartidos se emiten una sola
vez, dedup por identidad); los controles se juntan por nombre en orden de
aparición (reusar un nombre con otro default es error). **Solo los operadores
`+ - * /`** componen UGens (`Add`/`Sub`/`Mul`/`Div` — los únicos math UGens del
servidor); cualquier otro operador o función matemática (`.sin()`, `%`, `min`,
comparaciones) levanta `TypeError` claro: para eso está un Faust def
(`signals`). Los outputs deben ser UGens (`out`/`replace_out`/`local_out`).

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.defs import SynthDef, control, sin_osc, out
freq, amp = control("freq", 440.0), control("amp", 0.2)
sig = sin_osc(freq) * amp                      # los operadores → UGens Mul/Add/…
sd = SynthDef("py_default", out(0.0, sig), out(1.0, sig))
print(sd.dump_def())                           # el JSON que ve /d_recv
PY
```

En vivo (regla E2E, misma invocación Bash): `add_synthdef` bloquea hasta
`/done`, después `synth(...)` instancia igual que una def interna.

```sh
(./target/debug/clausters --no-persist & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 -c "
from clausters.defs import Server, SynthDef, control, sin_osc, out
srv=Server()
sig = sin_osc(control('freq',440.))*control('amp',0.2)
print('add_synthdef ->', srv.add_synthdef(SynthDef('py_beep', out(0.,sig), out(1.,sig))))
n=srv.synth('py_beep', {'freq':330.}); srv.sync()
print('synths/defs:', srv.status()[2], srv.status()[4]); srv.free(n); srv.close()
"; kill $SRV 2>/dev/null)
```

### Programa de grafo con `GraphDef` (M18, `/d_graph`)

La tercera clase de def: donde `SynthDef`/`FaustDef` describen **un** nodo,
`GraphDef` describe un **patch entero** — varios nodos miembro cableados por
buses internos — que el servidor guarda e instancia como una unidad, con una
**superficie de parámetros nombrados** (puertos que mapean a controles internos,
con escala opcional). Se maneja la instancia por los nombres de puerto, nunca
por los ids de los nodos miembro (encapsulamiento). Es un builder JSON fino,
como las otras dos defs: se compone y se manda con `server.add_graphdef`
(`/d_graph`).

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.defs import GraphDef
g = GraphDef("duo")
mix = g.bus("mix")                          # un bus interno privado (audio)
t1 = g.add("tone", out=mix)                 # dos osciladores suman en `mix`
t2 = g.add("tone", out=mix)
amp = g.add("gain", **{"in": mix})          # lee `mix` -> salida
g.port("freq", t1["freq"], t2["freq"].scaled(1.5), default=220.0)  # un puerto, dos destinos
g.port("gain", amp["gain"], default=0.4)
print(g.dump_def())                         # el JSON que ve /d_graph
PY
```

Ejemplo idiomático completo, renderizado offline (necesita `embed,realtime`):
`python3 examples/graphdef.py` — imprime el JSON, instancia el grafo
(`server.graph("duo", {...})`), barre el puerto `freq` y rinde audio. La
contraparte de grupo (`/n_set` que se propaga a todo un grupo, semántica
scsynth) está en `python3 examples/group_set.py`. El valor de un control miembro
puede ser un número, una referencia de bus (`g.bus(...)`, se wirea el control a
ese bus) o `"OUT"` (bus de hardware 0).

**Partición shared/per-voz (instrumento polifónico).** Un miembro con
`voice=True` es per-voz: la parte **compartida** se instancia una vez con
`server.graph(...)` (el bus privado, el mixer) y cada nota agrega una voz con
`server.graph_voice(instancia, {...})`, cableada al mismo bus. Un puerto de
superficie apunta o a miembros compartidos o a per-voz (no mezcla). Ejemplo
completo: `python3 examples/graphdef_poly.py` (mixer compartido + oscilador
per-voz, arpegio de voces solapadas). Es el mismo modelo que usa
`/midi_bind canal nombreGraph` en el servidor: instancia la parte compartida al
bindear y cada nota spawnea una voz.

### Ejemplos combinados: el ciclo de vida en vivo (`live_patch.py`, `persistent_graphdef.py`, `faust_soundfile.py`)

Necesitan hardware de audio real y el server con Faust. Construilo y corré (cada ejemplo **arranca y para su propio servidor** vía `subprocess`, usando `ServerOptions.args()`):

```sh
cargo build --release --features faust
python3 examples/live_patch.py            # config + launch + grupos + FaustDef + SynthDef + buses + buffer
python3 examples/persistent_graphdef.py   # lo anterior, empaquetado como GraphDef persistente
python3 examples/faust_soundfile.py       # FaustDef que lee un buffer del servidor via soundfile
```

`live_patch.py` cablea el patch a mano: `ServerOptions(audio_buses, control_buses, sample_rate)` lanza un server y dimensiona los allocators (verificado con `query_info`); dos grupos (fuentes y salida) dan orden de ejecución; una voz **FaustDef** y un reproductor de buffer **SynthDef** escriben a buses de audio privados que un mixer **SynthDef** suma a las salidas; un bus de **control** mapeado a `freq` reafina la voz con un solo `/c_set`; el buffer se genera (WAV con `wave`) y se carga por `/b_allocRead` (vía `server.request`, ya que `alloc_buffer` solo hace el `/b_alloc` vacío). Debe sonar (RMS verificado capturando la salida con `pw-record`).

`faust_soundfile.py` lleva el op `soundfile` de Faust al cliente idiomático: genera un motivo corto (WAV con `wave`), lo carga por `/b_allocRead` (mismo idiom que `live_patch.py`: índice del allocator + `server.request`) y lo loopea desde **adentro** de un `FaustDef` que lo lee con `soundfile("<bufnum>", 1)` — sin `PlayBuf` ni bus de por medio. El def se arma con `FaustDef.from_source` (la API de señales `signals` no expone `soundfile`); la etiqueta del op es el índice que el cliente asignó. `gain`/`speed` se modulan en vivo con `/n_set`. Lo que demuestra: el bind es una **foto** tomada en `/s_new` — recarga el buffer a mitad de reproducción y la voz que ya suena no cambia; solo un `/s_new` nuevo ve el contenido nuevo (el ejemplo lanza una segunda voz para mostrarlo). Es un demo en vivo, pero `soundfile` también anda en el renderer NRT offline: el espejo de buffers que `make_synth` pasa a `FaustSynth::new` se llena tanto en el server RT como en `render.rs` (regresión cubierta por `soundfile_reads_a_score_buffer_in_nrt` en `tests/golden.rs`). Debe sonar.

`persistent_graphdef.py` muestra la persistencia: lanza el server con `--data-dir` en un **subdirectorio dentro de la carpeta del ejemplo** (`examples/defs_store/`, gitignoreado), manda los defs miembro y el `GraphDef` (el server los agrupa bajo `defs/`: `defs/synthdefs/`, `defs/faustdefs/`, `defs/graphdefs/`), y en una **segunda fase** relanza un server nuevo sobre el mismo `--data-dir` e instancia el GraphDef **sin reenviar nada** (solo suena porque los defs se recargaron de disco al bootear; `server.status()[4]` reporta el conteo). El directorio **queda en disco** para que abras los JSON persistidos y los explores; borralo a mano cuando termines (`rm -rf examples/defs_store`).

### Introspección del árbol de nodos (`query_tree`, `node_query`, `dump_graph`)

El estado del árbol (nodos, ids, def, controles, maps, buses) se obtiene del server como **datos estructurados** (nunca se parsea el log). Tres métodos del `Server`, demostrados en `python3 examples/introspect_tree.py`:

```python
tree = server.query_tree()              # /g_queryTree -> dict anidado {id, children|def+controls}
info = server.node_query(node)          # /n_query -> {id, parent, prev, next, is_group, def, controls, maps, reads, writes}
print(server.dump_graph(group.id))      # /g_dumpGraph -> texto legible del grafo de buses
```

Los logs del server (stderr) son aparte y los controla el cliente con `/verbosity` (nivel) y `/dumpOSC` (target OSC), o el binario con `-v`/`RUST_LOG` (ver `GUIA.md` raíz).

Para recorrerlo paso a paso al estilo notebook (config + boot + grupos + nodos + buses, logueando el árbol en cada paso): `examples/interactive_session.py`, en celdas `# %%` (se ejecuta por partes en IPython/VS Code/Jupyter, o entero como script).

## 5. Secuenciación: patterns y eventos (C5)

Un `Pbind` toca una secuencia de notas; corre **NRT** (score → `render()`) o
**RT** (en vivo) solo cambiando la interfaz de la `Server`. El tiempo es exacto
por `yield`: con `dur=0.5` los `/s_new` caen en `0, 0.5, 1.0, 1.5` exactos.

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.base import TempoClock, OscNrtInterface
from clausters.defs import Server
from clausters.seq import Pbind, Pseq
server = Server(interface=OscNrtInterface()); clock = TempoClock(tempo=1.0)
Pbind(instrument="default", freq=Pseq([262.,330.,392.,523.]), dur=0.5, amp=0.2).play(clock, server)
clock.render()                                          # drena la cola (NRT)
import struct
from clausters.base import _osclib as osc
def inner(raw): n=struct.unpack(">i",raw[16:20])[0]; return osc.decode(raw[20:20+n])[0]
starts = sorted(w for w,raw in server.interface.score.bundles if inner(raw)=="/s_new")
print("starts (exactos):", starts)                      # [0.0, 0.5, 1.0, 1.5]
samples, frames = server.render(sample_rate=48000.0, channels=2)
print(f"render: {frames} frames, peak {max(abs(s) for s in samples):.3f}")
PY
```

En vivo (mismo `Pbind`, servidor UDP), regla E2E (misma invocación Bash):

```sh
(./target/debug/clausters & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 -c "
from clausters.base import TempoClock
from clausters.defs import Server
from clausters.seq import Pbind, Pseq
srv=Server(latency=0.1); clock=TempoClock(tempo=4.0)
Pbind(instrument='default', freq=Pseq([262.,330.,392.,523.]), dur=1.0, amp=0.2).play(clock, srv)
clock.run(1.3); print('synths:', srv.status()[1:4]); srv.close()
"; kill $SRV 2>/dev/null)
```

### Exportar a un archivo MIDI estándar (`.mid`) (M17 sub-parte 1)

El **mismo** `Pbind` puede apuntar a un destino MIDI en vez de la `Server` OSC:
`MidiServer` es la contraparte por doble-dispatch. Cada nota se realiza como
note on/off en un `MidiScore` (en beats) y `write` lo serializa a `.mid` por el
crate `clausters-midi`. Sin servidor ni audio: solo un archivo.

```sh
cargo build -p clausters-midi            # construye libclausters_midi
PYTHONPATH=. python3 - <<'PY'
from clausters.base import TempoClock, MidiServer
from clausters.seq import Pbind, Pseq
midi = MidiServer(channel=0, ppq=480); clock = TempoClock(tempo=2.0)
Pbind(instrument="default", midinote=Pseq([60,64,67,72]), dur=0.5, amp=0.6).play(clock, midi)
clock.render()                           # NRT: drena la rutina, sin dormir
ons = [(b, m[1]) for b, m in midi.score.sorted() if (m[0] & 0xF0) == 0x90 and m[2] > 0]
print("note-ons (beat, nota):", ons)     # [(0.0,60),(0.5,64),(1.0,67),(1.5,72)]
midi.write("/tmp/out.mid")
print("MThd:", open("/tmp/out.mid","rb").read(4) == b"MThd")
PY
```

El número de nota sale de `Event.midinote()` (de `midinote`/`degree`, o de un
`freq` explícito vía `cpsmidi`), la velocity de `amp` (0..1 → 0..127). Ejemplo
comentado: `examples/midi_file.py` (con `--clip`).

**Clip MIDI 2.0 (resolución completa):** `midi.write(path, fmt="clip")` escribe
un archivo SMF2CLIP (cabecera `SMF2CLIP`) con la velocity a **16 bits** en vez
de 7. El crate lo arma con los mensajes UMP de `midi2` (el `midi2-clip` pautado
resultó un stub `todo!()`).

**Salida en vivo (sub-parte 2):** con el cdylib construido con `--features live`,
`MidiServer(interface=MidiRtInterface("clausters"))` **crea un puerto virtual de
salida ALSA** (vía `midir`) y emite cada nota en tiempo real (note-on en su beat,
note-off programado con `clock.sched_abs`). Ese puerto es un cable suelto hasta
que lo enrutás con `aconnect` a un destino con entrada MIDI (un synth, un DAW, o
la entrada MIDI del propio servidor). Orden: primero corré el script (crea el
puerto), después arrancá el destino, y al final conectalos:

```sh
cargo build --release -p clausters-midi --features live
python3 examples/midi_live.py clausters 4         # crea el puerto "clausters" y toca 4 s
# en otra terminal, el destino (la entrada MIDI del servidor):
cargo run -- --midi clausters-in
# y los unís (ver todos los puertos con `aconnect -l`):
aconnect clausters clausters-in
```

Al cerrar el puerto, `MidiRtInterface.close()` envía un **all-notes-off (CC 123)
en los 16 canales** (el "panic" MIDI estándar): si el reloj se detiene a mitad
del patrón, los note-off que quedaron agendados más allá del corte no se emiten,
así que sin esto la última nota quedaría colgada sonando en el destino.

El loop completo cliente→servidor (salida del cliente → `aconnect` → `--midi`
del servidor → crea synths) está verificado en `LOG.md` (M17 sub-parte 2).

### Anclado al reloj del servidor (C6, en vivo por UDP)

Para timing sample-accurate sin drift, anclá el reloj al **sample-clock del
servidor** (`/sched` en vez de timetag NTP). `Server.sample_clock()` modela el
reloj por `/clock`; su `.timebase()` alimenta el `TempoClock`:

```sh
(./target/debug/clausters & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 -c "
from clausters.base import TempoClock
from clausters.defs import Server
from clausters.seq import Pbind, Pseq
srv=Server(latency=0.2); sc=srv.sample_clock(); sc.warmup(); sc.track()
clock=TempoClock(tempo=4.0, timebase=sc.timebase())   # paça contra el servidor
Pbind(instrument='default', freq=Pseq([262.,330.,392.,523.]), dur=1.0, amp=0.2).play(clock, srv)
clock.run(1.4); print('synths:', srv.status()[1:4]); sc.close(); srv.close()
"; kill $SRV 2>/dev/null)
```

### Con `Session` (ergonomía sin globales)

`Session` agrupa `Server`+reloj con fábricas; varias sesiones coexisten (NRT +
RT en el mismo script). Equivale a lo de arriba, más corto:

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters import Session
from clausters.seq import Pbind, Pseq
s = Session.nrt(tempo=2.0)
s.play(Pbind(instrument="default", freq=Pseq([262.,330.,392.,523.]), dur=0.5, amp=0.2))
samples, frames = s.render()        # drena el reloj y rinde el score
print(f"session NRT: {frames} frames, peak {max(abs(x) for x in samples):.3f}")
PY
```

### Transporte TCP (C8)

El servidor también habla **OSC por TCP** con `--tcp` (length-prefixed: prefijo
de 4 bytes big-endian + bytes OSC, mismo framing que scsynth). `OscTcpInterface`
es drop-in de `OscUdpInterface`: la `Server` lo usa igual (request/reply,
`/d_recv`, synths). Regla E2E (misma invocación Bash):

```sh
(./target/debug/clausters --tcp --no-persist & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 -c "
from clausters.base import OscTcpInterface
from clausters.defs import Server
srv = Server(interface=OscTcpInterface().start())   # conecta por TCP
print('status:', srv.status())                      # round-trip enmarcado
import time; n = srv.synth('default', {'freq': 440.}); time.sleep(0.2)
print('synths:', srv.status()[2]); srv.free(n); srv.close()
"; kill $SRV 2>/dev/null)
```

**UDP siempre + TCP opcional.** El UDP es el transporte base y se bindea siempre
(no hay modo «solo TCP»); `--tcp` *agrega* el listener TCP sin reemplazarlo. Es
además **infraestructura interna**: el wake del loop TCP manda un datagrama UDP
de longitud 0 al propio socket del servidor, así que el UDP también sirve para
despertar el loop. `--tcp` toma un puerto opcional (default 57110, junto al UDP —
espacios de nombres separados).

| comando | UDP | TCP |
|---|---|---|
| `clausters` | ✅ 57110 | ❌ |
| `clausters --tcp` | ✅ 57110 | ✅ 57110 |
| `clausters --tcp 9000` | ✅ 57110 | ✅ 9000 |

El timing sigue en los timetags/`/sched`, así que la latencia de llegada no
afecta *cuándo* dispara un comando agendado.

## 6. Suite de tests

```sh
cd clients/python
python3 -m pytest -q   # test_smoke, test_base, test_defs, test_seq, test_synthdef, test_tcp, test_responders, test_timeline
```

`pytest` es la dependencia de dev (PEP 735 `[dependency-groups] dev`): a nivel
sistema `sudo apt install python3-pytest`, o `pip install --group dev`. Los
tests que necesitan los cdylibs hacen *skip* si no están construidos (apuntá
`CLAUSTERS_FFI_LIB`/`CLAUSTERS_LIB` o construilos con `cargo build`).

## 7. Checklist por milestone

| Milestone | Qué probar | Cómo |
|---|---|---|
| C0 | núcleo Rust + paridad con el servidor | `cargo test --workspace` |
| C1 | `_native` (builtins/rng/clock) + `render()` | sección 2 |
| C2 | builtins f32, rutinas, reloj, costura RT/NRT | sección 3 |
| C3 | `signals` + `FaustDef` + allocators + slice E2E | sección 4 (NRT y vivo) |
| C4 | reloj solo timing; `Server` posee comunicación y emite | secciones 3–5 |
| C5 | patterns/eventos; `Pbind` NRT/vivo; timing exacto | sección 5 |
| C5 leftover | `SynthDef` UGen (`/d_recv`); paridad byte-idéntica con `default` | sección 4 (def UGen) |
| C6 | anclaje sample-clock por UDP (`/clock` → modelo → `/sched`) | sección 5 (anclado) |
| C8 | transporte TCP (`--tcp`, length-prefixed); `OscTcpInterface` | sección 5 (TCP) |
| C9 | doc cross-lenguaje + ejemplo de secuenciación de alto nivel | `python3 examples/sequencing.py` (offline) y `--live` |
| M17 s1 | `Pbind` → `.mid` / clip MIDI 2.0 (`MidiServer` + crate) | sección 5 (export MIDI), `tests/test_midi.py`, `examples/midi_file.py` |
| M17 s2 | salida MIDI en vivo por puerto del SO (`MidiRtInterface`, `--features live`) | sección 5 (salida en vivo), `examples/midi_live.py`, smoke en `tests/test_midi.py` |
| M18 | `GraphDef` builder (`/d_graph`) + `server.graph` (`/graph_new`); superficie nombrada | sección 4 (GraphDef), `tests/test_graphdef.py`, `examples/graphdef.py` / `group_set.py` |
| C12 | wheel pip-instalable que empaqueta los cdylibs; venv autocontenido | sección 8 |
| C13 | responders `OscFunc`/`MidiFunc` (camino de entrada); `/transport` push-on-change | sección 9, `tests/test_responders.py`, `examples/osc_responder.py` / `midi_responder.py` |
| C16 | `Timeline` (estatica, acceso aleatorio) + `Playhead` (play/stop/locate/loop/position) | seccion 10, `tests/test_timeline.py`, `examples/timeline_transport.py` |

## 8. Empaquetado e instalación (C12)

El paquete es Python puro en runtime, pero alcanza Rust por artefactos que
**cargo** construye: dos cdylibs (`libclausters_ffi` y `libclausters` con
`embed,realtime`) y, desde C17, el **binario standalone** del servidor. El wheel
los **empaqueta a todos**, así un paquete instalado es autocontenido: no necesita
el directorio `target/` ni reconstruir nada al importar, y el servidor standalone
queda en el PATH como el comando `clausters` (ver seccion 11). Esto sirve también
para instalar desde este repo y correr pruebas autocontenidas en un venv de forma
simple y estándar.

Instalación recomendada en un venv nuevo (corré desde el repo, así el hook de
build encuentra el workspace de cargo):

```sh
python -m venv .venv && . .venv/bin/activate
pip install -e ./clients/python --group dev     # editable + grupo dev (pytest)
# o instalación normal:
pip install ./clients/python
```

`pip install` dispara `setup.py`, que corre `cargo build` para los dos cdylibs y
los deja en `clausters/_libs/` antes de empaquetarlos. Para construir un wheel
redistribuible:

```sh
python -m build --wheel clients/python          # -> clients/python/dist/*.whl
pip install clients/python/dist/clausters-*.whl # autocontenido, sin cargo
```

Para verificar que quedó autocontenido, importá y renderizá **desde otro
directorio** (sin `target/` a la vista):

```sh
cd /tmp
python -c "import clausters; print(clausters.__file__)"
python /ruta/al/repo/clients/python/examples/offline_render.py /tmp/c12.wav
```

Perillas (variables de entorno), todas opcionales: `CLAUSTERS_WORKSPACE` (ruta
al workspace si no se encuentra subiendo directorios), `CLAUSTERS_CARGO_FEATURES`
(features del lib embed, por defecto `embed,realtime`),
`CLAUSTERS_SKIP_NATIVE_BUILD` (empaquetar lo ya staged sin recompilar),
`CLAUSTERS_FFI_LIB`/`CLAUSTERS_LIB` (en runtime, apuntar un loader a un cdylib
concreto). Para stagear los libs a mano: `python clients/python/build_native.py`.

En un checkout sin instalar, los loaders caen al `target/{release,debug}/` del
workspace, así que el flujo histórico de compilar-y-correr sigue igual.

## 9. Responders: recibir OSC y MIDI (C13)

Hasta acá el cliente era solo salida. C13 agrega el **camino de entrada**:
`OscFunc`/`MidiFunc` reciben OSC/MIDI de cualquier aplicación, hacen match y
despachan a un callback (que puede a su vez emitir al servidor o a otros
programas). El servidor ganó además el **push-on-change** de `/transport`:
al fijar el transporte avisa a los clientes `/notify` con `/transport.reply`.

### Tests unitarios (OSC en loopback, MIDI inyectado)

```sh
cd clients/python
python3 -m pytest -q tests/test_responders.py   # 12 tests
```

El lado OSC corre de punta a punta sobre un socket UDP de loopback (un
`OscReceiver` real recibe y dispara el `OscFunc`); el lado MIDI prueba
`parse_midi` y el match de `MidiFunc` con mensajes inyectados (el puerto ALSA
real es la prueba manual de abajo).

### E2E del hub OSC contra un servidor vivo (`osc_responder.py`)

```sh
cargo build -p clausters-ffi --bin clausters
(./target/debug/clausters & PID=$!; sleep 1.5; \
 PYTHONPATH=clients/python python clients/python/examples/osc_responder.py; \
 kill $PID 2>/dev/null)
```

Esperado: imprime las cuatro `/note` recibidas (relayadas al servidor como
synths) y luego `transport changed: ... -> re-aligning` — el `OscFunc` sobre
`/transport.reply` reaccionó al push que el servidor emitió al fijar el
transporte. Una app externa haría lo mismo enviando, p.ej., `/note 60 0.5` al
puerto UDP 57121.

### Push-on-change del servidor (lado Rust)

```sh
cargo test --test osc transport   # transport_query_and_set + transport_pushes_on_change_to_notify_clients
```

### MIDI en vivo: tocar el servidor desde un teclado (manual, `midi_responder.py`)

Necesita el cdylib con `live` (puerto de entrada virtual ALSA):

```sh
cargo build --release -p clausters-midi --features live
cargo run --release   # servidor, en otra terminal
python clients/python/examples/midi_responder.py
```

Abre un puerto MIDI de entrada virtual `clausters-in`. Cableá una fuente
(teclado / DAW) con `pw-link` (listá puertos con `pw-link -o`/`-i`) o en
qpwgraph; con ALSA crudo, `aconnect`. Cada tecla suena un synth en el servidor
hasta que la soltás (note-on → `/s_new`, note-off → `/n_free`).

## 10. Timelines y playhead: transporte estilo DAW (C16)

Las rutinas/patterns son **generativas** (un generador que avanza, sin seek).
Una `Timeline` es lo opuesto: una lista estatica y editable de `(beat, item)`
ordenada por beat, con **acceso aleatorio por tiempo**. Eso habilita controles
de transporte de DAW sobre un `Playhead`: play/stop/locate/loop + song position
(el acceso aleatorio ocurre en los bordes; entre medio es un escaneo adelante).

### Tests unitarios (offline, sin servidor)

```sh
cd clients/python
python3 -m pytest -q tests/test_timeline.py   # 10 tests
```

Cubren: edicion (add/remove/move, orden estable), acceso aleatorio
(`index_at`/`range`/`at`), orden de reproduccion, `locate` (saltea + offsetea),
`loop` (wrap), items OSC/MIDI crudos, `stop` (desagenda el feeder), `position`,
y `from_pattern` (capturar un `Pbind` a una timeline).

### E2E del transporte en vivo (`timeline_transport.py`)

```sh
cargo build -p clausters-ffi --bin clausters
(./target/debug/clausters >/dev/null 2>&1 & PID=$!; sleep 1.5; \
 PYTHONPATH=clients/python python clients/python/examples/timeline_transport.py; \
 kill $PID 2>/dev/null; true)
```

Esperado: captura un patron a una `Timeline`, la edita, y la toca en vivo con el
`Playhead` — imprime la position interpolada (~2.40 beats tras 1.2 s a 2 bps),
hace `locate(1.0)`, `loop(0,2)` y `stop`, todo sin error. Las notas suenan en el
servidor.

### Transporte difundido por el servidor: director en lockstep (`transport_conductor.py`)

El `/transport` del servidor lleva un estado de reproduccion DAW (playing +
position): un director hace `transport_play`/`transport_stop`/`transport_locate`
y el servidor lo difunde a los clientes `/notify`; cada `Playhead.follow_transport`
rueda/para/busca en consecuencia. El servidor difunde CONTROL, no agenda audio.

Lado Rust (estado + difusion):

```sh
cargo test --test osc transport   # query_and_set + play_stop_locate + push_on_change
```

E2E multi-cliente:

```sh
cargo build -p clausters-ffi --bin clausters
(./target/debug/clausters >/dev/null 2>&1 & PID=$!; sleep 1.5; \
 PYTHONPATH=clients/python python clients/python/examples/transport_conductor.py; \
 kill $PID 2>/dev/null; true)
```

Esperado: dos followers (cada uno `lock_to` + `join_transport` + `follow_transport`)
arrancan juntos al `transport_play` del director e imprimen posiciones que
coinciden (lockstep; sample-exacto por el `lock_to`), luego `locate`/`stop`.

## 11. Servidor embebido como destino + wheel con todo (C17)

El servidor embebido (`clausters.Clausters`, que corre el motor en proceso vía el
cdylib) pasa a ser **un destino mas** del cliente de alto nivel, igual que UDP,
TCP o NRT. La pieza nueva es `OscEmbedInterface`: codifica el mismo OSC que UDP
(bundles NTP) pero entrega los bytes por *llamada a funcion* al servidor en
proceso y lee respuestas por polling. Como decodifica por el mismo camino que el
de red y comparte el wall clock, el timing es identico a UDP. Factory:
`Session.embed(...)`, hermana de `Session.nrt`/`Session.live`.

Ademas, el wheel ahora empaqueta tambien el **binario standalone**: queda en el
PATH del venv como el comando `clausters` (servidor separado, por red/memoria
compartida). Asi un solo `pip install` deja: cliente + servidor embebido +
servidor standalone.

### Tests unitarios (se saltean limpio sin audio)

```sh
cd clients/python
python3 -m pytest -q tests/test_session.py   # incluye 2 nuevos de embed
```

Cubren: `Session.embed` maneja el servidor en proceso con la misma API
(`status()`/`query_tree()` round-trip por la interfaz embed, el motor avanza al
tocar un `Pbind`), y una sesion embebida convive con una NRT sin estado global.

### E2E del servidor embebido (`embedded.py`)

No hay que arrancar nada: el motor corre en el proceso.

```sh
cargo build --features embed,realtime          # el cdylib que carga el embebido
PYTHONPATH=clients/python python clients/python/examples/embedded.py
```

Esperado: imprime el sample rate del servidor embebido (48000.0 Hz), toca el
arpegio y libera los synths tras su sustain; exit 0.

### El wheel: embebido + standalone en un solo paquete

```sh
python clients/python/build_native.py          # stagea cdylibs + binario en _libs/ y _bin/
python -m pip wheel clients/python --no-deps --no-build-isolation -w clients/python/dist
```

Verificar en un venv limpio, **desde un cwd neutro** (sin el arbol fuente en
`sys.path`, p. ej. `/tmp`), que estan las dos cosas:

```sh
python -m venv /tmp/venv && /tmp/venv/bin/pip install clients/python/dist/clausters-*.whl
cd /tmp
# 1) comando standalone instalado en el PATH (ejecuta el binario embebido en el wheel):
/tmp/venv/bin/clausters --help | head -1            # imprime "usage:"
# 2) servidor embebido en proceso:
/tmp/venv/bin/python -c "from clausters import Session; s=Session.embed(); print(s.server.status()[0]); s.close()"
```

E2E del standalone instalado contra un cliente UDP (servidor + cliente en la
misma invocacion, regla E2E):

```sh
(/tmp/venv/bin/clausters >/tmp/srv.log 2>&1 & PID=$!; sleep 1.5; \
 /tmp/venv/bin/python -c "from clausters import Session; s=Session.live(); print(s.server.status()); s.close()"; \
 kill $PID 2>/dev/null; true)
```

Esperado: (1) `clausters --help` imprime el usage del servidor; (2) la sesion
embebida responde `status()[0] == 1`; (3) el standalone responde el
`/status.reply` por UDP.

## 12. GUI host: protocolo de widgets (G2)

El host de GUI (`clausters-gui`) es un par mas del sistema: un *servidor de GUI*
para los lenguajes (habla el vocabulario `/gui_*` sobre OSC, la misma codificacion
que el servidor de audio) y a la vez un *cliente del servidor de audio*. En G2 es
un esqueleto sin ventana (sin GPU todavia): registra y consulta el arbol de
widgets y responde `/gui_query` con `/gui_info`.

El host vive en el crate independiente `clients/gui` (no romper el build del core):
se compila y corre desde ahi.

```sh
# 1) compilar el host (desde clients/gui, su propio workspace):
cd clients/gui && cargo build --bin clausters-gui && cd ../..

# 2) E2E del protocolo (host + cliente Python en la MISMA invocacion, regla
#    E2E). `--headless` corre el protocolo sin display (por defecto abre
#    ventanas, ver seccion 13). El driver Python esta en clausters.gui:
(clients/gui/target/debug/clausters-gui --headless --port 57219 -v 2>/tmp/gui_host.log & \
 PID=$!; sleep 1.0; \
 PYTHONPATH=clients/python python3 -c "
from clausters.gui import GuiHost, knob, slider, waveform, window
tree = window(knob(10, label='cutoff', min=20.0, max=20000.0, value=800.0),
              slider(11, label='res', min=0.0, max=1.0, value=0.2),
              waveform(12, buffer=0),
              title='Filter', w=480, h=240, layout='col')
with GuiHost(port=57219) as g:
    g.define(1, tree)
    print('query 10 ->', g.query(10))
    g.set(10, value=440.0); print('tras set ->', g.query(10))
    g.free(1); print('tras free ->', g.query(10))
"; kill $PID 2>/dev/null; true); cat /tmp/gui_host.log
```

Esperado:

- En el log del host: la linea de arranque (`listening on udp://...`) y el
  **arbol parseado** indentado tras `/gui_def 1: 4 widget(s)`
  (`[1] window ...`, `  [10] knob ...`, etc.), luego lineas por cada
  `/gui_query`/`/gui_set`/`/gui_free`.
- En stdout del cliente: `query 10 -> ('knob', {... 'value': 800.0})` (el float
  vuelve float, el int `buffer` vuelve int), `tras set -> ('knob', {... 'value':
  440.0})`, y `tras free -> ('', {})` (tipo vacio = no existe el widget; igual
  responde, como el servidor en un miss).

Tambien hay un ejemplo comentado equivalente:

```sh
clients/gui/target/debug/clausters-gui --headless -v &   # puerto 57210
PYTHONPATH=clients/python python clients/python/examples/gui_skeleton.py
```

El host acepta `--server host:port` para enganchar la tercera pata (host ->
servidor de audio); en G2 solo se construye y se loguea (los bindings que la
usan llegan en un milestone posterior).

## 13. GUI host: primera ventana real (G3)

Por defecto (sin `--headless`) el host abre ventanas: un `/gui_def` con raiz
`window` instancia una ventana winit con superficie wgpu que hostea los
renderers. G3 estandariza `window` + `panel`/layout (`row`/`col`/`grid`/`free`) +
`label`, mas la vista pesada `waveform` alimentada por datos inline (`data`) o por
un blob binario que viaja en el mismo `/gui_def` (`blob`). Los paneles/labels se
pintan como rectangulos de chrome; el texto de los labels (glifos) llega en un
milestone posterior. La waveform navega: rueda = zoom hacia el cursor, arrastrar =
pan, `R` = reset, `Esc`/cerrar = cierra esa ventana (el host sigue vivo).

Necesita display y un adaptador Vulkan/Metal/DX12/GL.

```sh
# host en modo ventana (una terminal):
cd clients/gui && cargo run --bin clausters-gui -- -v
# en otra, con el cliente importable:
PYTHONPATH=clients/python python clients/python/examples/gui_window.py
```

Esperado: se abre una ventana "clausters-gui - waveform" mostrando un seno que
decae; el log del host imprime `gui_def 1: ...`, el arbol indentado y
`gui_def 1: opened window`. Probar rueda/arrastre/`R`. Cerrar la ventana no mata
el host (sigue escuchando OSC).

Notas:

- El tamano del `/gui_def` esta acotado por el datagrama UDP (~64 KB), asi que un
  `blob` de waveform entra hasta ~16000 `f32`; mover buffers grandes sin
  reenviarlos es un milestone posterior (`samples_to_blob` arma el blob LE).
- Varias ventanas a la vez: un `/gui_def` por id de def abre su propia ventana;
  re-enviar el mismo id la reconstruye; `/gui_free <id>` la cierra.
- En una maquina sin display, el modo ventana falla con un mensaje claro
  (`use --headless`); usar `--headless` para el protocolo.
