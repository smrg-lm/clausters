# Notas de completado

Registro de lo implementado por Claude en cada milestone (ver PLAN.md).

## M0 — Esqueleto (completado 2026-06-10)

**Qué hay:** un binario que abre el dispositivo de audio por defecto con cpal y
suena una sinusoide de 440 Hz a amplitud 0.2. Verificado en esta máquina:
44100 Hz, 2 canales, sin errores de stream.

### Estructura

```
src/
├── lib.rs              # crate lib para que los tests usen el motor
├── main.rs             # arranca el backend y queda esperando (Ctrl-C)
├── server/
│   ├── engine.rs       # Engine: process_block() de 64 frames, no conoce cpal
│   └── backend.rs      # cpal + BlockAdapter (solo con feature `realtime`)
├── dsp/sinosc.rs       # SinOsc por acumulación de fase (fase en f64)
├── node/mod.rs         # stub — M2
└── osc/mod.rs          # stub — M1
tests/sine.rs           # tests offline del motor (2 tests, pasan)
```

### Decisiones tomadas

- **Motor desacoplado del backend**: `Engine::process_block(&mut [f32])` procesa
  bloques de `BLOCK_SIZE = 64` frames intercalados contra memoria. cpal vive solo
  en `backend.rs`. Esto habilita los tests sin dispositivo y el futuro modo NRT (M7).
- **Feature `realtime`** (default on): cpal es dependencia opcional;
  `cargo test --no-default-features` corre sin ALSA — es lo que debe usar CI.
- **`BlockAdapter`**: cpal entrega buffers intercalados de tamaño variable, no
  múltiplo de 64; el adapter pide bloques al motor y retiene el sobrante entre
  callbacks (`pos` arranca saturado para forzar el primer bloque).
- **Formatos de sample**: f32, i16, u16 vía `cpal::FromSample`; otros formatos
  devuelven error explícito.
- **Fase del oscilador en `f64`** para no degradar afinación en sesiones largas.
- El callback no aloca (todo pre-alocado en la construcción del adapter); aún sin
  guardián `assert_no_alloc` — entra en M2 junto con los FIFOs.

### Verificación

- `cargo test --no-default-features`: 2 tests pasan — frecuencia 440 Hz ±5 por
  cruces por cero, RMS ≈ 0.2/√2, sin NaN, canales coherentes.
- `cargo run --release` abre el stream y suena (probado 2026-06-10).

### Dependencias del sistema

- Linux: requiere `libasound2-dev` y `pkg-config` para compilar con la feature
  `realtime` (alsa-sys los necesita).

## M1 — Servidor OSC (completado 2026-06-10)

**Qué hay:** el binario ahora levanta, además del audio, un servidor OSC por UDP
en `127.0.0.1:57110` que implementa `/status`, `/quit`, `/notify` y `/dumpOSC`
con la semántica de scsynth. Verificado de punta a punta contra el binario real:
`/status` responde `/status.reply` con los sample rates del dispositivo y `/quit`
apaga el servidor limpiamente.

### Qué se agregó

```
src/osc/server.rs       # OscServer: socket UDP, dispatch de comandos, replies
src/main.rs             # arranca backend + OSC; el loop OSC corre en el main thread
src/lib.rs              # re-exporta rosc para tests y clientes
examples/osc_ping.rs    # cliente mínimo: /status (+ /quit) para pruebas a mano
tests/osc.rs            # 5 tests de integración por UDP real
```

### Comportamiento implementado

- **`/status`** → `/status.reply` con el formato scsynth de 9 argumentos:
  `(1, #UGens, #synths, #groups, #defs, avg_cpu, peak_cpu, sr_nominal, sr_real)`.
  Los contadores van en cero hasta que M2 conecte el node tree; los sample rates
  son los reales del dispositivo (Double).
- **`/notify 1|0`** → registra/desregistra la dirección del cliente y responde
  `/done /notify clientID` (IDs desde 1; registrar dos veces conserva el ID).
  La lista de clientes queda lista para las notificaciones `/n_go`/`/n_end` de M2.
- **`/quit`** → responde `/done /quit` y el loop retorna; main dropea el backend.
- **`/dumpOSC 0|1`** → activa/desactiva el log de mensajes parseados por stdout.
- **Comando desconocido / argumentos inválidos** → `/fail <cmd> <motivo>`, sin
  matar el servidor.
- **Bundles**: se ejecutan inmediatamente (recursivo); el scheduling por timetag
  es M6.

### Decisiones tomadas

- El servidor OSC corre **en el main thread** (bloqueante en `recv_from`); el
  audio vive en el hilo del callback de cpal. El hilo de red puede alocar y hacer
  I/O libremente — la frontera RT-safe (FIFOs) llega en M2.
- `rosc` se **re-exporta desde la lib** para que los tests de integración y los
  clientes usen exactamente la misma versión.
- Bind a `127.0.0.1` (no `0.0.0.0`) por defecto; exponerlo será opción de CLI.
- `ECONNREFUSED` en `recv_from` (rebote ICMP de un reply a un cliente ya cerrado,
  comportamiento de Linux) se ignora y se sigue sirviendo.
- Tests de integración con servidor en **puerto efímero** (`127.0.0.1:0`) y UDP
  real, hilo joineado tras `/quit` — corren en paralelo sin colisiones.

### Verificación

- `cargo test`: 7 tests pasan (5 de OSC + 2 del motor M0).
- E2E manual: `cargo run --release` + `cargo run --example osc_ping -- quit`
  (probado 2026-06-10; el servidor salió limpio tras `/quit`).

## Próximo: M2 — FIFO RT-safe + node tree

Ring buffers de comandos y basura (`rtrb`), `NodeTree` con grupos, synth
hardcodeado vía `/s_new`/`/n_free`, contadores reales en `/status.reply`, test
guardián con `assert_no_alloc`. Ver skills `realtime-audio` y `audio-testing`.
