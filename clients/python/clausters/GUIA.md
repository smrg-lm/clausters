# Guía de prueba del cliente Python

Cómo probar a mano la librería cliente de **Clausters** (paquete
`clausters`, port Faust-first de sc3) milestone a milestone (C0–C3). Pensada
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

**La costura** (lo central): una misma rutina + `TempoClock` produce un score
NRT solo dándole una `OscNrtInterface`; el score se rinde a audio. Cambiar la
interfaz por `OscUDPInterface` lo manda en vivo, sin tocar la rutina.

```sh
PYTHONPATH=. python3 - <<'PY'
from clausters.base import Routine, TempoClock, NetAddr, OscNrtInterface
def arpegio(clock):
    for i, freq in enumerate([262.,330.,392.,523.,659.]):
        nid = 1000+i
        clock.send_bundle(("/s_new","default",nid,1,0,"freq",freq,"amp",0.2))
        clock.send_bundle(("/n_free",nid), delay_beats=0.9)
        yield 1.0
    clock.send_bundle(("/n_free",0))                     # cierra
nrt = OscNrtInterface()
clock = TempoClock(tempo=2.0, target=NetAddr(), interface=nrt)
clock.play(Routine(arpegio)); clock.render()             # drena la cola (NRT)
samples, frames = nrt.render(sample_rate=48000.0, channels=2)
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
phasor = S.rec(lambda s: (s + freq/48000.0) % 1.0)       # feedback explícito
sine = S.sin(phasor * 6.283185307179586) * 0.2
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
from clausters.base import Routine, TempoClock, NetAddr, OscNrtInterface
from clausters.defs import signals as S, FaustDef
freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
phasor = S.rec(lambda s: (s + freq/48000.0) % 1.0)
fdef = FaustDef.from_signals("c3sine", S.sin(phasor*6.283185307179586)*0.2)
nrt = OscNrtInterface(); clock = TempoClock(tempo=1.0, target=NetAddr(), interface=nrt)
def play(clock):
    clock.send_bundle(("/d_faust", fdef.name, fdef.payload()))   # def primero
    clock.send_bundle(("/s_new", fdef.name, 1000, 1, 0))
    yield 0.5
    clock.send_bundle(("/n_set", 1000, "freq", 660.0))           # control por reloj
    yield 0.5
    clock.send_bundle(("/n_free", 1000)); clock.send_bundle(("/n_free", 0))
clock.play(Routine(play)); clock.render()
samples, frames = nrt.render(sample_rate=48000.0, channels=2)
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
phasor = S.rec(lambda s: (s + freq/48000.0) % 1.0)
fdef = FaustDef.from_signals("livesine", S.sin(phasor*6.283185307179586)*0.2)
print("add_def ->", srv.add_def(fdef))        # bloquea hasta /done (compila Faust)
syn = srv.synth("livesine", target=0)         # /s_new, id asignado por el cliente
srv.set(syn, {"freq": 440.0}); srv.sync(); srv.free(syn)
print("LIVE OK"); srv.close()
PY
 kill $SRV 2>/dev/null)
```

Con altavoces deberías oír un tono que cambia de 330 a 440 Hz antes de
liberarse. (Sin placa de audio, el server igual responde por OSC pero no suena.)

## 5. Suite de tests

```sh
cd clients/python
python -m pytest                  # tests/test_smoke.py, test_base.py, test_defs.py
```

Si no tenés `pytest`, cada archivo de test corre también como script
skip-aware: `python tests/test_defs.py`.

## 6. Checklist por milestone

| Milestone | Qué probar | Cómo |
|---|---|---|
| C0 | núcleo Rust + paridad con el servidor | `cargo test --workspace` |
| C1 | `_native` (builtins/rng/clock) + `render()` | sección 2 |
| C2 | builtins f32, rutinas, reloj, costura RT/NRT | sección 3 |
| C3 | `signals` + `FaustDef` + allocators + slice E2E | sección 4 (NRT y vivo) |
