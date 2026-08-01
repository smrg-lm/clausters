//! Process-wide logging: a `tracing` subscriber the **binary** installs, with a
//! runtime-reloadable filter so the `/server_verbosity` and `/server_dumpOsc` OSC commands can
//! retune the server's verbosity live (the client controls the server's logs).
//!
//! The audio thread never calls the `tracing` macros: it reports conditions over
//! the lock-free FIFOs and the **network thread** logs them (see
//! `server::engine` and `osc::server`). Logs go to **stderr**; stdout is left
//! for program output (the startup banner, the NRT render summary).
//!
//! Library and embed users do not call [`init`]; without a subscriber the
//! `tracing` macros compile to near-nothing, so the controls here are no-ops for
//! them (every setter returns `Ok` without doing anything until `init` runs).

use std::sync::{Mutex, OnceLock};
use tracing_subscriber::EnvFilter;

/// The OSC-traffic dump target. `/server_dumpOsc` toggles `clausters::osc=trace`, so
/// incoming/scheduled messages are logged through the same system as everything
/// else (no ad-hoc console print).
pub const OSC_TARGET: &str = "clausters::osc";

struct LogState {
    /// Base filter directive (from `-v`/`RUST_LOG` or `/server_verbosity`), without the
    /// OSC-dump overlay.
    base: String,
    /// Whether the OSC-traffic dump overlay is currently on.
    osc_dump: bool,
    /// Swaps the live `EnvFilter`. Boxed to erase the reload handle's type.
    reload: Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>,
}

static STATE: OnceLock<Mutex<LogState>> = OnceLock::new();

/// Maps a cumulative verbosity (`-q` = -1, default 0, `-v` = 1, `-vv` = 2, ...)
/// to a base filter directive. External crates stay at `warn` so a high level
/// does not drown the log in dependency noise.
pub fn directive_for(verbosity: i8) -> String {
    let level = match verbosity {
        i8::MIN..=-1 => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    format!("warn,clausters={level}")
}

/// Installs the global subscriber (idempotent: a second call, or a pre-existing
/// global subscriber, is ignored). The initial filter is `$RUST_LOG` if set,
/// otherwise the level derived from `verbosity`.
pub fn init(verbosity: i8) {
    use tracing_subscriber::{fmt, prelude::*, reload};

    let base = match std::env::var("RUST_LOG") {
        Ok(v) if !v.is_empty() => v,
        _ => directive_for(verbosity),
    };
    let Ok(env) = EnvFilter::try_new(&base) else {
        return;
    };
    let (filter, handle) = reload::Layer::new(env);
    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_target(verbosity >= 2),
        )
        .try_init()
        .is_ok();
    if !installed {
        return;
    }
    let reload = Box::new(move |f: EnvFilter| handle.reload(f).map_err(|e| e.to_string()));
    let _ = STATE.set(Mutex::new(LogState {
        base,
        osc_dump: false,
        reload,
    }));
}

/// Rebuilds the live filter from the current base plus the OSC-dump overlay.
fn apply(state: &LogState) -> Result<(), String> {
    let directive = if state.osc_dump {
        format!("{},{OSC_TARGET}=trace", state.base)
    } else {
        state.base.clone()
    };
    let filter = EnvFilter::try_new(&directive).map_err(|e| e.to_string())?;
    (state.reload)(filter)
}

/// Sets the base filter to an arbitrary `EnvFilter` directive (e.g. `"info"` or
/// `"clausters::osc=trace,warn"`). A no-op (returning `Ok`) when no subscriber
/// is installed, so it is safe to call from the OSC server in tests/embed.
pub fn set_base(directive: &str) -> Result<(), String> {
    EnvFilter::try_new(directive).map_err(|e| e.to_string())?;
    let Some(state) = STATE.get() else {
        return Ok(());
    };
    let mut state = state.lock().unwrap();
    state.base = directive.to_string();
    apply(&state)
}

/// Sets the base filter from a verbosity level (see [`directive_for`]).
pub fn set_verbosity(verbosity: i8) -> Result<(), String> {
    set_base(&directive_for(verbosity))
}

/// Turns the OSC-traffic dump overlay on or off, independently of the base
/// level. A no-op when no subscriber is installed.
pub fn set_osc_dump(on: bool) -> Result<(), String> {
    let Some(state) = STATE.get() else {
        return Ok(());
    };
    let mut state = state.lock().unwrap();
    state.osc_dump = on;
    apply(&state)
}
