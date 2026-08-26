//! The memory behind one Faust `soundfile` zone, built the same way on both
//! backends.
//!
//! A `soundfile("<bufnum>", n)` in a def is a pointer, in the DSP struct, to a
//! flat [`ffi::Soundfile`] and to the planar sample arrays it names. Faust
//! never builds one and never frees one: whoever instantiates the DSP owns it
//! for the instance's life.
//!
//! **Where the two backends differ is only how the pointer is delivered.**
//! Natively libfaust asks for it through the UI glue's `addSoundfile`
//! callback; in a page there is no glue and no callback — the module's own
//! JSON declares the byte offset of each `Soundfile*` field, moved to the front
//! of the struct by the wasm backend, and the host stores the address there
//! itself. What is *in* the struct is this one piece of code either way, which
//! is the point of it living here: a soundfile that read a buffer differently
//! in a tab would be the same file sounding two ways.

use std::ffi::{c_int, c_void};

use crate::dsp::buffer::Buffer;
use crate::faust::ffi;

/// Backing memory for one Faust `soundfile` zone: the packed [`ffi::Soundfile`]
/// plus everything it points at. The DSP holds raw pointers into this, so it
/// must outlive the instance (the synth keeps it in `_soundfiles`).
pub struct SoundfileData {
    /// The struct handed to the DSP. Boxed for a stable address.
    pub soundfile: Box<ffi::Soundfile>,
    // Kept alive; the `Soundfile` fields point into these. Never read directly.
    _channel_ptrs: Vec<*mut f32>,
    _channels: Vec<Vec<f32>>,
    _length: Vec<c_int>,
    _sr: Vec<c_int>,
    _offset: Vec<c_int>,
}

impl SoundfileData {
    /// Builds a one-part `Soundfile` from a server buffer (deinterleaved to
    /// planar f32), or a silent placeholder when `buffer` is `None`/empty.
    pub fn new(buffer: Option<&Buffer>) -> Self {
        let (frames, channels, sr) = match buffer {
            Some(b) if b.frames() > 0 && b.channels() > 0 => {
                (b.frames(), b.channels(), b.sample_rate() as c_int)
            }
            _ => (
                ffi::FAUST_SOUNDFILE_EMPTY_FRAMES as usize,
                1,
                ffi::FAUST_SOUNDFILE_EMPTY_SR,
            ),
        };
        let cur_chan = channels.min(ffi::FAUST_MAX_CHAN);

        // Planar channels. The read index is inclusive (`min(i, fLength)`), so
        // pad one extra sample (a copy of the last) to guard the top read.
        let mut channels_data: Vec<Vec<f32>> = (0..cur_chan)
            .map(|c| {
                let mut v = vec![0.0f32; frames + 1];
                if let Some(b) = buffer {
                    // Read cell by cell: a soundfile bind is a **snapshot**
                    // taken at instantiation, so what it must not do is borrow
                    // a slice of a buffer the engine may be recording into.
                    for (f, slot) in v[..frames].iter_mut().enumerate() {
                        *slot = b.at(f * channels + c);
                    }
                    v[frames] = v[frames - 1];
                }
                v
            })
            .collect();

        // `fBuffers`: MAX_CHAN pointers — real channels first, the rest alias
        // earlier ones (Faust's `shareBuffers`).
        let mut channel_ptrs: Vec<*mut f32> = vec![std::ptr::null_mut(); ffi::FAUST_MAX_CHAN];
        for (c, buf) in channels_data.iter_mut().enumerate() {
            channel_ptrs[c] = buf.as_mut_ptr();
        }
        for c in cur_chan..ffi::FAUST_MAX_CHAN {
            channel_ptrs[c] = channel_ptrs[c % cur_chan];
        }

        // One part, repeated across the whole parts array (offset 0, full len).
        let length = vec![frames as c_int; ffi::FAUST_MAX_SOUNDFILE_PARTS];
        let sr_arr = vec![sr; ffi::FAUST_MAX_SOUNDFILE_PARTS];
        let offset = vec![0 as c_int; ffi::FAUST_MAX_SOUNDFILE_PARTS];

        let soundfile = Box::new(ffi::Soundfile {
            fBuffers: channel_ptrs.as_ptr() as *mut c_void,
            fLength: length.as_ptr() as *mut c_int,
            fSR: sr_arr.as_ptr() as *mut c_int,
            fOffset: offset.as_ptr() as *mut c_int,
            fChannels: cur_chan as c_int,
            fParts: 1,
            fIsDouble: 0,
        });
        Self {
            soundfile,
            _channel_ptrs: channel_ptrs,
            _channels: channels_data,
            _length: length,
            _sr: sr_arr,
            _offset: offset,
        }
    }
}
