//! What an offline render logged, kept for the caller.
//!
//! A render in the standalone `clausters --nrt` process logs to its stderr and
//! the client reads it there. A render **inside the client's own process** --
//! the embed C ABI's `clausters_render`, the web client's wasm entry point --
//! has no logger: nothing installed a subscriber, so a warning (a node the
//! engine rejected) or a `Poll`'s line would go nowhere. So while a render
//! runs, [`during`] puts a subscriber in front of whatever was there that
//! **keeps** each event at `info` or above as a [`LogLine`] and **passes every
//! event on** to the previous one, so a process that does print (the CLI)
//! prints exactly as before. The lines ride out on
//! [`RenderStats::log`](super::render::RenderStats::log), and each client hands
//! them to its own logger at their level.
//!
//! Only the calling thread is watched: a scoped dispatcher is thread-local.
//! Everything the render reports from -- the translator, the done/garbage
//! collection, the reply drain -- runs on the thread that drives the render.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Dispatch, Event, Level, Metadata, Subscriber};

/// One line a render logged: its level and its text (the message, then any
/// other fields as `name=value`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub level: Level,
    pub message: String,
}

/// The most lines one render keeps. A `Poll` firing at audio rate would
/// otherwise grow the list without bound; past this, the lines are counted
/// and the last one says how many were not kept.
pub const MAX_LINES: usize = 1000;

/// Runs `f` with the calling thread's events at `info` and above kept, and
/// returns what it returned with the lines, in order.
pub fn during<T>(f: impl FnOnce() -> T) -> (T, Vec<LogLine>) {
    let kept = Arc::new(Mutex::new(Kept::default()));
    let inner = tracing::dispatcher::get_default(Dispatch::clone);
    let keeper = Dispatch::new(Keeper {
        inner,
        kept: Arc::clone(&kept),
    });
    let out = tracing::dispatcher::with_default(&keeper, f);
    let kept = std::mem::take(&mut *kept.lock().unwrap_or_else(|e| e.into_inner()));
    let mut lines = kept.lines;
    if kept.dropped > 0 {
        lines.push(LogLine {
            level: Level::WARN,
            message: format!("... {} more log lines not kept", kept.dropped),
        });
    }
    (out, lines)
}

/// The lines as the text both in-process carriers hand over (the embed ABI's
/// `clausters_render_log`, the wasm `last_render_log`): one line per entry,
/// `LEVEL<TAB>message`, the level one of `ERROR`, `WARN`, `INFO`. A message
/// never holds a newline, so the text splits on them.
pub fn encode(lines: &[LogLine]) -> String {
    let mut text = String::new();
    for line in lines {
        let _ = writeln!(text, "{}\t{}", line.level, line.message);
    }
    text
}

#[derive(Default)]
struct Kept {
    lines: Vec<LogLine>,
    dropped: usize,
}

/// Keeps what is at `info` or above, and passes everything to `inner`.
struct Keeper {
    inner: Dispatch,
    kept: Arc<Mutex<Kept>>,
}

fn kept_level(meta: &Metadata<'_>) -> bool {
    *meta.level() <= Level::INFO
}

impl Subscriber for Keeper {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        let inner = self.inner.register_callsite(meta);
        if kept_level(meta) {
            Interest::always()
        } else {
            inner
        }
    }

    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        kept_level(meta) || self.inner.enabled(meta)
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        self.inner.new_span(span)
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        self.inner.record(span, values)
    }

    fn record_follows_from(&self, span: &Id, follows: &Id) {
        self.inner.record_follows_from(span, follows)
    }

    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();
        if kept_level(meta) {
            let mut text = Text::default();
            event.record(&mut text);
            let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
            if kept.lines.len() < MAX_LINES {
                kept.lines.push(LogLine {
                    level: *meta.level(),
                    message: text.finish(),
                });
            } else {
                kept.dropped += 1;
            }
        }
        if self.inner.enabled(meta) {
            self.inner.event(event);
        }
    }

    fn enter(&self, span: &Id) {
        self.inner.enter(span)
    }

    fn exit(&self, span: &Id) {
        self.inner.exit(span)
    }

    fn clone_span(&self, id: &Id) -> Id {
        self.inner.clone_span(id)
    }

    fn try_close(&self, id: Id) -> bool {
        self.inner.try_close(id)
    }
}

/// An event's fields as one line of text.
#[derive(Default)]
struct Text {
    message: String,
    fields: String,
}

impl Text {
    fn finish(self) -> String {
        let mut line = self.message;
        line.push_str(&self.fields);
        // One line per entry: the carriers split on newlines.
        line.replace('\n', " ")
    }
}

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_info_and_above_in_order_with_their_fields() {
        let ((), lines) = during(|| {
            tracing::debug!("not kept");
            tracing::info!(value = 3, "a poll");
            tracing::warn!("a rejected node");
            tracing::error!("broken");
        });
        assert_eq!(
            lines,
            vec![
                LogLine {
                    level: Level::INFO,
                    message: "a poll value=3".into()
                },
                LogLine {
                    level: Level::WARN,
                    message: "a rejected node".into()
                },
                LogLine {
                    level: Level::ERROR,
                    message: "broken".into()
                },
            ]
        );
    }

    #[test]
    fn encodes_one_tab_separated_line_per_entry() {
        let lines = [
            LogLine {
                level: Level::WARN,
                message: "rejected node 1000".into(),
            },
            LogLine {
                level: Level::INFO,
                message: "level: 0.25".into(),
            },
        ];
        assert_eq!(
            encode(&lines),
            "WARN\trejected node 1000\nINFO\tlevel: 0.25\n"
        );
    }

    #[test]
    fn keeps_at_most_max_lines_and_says_how_many_it_dropped() {
        let ((), lines) = during(|| {
            for i in 0..MAX_LINES + 5 {
                tracing::info!("line {i}");
            }
        });
        assert_eq!(lines.len(), MAX_LINES + 1);
        assert_eq!(lines[MAX_LINES].message, "... 5 more log lines not kept");
    }

    #[test]
    fn passes_every_event_on_to_the_subscriber_already_there() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // A subscriber that counts events, standing in for the CLI's.
        struct Counter(Arc<AtomicUsize>);
        impl Subscriber for Counter {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &Attributes<'_>) -> Id {
                Id::from_u64(1)
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn event(&self, _: &Event<'_>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
        }
        let count = Arc::new(AtomicUsize::new(0));
        let outer = Dispatch::new(Counter(Arc::clone(&count)));
        tracing::dispatcher::with_default(&outer, || {
            let ((), lines) = during(|| {
                tracing::trace!("passed, not kept");
                tracing::warn!("passed and kept");
            });
            assert_eq!(lines.len(), 1);
        });
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}
