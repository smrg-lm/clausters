//! **The audio editor, carried out by the application that holds its
//! engine**: every action of the editor's coordinator run to its end in the
//! application's process.
//!
//! The coordinator (`Editing::audio_act`, in `clausters_apps::editing`)
//! carries an action as far as it goes and asks its caller for what is the
//! caller's ([`Services`]): a file's header, the files on disk, steps carried
//! out, files deleted. [`AudioApp`] answers those here, over an engine the
//! application holds, runs on threads of its own the renders a run waits on,
//! and hands back only what is the window's (props to set, the window drawn
//! again, an answer, the file bound), which the host draws.
//!
//! **A render does not hold the application.** A run that waits on a render
//! hands it to a thread of its own and stops; the next [`AudioApp::landed`]
//! resumes it with how the render went, so a render of another file starts
//! meanwhile and either lands first. A history step is carried to its end in
//! place, renders included, since what it undoes is waited for by the step
//! that asked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use clausters_apps::editing::{Editing, Header, MemberId, Services, call_json};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_editing::apply::Step;
use serde_json::{Value, json};

pub use clausters_apps::editing::Raised;

use crate::AppEngine;
use crate::held::HeldAudio;

/// **An action carried as far as it goes**: the effects that are the
/// window's, in order, and its end -- `None` while it waits on a render.
#[derive(Debug)]
pub struct Ran {
    /// The member it is of.
    pub member: MemberId,
    /// What the caller named the action by, handed back with its end.
    pub token: u64,
    /// The window's effects, each `{"kind", ...}` as the coordinator says
    /// them (`clausters_apps::editing::Ran`), and `progress` (`label`,
    /// `done`, `total`; `done` null once a render has ended).
    pub effects: Vec<Value>,
    /// Its result, or what it raised; `None` while a render runs.
    pub end: Option<Result<Value, Raised>>,
}

/// What a render thread says back.
enum Note {
    /// How far a render has got.
    Progress {
        member: MemberId,
        /// What the render is for, in its verb's words.
        label: String,
        done: Option<u64>,
        total: u64,
    },
    /// A render ended: the run it was for, and how it went.
    Landed {
        member: MemberId,
        token: u64,
        run: u64,
        result: Result<(), String>,
        lease: bool,
    },
}

/// **The audio editor's application half**: the engine it holds, the ids
/// its nodes, buses and buffers are numbered from, and the renders in
/// flight.
pub struct AudioApp {
    engine: Arc<Mutex<AppEngine>>,
    ids: IdSpaces,
    notes: (Sender<Note>, Receiver<Note>),
    /// Each member's renders under way, which [`AudioApp::call_off`] lets go.
    live: HashMap<MemberId, Vec<Arc<AtomicBool>>>,
}

impl std::fmt::Debug for AudioApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioApp").finish_non_exhaustive()
    }
}

/// **What the application does for a run, at once** ([`Services`]): the
/// disk, and the steps carried out on its turn.
struct Here<'a> {
    live: &'a mut HashMap<MemberId, Vec<Arc<AtomicBool>>>,
}

impl Services for Here<'_> {
    fn header(&mut self, path: &str) -> Result<Header, String> {
        let info = clausters::server::nrt::probe_audio(path)?;
        Ok(Header {
            frames: info.frames,
            channels: info.channels as u32,
            rate: info.sample_rate,
            codec: info.codec,
            sample_format: info.sample_format,
        })
    }

    fn stamps(&mut self, paths: &[String]) -> Value {
        let mut stamps = serde_json::Map::new();
        for path in paths {
            let stamp = clausters_core::files::metadata(path)
                .ok()
                .map(|m| json!({"modified": m.modified_ms.unwrap_or(0.0), "size": m.len}));
            stamps.insert(path.clone(), stamp.unwrap_or(Value::Null));
        }
        Value::Object(stamps)
    }

    fn carry(&mut self, steps: &[Step]) -> Result<(), String> {
        crate::steps::carry(steps, |_, _, _| {})
    }

    fn delete(&mut self, files: &[String]) {
        for path in files {
            if let Err(e) = clausters_core::files::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("the audio editor could not delete {path}: {e}");
            }
        }
    }

    fn call_off(&mut self, member: MemberId) {
        for live in self.live.remove(&member).unwrap_or_default() {
            live.store(false, Ordering::Release);
        }
    }
}

impl AudioApp {
    /// The application half over `engine`.
    pub fn new(engine: Arc<Mutex<AppEngine>>) -> AudioApp {
        AudioApp {
            engine,
            ids: IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE),
            notes: channel(),
            live: HashMap::new(),
        }
    }

    /// **Calls off member `member`'s renders**: each stops before its next
    /// step, where its window closed in the middle of one -- a reverse of a
    /// day goes on writing its pieces otherwise, for a folder that is gone.
    pub fn call_off(&mut self, member: MemberId) {
        Here {
            live: &mut self.live,
        }
        .call_off(member);
    }

    /// The engine it holds.
    pub fn engine(&self) -> &Arc<Mutex<AppEngine>> {
        &self.engine
    }

    /// **Has `editing` play its audio members on this engine**; called once,
    /// before the first opens.
    pub fn hold(&self, editing: &mut Editing) {
        editing.hold_engine(Box::new(HeldAudio::new(Arc::clone(&self.engine))));
    }

    /// One verb of the context's door.
    pub fn call(&mut self, editing: &mut Editing, request: &Value) -> Value {
        serde_json::from_str(&call_json(editing, &request.to_string())).unwrap_or(Value::Null)
    }

    /// **What a script reads of audio member `member`**: `transport` or
    /// `output` (`Editing::audio_query`).
    pub fn query(&mut self, editing: &mut Editing, member: MemberId, query: &str) -> Value {
        editing.audio_query(member, query, &mut self.ids)
    }

    /// **Starts `action` on audio member `member`** and carries it as far as
    /// it goes; `token` comes back with its end. `lease` says the action
    /// holds the clipboard lease its event was prepared with.
    pub fn act(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        action: Value,
        token: u64,
        lease: bool,
    ) -> Ran {
        let ran = editing.audio_act(
            member,
            &action,
            lease,
            &mut Here {
                live: &mut self.live,
            },
            &mut self.ids,
        );
        self.went(member, token, ran, lease)
    }

    /// **Takes back what the engine handed over** and resumes every run whose
    /// render landed, in the order they landed; a render's progress comes
    /// back as a `progress` effect of a [`Ran`] with no end.
    pub fn landed(&mut self, editing: &mut Editing) -> Vec<Ran> {
        self.engine.lock().unwrap_or_else(|e| e.into_inner()).pump();
        let mut out = Vec::new();
        while let Ok(note) = self.notes.1.try_recv() {
            match note {
                Note::Progress {
                    member,
                    label,
                    done,
                    total,
                } => out.push(Ran {
                    member,
                    token: 0,
                    effects: vec![json!({"kind": "progress", "label": label,
                        "done": done, "total": total})],
                    end: None,
                }),
                Note::Landed {
                    member,
                    token,
                    run,
                    result,
                    lease,
                } => {
                    let ran = editing.audio_landed(
                        member,
                        run,
                        result,
                        lease,
                        &mut Here {
                            live: &mut self.live,
                        },
                        &mut self.ids,
                    );
                    out.push(self.went(member, token, ran, lease));
                }
            }
        }
        out
    }

    /// What a run came to, its render handed to a thread of its own.
    fn went(
        &mut self,
        member: MemberId,
        token: u64,
        ran: clausters_apps::editing::Ran,
        lease: bool,
    ) -> Ran {
        if let Some(render) = ran.render {
            self.hand_off(member, token, render.run, render.steps, lease);
        }
        Ran {
            member,
            token,
            effects: ran.effects,
            end: ran.end,
        }
    }

    /// **A render carried out off the application's turn**: the steps on a
    /// thread of their own, their progress and their end said back.
    fn hand_off(&mut self, member: MemberId, token: u64, run: u64, steps: Vec<Step>, lease: bool) {
        let tx = self.notes.0.clone();
        let live = Arc::new(AtomicBool::new(true));
        let renders = self.live.entry(member).or_default();
        renders.retain(|r| r.load(Ordering::Acquire) && Arc::strong_count(r) > 1);
        renders.push(Arc::clone(&live));
        // the window is told at once that a render runs, not a tenth of a
        // second into it
        if let Some(Step::Render(job)) = steps.iter().find(|s| matches!(s, Step::Render(_))) {
            let _ = tx.send(Note::Progress {
                member,
                label: job.label.clone(),
                done: Some(0),
                total: 1,
            });
        }
        let spawned = clausters_core::threads::spawn("audio-render", move || {
            let mut last = web_time::Instant::now();
            let mut told = |job: &clausters_editing::apply::Render, done, total| {
                // at most ten times a second, so a long render costs the
                // window nothing
                if last.elapsed().as_millis() >= 100 {
                    last = web_time::Instant::now();
                    let _ = tx.send(Note::Progress {
                        member,
                        label: job.label.clone(),
                        done: Some(done),
                        total,
                    });
                }
            };
            let result = crate::steps::carry_while(&steps, &live, &mut told);
            drop(live);
            let _ = tx.send(Note::Progress {
                member,
                label: String::new(),
                done: None,
                total: 0,
            });
            let _ = tx.send(Note::Landed {
                member,
                token,
                run,
                result,
                lease,
            });
        });
        if let Err(e) = spawned {
            let _ = self.notes.0.send(Note::Landed {
                member,
                token,
                run,
                result: Err(format!("no thread for the render: {e}")),
                lease,
            });
        }
    }
}
