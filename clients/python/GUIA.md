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

### Controles tipados y UGens nuevas (S2, offline)

El espejo cliente de S2: un `control` lleva un **tipo** (`rate="tr"` gatillo,
`rate="ir"` escalar) y/o un **lag** (`lag=`, con `lag_down=` opcional), que
`SynthDef.spec()` emite como los campos `rate`/`lag`/`lag_down`; y se suman las
UGens `lag`/`var_lag` (suavizadores), `sample_rate`/`rand` (escalares `ir`) y el
par `dseq`/`demand` (`dr`), cada output con su `rate` (fijado con
`Ugen.at_rate`). Un `rate` desconocido o un `lag_down` sin `lag` levanta
`ValueError`. Cubierto por `tests/test_synthdef.py`
(`test_control_types_and_lag_serialize`, `test_new_ugens_and_rates_serialize`,
`test_bad_control_type_and_lag_down_raise`).

```sh
.venv/bin/python -m pytest tests/test_synthdef.py -q     # 20 tests, verdes
```

El ejemplo `typed_controls.py` rinde un WAV **offline** (como `offline_render.py`,
sin servidor) con las tres formas: `freq` con `lag` (glissando/portamento),
`gate` gatillo (`tr`, re-pulsa una envolvente perc en cada `/n_set`) y `detune`
sorteado una vez con `rand` (`ir`). Un `Routine` setea `freq`/`gate` por nota con
`send_bundle` (timetagged: en un score NRT los cambios inmediatos caerían todos
en t=0). El RMS del WAV muestra 8 plucks distintos:

```sh
.venv/bin/python examples/typed_controls.py /tmp/typed.wav
# -> rendered 120000 frames (2.50 s) | peak 0.200
```

### Matemática en el grafo: op UGens (S3)

Más allá de `+ - * /`, **todo** el set unario/binario funciona sobre un grafo de
`SynthDef`: `%`, `min`/`max`, comparaciones, `.sin()`, `.midicps()`,
`.distort()`, `.clip2(x)`, … componen las UGens genéricas `BinaryOpUGen`/
`UnaryOpUGen` con un índice `op` del core. Es el mismo set que el value-side
(`clausters.base.builtins`) calcula fuera de la ruta RT — una sola implementación
`clausters-core` — así que un valor precalculado y la op en el hilo de audio dan
**bit-idéntico** para las ops nativas. Los cuatro `+ - * /` conservan sus kinds
`Add`/`Sub`/`Mul`/`Div`; se exponen además `mul_add`/`sum3`/`sum4`. Un valor de
las conversiones (`midicps`, `dbamp`, …) ahora pasa por el core (f32), no Python
f64. Cubierto por `tests/test_synthdef.py` (`test_math_operators_compose_op_ugens`)
y `tests/test_base.py` (`test_extended_ops_s3`, `test_music_helpers`).

```sh
.venv/bin/python -m pytest tests/test_synthdef.py tests/test_base.py -q   # verdes
```

El ejemplo `graph_maths.py` rinde un WAV **offline** de un lead cuya altura
(`.midicps()`), timbre (`.distort()`) y trémolo (una comparación + `.clip2()`)
son toda matemática de grafo — sin Faust:

```sh
.venv/bin/python examples/graph_maths.py /tmp/maths.wav
# -> rendered 120000 frames (2.50 s) | peak 0.120
```

### Pausar y reanudar nodos: `/n_run` + done actions completas (S4)

`Server.pause(node)` / `Server.resume(node)` / `Server.run(node, flag)` emiten
`/n_run` (aceptan un objeto nodo o un id crudo, así que sirve para un synth **o
un grupo entero**). Un nodo pausado queda en el árbol con su estado y se saltea al
procesar (silencio, sin CPU); reanuda exactamente donde iba. Eso es lo que
reanuda un synth aparcado por `DoneAction.PAUSE_SELF` — la pausa dejó de ser
terminal. `DoneAction` espeja además el enum completo **0-15** de scsynth: las
acciones relativas (`FREE_SELF_AND_NEXT`, `FREE_SELF_TO_TAIL`,
`FREE_SELF_RESUME_NEXT`, …) actúan sobre los vecinos del synth en su grupo.
Cubierto por `tests/test_defs.py` (`test_server_run_pause_resume_emit_n_run`,
`test_done_action_full_set`).

```sh
.venv/bin/python -m pytest tests/test_defs.py -q   # verdes
```

El ejemplo `pause_resume.py` rinde un WAV **offline** de un drone que se pausa un
beat y vuelve (en un score NRT los toggles van timetagged con `send_bundle`; en
vivo se usaría `srv.pause`/`srv.resume` directo):

```sh
.venv/bin/python examples/pause_resume.py /tmp/pr.wav
# -> beat RMS: 0.141 (on) 0.000 (paused) 0.141 (resumed)
```

### UGens de efecto colateral y cadena espectral (S9 + S8)

**S9 — efecto colateral sin `out`.** `send_trig(trig, id, value)`,
`send_reply(trig, *values, cmd="/reply", reply_id=-1)` y
`poll(trig, signal, label="poll", trig_id=-1)` disparan un reply OSC o un post en
consola en lugar de audio. Un `SynthDef` puede consistir **sólo** en ellos, sin
`out` — se pasan como **raíces** del def (el builder relajó la exigencia de un
`out`, C19). Cubierto por `tests/test_synthdef.py::test_side_effect_ugens_need_no_out`.

**S8 — cadena en frecuencia `fft` → `pv_*` → `ifft`.** `fft(source, active=1.0,
fft_size=1024, hop=0.5, wintype=0)` abre una cadena espectral, los filtros
`pv_mag_above`/`pv_mag_below`/`pv_brick_wall` transforman el frame, e `ifft`
resintetiza audio. Se cablean en orden. El frame es **scratch privado del synth**
en el servidor (modelo `LocalBuf`): no se aloca ningún buffer. Sólo `fft` nombra
el tamaño; el servidor lo propaga por la cadena. El tipo de ventana se cambia en
vivo con `Server.u_cmd(synth, fft_index, "window", Window.BLACKMAN)`.

Las **ventanas** se comparten con el servidor por el núcleo nativo
(`clausters._native.window(Window.HANN, n)` — misma forma que aplica el `FFT`,
paridad binaria; ABI del core subió a v4). Cubierto por
`tests/test_synthdef.py` (`test_fft_chain_serializes_with_static_fields`,
`test_fft_defaults`) y `tests/test_base.py::test_smoothing_windows_s8`.

```sh
cargo build --release -p clausters-ffi   # el core v4 (window) que carga _native
.venv/bin/python -m pytest tests/test_synthdef.py tests/test_base.py -q
```

El ejemplo `spectral.py` rinde un WAV **offline**: ruido blanco pasa-bajo en el
dominio espectral (canal derecho) junto al ruido crudo (izquierdo):

```sh
.venv/bin/python examples/spectral.py /tmp/spectral.wav
# -> rendered 96000 frames (2.00 s) | peak ~0.27
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

## 14. GUI host: controles, /gui_set en vivo y eventos (G4)

G4 agrega los controles estandar y los dos caminos que faltaban: actualizar un
widget en vivo (`/gui_set`) y recibir interacciones del usuario (`/gui_event`,
`/gui_closed`). Controles: `slider`, `knob`, `number` (rango con valor),
`button` (momentaneo), `toggle`, `menu` (un click cicla la opcion) y `text`
(muestra su valor, lo cambia el script). Los labels y valores se dibujan con una
fuente bitmap 5x7 embebida (el texto que G3 habia diferido). Interaccion: arrastrar
slider (sigue el cursor en x), knob/number (arrastre vertical), click en
toggle/menu/button; cada cambio emite `/gui_event <id> <valor>` al script que creo
la ventana; cerrar la ventana emite `/gui_closed <id>`.

Necesita display y un adaptador Vulkan/Metal/DX12/GL.

```sh
# host en modo ventana (una terminal):
cd clients/gui && cargo run --bin clausters-gui -- -v
# en otra, con el cliente importable: arma un panel, hace un /gui_set y escucha:
PYTHONPATH=clients/python python clients/python/examples/gui_panel.py
```

Esperado:

- Se abre una ventana "Filter" con dos filas: knobs (cutoff/res/gain) y
  slider/toggle/button/menu. El script imprime `set cutoff to 2000` (un
  `/gui_set` desde el script mueve el knob al instante).
- Al mover un knob/slider, hacer click en el toggle/menu o apretar el boton, el
  script imprime `event from widget <id>: [<valor>]` (el boton da `[1]` al apretar
  y `[0]` al soltar; el toggle `[0]/[1]`; el menu el indice; knob/slider el float).
- Al cerrar la ventana, imprime `window 1 closed` y el script termina.

Notas:

- El valor preserva tipo: knob/slider/number emiten float, toggle/menu/button int.
- `/gui_set` tambien se ve por `/gui_query` (modo `--headless`, seccion 12): el
  registro generico y el arbol tipado de la ventana se actualizan juntos.
- El texto de los labels es una fuente bitmap mayuscula (suficiente para paneles);
  tipografia proporcional/grande es una mejora posterior.
- **Slider vertical:** `slider(20, ..., vertical=True)` lo orienta en el eje y
  (min abajo, max arriba; arrastrar hacia arriba sube el valor). Sin la prop es
  horizontal como siempre. El **agarre** del slider es un grip corto centrado en
  la pista (no abarca todo el ancho/alto del cuerpo): rectangulo transversal de
  ~18px en el eje cruzado y 8px sobre el eje de recorrido.

## 15. GUI host: cliente del servidor de audio + meters/scope por memoria (G5)

G5 cierra la tercera pata de la topologia: el host se conecta al servidor de
audio como cliente. Dos caminos nuevos:

- `meter` y `scope` leen un **bus de control directo desde el segmento de memoria
  compartida** del servidor, cada frame, sin un solo mensaje OSC; el script solo
  escribe el bus con `/c_set`.
- un `waveform` puede referenciar un **numero de buffer del servidor**; el host lo
  trae con `/b_query` + `/b_getn` y lo dibuja.

Para el meter ambos procesos mapean el *mismo* archivo de segmento, asi que el
servidor va con `--shm <path>` y el host con el mismo `--shm <path>`; el host
ademas necesita `--server` para traer el buffer. Necesita display y un adaptador
Vulkan/Metal/DX12/GL.

```sh
# servidor de audio con segmento compartido (una terminal, desde la raiz):
cargo run -- --shm /dev/shm/clausters_g5

# host en modo ventana, atado a ese servidor y segmento (otra terminal):
cd clients/gui && cargo run --bin clausters-gui -- \
    --server 127.0.0.1:57110 --shm /dev/shm/clausters_g5 -v

# el script: carga un buffer, anima un bus, arma la escena (otra terminal):
PYTHONPATH=clients/python python clients/python/examples/gui_meters.py
```

Esperado:

- El log del host muestra `shared segment mapped at /dev/shm/clausters_g5 (1024
  control buses, ...)` y, al llegar el `/gui_def`, `opened window "Meters + server
  buffer"`.
- En la ventana, el `meter` (barra) y el `scope` (traza) se mueven siguiendo el
  bus que el script anima con una sinusoide de 0.5 Hz; ningun mensaje OSC viaja al
  host por eso (lo confirma que solo hay `/gui_def`/`/gui_free` en su log).
- El log del host muestra `buffer 0: 24000 frames loaded into 1 waveform(s)` y el
  `waveform` dibuja la onda del seno cargado en el servidor; se puede hacer
  zoom/pan con rueda/arrastre como cualquier waveform.
- Cerrar la ventana termina el script.

Notas:

- Sin `--shm` el meter/scope leen 0 (no animan); sin `--server` el waveform por
  buffer queda vacio y el host avisa por log.
- El lector de memoria es **solo lectura** y valida `MAGIC`/`ABI_VERSION` del
  segmento: si el ABI del servidor cambia, el host rechaza el mapeo en vez de leer
  basura. Es Unix-only, igual que el segmento del servidor.
- `/b_get`/`/b_getn` son lecturas estandar de scsynth que sirven a cualquier
  cliente (no solo la GUI); para buffers muy grandes el camino de transferencia
  masiva es un milestone posterior.

## 16. GUI host: bindings (`/gui_bind`), el valor saltea al script (G6)

G6 agrega el camino interactivo de baja latencia: un widget *bindeado* manda su
valor **directo al servidor de audio**, sin pasar por el script. Un knob
bindeado a `freq` de un synth manda `/n_set <node> freq <valor>` al servidor en
cada giro; uno sin bindear emite `/gui_event` al script como siempre. `bind`
cambia uno por el otro; `unbind` lo devuelve. El host necesita `--server` para
que el valor llegue al servidor de audio. Necesita display y un adaptador
Vulkan/Metal/DX12/GL.

```sh
# servidor de audio (una terminal, desde la raiz):
cargo run

# host en modo ventana, atado a ese servidor (otra terminal, desde clients/gui):
cd clients/gui && cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

# el script: arma un knob, un synth seno y bindea el knob a su freq (otra terminal):
PYTHONPATH=clients/python python clients/python/examples/gui_bind.py
```

Esperado:

- El log del host muestra `opened window "Bound knob -> synth freq"` y luego
  `/gui_bind 10 -> audio server /n_set [Int(1000), String("freq")]` (el nodo real
  varia). Notar que el id va como `Int` y el nombre como `String`: se preserva la
  distincion int/float en el prefijo del binding.
- Girar el knob cambia el tono del seno en el acto y la terminal del script **no
  imprime nada** por el knob: el valor fue del host al servidor sin volver a
  Python (lo confirma que en el log del servidor de audio aparecen `/n_set` que el
  script nunca mando).
- **El binding vive en el host, no en el script.** El ejemplo corre una demo
  corta y termina **sin desbindear ni liberar el synth**: el knob sigue
  controlando el tono despues de que el script salio (probarlo: tras el mensaje
  `script exiting...`, un `/g_queryTree 0 1` al servidor muestra que el nodo
  sigue vivo; girar el knob sigue cambiando el tono). Ese es el sentido del
  `/gui_bind`: saltea el script y le sobrevive. Para silenciar, liberar el synth
  desde otro cliente o cerrar el servidor (`/quit`).
- **Arrastrar el knob fuera de la ventana:** al presionar un knob/number el host
  bloquea el puntero (`CursorGrabMode::Locked`, oculta el cursor) y mueve el valor
  con el movimiento relativo del mouse (`DeviceEvent::MouseMotion`). Asi el
  arrastre no depende de donde esta el cursor: no se traba al pasar por la barra
  de titulo (donde Wayland se traga los `CursorMoved`) ni al salir de la ventana,
  y tiene rango ilimitado. Si el compositor no soporta lock, cae a `Confined` (el
  cursor no puede salir del area de cliente) y sigue andando por `CursorMoved`. Al
  soltar se libera el grab y reaparece el cursor. (El slider mapea la posicion
  absoluta del cursor en su body, sin ese problema.)

Notas:

- Sin `--server` el host acepta el `bind` igual pero avisa por log que no tiene a
  donde reenviar; el valor se traga (no vuelve al script como evento) - el binding
  expresa la intencion "no es del script", aunque no haya servidor.
- El reenvio solo ocurre por **interaccion del usuario** (arrastre/click): un
  `/gui_set` del script no se reenvia (ya viene del script, que puede hablarle al
  servidor por su cuenta).
- Liberar el widget (`/gui_free`, o un `/gui_def` que lo redefine afuera) tira su
  binding, asi que un id viejo no sigue reenviando.
- Cualquier direccion sirve, no solo `/n_set`: `/c_set <bus>`, `/n_setn`, etc. El
  valor del widget se agrega siempre al final del prefijo fijo del `bind`.

## 17. Transferencia masiva por recursos compartidos + DSP compartido (G7)

G7 separa dos cosas que el sistema ya pedia:

- **Los datos masivos NO van por OSC.** Un datagrama UDP corta cerca de 64 KB y
  trocear un buffer por `/b_getn` re-recorre la red para datos que ya estan en
  RAM local. Un `waveform` ahora referencia un **archivo local** que el host
  **mmapea** y lee sin copia: `path=` (f32 little-endian crudo) o `cache=` (una
  pirámide de picos prearmada, la forma mas compacta, el host no carga ni los
  samples crudos). La red (`/b_getn`) queda como fallback asíncrono.
- **El algoritmo (FFT, picos) vive una sola vez** en `clausters-core`: el
  spectrogram usa el FFT del core (`microfft`), y los picos (`peaks`) viven en el
  core y se exponen por el FFI, asi un cliente arma la **misma** cache que el host
  lee. El FFT queda listo para las futuras UGens FFT/IFFT del servidor.

**IMPORTANTE: tras G7 hay que recompilar.** El binario `clausters-gui` y el cdylib
`libclausters_ffi` cambiaron (un host viejo ignora `path`/`cache` y dibuja una
ventana vacia; el FFI subio a ABI v3). En un checkout fuente, `cargo run --bin
clausters-gui` recompila el host, y `cargo build -p clausters-ffi` el cdylib
(refrescar el bundle `clients/python/clausters/_libs/` o usar `CLAUSTERS_FFI_LIB`).

```sh
# servidor de audio (una terminal, desde la raiz):
cargo run

# host en modo ventana, atado al servidor (otra terminal, desde clients/gui):
cd clients/gui && cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

# el script: arma archivos grandes (raw + cache), exporta un buffer y los muestra:
PYTHONPATH=clients/python python clients/python/examples/gui_bulk.py
```

Esperado:

- El script imprime el tamano del archivo crudo (~2 MB para 500k samples) y de la
  cache de picos (decenas de KB), y `server exported buffer N -> ... B`.
- El log del host muestra, por cada waveform, `waveform: mapped 500000 samples
  from ... (no OSC, no re-send)` y `waveform: mapped peak cache ... (no raw data,
  no OSC)`, y `opened window`. La ventana dibuja las tres ondas; se hace zoom/pan
  con rueda/arrastre. **Ningun sample viaja por OSC.**
- Si la ventana sale vacia: casi seguro el host es viejo (no tiene el codigo de
  `path`/`cache`) — recompilar el binario y reabrir.

Notas:

- Las rutas se pasan **absolutas**: el host es otro proceso y resuelve la ruta
  desde su propio directorio.
- Para `path`, el host construye la pirámide una vez y la cachea al lado como
  `<path>.<base_bucket>.peaks`, asi reabrir un buffer grande no recalcula.
- `samples_to_file` (crudo) y `peaks_cache_file` (cache via FFI) son las dos
  formas de preparar los datos; `peaks_cache_file` usa `clausters_core_peaks_*`
  del FFI, asi la cache es byte-identica a la que arma el host.
- `/b_export bufnum path` es la version servidor: vuelca un buffer RT a un archivo
  local que el host mmapea (en vez de traerlo por `/b_getn`). Es sincrono en el
  hilo de red, no en el de audio.


## 18. GUI host: arbol de nodos en vivo + plot de un render NRT (G8)

G8 agrega dos vistas de solo lectura que ejercitan la pata "el gui es cliente del servidor de audio", ambas baratas (el painter de geometria plana + texto bitmap, sin pipeline GPU propia) y agregadas por extension (un `WidgetKind` nuevo + su renderer, sin cambio de protocolo). El servidor no se toca: G8 reusa su camino de query/notificacion que ya existia (`/g_queryTree`, `/notify`, `/n_go`/`/n_end`).

- **`nodetree`** muestra el arbol de nodos del servidor (grupos, synths, def, controles) y lo mantiene al dia: el host lo espeja por su pata cliente (`/g_queryTree`), refrescando al crearse o liberarse un nodo (`/n_go`/`/n_end`) y con un poll de baja frecuencia (200 ms) que toma los cambios de `/n_set` (que no generan notificacion). Nada en el script empuja el arbol al gui; el host lo lee del servidor. Necesita arrancar el host con `--server`.
- **`plot`** es la version liviana del `waveform` pesado: dibuja una senal una vez (una polilinea si entra en el ancho, una envolvente min/max por columna si no), sin zoom ni pan. Su senal se produce **offline** con el renderer NRT (sin servidor ni placa) y se le pasa al host como un **archivo local mmapeado** (el camino masivo de G7: los samples no viajan por OSC).

### 18a. Arbol de nodos en vivo (`gui_nodetree.py`)

```sh
# servidor de audio (una terminal, desde la raiz):
cargo run

# host en modo ventana, atado al servidor (otra terminal, desde clients/gui):
cd clients/gui && cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

# el script: crea un grupo con synths, abre una ventana con el arbol y mueve cosas:
PYTHONPATH=clients/python python clients/python/examples/gui_nodetree.py
```

Esperado:

- El log del host: `opened window "Live node tree"`. Con `-vv` (debug) ademas `node tree for group 0 updated (N top-level node(s))` cada vez que el arbol cambia.
- En la ventana, el arbol indentado muestra el grupo y sus synths. Un `freq` barre (un `/n_set` por tick) y se ve cambiar el valor del control en vivo; un tercer synth aparece y desaparece (un hijo del grupo entrando y saliendo). Todo se refleja sin que el script le mande nada al host.
- Si dice `no server` en la vista: el host se arranco sin `--server`; relanzarlo atado al servidor.

### 18b. Plot de un render NRT (`gui_plot.py`)

Solo dos procesos: el **host** y el **script** (no hace falta servidor de audio, el audio ya se renderizo offline). Necesita el cdylib embed (el renderer): `cargo build --release --features embed,realtime`.

```sh
# host en modo ventana (una terminal, desde clients/gui); SIN --server:
cd clients/gui && cargo run --bin clausters-gui -- -v

# el script: renderiza un arpegio offline, lo escribe a archivo y lo plotea:
PYTHONPATH=clients/python python clients/python/examples/gui_plot.py
```

Esperado:

- El script imprime `rendered N frames ... offline, no server` y el tamano del archivo f32.
- El log del host: `plot: mapped N samples from ... (no OSC)` y `opened window "Plot of an NRT render"`. La ventana dibuja la senal renderizada (la envolvente del arpegio). **Ningun sample viaja por OSC.**

Notas:

- Las dos vistas son de **solo lectura**: no responden a clicks (un click sobre ellas no hace nada). El scroll del arbol queda para mas adelante (hoy se corta al alto del cuerpo).
- El `nodetree` toma un `group` (default 0, el grupo raiz) y un flag `controls` (default true, muestra los pares nombre/valor de cada synth).
- El `plot` toma sus samples por `path=` (archivo f32 crudo mmapeado, el camino sin OSC; `channels=` desentrelaza el canal 0), o inline por `data=`/`blob=` para senales chicas. El rango vertical es `min`/`max` (default bipolar -1/1).
- Un host viejo (anterior a G8) ignora `nodetree`/`plot` y los deja sin pintar (los "tipos desconocidos" se maquetan pero no se dibujan): recompilar el binario.


## 19. GUI host: canvas con shader WGSL (G9)

G9 agrega un widget `canvas` que corre un **shader WGSL provisto por el script** sobre su area (estilo ShaderToy), agregado por extension (un `WidgetKind` nuevo + una vista GPU, sin cambio de protocolo, el servidor de audio no se toca). El usuario escribe una funcion `shade`; el host la envuelve con un preludio fijo (el bloque de uniforms + un vertex shader de triangulo a pantalla completa) y un `fs_main`, compila el pipeline y le pasa `u.resolution`, `u.time` y un `u.params` (vec4 de 4 floats). Los 4 params se manejan de **dos formas** -- el punto del widget: desde el **script** (`gui.set(id, param0=...)`, un valor OSC -> `u.params.x..w`) y desde un **bus de control leido de la memoria compartida cada frame** (cero OSC, el mismo camino que los meters); `buses=[..]` mapea un bus a un slot de param (`-1` lo deja script-driven). Asi un mismo shader anima desde un parametro OSC y desde audio del servidor en vivo a la vez. Un shader que no compila se atrapa (un error scope de validacion de wgpu): la canvas queda sin pintar con un warning, sin tirar el host. Una ventana con canvas se anima sola (~30fps, por el tiempo, sin depender de `--shm`).

Tres procesos cooperan, como en `gui_meters.py`: el **servidor** (con el segmento), el **host** (que lo mapea con `--shm`) y el **script**.

```sh
# servidor de audio con segmento compartido (una terminal, desde la raiz):
cargo run -- --shm /dev/shm/clausters_g9

# host en modo ventana, atado al servidor y al segmento (otra terminal, desde clients/gui):
cd clients/gui && cargo run --bin clausters-gui -- --server 127.0.0.1:57110 --shm /dev/shm/clausters_g9 -v

# el script: define la canvas, barre param0 (OSC) y escribe el bus (shm):
PYTHONPATH=clients/python python clients/python/examples/gui_canvas.py
```

Esperado:

- El log del host: `shared segment mapped at ... (1024 control buses ...)` y `opened window "Canvas (shader)"`. NO debe aparecer `canvas shader failed` (el shader del ejemplo es valido) ni panic.
- En la ventana, un shader animado: el anillo pulsa siguiendo `param0` (el valor OSC que barre el script) y el canal verde sigue el bus de control (que el host lee de la memoria compartida). Ambas fuentes mueven el mismo shader.
- Si el shader tiene un error de WGSL: el log muestra `canvas shader failed to compile: Validation Error`, la ventana igual abre (la canvas queda en negro), sin panic. Util para iterar el shader en vivo con `gui.set(id, shader=...)`.

Notas:

- El shader es el cuerpo de `fn shade(uv: vec2<f32>, frag: vec4<f32>) -> vec4<f32>`; adentro hay `u.resolution`, `u.time`, `u.params`. `uv` va 0..1 con origen arriba-izquierda.
- `param0`..`param3` -> `u.params.x`..`.w`. Se setean por `gui.set(id, param0=...)` (OSC) o por bus (`buses=[busPara0, busPara1, ...]`, `-1` = script). El bus pisa al valor del script en ese slot cada frame.
- `gui.set(id, shader=...)` recompila el shader en vivo (solo si el texto cambio); sin `--shm` los slots de bus leen 0 (el shader igual anima por `u.time` y los params de script).

## 20. GUI host: bundle standalone (GuiDef + GraphDefs), sin cliente (G10)

G10 cierra la pata "aplicacion guardada": un *bundle* es un directorio de datos que tiene un GuiDef con nombre al lado de los SynthDefs/GraphDefs que necesita, y `clausters-gui --standalone <nombre>` lo arranca como un instrumento autocontenido -- con un servidor de audio **embebido** en el propio proceso, sin servidor aparte y sin cliente de lenguaje corriendo. Es el equivalente GUI del preset MIDI-standalone del servidor (seccion 17 / M19): las definiciones guardadas alcanzan para lanzar un programa solo.

Los GuiDef persisten como persisten los defs del servidor: `host::store::GuiStore` espeja `src/server/defstore.rs` (la misma resolucion de directorio de datos, `sanitize_name`, escritura atomica) y guarda `defs/guidefs/<nombre>.json`, un registro `{id, gui}`, al lado de `defs/synthdefs`/`defs/graphdefs`. La GUI tiene su propio store chico que espeja el del servidor. Dos props hacen que un arbol guardado se maneje solo, sin script: un `boot` en la raiz (una lista de mensajes OSC que el host standalone manda apenas cargan los defs, p.ej. un `/s_new`) y un prop `bind` en el widget (la forma declarativa de `/gui_bind`). El servidor embebido se linkea directo del crate `clausters` (con `embed,realtime`) **detras de la feature `standalone`**: `clausters-gui` compilado con esa feature crea un `clausters::embed::Clausters` in-process por su API Rust (`Clausters::open`/`send`/`poll_into`). Esta off por defecto porque arrastra el engine + backend de audio; tenerlo opt-in es la razon de tamano por la que la gui es un crate aparte. (El cliente Python alcanza ese mismo server por la C ABI con ctypes; aca es un link de crate directo, no FFI.)

El ejemplo `gui_standalone.py` **escribe** el bundle (no habla con nada) e imprime el comando de lanzamiento. La distincion int/float se preserva: los ids de nodo se escriben enteros (`1000`) y siguen enteros en el cable; los valores de control son floats.

```sh
# 1) autorear el bundle: escribe el SynthDef y el GuiDef en el directorio de datos:
PYTHONPATH=clients/python python clients/python/examples/gui_standalone.py /tmp/clausters-bundle

# 2) lanzar el bundle como instrumento autocontenido (desde clients/gui, con la feature standalone):
cd clients/gui && cargo run --features standalone --bin clausters-gui -- --standalone drone --data-dir /tmp/clausters-bundle -v
```

Esperado:

- El paso 1 imprime `wrote .../defs/synthdefs/gui_standalone_drone.json` y `wrote .../defs/guidefs/drone.json`, y debajo el comando exacto del paso 2.
- El log del host (paso 2) en orden: `standalone: embedded audio server started`, `standalone: loaded 1 def(s) ...`, `standalone: sent 1 boot message(s)`, `/gui_def 1: 2 widget(s)`, `opened window "Standalone drone"`. NO debe aparecer panic.
- Se abre una ventana con una perilla; girarla cambia el `freq` del drone en el servidor **embebido** (el binding `/n_set 1000 freq` va directo, sin script de por medio). Se escucha el cambio de tono. Cerrar la ventana detiene todo.

Notas:

- El bundle es solo archivos: se puede inspeccionar (`cat .../defs/guidefs/drone.json`) y editar a mano. El registro del GuiDef es `{"id":1,"gui":<arbol>}`; el del SynthDef es el spec crudo de `/d_recv`.
- El `--data-dir` por defecto es el mismo que usa el servidor (`$CLAUSTERS_DATA_DIR`, `$XDG_DATA_HOME/clausters`, `~/.local/share/clausters`); pasarlo explicito sirve para un bundle de prueba aislado.
- Camino en vivo (sin `--standalone`): un `clausters-gui --data-dir <dir>` corriente **autopersiste** cualquier `/gui_def` cuyo arbol raiz tenga un prop `name`, y `/gui_load <name>` reinstancia uno guardado. Asi se arma el bundle interactivamente y despues se lo lanza standalone.
- Si compilas `clausters-gui` SIN la feature `standalone` y pasas `--standalone`, da un error claro: `this clausters-gui was built without standalone support; rebuild with --features standalone`. El binario por defecto (sin la feature) no compila el crate server.

## 21. GUI host: costura de plataforma (nucleo agnostico + traits, build wasm) (G11)

G11 no agrega codigo de browser: solo parte el host por la costura de plataforma, para que los hitos web siguientes sean rellenar traits y no reescribir. El host queda dividido en un **nucleo agnostico** (el arbol de widgets, el layout, el dispatch del protocolo, el dibujo de los widgets livianos) que compila para `wasm32` tal cual, y una **cascara de E/S nativa** detras de cuatro traits chicos: `Transport` (mandar OSC al servidor de audio), `BusSource` (un bus de control, leido de memoria compartida en nativo), `BulkLoader` (resolver el `path`/`cache` local de un waveform/plot a samples o a una piramide de picos) y `DefStore` (persistir GuiDefs con nombre). Lo unico nativo-y-punto es el servidor embebido (standalone); un host de browser siempre habla con un servidor de audio **aparte** por WebSocket.

No es una feature de usuario: es una refactorizacion estructural cuya prueba es que el nativo siga **identico** y que el nucleo compile para el browser. La verificacion manual (desde `clients/gui`, su propio workspace):

```sh
# 1) el nativo, identico: build + tests + clippy (todo verde, 81 tests)
cd clients/gui
cargo build && cargo test && cargo clippy --all-targets

# 2) E2E del protocolo igual que la seccion 12 (host headless + cliente, misma invocacion):
(./target/debug/clausters-gui --headless --port 57219 -v 2>/tmp/gui_host.log & \
 PID=$!; sleep 1; \
 PYTHONPATH=../python python ../python/examples/gui_skeleton.py; \
 kill $PID 2>/dev/null)

# 3) la puerta de browser: el nucleo agnostico compila para wasm32 con la cascara nativa excluida
rustup target add wasm32-unknown-unknown   # una sola vez
./check-wasm.sh
```

Esperado:

- Paso 1: build, 81 tests y clippy limpios (igual que antes de G11).
- Paso 2: el cliente imprime `widget 10 is a 'knob' ...`; el log del host muestra `/gui_def 1: 4 widget(s)` y `/gui_query 10 -> /gui_info (knob)`. Identico a la seccion 12.
- Paso 3: `Finished ... target(s)` sin errores. `check-wasm.sh` corre `cargo build --lib --target wasm32-unknown-unknown` (solo la lib, no los binarios nativos), que falla si algun hito posterior vuelve a acoplar el nucleo a E/S nativa.

Notas:

- El unico `#[cfg(not(target_arch = "wasm32"))]` que queda dentro de `host` esta sobre la cascara de E/S (los modulos `client`/`store`/`transport`/`bulk`/`gui`, la variante `ServerLink::Udp`, los metodos que devuelven el socket), nunca sobre la logica de widgets/protocolo.
- `wgpu` compila al backend WebGPU en `wasm32`, asi que los renderers (`waveform`/`spectrogram`/`canvas`) y el `Painter` ya viajan con el nucleo; lo que falta para el browser es la superficie `<canvas>` y el bring-up async del GPU (G12) y el transporte WebSocket (G13).

## 22. GUI host: primeros pixeles en el browser (`<canvas>` WebGPU) (G12)

G12 son los **primeros pixeles en el browser**: una GuiDef compilada-adentro se renderiza en una pestana sobre WebGPU, por el **mismo** codigo de render que el host nativo. Todavia sin transporte (eso es G13), asi que el arbol se arma en Rust (parseado del mismo JSON que mandaria un cliente) y los meters leen cero.

La pieza de reuso es `host::frame::render`: el render por ventana se saco **verbatim** del `gui::App::render` nativo a un modulo agnostico, asi los dos frentes (nativo y browser) dibujan por una sola funcion -> el browser es fiel al pixel por construccion, no un renderer paralelo. El nativo lo llama con sus entradas vivas (`FrameInputs`: el bus de memoria compartida, las historias de scope, los node-trees, el boton apretado); el browser con los defaults. El `Gpu` (bring-up de wgpu) se movio a `crate::gpu` (agnostico, compila al backend WebGPU). `host::web` es un `start` de `wasm-bindgen` que crea una ventana winit sobre un `<canvas>`, pide el adapter/device de WebGPU **async** (sin `block_on`, el main thread del browser nunca se bloquea) y dibuja en cada `RedrawRequested`.

Verificacion manual (necesita el target wasm32, `wasm-bindgen-cli` de la version del `Cargo.lock`, y un browser con WebGPU):

```sh
# 1) una sola vez:
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126   # la version de wasm-bindgen en Cargo.lock

# 2) generar el bundle (desde clients/gui): compila a wasm + corre wasm-bindgen a web/
cd clients/gui && ./web/build.sh

# 3) servirlo (WebGPU necesita contexto seguro; localhost vale) y abrirlo en un browser con WebGPU:
(cd web && python3 -m http.server)   # luego abrir http://localhost:8000/
```

Esperado:

- Paso 2: imprime `bundle written to .../web/ (clausters_gui.js + clausters_gui_bg.wasm)`.
- Paso 3: en una pestana con WebGPU (Chrome/Edge recientes, o Firefox Nightly) se abre el `<canvas>` con el panel: el label, un slider `cutoff`, una perilla `res`, un toggle `gate`, un boton `ping` y una waveform (un seno que decae) -- identico al panel nativo. La consola del browser loguea `clausters-gui web host starting`, `opened window over <canvas>`, `WebGPU ready; rendering the GuiDef`.

Notas:

- Sin transporte aun: la GuiDef esta compilada adentro (`host::web::demo_guidef`) y no hay eventos de vuelta. G13 trae el transporte WebSocket para manejar el host en vivo desde un cliente.
- Los `.js`/`.wasm` generados quedan git-ignored; solo `web/index.html` y `web/build.sh` se trackean.
- Headless no sirve para *ver* los pixeles (Chrome headless no lee de vuelta el canvas WebGPU a la captura), pero la consola confirma que el camino corre entero (device async arriba + `frame::render`). Para ver el render hay que abrirlo en un browser real.

## 23. GUI host: manejarlo en vivo desde el browser (binding surface + WebSocket) (G13)

G13 deja de ser estatico: el host del browser corre el **`Host` de verdad** (mismo dispatch del protocolo, mismo arbol, mismas bindings y `forward`), alimentado en vivo por una *binding surface* y forwardeando los widgets bindeados a un servidor de audio `--ws`. Reusa todo el dispatch, el formato WS de G1 y el render compartido; lo nuevo es el carrier y el pegamento de la pagina.

La logica de interaccion (hit-test + setear valor/toggle/menu) se saco verbatim a `host::interact` (agnostico): el frente nativo delega ahi y el browser llama las mismas funciones, asi una perilla girada actualiza el arbol y decide bound-vs-evento igual en las dos plataformas. La *binding surface* es un `GuiBridge` (wasm-bindgen): `def(id, json)` mete un `/gui_def` (el mismo JSON que emiten los builders de Python), `feed(packet)` mete un paquete OSC crudo, `poll()` drena los `/gui_event`/`/gui_info` salientes, y `connect_server(url)` ata la pata del servidor de audio. Esa pata es `ServerLink::Ws` (un `WebSocket` nativo del browser a un servidor `--ws`), asi un widget bindeado forwardea directo al servidor sin pasar por script -- el camino de bypass, en el browser.

Verificacion manual (necesita el target wasm32, `wasm-bindgen-cli`, y un browser con WebGPU; opcional un servidor `--ws` para probar el bind):

```sh
# 1) generar el bundle (desde clients/gui) y servirlo:
cd clients/gui && ./web/build.sh
(cd web && python3 -m http.server)   # abrir http://localhost:8000/

# 2) (opcional, para el bind) un servidor de audio con --ws en otra terminal (desde la raiz):
cargo run -- --ws &    # escucha WebSocket (57120); arranca un synth con node id 1000
# ... y abrir la pagina con el server: http://localhost:8000/?server=ws://127.0.0.1:57120
```

Esperado:

- Se abre el `<canvas>` con el panel (label, slider `cutoff`, perillas `res` y `freq`, toggle `gate`, boton `ping`, waveform). La consola del browser loguea `clausters-gui web host starting`, `opened window over <canvas>`, `/gui_def 1: window opened from the page`, `WebGPU ready`.
- Girar `cutoff`/`res`, togglear o apretar el boton loguea en consola `-> /gui_event [...]` (la perilla/slider manda el valor a la pagina); el contador de eventos en el `#note` sube. El frente nativo y el browser deciden el evento por el mismo `host::interact`.
- Con `?server=ws://127.0.0.1:57120`: la consola loguea `audio-server WebSocket open`; girar la perilla `freq` (bindeada con `["/n_set", 1000, "freq"]`) **no** emite `/gui_event` -- el valor va directo al servidor `--ws` como `/n_set 1000 freq <v>` (bypass del script), y se escucha el cambio de tono.

Notas:

- Sin `?server`, la perilla `freq` esta bindeada pero sin destino: el valor se traga (no emite `/gui_event`), igual que un bind sin `--server` en el nativo.
- El harness `web/index.html` es **descartable**, no un cliente: solo prueba el host emitiendo el mismo GuiDef JSON que los builders de Python. El cliente TypeScript de verdad es el track aparte `clients/web` (no planificado aun), no parte de G11-G16.
- Headless de Chrome confirma por consola que el camino corre entero (start -> ventana -> `/gui_def ... window opened` -> WebGPU ready) pero no captura los pixeles del canvas WebGPU; para ver e interactuar hay que abrirlo en un browser real.
- **Necesita WebGPU habilitado.** Si `requestAdapter` no encuentra adapter (Chrome en Linux suele necesitar `chrome://flags/#enable-unsafe-webgpu` + Vulkan, o un browser con WebGPU), el host **no panica**: loguea el mensaje y lo escribe en el `#note` de la pagina (`no suitable GPU adapter ...; the browser may not have WebGPU enabled ...`), el canvas queda en blanco pero la pagina sigue viva. `Gpu::new` devuelve `Result`; el nativo tambien maneja el error (warn y no abre la ventana).

## 24. GUI host: meters/scopes en el browser (buses de control por WebSocket) (G14)

G14 le da datos vivos al browser: un `meter`/`scope` (y los `buses` de un `canvas`) que en nativo leen la memoria compartida, en el browser se alimentan de un **stream de buses por WebSocket**. La contraparte del servidor es el comando nuevo **`/c_stream periodMs bus...`** (documentado en `docs/schemas.md`): una suscripcion por cliente que el servidor responde con snapshots `/c_set (bus valor)...` periodicos (piso 10 ms, max 128 buses) -- disenado una sola vez para dos consumidores: este host y el futuro cliente TS (W4).

En el host, la logica compartida se movio a `host::live` (historia de scopes, coleccion de buses vivos, `StreamedBuses` -- el `BusSource` del browser); el frente web abrio la pata de **entrada** del WebSocket (`onmessage` -> `decode_packet`, antes era send-only), deriva la suscripcion del arbol (se re-suscribe solo cuando el conjunto de buses cambia) y corre un tick de animacion de 33 ms por `setInterval` (en wasm no hay `Instant`). El dibujo de meters/scopes y el muestreo por tick se reusan sin cambios.

Verificacion manual (bundle + servidor `--ws`; el demo es autocontenido -- la perilla `bus 10` esta bindeada a `/c_set 10`, asi que el lazo es perilla -> servidor -> stream -> meter):

```sh
# 1) bundle y servidor (dos terminales, o el paso 3 headless):
cd clients/gui && ./web/build.sh
(cd web && python3 -m http.server)      # http://localhost:8000/
cargo run -- --ws                       # desde la raiz (WebSocket en 57120)

# 2) en el browser: abrir http://localhost:8000/, "connect" y despues "meters".
#    Girar la perilla `bus 10`: el meter y el scope siguen el valor, fluido (~30 fps).
#    Tambien se puede animar el bus desde afuera:
PYTHONPATH=clients/python python3 -c "
from clausters.defs import Server; import math, time
s = Server()
for i in range(300): s.set_bus(10, math.sin(i/10)); time.sleep(0.03)
s.close()"

# 3) (headless, evidencia por consola) el pase scripteado de paridad:
#    ver la seccion 26 -- parity.html loguea '/c_stream subscription: [10]' y
#    'bus stream flowing: [Int(10), Float(...)]'.
```

Esperado:

- La consola del browser loguea `/c_stream subscription: [10]` al abrir el demo y `bus stream flowing: [...]` con el primer snapshot; el meter sube/baja y el scope scrollea con el bus, igual que la seccion 15 (G5) por memoria compartida.
- Cerrar la ventana (o abrir un demo sin widgets vivos) cancela la suscripcion (`/c_stream 0`): el servidor deja de mandar snapshots a ese cliente.
- El wrapper Python del comando es `Server.stream_buses(period_ms, *buses)` (test unitario en `tests/test_defs.py`); el E2E del comando en el servidor esta en `tests/osc.rs` (`c_stream_*`, 3 tests).

Notas:

- La suscripcion es **por cliente** y muere con la conexion WS/TCP (el servidor poda streams y `/notify` al desconectar); por UDP/ring dura hasta la cancelacion explicita o `/quit` (misma postura que `/notify` en scsynth).
- El nativo no cambia: con `--shm` sigue leyendo la memoria compartida con cero mensajes; el stream es el fallback de red para quien no puede mapear el segmento.

## 25. GUI host: bulk data en el browser (fetch + /b_getn por WebSocket) (G15)

G15 cierra el camino de datos masivos: en el browser un `waveform`/`plot` con `path`/`cache` se resuelve como **URL contra el origen de la pagina** (`fetch` -> `ArrayBuffer`), y una referencia `buffer` a un buffer del servidor se baja por **`/b_query` + `/b_getn` en chunks** sobre la misma conexion WebSocket. La piramide de picos de un fetch crudo se construye **en wasm** (el analisis vive en `clausters-core`, sin FFI); un `cache` fetcheado se mapea directo a la piramide sin cargar samples.

La maquina de estados del fetch de buffers (que ya existia en el frente nativo) se extrajo a `host::fetch` (`BufferFetches`): pura, compartida por los dos frentes y testeada sin GPU ni socket (secuencia de chunks, de-interleave multicanal, buffer vacio, ventana cerrada a mitad de bajada).

Verificacion manual:

```sh
# 1) generar los archivos bulk que sirve la pagina (junto a index.html):
PYTHONPATH=clients/python python3 -c "
import math
from clausters.gui.guidef import samples_to_file, peaks_cache_file
n = 48000
s = [0.8*math.sin(2*math.pi*220*i/48000)*(1-i/n) for i in range(n)]
samples_to_file(s, 'clients/gui/web/sine.f32')
peaks_cache_file(s, 'clients/gui/web/sine.peaks', base_bucket=256)"

# 2) servidor --ws con el buffer 0 lleno (un seno via /b_gen):
cargo run -- --ws &
PYTHONPATH=clients/python python3 -c "
from clausters.defs import Server
s = Server(); b = s.alloc_buffer(48000, 1)
s.send_msg('/b_gen', b.bufnum, 'sine1', 5, 1.0); s.sync(); s.close()"

# 3) bundle + pagina: abrir http://localhost:8000/, "connect" y "bulk".
cd clients/gui && ./web/build.sh && (cd web && python3 -m http.server)
```

Esperado:

- Los tres waveforms renderizan: el cache de picos (sin datos crudos), el f32 crudo (piramide construida en wasm) y el buffer del servidor (bajado en chunks de 8192 por WS). La consola loguea `waveform: fetched peak cache sine.peaks (48000 samples, no raw data)`, `waveform: fetched 48000 samples from sine.f32 (pyramid built in wasm)` y `buffer 0: 48000 frames loaded into 1 waveform(s)`.
- La navegacion (zoom/pan) tiene la misma calidad que en nativo: nunca se resuelve mas fino que la pantalla (misma `Pyramid`, mismos regimenes).

Notas:

- Semantica de `path`/`cache`: en nativo son rutas de archivo mmapeadas; en el browser son URLs relativas al origen de la pagina. El builder de Python pasa el string tal cual.
- `web/sine.f32`/`web/sine.peaks` son generados y estan git-ignored (como el `.js`/`.wasm` del bundle).
- No hay `/b_export` en el browser (no puede mapear archivos): el camino del buffer es siempre `/b_getn` -- exactamente el "async fallback" que la decision de bulk de G7 reservo para este cliente.

## 26. GUI host: empaquetado wasm y pase de paridad nativo/browser (G16)

G16 empaqueta el host wasm y prueba que el reuso se sostuvo. El empaquetado queda en `web/build.sh` (cargo wasm + `wasm-bindgen` CLI, sin toolchain nuevo); `web/index.html` es el harness documentado (conectar + demos panel/meters/bulk, los mismos GuiDefs que emiten los builders de Python) y `web/parity.html` es el **pase de paridad scripteado**: se conecta solo y abre los tres demos en secuencia, con la evidencia en la consola. El quick-start publico esta en `docs/clients.md` ("The GUI host in the browser"), cross-linkeado con el libro Python (`examples.md`).

```sh
# 1) preparar todo (bundle + archivos bulk + servidor con buffer 0): pasos 1-3 de las
#    secciones 24 y 25 (servidor --ws corriendo, http.server sirviendo web/).

# 2) paridad NATIVA (referencia): los tres ejemplos contra el mismo servidor
#    (host nativo con --server y --shm, ver secciones 15 y 17):
cd clients/gui && cargo run --bin clausters-gui -- --server 127.0.0.1:57110 --shm /dev/shm/clausters_g5 &
PYTHONPATH=clients/python python3 clients/python/examples/gui_panel.py
PYTHONPATH=clients/python python3 clients/python/examples/gui_meters.py
PYTHONPATH=clients/python python3 clients/python/examples/gui_bulk.py

# 3) paridad BROWSER: abrir http://localhost:8000/parity.html?server=ws://127.0.0.1:57120
#    (o headless, evidencia completa por consola):
google-chrome --headless=new --disable-gpu --enable-unsafe-swiftshader --no-sandbox \
  --enable-logging=stderr 'http://127.0.0.1:8000/parity.html?server=ws://127.0.0.1:57120' \
  2>&1 | grep -E 'window opened|WebSocket open|c_stream|stream flowing|fetched|buffer 0|pass complete'
```

Esperado (el paso 3 headless imprime exactamente esta evidencia, una linea por camino):

- `/gui_def 1: window opened from the page` (x3: panel, meters, bulk) -- el mismo GuiDef JSON abre el mismo arbol en los dos frentes (mismo `GuiNode::parse`/`Widget::from_node`).
- `audio-server WebSocket open` + `/c_stream subscription: [10]` + `bus stream flowing: [Int(10), Float(...)]` -- meters por el cable (G14).
- `waveform: fetched peak cache ...` + `waveform: fetched 48000 samples ...` + `buffer 0: 48000 frames loaded into 1 waveform(s)` -- los tres caminos bulk (G15).
- `parity: pass complete`.

Notas:

- El mismo arbol y el mismo comportamiento por construccion: parse, layout, render (`frame::render`), interaccion (`interact`) y la maquina de fetch (`host::fetch`) son codigo compartido; las diferencias son solo la cascara de E/S (shm vs `/c_stream`, mmap vs `fetch`, `/b_export` vs `/b_getn`).
- El camino embed/standalone es explicitamente nativo-only y queda fuera; el cliente TypeScript de producto es el track aparte `clients/web` (W0-W5), que consume el mismo `/c_stream` (nota en su PLAN, W4).
- Chrome headless corre todo el camino con SwiftShader (WebGL2); para *ver* los pixeles hay que abrirlo en un browser real -- el render es fiel al pixel por construccion (una sola funcion de render para los dos frentes).

## 27. Auditoria del seam Python -> core (C21, pre-pista W)

El pase de auditoria que pide la build strategy de `clients/PLAN.md` antes de
encarar el cliente TypeScript: toda logica de **valor/tiempo** que quedaba en
Python bajo al core nativo (ABI del core v4 -> v5). Que bajo: la cola del
`TempoClock` (el `Scheduler` del core, ids planos <-> rutinas), el modelo de
minimos cuadrados del sample clock (`clausters_clocksync_*`), el RNG de los
patrones con seed (`Pwhite`/`Prand` sobre `clausters_rng_*` -- misma secuencia
en cualquier lenguaje cliente, nunca mas el Mersenne Twister de Python), el
empaquetado de timetags NTP (arreglo real: Python truncaba la fraccion, el
core redondea), el redondeo segundos->sample del camino `/sched`, el snap de
`quant` y degree->midinote. Excepcion documentada (en `docs/clients.md`): el
codec de bytes OSC queda por lenguaje (argumentos estructurados no cruzan la
C ABI plana).

Verificacion manual:

```sh
cargo test -p clausters-core -p clausters-ffi     # 46 + 8 verdes (nuevas superficies incluidas)
cd clients/python && .venv/bin/python -m pytest tests/ -q   # 129 passed, 4 skipped
# E2E vivo (misma invocacion): servidor + Pbind con degree + lock_to (/sched)
(./target/debug/clausters & PID=$!; sleep 1.5; \
 clients/python/.venv/bin/python clients/python/examples/live_udp.py; kill $PID)
```

Notas:

- Los tests del cliente son de comportamiento (timing yield-exacto, golden
  byte-identico, snapping de quant): pasan sin cambios, o sea el descenso no
  altero la semantica observable. Lo unico que cambia con seed fijo son los
  *valores* de `Pwhite`/`Prand` (otro generador, a proposito).
- En un checkout fuente, si `clausters/_libs/` tiene una cdylib staged vieja
  (build previo de la wheel), tiene precedencia sobre `target/`: refrescarla
  (`cp target/debug/libclausters_ffi.so clients/python/clausters/_libs/`) o
  borrarla si el ABI no coincide.
- `clients/gui/check-wasm.sh` pasa (los modulos nuevos del core compilan para
  wasm32). Ojo: correrlo desde `clients/gui/` -- desde la raiz `cargo build
  --lib` resuelve al crate raiz (el servidor) y falla con `getrandom`, que
  parece una rotura del gate pero no lo es; el script ahora hace `cd` a su
  propio directorio.

## 28. Contexto aleatorio unico (seguimiento de C21)

Correccion de diseño sobre C21 a pedido del usuario: los patrones aleatorios
**no llevan seed propio** (se elimino el parametro `seed` de `Pwhite`/`Prand`).
Todo lo aleatorio sale de **un solo contexto sembrable**, el modelo de sclang:
`main.seed(n)` siembra la raiz; cada `Routine` deriva su generador propio del
contexto que la crea (`RngStream.spawn`, seed hijo = proxima palabra del
padre); cada draw usa el generador de la rutina en curso (`main.current_tt`,
thread-local) con fallback a la raiz. Con una sola semilla el script entero es
reproducible de punta a punta, y rutinas concurrentes (varios clocks, RT junto
a NRT) son reproducibles por rutina sin importar el interleaving. Se exponen
ademas `clausters.next_f64()` / `uniform(lo, hi)` / `next_below(n)` /
`choice(items)` sobre el mismo contexto. ABI del core v5 -> v6 (se agrego
`clausters_rng_next_u64` para la derivacion).

Verificacion manual:

```sh
cargo test -p clausters-core -p clausters-ffi        # 46 + 8 verdes
cd clients/python && .venv/bin/python -m pytest tests/ -q   # 132 passed, 4 skipped
# (test_seq: replay bajo main.seed, funciones expuestas, reproducibilidad
#  por rutina con orden de scheduling invertido)
```

Notas:

- Cambio incompatible a proposito: `Pwhite(..., seed=n)` ahora es TypeError;
  ejemplos y docs migrados a `main.seed(n)` antes de tocar.
- Para aislar material, tocarlo en su propia rutina: obtiene su stream
  derivado por construccion (ese es el override local correcto).

## 29. API Python de boxes: las librerías de Faust como boxes (C22)

`clausters.defs.boxes` — la contrapartida box de `signals`, una API completa
por derecho propio: el álgebra point-free de Faust (`seq`/`par`/`split`/
`merge`/`rec`, `wire`/`cut`) como callables en minúscula que emiten el JSON
del esquema del server, con operadores sobre `Box` y aridades calculadas en
el cliente (`num_inputs`/`num_outputs`). La adición que abre las librerías:
`box.faust(src, *eval_args, defs=, ins=, outs=)` compila cualquier expresión
Faust en un `Box` componible — `fi.lowpass`, `os.osc`, `re.`, `pm.` se usan
sin transcribirlas. Las dos etapas de aplicación quedan separadas en la
sintaxis: los argumentos de `faust()` se splicean en el texto (etapa de
evaluación: orden del filtro, listas de coeficientes); los del `__call__`
se cablean como boxes (etapa de composición, `seq(par(args), box)`).
Selección de canal con `st[k]` / `.outs()` (fragmentos declaran `outs=`).
Regla del wire: cada `wire()` es una entrada distinta; reusar el mismo
objeto en dos posiciones lo rechaza `from_box` con error claro (lint por
identidad). El reuso por valor de cualquier otra expresión comparte el
cómputo — garantizado server-side por la memoización de fragmentos
`CDSPToBoxes` por `src` (suite CSE en `tests/faust_box.rs`).

De yapa, dos fixes del server que salieron de los tests de este hito: la
memoización de fragmentos (sin ella, cada llamada a `CDSPToBoxes` genera
símbolos de recursión frescos y los duplicados no comparten — 2^10 copias
inflaban el bitcode 27x) y `normal_precision` (el renderer NRT compilaba
libfaust con FTZ/DAZ armado y el álgebra de intervalos abortaba el proceso
con `fi.lowpass` vía composición; el server vivo no lo veía porque compila
en otro thread).

Verificación manual:

```sh
# Rust: suite CSE + paridad mixta + regresión FTZ (single-threaded, regla libfaust)
cargo test --features faust --test faust_box --test denormals -- --test-threads=1   # 7 + 4 verdes
# Python: unit del módulo (JSON del esquema, splicing, aridad, lint, __call__/outs)
cd clients/python && ../../.venv/bin/python -m pytest tests/test_boxes.py -q        # 14 verdes
../../.venv/bin/python -m pytest tests/ -q                                          # 147 passed, 3 skipped
# Ejemplo offline (cdylib con embed,realtime,faust): osc -> lowpass -> freeverb estéreo
../../.venv/bin/python examples/boxes_library.py /tmp/boxes.wav   # 3.00 s, peak ~0.69
# E2E vivo (misma invocación): def de boxes -> /d_faust -> /s_new -> /n_set
(cd ../.. && ./target/debug/clausters & PID=$!; sleep 1.5; \
 cd ../.. && PYTHONPATH=clients/python .venv/bin/python -c "
from clausters.defs import FaustDef, Server; from clausters.defs import boxes as box
f = box.hslider('freq', 110.0, 20.0, 2000.0, 0.1)
saw = box.faust('os.sawtooth', ins=1, outs=1)(f) * 0.2
fx = box.faust('fi.lowpass', 3, ins=2, outs=1)(box.hslider('cutoff', 500.0, 50.0, 8000.0, 1.0), saw)
srv = Server(); srv.add_faustdef(FaustDef.from_box('box_e2e', fx))
n = srv.synth('box_e2e', {'cutoff': 300.0}); srv.set(n, {'cutoff': 2000.0}); srv.sync(); srv.free(n)
print('E2E OK')"; kill $PID)
```

Notas:

- El ejemplo NRT necesita la cdylib con features (`cargo build --release
  --features embed,realtime,faust`) y, en checkout fuente, refrescar
  `clausters/_libs/` (ver nota del §27) — **también `libclausters.so`**, no
  solo la ffi: una copia staged sin `faust` da "server built without faust
  support" en el render.
- Si un render NRT aborta el proceso Python con un assert de
  `intervalPow.cpp`, es la regresión de FTZ (fixeada): compilar libfaust en
  un thread con flush-to-zero armado. `tests/denormals.rs` la cubre.
- El pitch del módulo, acordado: boxes no es "la forma de usar librerías"
  ni una capa sobre signals — es la otra mitad del par (procesadores
  componibles vs. una salida por vez); las librerías son la adición.

## 30. Osciloscopio audio-rate sobre taps del servidor (G18)

G18 agrega los **taps de audio**: anillos de muestras pre-asignados dentro del
segmento de memoria compartida (ABI v2 → v3; `--taps` × `--tap-frames`, default
8 × 16384). `/tap tap bus` (o `Server.tap(tap, bus)`) rutea un bus de audio al
anillo; desde ahi el widget `scope` en su forma audio-rate (prop `tap`) dibuja
un **osciloscopio con trigger**: cada frame lee la ventana mas nueva del anillo
directo de memoria compartida (cero OSC por frame) y la alinea al ultimo cruce
ascendente del nivel de trigger, con histeresis; sin cruce corre libre.

### Tests unitarios y E2E automaticos

```sh
cargo test --quiet                       # ipc: anillos (write/wrap/read/refusals) + tamano v3 pineado
                                         # osc: /tap + /tap_stream contra audio vivo; /fail sin segmento
                                         # rt_safety: el write del tap no aloca en el thread de audio
cd clients/gui && cargo test --quiet     # lector v3 del host + trigger/ventana (oscil.rs); 93 verdes
cd clients/python && python -m pytest -q # ServerOptions/ServerInfo con taps; 147 verdes
```

### E2E headless: capturar un tap por `/tap_stream` desde Python

Sin GUI ni display — el camino del browser y de la captura headless (una sola
invocacion, regla del sandbox):

```sh
(./target/debug/clausters -q & PID=$!; sleep 1.5; \
 PYTHONPATH=clients/python python - << 'PY'
import struct, threading
from clausters import Session
from clausters.base import OscReceiver
from clausters.defs import SynthDef, control, out, sin_osc
from clausters.responders import OscFunc

got = threading.Event()
def on_data(msg, t, src):
    n = len(msg[3]) // 4
    peak = max(abs(s) for s in struct.unpack(f"<{n}f", msg[3]))
    print(f"/tap_data tap={msg[1]} end={msg[2]} window={n} peak={peak:.3f}")
    got.set()

with Session.live() as session:
    server = session.server
    print(server.query_info())           # taps=8 tap_frames=16384
    server.add_synthdef(SynthDef("tone", out(0.0, sin_osc(220.0) * 0.5)))
    synth = server.synth("tone"); server.tap(0, 0); server.sync()
    recv = OscReceiver().start()         # suscribir desde el socket receptor
    OscFunc(on_data, "/tap_data", recv=recv)
    recv.send(server.target.addr(), "/tap_stream", 20, 1024, 0)
    assert got.wait(3.0)
    server.tap(0, -1); server.free(synth)
PY
kill $PID 2>/dev/null)
```

Esperado: `query_info` reporta `taps=8 tap_frames=16384` y llegan `/tap_data`
con `peak≈0.5` (el seno vivo, no silencio).

### Manual con ventana: el osciloscopio con trigger (`gui_scope.py`)

```sh
# servidor con segmento (una terminal, desde la raiz):
cargo run -- --shm /dev/shm/clausters_tap

# host en ventana sobre el mismo segmento (otra terminal):
cd clients/gui && cargo run --bin clausters-gui -- --shm /dev/shm/clausters_tap -v

# el script: un seno en el bus 0, ruteado al tap 0, dos scopes (otra terminal):
PYTHONPATH=clients/python python clients/python/examples/gui_scope.py
```

Esperado:

- Dos trazas del mismo tap: la de arriba (**triggered**, nivel 0.0) queda
  clavada en su lugar mientras el pitch barre 220–440 Hz — cada redibujado se
  alinea a un cruce ascendente por cero; la de abajo (**free-running**, nivel
  9.0 que la senal nunca cruza) deriva por la ventana. Esa es exactamente la
  diferencia que el trigger existe para resolver.
- Cero trafico OSC por frame: el host lee el anillo del segmento; en el log del
  host solo hay `/gui_def`/`/gui_free`.
- `scope(bus=...)` sin `tap` sigue siendo el historial de bus de control (§15);
  es el mismo widget con dos rates.

Notas:

- El servidor sin `--shm` pero con taps arma un segmento en memoria: `/tap` y
  `/tap_stream` funcionan igual (asi corre el E2E headless de arriba); solo el
  host mapeado necesita el archivo.
- `--taps 0` quita la region: `/tap` y `/tap_stream` responden `/fail` y
  `query_info` reporta `taps=0`.
- En el browser el mismo widget se suscribe solo con `/tap_stream` (el wasm no
  puede mapear el segmento) y recibe `/tap_data`; la logica de trigger es la
  misma (`host/oscil.rs`, compartida).

### Manual con ventana: phasescope + spectrum (`gui_analyzer.py`)

Los otros dos consumidores del tap: el goniometro (phasescope, un par estereo de
taps) y el espectro en vivo (spectrum, una FFT por frame de un tap).

```sh
# servidor con segmento (una terminal, desde la raiz):
cargo run -- --shm /dev/shm/clausters_tap

# host en ventana sobre el mismo segmento (otra terminal):
cd clients/gui && cargo run --bin clausters-gui -- --shm /dev/shm/clausters_tap -v

# el script: fuente estereo cuyo image barre mono->wide->antifase (otra terminal):
PYTHONPATH=clients/python python clients/python/examples/gui_analyzer.py
```

Esperado:

- **Phasescope** (taps 0/1): el goniometro colapsa a una **linea vertical** en
  mono (r = +1), se abre a un **lozenge** cuando el canal derecho se decorrelaciona
  (r ~ 0) y cae a una **linea horizontal** en antifase (r = -1); la barra de
  correlacion abajo sigue el swing +1 -> 0 -> -1, y el script narra `mono` /
  `wide` / `anti-phase` en la consola al cruzar cada landmark.
- **Spectrum** (tap 0, eje log): el seno del canal izquierdo se ve como un pico
  unico y estable, que se desplaza por el eje logaritmico cuando el pitch deriva;
  el peak-hold deja una traza que decae despacio y el promediado evita el
  parpadeo cuadro a cuadro.
- Cero trafico OSC por frame: como el osciloscopio, el host lee los anillos del
  segmento; el log del host solo muestra `/gui_def`.

Notas:

- La correlacion (Pearson r) y la geometria Lissajous del goniometro viven en
  `clausters-core::measure` (con export FFI): estan tambien como funciones
  `clausters.gui.correlation` / `clausters.gui.lissajous` para analisis headless
  del estereo, y el browser las computa en wasm igual que el host nativo. La FFT
  del spectrum es la misma `clausters_core::fft` que usa el espectrograma, asi
  que las dos vistas coinciden bin a bin.
- `spectrum(fft_size=...)` acepta potencias de dos 256..4096 (default 2048); un
  valor no soportado cae a 2048 en vez de fallar. `phasescope(tap, tap2=...)`
  toma el par estereo, con `tap2` por defecto `tap + 1`.
- Rapido, sin ventana ni display, la correlacion contra taps capturados por
  `/tap_stream`:

```python
from clausters import Session
from clausters.gui import correlation
with Session.live() as s:
    srv = s.server
    srv.tap(0, 0); srv.tap(1, 1)
    l = srv.stream_taps(20, 1024, 0, count=1, timeout=1.0)  # ver §gui_scope
    # correlacion(canalL, canalR) -> +1 mono, 0 decorrelado, -1 antifase
    print(correlation(list(l), list(l)))  # +1 con el mismo canal
```

## 31. Waveform y espectrograma de nivel editor (G20)

G20 profundiza las dos vistas pesadas hasta calidad de editor de audio y cablea
por fin el widget `spectrogram` al host (existia solo como binario demo):
**multicanal** (todos los canales de un `path`/`buffer`/`cache`, lanes apiladas
o trazas superpuestas con `overlay=True`), **zoom sin saltos** (crossfade entre
niveles de la piramide de picos), **reglas** (eje de tiempo 1-2-5 en
`h:mm:ss.mmm` o samples; eje de Hz en el espectrograma, log o lineal, calcado
del mapeo del shader), **seleccion** arrastrable (`/gui_event id "selection"
start len`, tambien seteable por `/gui_set`), **playhead** que sigue el reloj
de samples del engine (cero mensajes por frame via el segmento; `/clock` por
tick en el browser) y **readout** de cursor. La cache de picos multicanal es
**un solo archivo** (CLPK v2, un canal tras otro; las v1 mono siguen andando),
construible desde Python con `peaks_cache_file(samples, path, channels=N)`
(FFI, CORE_ABI v8).

### Tests automaticos

```sh
cargo test -p clausters-core peaks       # MultiPyramid: build/round-trip/v1/size
cargo test -p clausters-ffi              # peaks_multi_build byte-identico
cd clients/gui && cargo test --quiet     # 119 verdes: crossfade, reglas, lanes,
                                         # parse/apply de ambos widgets, fetch multicanal
```

### Manual con ventana: el editor (`gui_editor.py`)

```sh
# servidor con segmento (una terminal, desde la raiz):
cargo run -- --shm /dev/shm/clausters_editor

# host en ventana, con leg al servidor y el mismo segmento (otra terminal):
cd clients/gui && cargo run --bin clausters-gui -- \
  --server 127.0.0.1:57110 --shm /dev/shm/clausters_editor -v

# el script (otra terminal):
PYTHONPATH=clients/python python clients/python/examples/gui_editor.py
```

Esperado:

- La ventana muestra un **waveform de dos lanes** (frase estereo renderizada
  offline, mapeada de un archivo f32 intercalado — cero OSC para las muestras)
  sobre un **espectrograma de dos lanes** del mismo archivo, cada uno con su
  regla de tiempo abajo y el espectrograma con la regla de Hz (log) a la
  izquierda.
- **Rueda** = zoom hacia el cursor (sin "pop" al cruzar niveles de detalle),
  **Shift+arrastrar** = pan, **arrastrar** = seleccion (la banda translucida
  sigue el cursor y el script imprime los eventos `selection` en samples y
  segundos), **r** = reset de vista.
- La **linea naranja (playhead)** recorre ambas vistas sincronizada con lo que
  suena (un PlayBuf en loop; cada pasada se re-ancla con `/clock`). Sin
  `--shm` en host/servidor el playhead simplemente no se dibuja.
- El **readout** en la esquina inferior derecha sigue al cursor: tiempo +
  amplitud en el waveform, tiempo + Hz en el espectrograma.
- `gui.set(id, db_floor=..., log_freq=0, colormap=1)` recontrasta/reescala el
  espectrograma en vivo sin recomputar nada (uniforms del shader);
  `window_size`/`hop` son de definicion (recomputar = redefinir el def).
