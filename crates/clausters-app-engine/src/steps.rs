//! **The steps an application carries out itself**: what an editor's
//! coordinator hands a client to do beside its server, done in the
//! application's process.
//!
//! An edit that makes new samples (a stroke, a gain applied, a mix, the peaks
//! of a file opened with none) is an offline render of a score into a file
//! ([`Step::Render`]), and a save over a file a list reads moves files
//! ([`Step::Move`]). An application that holds its engine runs the render on
//! the core's offline render, in the thread that calls this, and moves the
//! file in its filesystem. The other steps are a server's (a message, a wait
//! for its `/done`, a barrier): an application that holds its engine has no
//! server, so one reaching here is a defect of whoever made it, said and
//! left out.

use clausters::server::render::{
    OscScore, RenderConfig, RenderStats, render_discarding, render_to_wav_with,
};
use clausters_editing::apply::{Render, Step};
use std::sync::atomic::{AtomicBool, Ordering};

/// **Carries `steps` out in order**, telling `progress` how the renders go:
/// the render under way, the frames done and the frames they come to, over
/// all of them -- an edit rendered in pieces (a long reverse) is one bar
/// filling once, not one per piece. The first that fails stops the rest,
/// and is what is answered.
pub fn carry(steps: &[Step], progress: impl FnMut(&Render, u64, u64)) -> Result<(), String> {
    carry_while(steps, &AtomicBool::new(true), progress)
}

/// [`carry`], **while `live` holds**: a step not begun once it is let go is
/// not begun, and what is answered is that the steps were called off. What
/// an editor closed in the middle of a long render asks, so its pieces stop
/// being written for nobody.
pub fn carry_while(
    steps: &[Step],
    live: &AtomicBool,
    mut progress: impl FnMut(&Render, u64, u64),
) -> Result<(), String> {
    let frames: Vec<u64> = steps
        .iter()
        .map(|step| match step {
            Step::Render(job) => OscScore::from_bytes(&job.score).map_or(0, |score| {
                (score.duration() * job.rate).round().max(0.0) as u64
            }),
            _ => 0,
        })
        .collect();
    let total: u64 = frames.iter().sum();
    let mut before = 0;
    for (step, frames) in steps.iter().zip(&frames) {
        if !live.load(Ordering::Acquire) {
            return Err("called off".into());
        }
        match step {
            Step::Render(job) => {
                render(job, |done, _| {
                    progress(job, (before + done).min(total), total.max(1))
                })?;
                before += frames;
            }
            Step::Move { from, to } => std::fs::rename(from, to)
                .map_err(|e| format!("moving {from} to {to} failed: {e}"))?,
            Step::Send(message) => {
                tracing::warn!(
                    "app engine: {} is a server's message and is not carried out here",
                    message.addr
                );
            }
            Step::AwaitDone { .. } | Step::Sync(_) => {}
        }
    }
    Ok(())
}

/// **One render**: the score rendered into its file, or into nothing for one
/// whose work is what its synths write (a `PeaksOut` measuring a file).
pub fn render(job: &Render, mut progress: impl FnMut(u64, u64)) -> Result<RenderStats, String> {
    let score = OscScore::from_bytes(&job.score)?;
    let cfg = RenderConfig {
        sample_rate: job.rate,
        channels: job.channels.max(1),
        ..RenderConfig::default()
    };
    // **A render of no frames is a file of no frames**: an empty take saved
    // is its header, which the offline render, which refuses a score that
    // ends where it starts, does not write.
    if score.duration() <= 0.0 {
        if !job.path.is_empty() {
            clausters::dsp::pcm::Writer::create(&job.path, cfg.channels, job.rate, &job.format)
                .and_then(|w| w.finalize())
                .map_err(|e| format!("the render into {} failed: {e}", job.path))?;
        }
        progress(0, 0);
        return Ok(RenderStats {
            frames: 0,
            events: 0,
            peak: vec![0.0; cfg.channels],
            rms: vec![0.0; cfg.channels],
            seed: 0,
            log: Vec::new(),
        });
    }
    if job.path.is_empty() {
        render_discarding(&score, &cfg, progress)
    } else {
        render_to_wav_with(&score, &cfg, &job.path, &job.format, progress)
    }
    .map_err(|e| format!("the render into {} failed: {e}", job.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Steps called off are not begun**: a move after the call is not made,
    /// and the answer says the steps were called off.
    #[test]
    fn steps_called_off_are_not_begun() {
        let dir = std::env::temp_dir().join(format!("clausters-carry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (from, to) = (dir.join("a"), dir.join("b"));
        std::fs::write(&from, b"x").unwrap();
        let steps = [Step::Move {
            from: from.to_string_lossy().into_owned(),
            to: to.to_string_lossy().into_owned(),
        }];
        let off = AtomicBool::new(false);
        assert_eq!(
            carry_while(&steps, &off, |_, _, _| {}),
            Err("called off".into())
        );
        assert!(from.exists() && !to.exists(), "nothing was moved");
        assert_eq!(carry(&steps, |_, _, _| {}), Ok(()));
        assert!(to.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
