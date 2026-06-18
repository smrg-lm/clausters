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
interfaz por `OscUDPInterface` la misma rutina se manda en vivo, sin tocarla.

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
from clausters.defs import signals as S, FaustDef, NodeIDAllocator, AudioBusAllocator
freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
phasor = S.rec(lambda s: (s + freq/S.sr()) % 1.0)        # feedback explícito; S.sr() = ma.SR
sine = S.sin(phasor * S.TAU) * 0.2
fdef = FaustDef.from_signals("sine", sine)
print("controles:", fdef.control_names(), "+ reservados", fdef.reserved)
print("payload (recorte):", fdef.payload()[:70], "...")

ids = NodeIDAllocator(1000); print("ids:", ids.alloc(), ids.alloc())
buses = AudioBusAllocator(reserved=2); b = buses.alloc(2)
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
    server.send_bundle(("/d_faust", fdef.name, fdef.payload()))   # def primero
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
print(sd.payload())                            # el JSON que ve /d_recv
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
de 4 bytes big-endian + bytes OSC, mismo framing que scsynth). `OscTCPInterface`
es drop-in de `OscUDPInterface`: la `Server` lo usa igual (request/reply,
`/d_recv`, synths). Regla E2E (misma invocación Bash):

```sh
(./target/debug/clausters --tcp --no-persist & SRV=$!; sleep 1.5; \
 PYTHONPATH=clients/python python3 -c "
from clausters.base import OscTCPInterface
from clausters.defs import Server
srv = Server(interface=OscTCPInterface().start())   # conecta por TCP
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
python3 -m pytest -q   # test_smoke, test_base, test_defs, test_seq, test_synthdef, test_tcp
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
| C8 | transporte TCP (`--tcp`, length-prefixed); `OscTCPInterface` | sección 5 (TCP) |
| C9 | doc cross-lenguaje + ejemplo de secuenciación de alto nivel | `python3 examples/sequencing.py` (offline) y `--live` |
