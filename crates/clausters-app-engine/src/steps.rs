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

/// **Carries `steps` out in order**, telling `progress` how each render goes:
/// the render, the frames done and the frames it comes to. The first that
/// fails stops the rest, and is what is answered.
pub fn carry(steps: &[Step], mut progress: impl FnMut(&Render, u64, u64)) -> Result<(), String> {
    for step in steps {
        match step {
            Step::Render(job) => {
                render(job, |done, total| progress(job, done, total))?;
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
pub fn render(job: &Render, progress: impl FnMut(u64, u64)) -> Result<RenderStats, String> {
    let score = OscScore::from_bytes(&job.score)?;
    let cfg = RenderConfig {
        sample_rate: job.rate,
        channels: job.channels.max(1),
        ..RenderConfig::default()
    };
    if job.path.is_empty() {
        render_discarding(&score, &cfg, progress)
    } else {
        render_to_wav_with(&score, &cfg, &job.path, &job.format, progress)
    }
    .map_err(|e| format!("the render into {} failed: {e}", job.path))
}
