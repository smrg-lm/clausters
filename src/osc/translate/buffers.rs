//! The `/buffer_*` commands, parsed into the NRT jobs that perform them.
//!
//! Buffer work allocates and touches the disk, so none of it happens here:
//! each command becomes an [`NrtJob`] the caller hands to the NRT runner, and
//! the reply follows when that finishes. What the parse needs from the current
//! contents it reads from the network-side pool mirror.

use super::*;
use crate::server::nrt::{EditOp, SampleWrite};

/// Parses one `/buffer_*` command (except the synchronous `/buffer_query`) into the
/// buffer index and the NRT job that performs it. `mirror` is the
/// network-side pool: commands that keep or reuse the current contents
/// (`/buffer_read`, `/buffer_write`, `/buffer_zero`, `/buffer_set` and
/// `/buffer_setRange`) read shape and data from it — which is state as of the
/// last *completed* command, so one of those right after a `/buffer_alloc`
/// needs a `/server_sync` between them, exactly as `/buffer_gen` does.
pub fn parse_buffer_msg(
    addr: &str,
    args: &[OscType],
    mirror: &BufferPool,
    default_sample_rate: f64,
) -> Result<(i32, NrtJob), String> {
    let (index, job) = match addr {
        "/buffer_alloc" => {
            let (index, frames) = match args {
                [OscType::Int(index), OscType::Int(frames), ..] => (*index, *frames),
                _ => return Err("expected: bufnum, frames [, channels]".into()),
            };
            let channels = int_arg(args, 2).unwrap_or(1);
            if frames <= 0 || channels <= 0 {
                return Err("frames and channels must be positive".into());
            }
            (
                index,
                NrtJob::Alloc {
                    frames: frames as usize,
                    channels: channels as usize,
                    sample_rate: default_sample_rate,
                },
            )
        }
        "/buffer_allocRead" | "/buffer_allocReadChannel" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames]".into()),
            };
            // The channel list is the tail, and the positions before it are
            // therefore required rather than optional: with a variadic tail
            // there is no telling a `fileStart` from a channel index.
            let channels = if addr == "/buffer_allocReadChannel" {
                channel_list(
                    args,
                    4,
                    "bufnum, path, fileStart, numFrames [, channels...]",
                )?
            } else {
                Vec::new()
            };
            (
                index,
                NrtJob::AllocRead {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(0) as i64,
                    channels,
                },
            )
        }
        // `leaveOpen` is accepted and ignored (no streaming yet). The buffer
        // must already exist; its shape is kept.
        "/buffer_read" | "/buffer_readChannel" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames, bufStart]".into()),
            };
            let channels = if addr == "/buffer_readChannel" {
                channel_list(
                    args,
                    5,
                    "bufnum, path, fileStart, numFrames, bufStart [, channels...]",
                )?
            } else {
                Vec::new()
            };
            let Some(current) = mirror_buffer(mirror, index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                index,
                NrtJob::Read {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(-1) as i64,
                    buf_start: int_arg(args, 4).unwrap_or(0).max(0) as usize,
                    current,
                    channels,
                },
            )
        }
        // WAV only in v1.
        "/buffer_write" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => {
                    return Err(
                        "expected: bufnum, path [, headerFormat, sampleFormat, numFrames, startFrame]"
                            .into(),
                    );
                }
            };
            let header = string_arg(args, 2).unwrap_or("wav");
            if !header.eq_ignore_ascii_case("wav") && !header.eq_ignore_ascii_case("wave") {
                return Err(format!("unsupported header format {header:?}"));
            }
            let Some(buffer) = mirror_buffer(mirror, index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                index,
                NrtJob::Write {
                    path,
                    sample_format: string_arg(args, 3).unwrap_or("int16").to_string(),
                    num_frames: int_arg(args, 4).unwrap_or(-1) as i64,
                    buf_start: int_arg(args, 5).unwrap_or(0).max(0) as usize,
                    buffer,
                },
            )
        }
        // Buffers are immutable: zeroing builds a same-shape replacement.
        "/buffer_zero" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                *index,
                NrtJob::Alloc {
                    frames: current.frames(),
                    channels: current.channels(),
                    sample_rate: current.sample_rate(),
                },
            )
        }
        // `/buffer_fill bufnum [start count value]...`: runs of one repeated
        // value, addressed **flat and interleaved** like `/buffer_set` beside
        // it — not in frames like the edits below, because this is the writing
        // family's member and not an editor's verb. scsynth's `/b_fill`.
        "/buffer_fill" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            let rest = &args[1..];
            if rest.is_empty() || !rest.len().is_multiple_of(3) {
                return Err("expected (start, count, value) triples".into());
            }
            let len = current.len();
            let mut fills = Vec::with_capacity(rest.len() / 3);
            for triple in rest.chunks_exact(3) {
                let start = match triple[0] {
                    OscType::Int(i) if i >= 0 => i as usize,
                    _ => return Err("a fill's start must be a non-negative int".into()),
                };
                let count = match triple[1] {
                    OscType::Int(i) if i >= 0 => i as usize,
                    _ => return Err("a fill's count must be a non-negative int".into()),
                };
                let Some(value) = float_arg(triple, 2) else {
                    return Err("a fill's value must be a float or an int".into());
                };
                // Past the end fails rather than clamping, like every write
                // here: a short fill would leave samples the caller believes
                // it overwrote.
                if start.saturating_add(count) > len {
                    return Err(format!(
                        "sample range {}..{} is past the end of buffer {index} ({len} samples)",
                        start,
                        start + count
                    ));
                }
                fills.push((start, count, value));
            }
            (
                *index,
                NrtJob::Fill {
                    base: current,
                    fills,
                },
            )
        }
        // The destructive edits: a span of the current contents changed in
        // place — well, in a copy of it, like every other write here. The span
        // is in **frames**, not flat samples: a selection is a stretch of time
        // across every channel, and the arithmetic is `clausters_core::edit`,
        // shared rather than reimplemented per caller.
        "/buffer_gain" | "/buffer_reverse" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            let start = int_arg(args, 1).unwrap_or(0);
            let frames = int_arg(args, 2).unwrap_or(-1);
            if start < 0 {
                return Err(format!("start frame must not be negative, got {start}"));
            }
            let start = start as usize;
            // A negative count means "to the end", the reading commands'
            // convention, so a whole-buffer edit needs no arithmetic client-side.
            let frames = if frames < 0 {
                current.frames().saturating_sub(start)
            } else {
                frames as usize
            };
            let op = if addr == "/buffer_reverse" {
                EditOp::Reverse { start, frames }
            } else {
                let from = float_arg(args, 3)
                    .ok_or("expected a starting gain (float or int)".to_string())?;
                // One value is a constant gain: the common case says itself.
                let to = float_arg(args, 4).unwrap_or(from);
                EditOp::Gain {
                    start,
                    frames,
                    from,
                    to,
                    shape: int_arg(args, 5).unwrap_or(clausters_core::envshape::SHAPE_LINEAR),
                    curve: float_arg(args, 6).unwrap_or(0.0),
                }
            };
            // Bounds are the core's to judge, but a failure here is a `/fail`
            // the caller reads rather than a job that quietly did nothing, so
            // the span is checked at parse time as `/buffer_setRange`'s is.
            if start.saturating_add(frames) > current.frames() {
                return Err(format!(
                    "frame range {}..{} is past the end of buffer {index} ({} frames)",
                    start,
                    start + frames,
                    current.frames()
                ));
            }
            (*index, NrtJob::Edit { base: current, op })
        }
        // The write half of the read pair: `/buffer_set` takes (index, value)
        // pairs, `/buffer_setRange` takes (start, blob) runs — bulk samples ride
        // as one little-endian f32 blob, the convention `/bus_tapStream.reply`
        // and `/buffer_export` already follow. Both address samples flat and
        // interleaved, exactly as `/buffer_get` and `/buffer_getRange` read them
        // back.
        "/buffer_set" | "/buffer_setRange" | "/buffer_setChannel" | "/buffer_setRangeChannel" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            // The `*Channel` forms name **one** channel, before the runs,
            // because the runs are the variadic tail: with a tail there is no
            // telling a channel index from a start. Their positions are then
            // frames of that channel, which is the whole point — a channel of
            // interleaved storage is a strided span and no flat start names it.
            let channel = if addr.ends_with("Channel") {
                let channels = current.channels().max(1);
                let ch = match args.get(1) {
                    Some(OscType::Int(c)) if *c >= 0 => *c as usize,
                    _ => return Err("expected: bufnum, channel, then the runs".into()),
                };
                if ch >= channels {
                    return Err(format!(
                        "buffer {index} has {channels} channel(s); there is no channel {ch}"
                    ));
                }
                Some((ch, channels))
            } else {
                None
            };
            let rest = &args[if channel.is_some() { 2 } else { 1 }..];
            let mut writes = if addr.starts_with("/buffer_setRange") {
                parse_set_runs(rest)?
            } else {
                parse_set_pairs(rest)?
            };
            // One conversion, here: a frame of channel `ch` is the flat index
            // `frame * channels + ch`, and consecutive frames are `channels`
            // apart. Everything downstream — the bounds check, the job, the
            // copy-and-swap — is the flat one it always was.
            if let Some((ch, channels)) = channel {
                for write in &mut writes {
                    write.at = write
                        .at
                        .checked_mul(channels)
                        .and_then(|at| at.checked_add(ch))
                        .ok_or_else(|| format!("frame index too large in buffer {index}"))?;
                    write.stride = channels;
                }
            }
            // Writing past the end would silently drop samples, so it fails
            // rather than clamping the way the reads do: a short read hands
            // back less than was asked for, a short write loses data the
            // caller believes it stored.
            let len = current.len();
            for write in &writes {
                if write.end() > len {
                    return Err(match write.stride {
                        1 => format!(
                            "sample range {}..{} is past the end of buffer {index} ({len} samples)",
                            write.at,
                            write.end()
                        ),
                        stride => format!(
                            "frame range {}..{} is past the end of buffer {index} ({} frames)",
                            write.at / stride,
                            write.at / stride + write.values.len(),
                            len / stride
                        ),
                    });
                }
            }
            (
                *index,
                NrtJob::Set {
                    base: current,
                    writes,
                },
            )
        }
        "/buffer_free" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            (*index, NrtJob::Free)
        }
        other => return Err(format!("{other} is not a buffer command")),
    };
    // The mirror pool is sized to the boot-time `--max-buffers`, so its length
    // is the authoritative index bound.
    if index < 0 || index as usize >= mirror.len() {
        return Err(format!("buffer index out of range: {index}"));
    }
    Ok((index, job))
}

/// Parses a `/buffer_gen bufnum cmd ...` command into the buffer index and the NRT
/// job that fills it. The named `cmd` selects a generator (`sine1`/`sine2`/
/// `sine3`/`cheby`) or `copy`; the flag int and the trailing floats are pulled
/// per command. Needs an allocated buffer (its shape drives generation), read
/// from `mirror` — so a `/buffer_gen` right after a `/buffer_alloc` needs a `/server_sync`
/// between them, exactly like `/buffer_read`.
pub fn parse_buffer_gen(args: &[OscType], mirror: &BufferPool) -> Result<(i32, NrtJob), String> {
    use crate::dsp::wavetable::{EnvSegment, GenCommand, GenFlags};

    let (index, cmd) = match args {
        [OscType::Int(index), OscType::String(cmd), ..] => (*index, cmd.as_str()),
        _ => return Err("expected: bufnum, command name, args...".into()),
    };
    if index < 0 || index as usize >= mirror.len() {
        return Err(format!("buffer index out of range: {index}"));
    }
    let Some(current) = mirror_buffer(mirror, index) else {
        return Err(format!("no buffer allocated at {index}"));
    };
    let rest = &args[2..];

    let command = match cmd {
        "copy" => {
            // copy dstStart srcBufnum srcStart numSamples
            let [
                OscType::Int(dst_start),
                OscType::Int(src_buf),
                OscType::Int(src_start),
                OscType::Int(num),
            ] = rest
            else {
                return Err("copy expects: dstStart, srcBufnum, srcStart, numSamples".into());
            };
            let Some(src) = mirror_buffer(mirror, *src_buf) else {
                return Err(format!("no source buffer allocated at {src_buf}"));
            };
            GenCommand::Copy {
                dst_start: (*dst_start).max(0) as usize,
                src,
                src_start: (*src_start).max(0) as usize,
                num: *num as i64,
            }
        }
        "sine1" | "sine2" | "sine3" | "cheby" => {
            let Some((OscType::Int(flag_bits), tail)) = rest.split_first() else {
                return Err(format!("{cmd} expects: flags, then values"));
            };
            let flags = GenFlags::from_bits(*flag_bits);
            let values: Vec<f32> = tail.iter().filter_map(float_value).collect();
            match cmd {
                "sine1" => GenCommand::Sine1 {
                    flags,
                    amps: values,
                },
                "cheby" => GenCommand::Cheby {
                    flags,
                    coeffs: values,
                },
                "sine2" => GenCommand::Sine2 {
                    flags,
                    partials: values.chunks_exact(2).map(|c| (c[0], c[1])).collect(),
                },
                // sine3
                _ => GenCommand::Sine3 {
                    flags,
                    partials: values.chunks_exact(3).map(|c| (c[0], c[1], c[2])).collect(),
                },
            }
        }
        "prepare_partconv" => {
            // prepare_partconv fftSize srcBufnum -- the typed heir of
            // scsynth's PreparePartConv (dest holds the partitioned spectra
            // the Conv UGen reads; size it with dsp::conv::layout::frames).
            let [OscType::Int(fft_size), OscType::Int(src_buf)] = rest else {
                return Err("prepare_partconv expects: fftSize, srcBufnum".into());
            };
            if *fft_size < 0 || !clausters_core::fft::supports(*fft_size as usize) {
                return Err(format!(
                    "prepare_partconv: unsupported fftSize {fft_size}; use one of {:?}",
                    clausters_core::fft::SUPPORTED_SIZES
                ));
            }
            let Some(src) = mirror_buffer(mirror, *src_buf) else {
                return Err(format!("no source buffer allocated at {src_buf}"));
            };
            GenCommand::PreparePartConv {
                src,
                fft_size: *fft_size as usize,
            }
        }
        "env" => {
            // env level0 [level time shape curve]...
            let vals: Vec<f32> = rest.iter().filter_map(float_value).collect();
            let Some((initial, seg_vals)) = vals.split_first() else {
                return Err("env expects: level0, then (level, time, shape, curve) groups".into());
            };
            if seg_vals.is_empty() || seg_vals.len() % 4 != 0 {
                return Err("env segments must be (level, time, shape, curve) groups".into());
            }
            let segments = seg_vals
                .chunks_exact(4)
                .map(|c| EnvSegment {
                    level: c[0],
                    time: c[1],
                    shape: c[2].round() as i32,
                    curve: c[3],
                })
                .collect();
            GenCommand::Env {
                initial: *initial,
                segments,
            }
        }
        other => return Err(format!("unknown /buffer_gen command {other:?}")),
    };
    Ok((
        index,
        NrtJob::Gen {
            current,
            cmd: command,
        },
    ))
}

/// `/buffer_set`'s `(index, value)...` tail: each pair becomes a run of one,
/// so both write commands hand the NRT job the same shape.
fn parse_set_pairs(args: &[OscType]) -> Result<Vec<SampleWrite>, String> {
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Err("expected: bufnum, then (index, value) pairs".into());
    }
    args.chunks_exact(2)
        .map(|pair| {
            let (OscType::Int(index), Some(value)) = (&pair[0], float_value(&pair[1])) else {
                return Err("expected: bufnum, then (index, value) pairs".into());
            };
            let at =
                usize::try_from(*index).map_err(|_| format!("negative sample index {index}"))?;
            Ok(SampleWrite::flat(at, vec![value]))
        })
        .collect()
}

/// The trailing channel indices of the `*Channel` reads, starting at `from`.
///
/// Empty means every channel, which is also what the plain `/buffer_read` and
/// `/buffer_allocRead` mean — so the pair is one arm with one extra argument
/// rather than two implementations of reading a file.
fn channel_list(args: &[OscType], from: usize, usage: &str) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate().skip(from) {
        match a {
            OscType::Int(c) if *c >= 0 => out.push(*c as usize),
            _ => {
                return Err(format!(
                    "argument {i} is not a channel index; expected {usage}"
                ));
            }
        }
    }
    Ok(out)
}

/// An arg that may be sent as a float or as an int — a client writing `1` for
/// unity gain means 1.0, and refusing that would be pedantry on the wire.
fn float_arg(args: &[OscType], n: usize) -> Option<f32> {
    match args.get(n) {
        Some(OscType::Float(f)) => Some(*f),
        Some(OscType::Int(i)) => Some(*i as f32),
        _ => None,
    }
}

/// `/buffer_setRange`'s `(start, blob)...` tail: each run's samples ride as one
/// little-endian `f32` blob, so several runs pack into one message and the run
/// length is the blob's — there is no declared count that could disagree with
/// what arrived.
///
/// The blob is why this is the *bulk* form. A run of N samples as N float args
/// costs N type tags and N encode steps at both ends, which is what made the
/// per-sample argument list unusable for the multi-megabyte edit the command
/// exists for; as bytes it is one copy. Same shape `/bus_tapStream.reply` sends
/// its windows in and `/buffer_export` writes its files in.
fn parse_set_runs(args: &[OscType]) -> Result<Vec<SampleWrite>, String> {
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Err("expected: bufnum, then (start, blob) runs".into());
    }
    args.chunks_exact(2)
        .map(|run| {
            let (OscType::Int(start), OscType::Blob(bytes)) = (&run[0], &run[1]) else {
                return Err("expected: bufnum, then (start, blob) runs".into());
            };
            let at =
                usize::try_from(*start).map_err(|_| format!("negative sample index {start}"))?;
            if !bytes.len().is_multiple_of(4) {
                return Err(format!(
                    "a sample blob is little-endian f32: {} bytes is not a multiple of 4",
                    bytes.len()
                ));
            }
            let values = bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            Ok(SampleWrite::flat(at, values))
        })
        .collect()
}

fn mirror_buffer(mirror: &BufferPool, index: i32) -> Option<Arc<Buffer>> {
    usize::try_from(index)
        .ok()
        .and_then(|i| mirror.get(i))
        .and_then(|b| b.as_ref().map(Arc::clone))
}
