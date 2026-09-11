# El sistema de nodos del multipista — especificación para revisar

*Archivo temporal, en español, para trabajar sobre él. No es documentación
publicada y no vive en ningún libro. Cuando lo acordemos, su contenido se
reparte entre `PLAN.md` (servidor: GraphDef anidado y el buffer cosido),
`clients/gui/PLAN.md` (`G35.3`/`G35.4`/`G35.14`), `crates/clausters-document/PLAN.md`
(los nombres) y `docs/schemas.md` / `docs/gui-protocol.md`, y este archivo se
borra.*

---

## 0. Qué decide esta especificación

Una sola cosa, dicha de cuatro maneras: **cuáles son los nodos y grupos que
lleva una pieza, y a qué nombres responden**, para que los dos extremos se
junten — que una automatización cuyo `target` dice `gain` maneje *el mismo*
`gain` que muestra la perilla del header, y no dos palabras que se escriben
igual.

De ahí salen, en orden, cinco preguntas que hoy no tienen respuesta:

1. **La forma**: un `GraphDef` que se componga de otros `GraphDef`, y que sea
   dinámico en la cantidad de hijos (una pista tiene *n* clips, no un número
   fijo).
2. **Los nombres**: el espacio de nombres de la superficie, y cómo un `target`
   opaco del documento se resuelve contra él.
3. **La actuación**: cómo una curva llega a ser un control — sin mensajes por
   bloque, y sobreviviendo a un locate.
4. **Los canales**: cuántos tiene una pista, qué le pasa a un clip mono en una
   pista estéreo, y de dónde saca sus valores un vúmetro de *n* canales.
5. **El lector**: cómo suena una caja que es un *join* de fragmentos de varios
   buffers, sin instrucciones de control en tiempo de reproducción.

Lo que **no** decide, y **no descarta**: qué es un efecto, el ruteo libre entre
pistas, los sends y el streaming de disco. Los cuatro están en §10, cada uno con
qué haría falta para tomarlo y con la razón por la cual lo de acá no hay que
reescribirlo cuando llegue.

---

## 1. El dominio: qué hacen las DAW y qué tomamos

Todas las DAW de canal fijo (Pro Tools, Logic, Cubase, Ableton, Reaper) tienen
la **misma cadena de canal**, y las diferencias están en los bordes:

```
entrada → [inserts pre-fader] → fader (+ mute) → pan → [inserts post-fader]
        → sends (pre o post fader, por send) → salida (bus destino)
```

Los cuatro acuerdos del campo que vale la pena copiar tal cual:

- **`clip gain` y `track gain` son dos etapas distintas y las dos existen.** El
  primero corrige el material (una toma que entró baja), el segundo mezcla. Es
  literalmente el *gain staging* clásico, y es por eso que la envolvente de un
  clip no es "otro nombre" del fader de la pista.
- **Un send es pre o post fader, y esa elección es del send.** Un send post
  sigue al fader (reverb que baja cuando bajás la pista); uno pre no (un envío a
  auriculares).
- **Mute y solo no son ganancia.** Solo es una regla del *mixer* — quién queda
  en silencio cuando alguien está soleado —, no un estado de la pista; el
  documento ya lo dice así (`Track::soloed` guarda que *esta* está marcada, y
  nada más). El servidor recibe el resultado, no la regla.
- **Los canales son del canal, no del proyecto.** Reaper es el caso extremo (una
  pista tiene *n* canales arbitrarios); Pro Tools el conservador (mono/estéreo/
  multicanal declarado). Tomamos el conservador: una pista declara su ancho, y
  el upmix/downmix en cada borde es una regla escrita.

**Lo que esta especificación no toma todavía, con la puerta abierta**: el
**ruteo libre de matriz** — que la salida de una pista sea cualquier bus, que
una pista sea la entrada de otra (buses de grupo, *folders*, sidechains). No se
descarta ni se juzga: lo que decide acá es que la cadena de canal se especifica
primero, porque el ruteo se apoya sobre ella y no al revés. La forma de §2 (un
bus de salida `external` que provee quien instancia) **ya lo admite**: mandar
una pista a otro destino es cambiar qué bus se le pasa, no rediseñar nada.
Queda en §10.1 con lo que haría falta para tomarlo.

---

## 2. La forma: `GraphDef` anidado y con hijos dinámicos

### 2.1 Qué hay hoy

`src/osc/graphdef.rs` ya tiene casi todo lo que hace falta y le faltan dos
cosas. Lo que tiene:

- Un `GraphDefSpec` es **plano**: `buses` privados por instancia, `members`
  (cada uno una instancia de un `SynthDef`/`FaustDef`), y una `surface` que es
  `puerto → [(miembro, control, mul, add)]`. La actuación externa va **siempre**
  contra la superficie, nunca contra los ids privados.
- `/graph_new` lo expande en primitivas (un grupo, `/synth_new`s, `/node_map`),
  así que el hilo de audio nunca aprende la palabra "GraphDef".
- `voice: true` ya es un miembro **repetible**: `/graph_newVoice` lo instancia
  otra vez dentro de la misma instancia, cableado a los mismos buses privados.

Lo que le falta:

1. **`member.def` sólo resuelve a un `SynthDef`/`FaustDef`.** No se puede anidar.
2. **La repetición sólo tiene una forma, la voz**, y no tiene nombre: no se
   puede decir "este grafo tiene un slot llamado `clips` y otro llamado `fx`".

### 2.2 La propuesta mínima

**(a) Un miembro puede ser otro GraphDef.** `GraphMember` gana un campo:

```rust
pub enum MemberKind { Def, Graph }   // serde: "def" (default) | "graph"
```

Con `kind: "graph"`, instanciar el miembro es una llamada recursiva a la misma
expansión: un subgrupo dentro del grupo de la instancia, con **sus propios**
buses privados. El hilo de audio sigue sin enterarse: son grupos y sintes.

**(b) El hijo declara qué buses le pone el padre.** Hoy un bus privado se
asigna en la instanciación. Un hijo anidado necesita recibir el bus de salida
del padre en vez de inventarse uno, así que un `GraphBus` gana:

```rust
pub external: bool,   // este bus no se asigna: lo provee quien me instancia
```

y el miembro que es un grafo lo llena en `controls` con la misma
`ControlValue::Bus("...")` que ya existe — se resuelve contra los buses *del
padre*. Es la misma regla de encapsulamiento que ya rige, un nivel más arriba.

**(c) La superficie del hijo se re-exporta con prefijo.** `SurfaceTarget` gana
la forma "puerto de un miembro-grafo":

```rust
pub enum SurfaceTarget {
    Control { member: usize, control: String, mul: f32, add: f32 },  // hoy
    Port    { member: usize, port: String,    mul: f32, add: f32 },  // nuevo
}
```

Así `mt.track` puede exportar `fx/1/cutoff` apuntando al puerto `cutoff` de su
hijo. La resolución (`ResolvedSurface`) sigue aplanándose a
`(node_id, control_index, mul, add)` en la instanciación, así que un
`/node_set` contra la superficie sigue costando lo mismo: **el anidamiento es
de autoría, no de tiempo de ejecución.**

**(d) Los slots: la repetición con nombre.** `voice: bool` se generaliza a
`slot: Option<String>` (y `voice: true` pasa a ser `slot: "voice"`, mantenido
por compatibilidad). Aparecen dos comandos, que son `/graph_newVoice`
renombrado y generalizado:

```
/graph_addSlot instanceID slotName id [port value ...]   -> id del subgrupo
/graph_removeSlot id                                     (= /node_free)
```

Un slot es *exactamente* lo que un clip es en una pista y lo que un efecto es
en una cadena: algo de lo que hay una cantidad que cambia mientras suena.

**Por qué así y no con un `GraphDef` generado por pieza.** Se podría emitir un
`GraphDefSpec` completo por pieza y reenviarlo en cada edición. Se descarta:
un `def_send` por movimiento de clip es una recompilación por gesto, y pierde
la identidad del nodo (el clip que estaba sonando se corta). Con slots, mover
un clip es un `/node_set` y agregarlo es un `/graph_addSlot`.

### 2.3 Validación que hay que agregar

- **Ciclos**: un grafo no puede contener, transitivamente, a sí mismo. Chequeo
  en profundidad al instanciar, con un tope (`--max-graph-depth`, defecto 8).
- Un puerto no puede mezclar objetivos compartidos y de slot (ya está escrito
  para `voice`, se generaliza).
- El presupuesto de buses privados es ahora recursivo: la reserva
  (`GRAPH_AUDIO_BUS_RESERVED`) se agota más rápido y el error tiene que decir
  qué instancia lo agotó.

---

## 3. Los tres GraphDefs genéricos

Los nombres son `mt.*` (multitrack). Cada uno se instancia una vez por cosa del
documento, y **es el mismo def cada vez**: los nombres quedan definidos acá y
no en cada pieza.

### 3.1 `mt.clip` — un clip

```
group  <clip>
  bus  out      (audio, external)      <- lo provee la pista
  group control/automation
    curve       (por cada Automation del clip -> bus de control privado)
  group source
    reader      slot "source"          <- uno por segmento fuente (§7)
  group fx
    slot "fx"   (kind: graph, mt.fx)
  member gain/pan/width                <- la etapa de salida del clip
```

Superficie: `gain`, `pan`, `width`, `mute`, `start`, `span`, `at`, `fade/in`,
`fade/out`, `fx/<n>/<param>`.

`at`/`span`/`start` son los que ya escribe `playback.py`: dónde empieza la caja
en la línea de tiempo, cuánto dura y desde qué frame de la fuente lee. Siguen
siendo controles, no un nodo nuevo — con la excepción de `buf` y `loop`, que
son `ir` y por eso obligan a un nodo nuevo (y que la §7 elimina como problema).

### 3.2 `mt.track` — una pista

```
group  <track>
  bus  mix      (audio, privado, `channels` canales)
  bus  out      (audio, external)      <- lo provee el master o el bus destino
  group control/automation
    curve        (por cada Automation de la pista)
  group source
    slot "clips" (kind: graph, mt.clip; out -> mix)
  group fx
    slot "fx"    (kind: graph, mt.fx;  in/out -> mix)   [pre-fader]
  member fader   (gain + mute, mix -> post)
  member panner  (pan + width, `channels` -> `channels`)
  group post
    slot "post"  (kind: graph, mt.fx)                   [post-fader]
  member sends   slot "send" (gain + tap pre|post -> bus destino)
  member meter   (tap de `channels` canales, §6)
```

Superficie: `gain`, `pan`, `width`, `mute`, `send/<name>/gain`,
`fx/<n>/<param>`, `post/<n>/<param>`. `mute` es un **control del fader**, no un
estado del grupo — el porqué está en §3.5.

**El orden importa y es el del campo**: inserts pre-fader, fader, pan, inserts
post-fader, sends. El `sort` automático del grupo por conexiones de bus lo
respeta porque los buses lo declaran; no hay que ordenarlo a mano.

### 3.3 `mt.fx` — el hueco declarado, y nada más

**Trabajo a futuro, anotado en §10.2 con la puerta abierta.** Acá queda
únicamente la *forma* del hueco, para que `mt.track` no haya que reescribirla
cuando llegue: un `GraphDef` con un bus `in` y uno `out` externos, una
superficie que re-exporta los controles de lo que tenga adentro, y `bypass` y
`wet`.

Y una observación que hace toda la diferencia y por la cual el hueco tiene esta
forma y no otra: **un efecto puede ser, él mismo, un `GraphDef`** — con lo cual
"cadena de efectos" y "grafo anidado" son la misma cosa y no dos, y un efecto
compuesto (tres nodos y un bus interno) no necesita ningún mecanismo que §2 no
dé ya. Lo que falta para tomarlo no es la forma: es qué es un efecto como
*documento* (preset, latencia declarada y su compensación, orden y estado del
bypass).

### 3.4 `mt.master`

Una `mt.track` sin `source`: sus entradas son los buses de las pistas. Se
instancia una vez por pieza, sale a `0..channels` del hardware, y es donde vive
el vúmetro maestro.

### 3.5 Mute, y por qué no es `run 0` sobre el grupo

`/node_run 0` sobre el grupo de la pista es, sin discusión, **lo más barato**:
el grupo entero se saltea y no cuesta un ciclo. Y sin embargo el mute no puede
*ser* eso, por tres razones, en orden de peso:

1. **Un mute se automatiza.** Una lane de mute es una curva escalonada como
   cualquier otra, y §5 dice que una curva llega a su destino por un **bus de
   control**. `run` es un mensaje contra un nodo, no un control: no hay bus que
   lo maneje, no hay `transport_pos` que lo resuelva, y un locate al medio
   volvería a dejarlo donde no va. Un mute automatizado sobre `run` es la opción
   (a) de §5, que ya rechazamos.
2. **Un corte seco es un click.** Apagar el grupo entre dos muestras corta la
   señal donde esté. Un mute es una rampa corta (5–10 ms), que es lo que hace
   toda DAW, y una rampa la hace el fader.
3. **Corta lo que no debería.** Se lleva puesta la cola de un efecto (una
   reverb muteada tiene que apagarse, no desaparecer), y también los **sends
   pre-fader**, que por definición no siguen al fader.

Entonces: **`mute` es un control del fader**, multiplicando con lag — y así es
un puerto igual a todos, automatizable por el mismo camino, y el solo de §1
(que es una regla del mixer, no un estado) se resuelve escribiendo ese mismo
puerto en las pistas que corresponda.

**Y `run 0` sigue existiendo, como otra cosa.** Es una optimización de CPU que
la aplicación aplica cuando una pista *ya* está en silencio y no tiene nada que
seguir sonando — sin efectos con cola, sin sends pre-fader, y pasada la rampa.
Son dos preguntas distintas y conviene no confundirlas: **`mute` es la regla
audible; `run` es la regla de costo.** La segunda se puede tomar después sin
tocar la primera, y se mide antes de tomarla.

---

## 4. Los nombres, y cómo se resuelve un `target`

### 4.1 El espacio de nombres

Un **puerto** es una ruta con `/`:

| puerto | qué es | rango |
|---|---|---|
| `gain` | ganancia lineal | `0..` (la UI muestra dB) |
| `pan` | posición | `-1..1` |
| `width` | ancho estéreo | `0..2` |
| `mute` | silenciado | `0`/`1` |
| `send/<name>/gain` | nivel de un envío | `0..` |
| `fx/<n>/<param>` | parámetro del efecto `n` | del efecto |
| `start`, `span`, `at` | ventana de un clip (sólo `mt.clip`) | frames/beats |
| `fade/in`, `fade/out` | bordes de un clip | segundos |

**La regla que cierra la brecha**: la perilla del header, la curva de
automatización y el `/node_set` del cliente escriben **el mismo puerto de la
misma instancia**. No hay un `gain` del dibujo y otro del sonido.

### 4.2 El `target` del documento

`Automation::target` es un `Opaque` — "en los términos del cliente y nunca
leído acá", que es lo correcto y hay que dejarlo así. Lo que se fija es **qué
escribe ahí el editor multipista**:

```json
{ "port": "gain" }
```

y nada más, para el caso normal. Un target que no resuelve contra la superficie
de la instancia es una curva **deshabilitada con motivo**, no un error que tira
la pieza abajo: se dibuja gris y el editor dice por qué (el efecto que la
nombraba ya no está).

**La pertenencia ya está bien y no se toca**: una `Automation` de un `Track`
resuelve contra la superficie de *esa* instancia de `mt.track`; una de un
`Region`/clip, contra la de *ese* `mt.clip`. Es el mismo anidamiento en el
documento y en el grafo, lo cual es exactamente el argumento para que el grafo
sea anidado.

### 4.3 Dónde vive la resolución

En Rust, una sola vez: una función del crate del documento (o de
`clausters-core`) que dado un `Multitrack` devuelve el **plan de instancias** —
qué grafos instanciar, con qué slots, y qué puerto es cada curva. Los dos
clientes la llaman y ninguno la reimplementa. Es el mismo lugar donde ya vive
`multitrack::picture`, y por la misma razón.

---

## 5. La actuación: una curva es un control, no una ráfaga de mensajes

Tres opciones, y la elegida:

- **(a) `/node_set` por bloque desde el cliente.** Rechazada: es el driver
  escrito a mano que acabamos de sacar del ejemplo, no sobrevive a un locate
  sin resincronizar, y su resolución es la del socket.
- **(b) Un `EnvGen` disparado al empezar.** Rechazada: un locate al medio de la
  pieza deja la envolvente en el lugar equivocado, y hay que reprogramarla en
  cada locate. Es (a) con menos mensajes.
- **(c) Un lector de tabla indexado por la posición del transporte.** Elegida.
  La curva se escribe en un buffer (una tabla muestreada al ritmo del bloque, o
  a una resolución declarada), y un nodo la lee con `transport_pos` — **lo mismo
  que ya hace el lector de una caja**, y por lo tanto probado. Su salida va a un
  bus de control privado, y el puerto se cablea con `/node_map`.

Consecuencias, que son las buenas:

- Un locate no requiere ningún mensaje: la curva está donde la posición diga.
- La curva sigue al transporte incluso en NRT, sin ninguna rama distinta.
- Editar una curva es reescribir un tramo del buffer (`/buffer_setRange`), que
  es una escritura en el lugar donde el lector ya está mirando — la semántica de
  buffer que el servidor ya declara.
- La resolución es un parámetro declarado, no una consecuencia de la red.

La aritmética beat→frame de la tabla vive en `clausters-core`, no en los
clientes (es exactamente la duplicación que dejamos anotada al cerrar el
playback).

---

## 6. Canales, mezcla y vúmetro

### 6.1 Cuántos canales tiene una pista

Una pista **declara** su ancho, con defecto **2**; el master declara el suyo,
con defecto el del hardware.

**Decidido: es un campo propio del documento, no un `Opaque`.** El ancho de una
pista hace a la estructura de la pieza — es configurable, se guarda, y reabrir
el proyecto tiene que devolver la misma mezcla. Un `Opaque` sirve para lo que un
cliente interpreta y otro no; esto lo tienen que escribir igual los dos, que es
exactamente la definición de un campo propio. Así que `Track` gana
`channels: usize` (serde `default = 2`, omitido cuando lo es, para que ningún
archivo escrito hasta hoy cambie), y `Multitrack` gana el suyo para el master.

### 6.2 Un clip mono en una pista estéreo

La regla, escrita una vez en `mt.clip` y no en cada pieza:

- **Mono → estéreo: se centra**, es decir la misma señal a los dos canales con
  la ley de paneo aplicada (`pan` a 0 da −3 dB en cada lado, no 0 dB en cada
  lado). Elegir un canal sería sorprendente y no es lo que hace ninguna DAW.
- **Estéreo → estéreo**: directo, y `pan` es un *balance* (atenúa un lado), no
  un paneo. Es la distinción clásica y confundirla es el bug clásico.
- **N → M con N > M**: downmix por suma con el coeficiente de la tabla estándar
  (ITU-R BS.775 para 5.1→estéreo); no es urgente pero el hueco queda nombrado.
- **N → M con N < M** y ninguno de los casos de arriba: los primeros N canales,
  el resto en silencio, y el editor lo dice.

La tabla de coeficientes va en `clausters-core` (`mixdown`), una sola vez.

### 6.3 El vúmetro

Un `meter` por pista, de `channels` canales, y uno del master. Necesita las dos
mitades:

- **El grafo**: un miembro `meter` en `mt.track` que escribe en **buses de
  control**, uno por canal (decidido: el pico/RMS ya es un escalar por bloque, y
  el *audio tap* está pensado para forma de onda, que es mover miles de muestras
  para mostrar una). Son buses privados de la instancia, consecutivos, así que
  el elemento lee un rango y no *n* buses sueltos — un `/bus_getRange` o un
  `/bus_stream` por pista, no uno por canal.

  **La eficiencia es el requisito, no un deseo**: un vúmetro por pista es lo que
  más seguido se actualiza en toda la aplicación. El nodo escribe **un valor por
  bloque por canal** y nada más; el que decide cada cuánto mirar es el host, a
  la tasa de cuadro y no a la del bloque.
- **El elemento GUI**: `clients/gui/src/host/elements/meter.rs` ya existe para
  un canal. Lo que falta es la tira vertical de *n* canales dentro del header de
  la pista (`G35.14`), leyendo *n* buses consecutivos.

Se mide **post-fader por defecto** (es lo que espera la mano), con la opción
pre-fader declarada por pista.

**La balística, que hoy no está hecha y hace falta.** Un vúmetro que dibuja el
pico crudo de cada bloque es ilegible: parpadea. Las tres reglas del campo, y
las tres van en `clausters-core` (una vez, para los dos clientes y el host —
son aritmética, no dibujo):

- **Ataque instantáneo**: el pico sube en el mismo bloque en que ocurre. Nunca
  se suaviza hacia arriba: para eso está el vúmetro.
- **Caída con constante declarada**: baja a una tasa fija en dB/s (el valor del
  campo son ~20 dB/s para el pico, y hay que poder cambiarlo). Es lo que hace
  que un transitorio se vea.
- **Retención del pico máximo**: una marca que se queda quieta un tiempo
  declarado (~1–2 s) y después cae, más el máximo absoluto de la pasada, que se
  retiene hasta que la mano lo borre — que es lo que se mira para saber si algo
  clippeó mientras uno no estaba mirando.

Lo que el nodo escribe en el bus es el valor **ya con balística aplicada**, para
que dos clientes distintos no dibujen dos caídas distintas de la misma señal, y
para que el host no tenga que muestrear más seguido que su propio cuadro. La
marca de retención es un segundo bus por canal.

---

## 7. El lector: de "un lector por caja" a "una caja es un buffer cosido"

### 7.1 El problema, dicho con precisión

Hoy `playback.py`/`playback.ts` tienen **un lector residente por caja**, y el
lector es un `buf_rd` sobre **un** buffer con un `start` y un `span`. Eso
alcanza para una caja que es una ventana sobre un archivo, incluso copiada — un
lector nuevo lee el rango que la caja dice, y está bien.

Falla en el caso que el documento **ya sabe expresar** y el servidor no sabe
reproducir: `Body::Segments`, una lista de `SegmentRef`. Es decir, una caja que
resulta de un *join* de fragmentos de archivos distintos, o del mismo archivo en
otro orden. Ahí el lector necesitaría cambiar de buffer y de posición **con
precisión de muestra**, y `bufnum` es un control `ir`: cambiarlo es un nodo
nuevo. Un nodo nuevo por costura es (a) una instrucción de control en tiempo de
reproducción, que es justamente lo que no queremos, y (b) imposible de ubicar
con precisión de muestra desde el cliente.

`Segment`/`Segments` es una buena abstracción justamente porque **no dice de qué
archivo son las muestras**. Lo que falta es la contraparte del servidor que
tampoco lo diga.

### 7.2 La idea a analizar: un pseudo-buffer de segmentos

Un buffer cuyo contenido no son muestras propias sino **una lista de tramos de
otros buffers**. Para todo lector es un buffer normal: tiene `frames`,
`channels`, `sample_rate`, y responde `sample(frame, channel)`. Adentro, ese
`sample` resuelve a qué tramo pertenece el frame y lee del buffer de origen.

**Veredicto: es viable, y es la respuesta correcta.** El análisis contra el
código real:

**Lo que lo hace viable.**

- `Storage` **ya es un enum** (`Owned` / `Shared`), o sea que la forma de una
  tercera variante ya está prevista: `Storage::Stitched { parts: Arc<[Part]> }`.
- Los UGens leen por `Buffer::sample(frame, channel)` y `Buffer::at(index)`, que
  son métodos, no acceso directo al slice. Ahí entra el despacho.
- Un `Part` es `{ src: Arc<Buffer>, src_start: usize, frames: usize }`. Los
  `Arc` se clonan en el hilo de red al construir el buffer cosido; el hilo de
  audio sólo los desreferencia. **No hay allocación, ni lock, ni I/O en el hilo
  de audio** — que es la única condición no negociable.
- El buffer cosido participa del mismo ciclo de vida que cualquier otro: se
  libera por la FIFO de basura, y los `Arc` a los orígenes se sueltan en el hilo
  de red. Un origen liberado mientras está cosido **sigue vivo** porque el
  cosido lo tiene tomado, que es justo lo que hace falta.
- No hay ninguna instrucción de control en reproducción: un locate es un cambio
  de posición y nada más. Un join deja de ser un caso especial del reproductor y
  pasa a ser un caso especial de la *construcción del buffer*, que es trabajo
  del hilo de red.

**Lo que hay que resolver, y no es gratis.**

1. **`cells()` no puede existir.** Devuelve `&[AtomicU32]`, un slice contiguo, y
   un cosido no tiene uno. Todo lo que hoy llama `cells()` — `to_vec`, `len`,
   `set_at`, la región compartida IPC, `/buffer_getRange`, la pirámide de la
   forma de onda — hay que revisarlo uno por uno. **Decidido: un buffer cosido es de
   sólo lectura y no expone celdas.** `cells()` deja de ser público en favor de
   `sample`/`at`, y las escrituras (`/buffer_set*`, `RecordBuf`, `BufWr`)
   **fallan con un mensaje claro** sobre un cosido. Un cosido es una vista, y
   una vista no se graba.

   Y eso no le quita nada, porque **un cosido se reemplaza por otro**, que es
   lo que el servidor ya hace con los buffers: `/buffer_alloc`, `/buffer_read` y
   `/buffer_gen` no escriben adentro, instalan uno nuevo entero
   (`docs/schemas.md` lo dice con esas palabras). Recoser es un comando, cuesta
   la lista de tramos y no las muestras, y el lector que estaba andando sigue
   andando. **Editar un join es recoserlo.**
2. **El costo por muestra — medido: 0,010% de un bloque por lector.** Una
   búsqueda del tramo por frame, con un **cursor** (el último tramo usado,
   verificado antes de buscar, y el siguiente después) que hace que la lectura
   hacia adelante sea una comparación y que sólo un salto real pague la binaria.

   El número, con el mismo método con que se midió el costo de los atómicos
   (bloques de 64 frames, lectura interpolada): buffer plano **212 ns**, cosido
   **345 ns**, o sea **+63%** — contra una aceptación del 10% que había escrito
   *antes* de medir. **Esa aceptación era la medida equivocada**: un
   microbenchmark que no hace más que la lectura convierte tres operaciones de
   más en un porcentaje enorme. Contra el presupuesto de un bloque los mismos
   números son 0,016% y 0,026%, o sea que un cosido cuesta **una centésima de
   porciento de un bloque más que un buffer plano, por lector** — que es
   exactamente el encuadre que usan los docs de `dsp::buffer` para los atómicos.

   De paso: reestructurar `Buffer::sample` para despachar sobre el
   almacenamiento una sola vez (en vez de pasar por `cells()` y un `Option`)
   bajó el plano de 379 a 212 ns y el cosido de 565 a 345 — los dos mejoraron y
   la razón empeoró, que es otra manera de decir lo mismo.

   **Y la medición a nivel de motor, que es la que decide algo** (`cargo test
   --release --test stitch_load -- --ignored --nocapture`: N lectores `PlayBuf`
   en un motor, cronometrando `process_block`). Con **128 lectores simultáneos**
   — 128 cajas sonando en el mismo instante, más de lo que una pieza suele tener
   — un bloque cuesta **5,0% de su presupuesto con buffers planos y 7,5% con
   cosidos**. La razón se sostiene en +50–65% en todos los conteos de lectores;
   el absoluto queda chico porque leer un buffer nunca fue en lo que un motor
   gasta el tiempo. Cuántos tramos tiene un cosido casi no importa (el cursor
   hace su trabajo: 8 tramos y 256 miden lo mismo), y el crossfade es como un
   quinto de la diferencia — el resto es la búsqueda por muestra, que son dos
   saltos de puntero más que una lectura indexada.

   **Las dos respuestas, en orden.** Primero, y gratis: **coser sólo lo que de
   verdad es un join**. Una caja que es una ventana sobre una toma es un buffer
   plano y cuesta exactamente lo que cuesta hoy; el cosido se arma cuando la
   mano corta uno, y ahí paga por una capacidad que está usando. Es una regla
   del paso de compilación del cliente (fase 4), no un cambio en el servidor.
   Segundo, **pospuesto a propósito y documentado donde va**: resolver el tramo
   una vez por corrida en vez de por muestra. Un lector avanza monótonamente y
   un bloque de 64 frames casi siempre cae dentro de un tramo, así que un
   `Buffer::run_at(frame)` que conteste la corrida contigua dejaría que el
   lector la sostenga y lea adentro sin ninguna búsqueda.

   **No es un cambio grande** — la razón de posponerlo no es el tamaño sino que
   todavía no hay con qué medir si sirve: haría falta una pieza con cien cajas
   sonando. Dónde va, para no volver a derivarlo: `src/dsp/buf.rs` (`read_lin` y
   sus dos llamadores, `PlayBuf` y `BufRd`), con el camino por muestra como
   respaldo para el bloque que cruza una costura y para una velocidad modulada o
   invertida; `src/dsp/stitch.rs` gana la consulta al lado de `Stitch::sample`
   (y la corrida de un buffer plano es su largo entero, así que el camino rápido
   es uniforme y no una rama sólo para cosidos); y `tests/stitch_load.rs` es el
   antes y después, con la aceptación de que un cosido quede a distancia de
   ruido de un buffer plano con 128 lectores. Los punteros están en el código de
   los dos lados, porque una nota que vive sólo en un plan es una nota que quien
   edita la función nunca ve.
3. **La interpolación en la costura.** `BufRd` interpolado lee los frames
   vecinos: en el borde de un tramo, el vecino está **en el tramo de al lado**,
   no fuera del buffer. Si se lee cero, cada costura hace un click. Hay que
   declararlo: la lectura interpolada cruza tramos, y por eso `sample()` es el
   único punto de acceso (resuelve cada frame por su cuenta).
4. **El crossfade de la costura.** Dos tramos que no continúan producen un salto
   de la señal, que es un click aunque la interpolación esté bien. Un `Part`
   gana `fade_in`/`fade_out` en frames, aplicado por el propio `sample()` — es
   el *crossfade de edición* que toda DAW pone por defecto en unos pocos ms.
5. **Los canales.** Los tramos pueden venir de buffers con distinta cantidad de
   canales. El cosido **declara** su ancho y cada `Part` trae un mapeo de canal
   (`Vec<usize>`, del canal del cosido al del origen), con las reglas de §6.2
   como defecto.
6. **Las tasas de muestreo.** Un tramo de un buffer a 44.1k dentro de un cosido
   a 48k es un resample, que **no** va acá. Decisión: los tramos tienen que
   compartir el `sample_rate` del cosido; si no, el comando falla y el cliente
   resamplea antes. El resample al importar ya es una necesidad conocida.
7. **Anidamiento.** ¿Un cosido puede tener de origen otro cosido? Sí, y sale
   gratis (`sample()` es recursivo), pero hay que ponerle un tope y chequear
   ciclos igual que en §2.3.

**El comando.** Uno solo, asíncrono como el resto de la familia:

```
/buffer_stitch bufnum channels sampleRate [srcBufnum srcStart frames fadeIn fadeOut chanMap...]...
```

El grupo de cada tramo es de **ancho fijo** (`5 + channels`) justamente porque
el mapa es variádico: con una cola de largo desconocido no hay manera de
distinguir una entrada del mapa del índice de buffer del tramo siguiente.

**Lo que no se pudo hacer, y por qué**: que `/buffer_query` traiga una bandera.
Su respuesta es un **grupo repetido de 4** por buffer, así que agregar un quinto
campo no es el apéndice inofensivo que sí admite `/server_query.reply` — todo
parser que la corta de a cuatro leería la bandera como el índice del buffer
siguiente. Por ahora un cliente se entera de que un buffer es un cosido cuando
lo rechazan al escribirlo, que es honesto pero tarde. La salida es una respuesta
propia (`/buffer_parts bufnum` → los tramos, que un editor de joins quiere de
todos modos), anotada en `PLAN.md`.

**El disco: no es urgente, pero es importante — anotado en §10.4.** Un `DiskIn`
transmitiendo sobre una lista de segmentos es el mismo problema una capa más
abajo: el que lee del disco tendría que cambiar de archivo con precisión de
muestra. Y la respuesta es la misma forma aplicada al *buffering* en vez de al
almacenamiento: **el buffer circular que `DiskIn` ya llena pasa a ser un cosido
de segmentos**, y el hilo que lo llena, que lee por adelantado, sabe cuál es el
próximo cambio de archivo **antes** de llegar a él. Esa es la propiedad que lo
hace andar: la costura se resuelve en el hilo de disco, con toda la anticipación
del mundo, y el hilo de audio sigue leyendo un buffer y nada más.

Lo que hay que medir acá es distinto de lo de arriba, y hay que medirlo igual:
**cuánta anticipación hace falta** para que una costura no llegue tarde —
cuántos tramos cortos seguidos (un *comping* picado a golpes de un cuarto de
segundo, cada golpe en otro archivo) aguanta el hilo de disco antes de quedarse
sin adelanto, y qué hace cuando se queda (silencio declarado, nunca un click).

**Y lo que sí resuelve, de yapa**: `buf` deja de ser un control `ir` que obliga
a un nodo nuevo. Una caja es siempre un cosido (de un tramo o de veinte), y
cambiar lo que suena es cambiar el buffer cosido, no el nodo.

### 7.3 La contraparte del cliente

El cliente compila `Vec<SegmentRef>` → las partes del comando. Esa compilación
es **una** y va en Rust (el crate del documento sabe qué es un `SegmentRef`; la
tabla de sesión sabe qué buffer es cada `SourceId`). Los clientes la llaman.

---

## 8. Fases de implementación

En este orden, porque cada una se puede probar sola:

1. ✅ **`Storage::Stitched` y `/buffer_stitch`** (servidor) — *hecho
   2026-09-10*. `src/dsp/stitch.rs`, la variante de `Storage`, el comando, once
   tests (`tests/buffer_stitch.rs`, los unitarios del cursor y la costura, y el
   de no-allocación en `tests/rt_safety.rs`) y la medición de arriba. Dos cosas
   cambiaron en el camino, las dos a propósito: `Buffer::cells` ahora devuelve
   un `Option` — un cosido no tiene celdas, y eso obligó a los cinco lugares que
   necesitan un tramo contiguo a decirlo en vez de leer un slice vacío en
   silencio — y todo comando que escribe adentro rechaza un cosido por nombre.
2. ✅ **GraphDef anidado y slots** (servidor) — *hecho 2026-09-10*.
   `kind: "graph"`, `external`, un `SurfaceTarget` con `port`, `/graph_addSlot`
   (del cual `/graph_newVoice` pasa a ser la escritura del slot `"voice"`), y el
   tope de profundidad que además es lo que rechaza un grafo que se contiene a
   sí mismo. Seis tests nuevos en `tests/graphdef.rs`, `docs/schemas.md`, y la
   misma superficie en los dos clientes (`add_slot`/`addSlot`, y un handle de
   miembro que contesta un control o un puerto según lo que el miembro sea).

   **Lo que obligó, y es la forma mejor igual**: instanciar es un árbol, y un
   árbol no se construye como se construía un nivel — un hijo que falla a mitad
   deja a sus hermanos parados. Así que `/graph_new` ahora **planifica y después
   realiza**: toda la caminata fallable ocurre primero y guarda con qué devolver
   todo si algo falla; realizar emite comandos y no puede fallar. La regla de
   todo-o-nada que tenía un nivel es la misma regla sobre un árbol.
3. ✅ **Los defs y el plan de instancias en Rust** — *hecho 2026-09-10*.
   `clausters_core::mixer` (los defs: `mt.reader`, `mt.strip.<i>x<o>`,
   `mt.clip.<i>x<o>`, `mt.track.<n>`, `mt.piece.<n>`, y el vocabulario de
   puertos) y `clausters_document::multitrack::nodes` (el plan: qué instanciar,
   con qué buffer, en qué frame, con qué nivel). `tests/mixer_graph.rs` los
   **escucha** en vez de leerlos: la pieza entera es un `/graph_new` y todo lo
   demás un slot, una toma mono se panea en vez de copiarse, una caja suena
   donde el transporte dice y en ningún otro lado, y un puerto en cualquier
   nivel llega al control que nombra.

   **Tres cosas que aparecieron haciéndolo, y las tres cambiaron el diseño**:
   (a) el master tiene que **contener** las pistas — el bus de mezcla del master
   es privado de su instancia y una pista instanciada aparte no podría nombrarlo
   nunca, así que la pieza es `mt.piece` con un slot `tracks`; (b) un `GraphDef`
   no podía decir **qué canal** de un bus recibe un miembro, y sin eso un mixer
   no se puede escribir: ahora `"mix:1"` (y `"OUT:1"`); (c) `Balance2` aplica la
   ley de paneo a un par estéreo, o sea que atenúa 3 dB en el centro — tres
   strips en serie le sacarían 9 dB a una pieza por nada, así que el balance se
   escribe acá como `min(1, 1 ∓ pan)`, unidad en el centro.
4. ✅ **`playback` reescrito sobre eso**, en los dos clientes — *hecho
   2026-09-10*. Dejó de armar nodos: ahora pide el plan (`multitrack_plan`),
   manda los defs que falten (`mixer_defs`) y **compara** lo planificado contra
   lo que ya suena, mandando la diferencia. Los dos clientes corren las mismas
   dos llamadas, que es por qué una pieza suena igual en los dos.

   Es un diff y no una reconstrucción porque los nodos tienen que **quedarse**:
   rehacer el árbol en cada edición reiniciaría todo lo que está sonando, y una
   mano arrastrando una caja escucharía su propio gesto como un tartamudeo. Lo
   único que no se puede decir con un `set` —un clip cuya fuente cambió de
   **ancho**, que es otro cableado— se rehace.

   Tres cosas aparecieron: (a) `BufRd` lee el bufnum una vez por bloque, así que
   `buf` no tiene por qué ser `ir` — cambiar de buffer es un `set`, que es
   exactamente lo que un cosido recosido necesita; (b) el fader de la pista se
   lee de `config["level"]`, que es donde el lector de filas del host ya lo
   escribe, y merece ser un campo propio al lado de `channels` (anotado); (c)
   **la unidad de `SegmentRef::start` estaba documentada al revés** — decía
   frames y todo lo que la lee (la picture, el host, los dos clientes) dice
   segundos.

   Lo que **no** cierra todavía: `G35.3`/`G35.4`. Los puertos existen y la
   perilla escribe el mismo que escribiría la curva, pero nada mapea todavía una
   `Automation` a un puerto ni la compila a la tabla que `transport_pos` lee
   (§5). Esa es la fase que falta para que la curva suene.
5. ✅ **La curva suena** — *hecho 2026-09-10*, y es lo que cierra
   `G35.3`/`G35.4`. `mt.curve` lee una tabla en `TransportPos` y escribe un bus
   de control; `/graph_map instanceID port bus` mapea el puerto a ese bus, que
   es la otra mitad de `/node_set` contra una superficie y lo único que permite
   que una curva maneje un control tres niveles abajo sin que nadie aprenda el
   nodo que hay detrás. La tabla la muestrea el crate, sobre el eje de frames,
   así que un cambio de tempo la dobla como dobla todo lo demás.

   **Desmapear no es opcional**: un puerto que quedó en un bus que nadie escribe
   se queda con lo último que hubo ahí, así que una curva borrada seguiría
   manejando el control con el último valor que dijo.

6. **Los canales y el vúmetro** (§6): el ancho de pista como campo del documento
   (hecho en la fase 3), las reglas de mezcla y la balística del vúmetro en
   `clausters-core`, el `meter` de *n* canales en el header (`G35.14`). El
   vúmetro necesita un UGen que todavía no existe: ataque instantáneo, caída
   declarada en dB/s y retención de pico.
7. **`mt.fx` de verdad**, cuando exista una especificación de efecto.

---

## 9. Lo que quedó decidido

Las cinco preguntas de la primera pasada, contestadas (2026-09-10), y dónde
quedó cada una escrita:

1. **El ancho de pista se guarda en el documento**, como campo propio y no como
   `Opaque`: es configurable y hace a la estructura de la pieza. — §6.1
2. **El vúmetro va por buses de control**, con la eficiencia como requisito, y
   con la balística (ataque instantáneo, caída declarada, retención del pico)
   en `clausters-core` — que hoy no está hecha. — §6.3
3. **El cosido es de sólo lectura**, y se reemplaza por otro, igual que un
   buffer. — §7.2
4. **Los sends** quedan anotados y no se toman ahora. — §10.3
5. **`mt.*`** como prefijo de los defs: los nombres tienen que ser lo menos
   ambiguos posible, y `track` o `clip` a secas en un espacio global de defs son
   lo más ambiguo que hay. — §3

Y una pregunta que apareció en la revisión, contestada en §3.5: **el mute no es
`run 0`**. Es un control del fader, porque se automatiza, porque un corte seco
es un click y porque `run 0` se lleva puesta la cola de un efecto y los sends
pre-fader. `run 0` queda como optimización de costo, que es otra pregunta.

---

## 10. Trabajo anotado, con la puerta abierta

Cuatro cosas que esta especificación **no** toma y que no está descartando. Cada
una dice qué haría falta para tomarla, y ninguna obliga a reescribir lo de
arriba — que es la prueba de que la puerta quedó abierta de verdad.

### 10.1 El ruteo libre: buses de grupo, folders, sidechain

Que la salida de una pista sea cualquier bus, que una pista sea la entrada de
otra, que un efecto lea una señal de otro lado.

**Lo que ya lo admite**: el bus de salida de `mt.track` es `external` — lo
provee quien la instancia (§2.2b). Mandar una pista a otro destino es pasarle
otro bus, y nada más.

**Lo que falta**: (a) el documento tiene que poder decir el destino, y el
destino tiene que ser una *identidad* (una pista, un bus de grupo) y no un
número de bus, que es un detalle de la instancia; (b) el orden de ejecución deja
de ser el del árbol — una pista que alimenta a otra tiene que sonar antes, o sea
un orden topológico sobre el ruteo, con el ciclo detectado y declarado; (c) la
latencia, que hoy no existe porque no hay efectos, y que en cuanto haya un
ruteo paralelo es lo único que decide si dos caminos suenan juntos.

### 10.2 Qué es un efecto

La forma del hueco está en §3.3 y no hay que volver a tocarla, incluido lo que
la hace suficiente: **un efecto puede ser un `GraphDef`**, así que un efecto
compuesto no necesita ningún mecanismo nuevo.

**Lo que falta es el efecto como documento**: su preset (qué se guarda y cómo
se vuelve a cargar), su **latencia declarada** y quién la compensa, el estado y
el orden del bypass, y qué pasa con la cola cuando se lo saca de la cadena.

### 10.3 Los sends

`mt.track` los declara en §3.2 y quedan **vacíos** hasta que haya efectos a
dónde mandarlos — un send a ninguna parte no se puede probar y no se puede oír.

**Lo que ya lo admite**: un send es un slot, o sea la misma repetición con
nombre que un clip o un efecto (§2.2d), y su destino es un bus `external`, o sea
el mismo mecanismo de §10.1. **Lo que falta**: la elección pre/post por send
(que es un cableado distinto, no un control), y el destino, que es §10.1.

### 10.4 El disco: `DiskIn` sobre segmentos

Anotado en §7.2 con la forma completa — el buffer circular de `DiskIn` pasa a
ser un cosido, y el hilo de disco resuelve la costura por adelantado — y con lo
que hay que medir: cuánta anticipación aguanta un comping picado.

No es urgente porque hoy el multipista trabaja sobre buffers en memoria. Es
importante porque una pieza de una hora no entra en memoria, y ese es el techo
que separa esto de una aplicación de verdad.
