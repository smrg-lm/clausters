//! **Drawing a signal**: the four pictures the signal element can be, and the
//! one column source under all of them.
//!
//! The element itself — which point of the presentation × source × capabilities
//! product a widget is, what a `/gui_set` means to it, what it loads and what
//! it accumulates between ticks — is [`elements::signal`](super::super::elements::signal).
//! What is here is only the picture, so each module is a set of functions over
//! a [`Draw`](crate::host::paint::Draw) with no element, no props and no
//! device in sight.
//!
//! The four are not four renderers of four widgets: they are the presentations
//! the one element chooses between, and they share their measurement chrome
//! (the rulers, the lanes, the readout) rather than each growing its own.
//!
//! - [`trace`] — the **one** answer to *what is the min/max over this pixel and
//!   what sample sits at it*, over raw interleaved samples or over a peak
//!   pyramid. Every drawing of a signal against time reads its columns here:
//!   the GPU waveform, a clip's inline take, the static plot.
//! - [`plot`] — the framed static view: rulers, lanes, an auto-fitted range and
//!   a cursor readout naming the exact sample or bin under it.
//! - [`spectrum`] — the magnitude curve of one FFT, with its per-bin averaging
//!   and peak-hold traces.
//! - [`phasescope`] — a stereo pair in the mid/side plane, plus the correlation
//!   readout that reads it.
//! - [`waterfall`] — the rolling transform a retained live time-frequency view
//!   draws: whole hops analyzed once, pushed on the back, dropped off the
//!   front.

pub mod phasescope;
pub mod plot;
pub mod spectrum;
pub mod trace;
pub mod waterfall;
