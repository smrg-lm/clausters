//! The `/buffer_*` commands, parsed into the NRT jobs that perform them.
//!
//! Buffer work allocates and touches the disk, so none of it happens here:
//! each command becomes an [`NrtJob`] the caller hands to the NRT runner, and
//! the reply follows when that finishes. What the parse needs from the current
//! contents it reads from the network-side pool mirror.

use super::*;

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
        "/buffer_allocRead" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames]".into()),
            };
            (
                index,
                NrtJob::AllocRead {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(0) as i64,
                },
            )
        }
        // `leaveOpen` is accepted and ignored (no streaming yet). The buffer
        // must already exist; its shape is kept.
        "/buffer_read" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames, bufStart]".into()),
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
        // The write half of the read pair: `/buffer_set` takes (index, value)
        // pairs, `/buffer_setRange` takes (start, count, value...) runs. Both
        // address samples flat and interleaved, exactly as `/buffer_get` and
        // `/buffer_getRange` read them back.
        "/buffer_set" | "/buffer_setRange" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            let writes = if addr == "/buffer_set" {
                parse_set_pairs(&args[1..])?
            } else {
                parse_set_runs(&args[1..])?
            };
            // Writing past the end would silently drop samples, so it fails
            // rather than clamping the way the reads do: a short read hands
            // back less than was asked for, a short write loses data the
            // caller believes it stored.
            let len = current.data().len();
            for (at, values) in &writes {
                if at.saturating_add(values.len()) > len {
                    return Err(format!(
                        "sample range {}..{} is past the end of buffer {index} ({len} samples)",
                        at,
                        at + values.len()
                    ));
                }
            }
            (*index, NrtJob::Set { current, writes })
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
fn parse_set_pairs(args: &[OscType]) -> Result<Vec<(usize, Vec<f32>)>, String> {
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
            Ok((at, vec![value]))
        })
        .collect()
}

/// `/buffer_setRange`'s `(start, count, value...)...` tail. `count` says how
/// many values belong to the run, so several runs pack into one message and a
/// truncated one is an error rather than a silent partial write.
fn parse_set_runs(args: &[OscType]) -> Result<Vec<(usize, Vec<f32>)>, String> {
    let mut writes = Vec::new();
    let mut rest = args;
    while !rest.is_empty() {
        let [OscType::Int(start), OscType::Int(count), tail @ ..] = rest else {
            return Err("expected: bufnum, then (start, count, value...) runs".into());
        };
        let at = usize::try_from(*start).map_err(|_| format!("negative sample index {start}"))?;
        let count =
            usize::try_from(*count).map_err(|_| format!("negative sample count {count}"))?;
        if tail.len() < count {
            return Err(format!(
                "run at {at} declares {count} values and carries {}",
                tail.len()
            ));
        }
        let values = tail[..count]
            .iter()
            .map(|a| float_value(a).ok_or_else(|| "sample values must be numbers".to_string()))
            .collect::<Result<Vec<f32>, _>>()?;
        writes.push((at, values));
        rest = &tail[count..];
    }
    if writes.is_empty() {
        return Err("expected: bufnum, then (start, count, value...) runs".into());
    }
    Ok(writes)
}

fn mirror_buffer(mirror: &BufferPool, index: i32) -> Option<Arc<Buffer>> {
    usize::try_from(index)
        .ok()
        .and_then(|i| mirror.get(i))
        .and_then(|b| b.as_ref().map(Arc::clone))
}
