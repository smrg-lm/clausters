//! The **signal element**: one element for every view of a signal, whatever
//! shape the signal arrives in and whatever the view lets you do with it.
//!
//! The catalog grew these one at a time — `waveform`, `plot`, `scope`,
//! `spectrum`, `spectrogram`, `phasescope` — and ended up spelling one idea six
//! ways. They differ along three axes and nothing else: the **presentation**
//! (the signal against time, its magnitude spectrum, its time-frequency
//! distribution, the phase of a stereo pair), the **source** (random-access —
//! a buffer, a file, an inline array — or forward-only, a bus at a rate), and
//! the **capabilities** the view offers over it.
//!
//! This module is that one element. It starts with the piece the six shared
//! most concretely: [`trace`], the single min/max-per-column source and the
//! mesh half of the signal presentation's renderer.

pub mod trace;
