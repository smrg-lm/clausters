//! **The status bar**: the last thing the host did, and the last thing it or
//! the owner refused.
//!
//! A host says things out loud that nothing was listening for. It refuses a
//! stroke where a pixel is more than one sample; it refuses a press on a body
//! that draws a rendering rather than the thing itself; and an owner answers
//! an edit with a `reason` the protocol has always carried and the mechanism
//! has never read ([`super::ack::Acked::reason`]). All of it was said into a
//! log file nobody has open, so a refused edit sprang back and taught "it
//! sometimes does not work" instead of "not here".
//!
//! This is where it lands instead. It is a **band along the bottom of the
//! window**, drawn by the host as chrome — not a widget, not a prop a script
//! writes, and nothing on the wire carries a line of it. That is the whole
//! design decision: the host already knows what it just did and what it
//! refused, so making the client re-send it would be asking the wire to carry
//! a fact back to the machine that produced it.
//!
//! # What goes in it
//!
//! Every `/gui_event` the host emits, at the one place it is stamped
//! (`gestures::effects::emit`), plus every owner's reason as its
//! acknowledgement is retired ([`super::Host::settle`]). So the line is the
//! **hand's own history**: what the last gesture asked for, and what came back.
//!
//! Consecutive lines from one widget with one verb **replace** rather than
//! stack, because a drag emits per motion and a log of four hundred `clip`
//! lines is a log of one.
//!
//! # Closed, open, hidden
//!
//! Closed it is one line: the newest. Clicking it **opens** it into the app's
//! log area — the last several lines, newest at the bottom — and clicking
//! again closes it. A window that wants neither says `status: false`, and the
//! band is not carved at all: the content gets the pixels back.
//!
//! It is per window and it does not travel. A status line is what this window
//! just did, in the same sense a selection is what this window is holding, and
//! the document is explicit that neither is part of what is edited.

use std::collections::VecDeque;

use clausters_core::osc::OscType;

use super::font;
use super::layout::Rect;
use super::metrics::Metrics;
use super::widget::{Widget, WidgetKind};

/// How many lines one window keeps. Past it the oldest is dropped: this is a
/// window's recent history, not a transcript — a session's transcript is the
/// log the host already writes.
pub const KEEP: usize = 128;

/// How many lines the **open** bar shows, before the window's own height has
/// its say ([`bar`]).
pub const OPEN_LINES: usize = 8;

/// What kind of thing a line reports, which is the only thing that colors it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Something was done: an edit went out.
    Did,
    /// Something was refused, by this host or by the owner that answered it.
    Refused,
}

/// One line the window has to say.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub kind: Kind,
    /// The widget it came from, when it came from one. What the collapse rule
    /// reads, so two widgets reporting the same verb keep two lines.
    pub widget: Option<i32>,
    /// The verb: the event's tag, or `"refused"`. The other half of the
    /// collapse key, and the first word of the drawn line.
    pub verb: String,
    /// The line as drawn, verb included.
    pub text: String,
}

impl Line {
    /// A line for an edit the host emitted: the verb and a short rendering of
    /// its payload.
    ///
    /// A **refusal** is recognized here rather than at the call site because
    /// this is the only place that reads an event's shape: the host refuses by
    /// emitting `"refused" <verb> <reason>`, which is an ordinary event on the
    /// wire and a different kind of line on the bar.
    pub fn of_event(widget: i32, args: &[OscType]) -> Line {
        let verb = match args.first() {
            Some(OscType::String(s)) => s.clone(),
            // A bare value — a knob's, a slider's. It has no tag on the wire
            // because there is nothing to disambiguate; here it needs a word.
            _ => "value".to_string(),
        };
        if verb == "refused" {
            let what = arg_str(args.get(1)).unwrap_or_default();
            let why = arg_str(args.get(2)).unwrap_or_default();
            let text = match (what.is_empty(), why.is_empty()) {
                (true, true) => "refused".to_string(),
                (false, true) => format!("refused {what}"),
                (true, false) => format!("refused: {why}"),
                (false, false) => format!("refused {what}: {why}"),
            };
            return Line {
                kind: Kind::Refused,
                widget: Some(widget),
                verb,
                text,
            };
        }
        let tail =
            summarize(&args[usize::from(matches!(args.first(), Some(OscType::String(_))))..]);
        let text = if tail.is_empty() {
            verb.clone()
        } else {
            format!("{verb} {tail}")
        };
        Line {
            kind: Kind::Did,
            widget: Some(widget),
            verb,
            text,
        }
    }

    /// A line for the reason an **owner** gave when it answered an edit — the
    /// `/gui_ack` string the mechanism deliberately does not read.
    pub fn of_reason(widget: Option<i32>, reason: &str) -> Line {
        Line {
            kind: Kind::Refused,
            widget,
            verb: "refused".to_string(),
            text: format!("refused: {reason}"),
        }
    }
}

/// The string an argument is, when it is one.
fn arg_str(arg: Option<&OscType>) -> Option<&str> {
    match arg {
        Some(OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// A payload's arguments as one short phrase: at most `SHOWN_ARGS` of them,
/// then how many were left.
///
/// It is a **sketch and says so**, because the alternative is a bar that
/// scrolls a piano-roll's whole note list past the reader one flat number at a
/// time. What the line is for is *which verb, on what, and did it land* — the
/// payload itself is on the wire for whoever owns the data.
fn summarize(args: &[OscType]) -> String {
    const SHOWN_ARGS: usize = 4;
    let mut parts: Vec<String> = args.iter().take(SHOWN_ARGS).map(one_arg).collect();
    if args.len() > SHOWN_ARGS {
        parts.push(format!("(+{} more)", args.len() - SHOWN_ARGS));
    }
    parts.join(" ")
}

/// One OSC argument, short. A float keeps three decimals and drops the tail of
/// zeros a placement in samples is full of.
fn one_arg(arg: &OscType) -> String {
    match arg {
        OscType::Int(n) => n.to_string(),
        OscType::Long(n) => n.to_string(),
        OscType::Float(f) => trim_zeros(&format!("{f:.3}")),
        OscType::Double(f) => trim_zeros(&format!("{f:.3}")),
        OscType::String(s) => s.clone(),
        OscType::Blob(b) => format!("<{} bytes>", b.len()),
        OscType::Bool(b) => b.to_string(),
        OscType::Nil => "nil".to_string(),
        other => format!("{other:?}"),
    }
}

/// `1.500` -> `1.5`, `2.000` -> `2`. Only for a string this module formatted,
/// so there is always a decimal point to find.
fn trim_zeros(s: &str) -> String {
    match s.find('.') {
        None => s.to_string(),
        Some(_) => s.trim_end_matches('0').trim_end_matches('.').to_string(),
    }
}

/// One window's status: what it has said, and whether the bar is open.
#[derive(Debug, Default)]
pub struct Status {
    lines: VecDeque<Line>,
    open: bool,
    /// How many lines **above the newest** the open log is anchored at. `0` is
    /// the bottom, which is where a log is read from.
    scroll: usize,
}

impl Status {
    /// Adds a line, **collapsing** it onto the last one when it is the same
    /// verb from the same widget.
    ///
    /// Without the collapse a drag would write a line per motion event: a clip
    /// pulled across a lane emits `clip` at every pointer sample, and the log
    /// of the gesture is one entry, not four hundred. Two *different* widgets
    /// reporting the same verb are two entries, which is why the widget is
    /// half the key.
    pub fn say(&mut self, line: Line) {
        if let Some(last) = self.lines.back_mut()
            && last.widget == line.widget
            && last.verb == line.verb
        {
            *last = line;
            return;
        }
        if self.lines.len() >= KEEP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        // **A reader who scrolled up is left where they were.** The anchor
        // counts from the newest line, so a line arriving under it would slide
        // the whole log up under the pointer -- which is how a log becomes
        // unreadable exactly while something is happening in it. At the bottom
        // (`0`) it follows, which is the other half of the same rule.
        if self.scroll > 0 {
            self.scroll = (self.scroll + 1).min(KEEP);
        }
    }

    /// The newest line, which is what the closed bar draws.
    pub fn last(&self) -> Option<&Line> {
        self.lines.back()
    }

    /// Every line kept, oldest first.
    pub fn lines(&self) -> impl DoubleEndedIterator<Item = &Line> {
        self.lines.iter()
    }

    /// How many lines are kept.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether nothing has been said yet.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Whether the bar is opened into the log area.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens or closes it. Returns whether that changed anything, so a front
    /// repaints on a toggle and not on a press that landed elsewhere.
    ///
    /// Closing returns it to the bottom: the closed bar draws the newest line
    /// and nothing else, so a bar reopened where it was left would be a bar
    /// showing one thing and then, on the next click, a different one.
    pub fn set_open(&mut self, open: bool) -> bool {
        let moved = self.open != open;
        self.open = open;
        if !open {
            self.scroll = 0;
        }
        moved
    }

    /// Where the open log is anchored: how many lines above the newest.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Moves the anchor by `lines` (positive is **back through the log**),
    /// with `visible` lines on screen. Returns whether it moved.
    ///
    /// The floor is the bottom and the ceiling is the oldest line still on
    /// screen: scrolling past either end would leave the band blank, and a log
    /// that can be scrolled into nothing reads as a log that lost its contents.
    pub fn scroll_by(&mut self, lines: isize, visible: usize) -> bool {
        let ceiling = self.lines.len().saturating_sub(visible.max(1));
        let want = (self.scroll as isize + lines).clamp(0, ceiling as isize) as usize;
        let moved = want != self.scroll;
        self.scroll = want;
        moved
    }

    /// How many of this log's lines fit in `band` — what the scroll clamps
    /// against, and what the drawing stops at. One function so the two agree:
    /// a clamp that allowed one line more than the band draws would scroll to
    /// a position that looks like the end of the log and is not.
    pub fn visible_lines(band: Rect, m: &Metrics) -> usize {
        let advance = font::line_advance(m.caption_scale).max(1.0);
        (((band.h - m.pad) / advance).floor().max(1.0)) as usize
    }

    /// Forgets every line. The bar stays where it is — clearing a log is not
    /// closing it.
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

/// Whether window `tree` carries a status bar at all — its `status` prop,
/// which is on by default.
///
/// Default **on** because the bar is the host's own voice, and a voice nobody
/// turns on is a voice nobody hears: the refusals this exists to show were
/// already being said, into nothing. A window that wants the pixels back says
/// `status: false`.
pub fn shown(tree: &Widget) -> bool {
    matches!(tree.kind, WidgetKind::Window { status: true, .. })
}

/// The band the status bar occupies inside `area`, or `None` when this window
/// has none.
///
/// **Both the renderer and the hit test call it**, which is the point of it
/// being one function: a bar drawn over pixels the layout also handed to a
/// widget would be a bar that swallows presses meant for the widget under it,
/// and a bar the layout avoided but the frame did not draw would be a strip of
/// window that does nothing.
///
/// Open, it asks for [`OPEN_LINES`] lines and takes what the window can spare:
/// a log that ate its own window would be a log with nothing to report about.
pub fn bar(tree: &Widget, status: Option<&Status>, area: Rect, m: &Metrics) -> Option<Rect> {
    if !shown(tree) {
        return None;
    }
    let line = m.status_h.max(1.0);
    let want = if status.is_some_and(Status::is_open) {
        line * OPEN_LINES as f32
    } else {
        line
    };
    // A third of the window, and never more than half of it: the bar is chrome
    // over the work, and the work stays the larger half of its own window.
    let h = want.min(area.h * 0.5).min(area.h);
    Some(Rect::new(area.x, area.y + area.h - h, area.w, h))
}

/// The height a **closed** status bar costs a window that is being fitted to
/// its content — zero when it has none.
///
/// Only the closed height, and deliberately: a window is sized when it opens,
/// and opening the log is a thing a hand does afterwards to a window that
/// already has a size. Growing the OS window under the reader's pointer to
/// make room for a log would move the work they were looking at.
pub fn bar_h(tree: &Widget, m: &Metrics) -> f32 {
    if shown(tree) {
        m.status_h.max(1.0)
    } else {
        0.0
    }
}

/// `area` with the status band taken off — where the window's tree is laid out.
pub fn content(tree: &Widget, status: Option<&Status>, area: Rect, m: &Metrics) -> Rect {
    match bar(tree, status, area, m) {
        Some(band) => Rect::new(area.x, area.y, area.w, (area.h - band.h).max(0.0)),
        None => area,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(verb: &str) -> Line {
        Line {
            kind: Kind::Did,
            widget: Some(1),
            verb: verb.to_string(),
            text: verb.to_string(),
        }
    }

    #[test]
    fn one_verb_from_one_widget_collapses_onto_its_own_line() {
        let mut s = Status::default();
        for _ in 0..400 {
            s.say(line("clip"));
        }
        assert_eq!(s.len(), 1, "a drag's motion events are one entry");
        s.say(line("lane"));
        assert_eq!(s.len(), 2, "a different verb is a different entry");
        let mut other = line("lane");
        other.widget = Some(2);
        s.say(other);
        assert_eq!(s.len(), 3, "the same verb from another widget is its own");
    }

    #[test]
    fn the_log_forgets_its_oldest_line_past_the_cap() {
        let mut s = Status::default();
        for i in 0..(KEEP + 10) {
            s.say(line(&format!("verb{i}")));
        }
        assert_eq!(s.len(), KEEP);
        assert_eq!(s.last().unwrap().verb, format!("verb{}", KEEP + 9));
    }

    #[test]
    fn the_log_scrolls_back_and_stops_at_both_ends() {
        let mut s = Status::default();
        for i in 0..20 {
            s.say(line(&format!("verb{i}")));
        }
        assert_eq!(s.scroll(), 0, "a log opens at its newest line");
        assert!(!s.scroll_by(-1, 5), "and does not scroll past it");
        assert!(s.scroll_by(3, 5));
        assert_eq!(s.scroll(), 3);
        // The ceiling is the oldest line still on screen, so the band never
        // scrolls into nothing.
        assert!(s.scroll_by(100, 5));
        assert_eq!(s.scroll(), 15, "20 lines, 5 of them visible");
        assert!(!s.scroll_by(1, 5));
    }

    #[test]
    fn a_line_arriving_leaves_a_scrolled_reader_where_they_were() {
        let mut s = Status::default();
        for i in 0..20 {
            s.say(line(&format!("verb{i}")));
        }
        s.scroll_by(4, 5);
        s.say(line("something-new"));
        assert_eq!(s.scroll(), 5, "the anchor followed its own lines up");
        // ...and closing it returns to the bottom, which is all the closed bar
        // can show.
        s.set_open(true);
        s.set_open(false);
        assert_eq!(s.scroll(), 0);
    }

    #[test]
    fn a_refusal_reads_as_one_and_carries_the_reason() {
        let args = vec![
            OscType::String("refused".into()),
            OscType::String("draw".into()),
            OscType::String("zoom in to draw: one pixel is 431 samples".into()),
        ];
        let l = Line::of_event(7, &args);
        assert_eq!(l.kind, Kind::Refused);
        assert_eq!(
            l.text,
            "refused draw: zoom in to draw: one pixel is 431 samples"
        );
    }

    #[test]
    fn an_owners_reason_is_a_refusal_too() {
        let l = Line::of_reason(Some(3), "a marker is the message it sends");
        assert_eq!(l.kind, Kind::Refused);
        assert_eq!(l.text, "refused: a marker is the message it sends");
    }

    #[test]
    fn an_edit_reads_as_its_verb_and_a_sketch_of_its_payload() {
        let args = vec![
            OscType::String("clip".into()),
            OscType::Float(1.5),
            OscType::Float(2.0),
        ];
        assert_eq!(Line::of_event(4, &args).text, "clip 1.5 2");
        let long: Vec<OscType> = std::iter::once(OscType::String("notes".into()))
            .chain((0..12).map(OscType::Int))
            .collect();
        assert_eq!(
            Line::of_event(4, &long).text,
            "notes 0 1 2 3 (+8 more)",
            "a payload is sketched, not transcribed"
        );
    }

    #[test]
    fn a_bare_value_still_has_a_word() {
        let l = Line::of_event(9, &[OscType::Float(0.42)]);
        assert_eq!(l.verb, "value");
        assert_eq!(l.text, "value 0.42");
    }
}
