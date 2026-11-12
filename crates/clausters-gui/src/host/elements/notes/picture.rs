//! **What a roll draws**: the notes as they are written, the veils over what
//! is not in hand, the slurs, the lines and the bars, the chrome of its axis,
//! and the reading under the pointer.

use super::*;

impl Notes {
    /// The axis' chrome over the grid: the shared selection band and the
    /// position cursor. What is under the pointer is said on the status band
    /// ([`Element::reading`]).
    /// **A roll in notation mode, over its notes**: each voice past a staff's
    /// first tinted, what a note sounds drawn inside what it is written as (a
    /// staccato is a short bar in a long note), and the dynamic a note states
    /// written in it where it fits.
    pub(super) fn draw_written(
        &self,
        d: &mut Draw,
        grid: Rect,
        nav: &View,
        axis: pianoroll::Pitches,
    ) {
        if self.written.voices.len() != 2 * self.notes.len() {
            return;
        }
        let (mesh, m, theme) = d.parts();
        for (i, n) in self.notes.iter().enumerate() {
            let Some(r) = pianoroll::note_rect(grid, nav, 0.0, n, axis) else {
                continue;
            };
            let tint = self
                .written
                .tints
                .get(i)
                .copied()
                .or_else(|| self.written.voice(i).map(|[_, voice]| voice));
            if let Some(tint) = tint.filter(|t| *t > 0) {
                mesh.rect(r, VOICE_TINTS[(tint - 1) % VOICE_TINTS.len()]);
            }
            // **What sounds is the note; the rest of what is written is its
            // outline, dotted.** The sounding stretch is closed by a solid
            // edge where the sound stops, and past it the written value is
            // emptied and drawn as a dotted frame -- legible whatever the
            // velocity fill is, since a quiet note's fill is close to the
            // ground. So a staccato reads as the note itself, shorter, in the
            // box of its value; a bar of its own laid inside the box read as
            // a mark on the note.
            if let Some(held) = self.written.held.get(i)
                && *held < n.dur - 1e-9
            {
                let x = pianoroll::to_x(n.start + held.max(0.0), nav, grid) as f32;
                let edge = m.divider_w;
                let (from, to) = (x.max(r.x + edge), r.x + r.w);
                if to - from > edge && r.h > 2.0 * edge {
                    mesh.rect(Rect::new(from, r.y, to - from, r.h), theme.lane);
                    mesh.rect(Rect::new(from - edge, r.y, edge, r.h), theme.note_edge);
                    let dotted = [
                        theme.note_edge[0],
                        theme.note_edge[1],
                        theme.note_edge[2],
                        0.7,
                    ];
                    let dash = (2.0 * edge).max(2.0);
                    let mut at = from + dash;
                    while at < to {
                        let w = dash.min(to - at);
                        mesh.rect(Rect::new(at, r.y, w, edge), dotted);
                        mesh.rect(Rect::new(at, r.y + r.h - edge, w, edge), dotted);
                        at += 2.0 * dash;
                    }
                    let mut y = r.y;
                    while y < r.y + r.h {
                        let h = dash.min(r.y + r.h - y);
                        mesh.rect(Rect::new(to - edge, y, edge, h), dotted);
                        y += 2.0 * dash;
                    }
                }
            }
            // **The page's marks on the note**, as a page writes them: its
            // articulations over the box, its breath at its end, its dynamic
            // under it. A symbol is drawn at twice the text, where a music
            // font's mark is a fraction of its em; a mark written as text at
            // the text's own size.
            let sized = |t: &str| {
                if t.chars().any(clausters_gui_draw::font::outline::has) {
                    m.text_scale * 2.0
                } else {
                    m.text_scale
                }
            };
            if let Some(marks) = self.written.marks.get(i).filter(|t| !t.trim().is_empty()) {
                let marks: Vec<&str> = marks.split_whitespace().collect();
                for (k, (x, y, scale)) in
                    mark_places(r, &marks, &sized, self.written.stacked, self.written.inside)
                        .into_iter()
                        .enumerate()
                {
                    font::text(mesh, marks[k], x, y, scale, theme.ruler_text);
                }
            }
            if let Some(breath) = self.written.breaths.get(i).filter(|t| !t.is_empty()) {
                let scale = sized(breath);
                font::text(
                    mesh,
                    breath,
                    r.x + r.w - font::width(breath, scale) * 0.5,
                    r.y - font::line_advance(scale),
                    scale,
                    theme.ruler_text,
                );
            }
            if let Some(label) = self.written.labels.get(i).filter(|l| !l.is_empty()) {
                font::text(mesh, label, r.x, r.y + r.h, sized(label), theme.ruler_text);
            }
        }
    }

    /// **The notes the roll does not take, veiled**: in notation mode a voice
    /// other than the one edited, in MIDI mode a channel other than the one
    /// edited (`dimmed`). A veiled note is no hit and has no velocity bar.
    pub(super) fn draw_veils(
        &self,
        d: &mut Draw,
        grid: Rect,
        nav: &View,
        axis: pianoroll::Pitches,
    ) {
        let (mesh, _, theme) = d.parts();
        let veil = [theme.lane[0], theme.lane[1], theme.lane[2], 0.65];
        for (i, n) in self.notes.iter().enumerate() {
            if !self.written.dimmed(i) {
                continue;
            }
            if let Some(r) = pianoroll::note_rect(grid, nav, 0.0, n, axis) {
                mesh.rect(r, veil);
            }
        }
    }

    /// **The section's slurs** (`slurs`), drawn as `slur_style` says: a band
    /// behind the boxes (`behind`), any other drawing over them. Cut at the
    /// grid; a slur whose ends are both off it is not drawn.
    pub(super) fn draw_slurs(
        &self,
        d: &mut Draw,
        grid: Rect,
        nav: &View,
        axis: pianoroll::Pitches,
        behind: bool,
    ) {
        let style = self.written.slur_style;
        if self.written.slurs.is_empty() || style.behind() != behind {
            return;
        }
        let (mesh, m, theme) = d.parts();
        let kept = mesh.clip();
        mesh.set_clip(Some(kept.map_or(grid, |k| k.intersect(grid))));
        let rect = |i: usize| {
            self.notes
                .get(i)
                .and_then(|n| pianoroll::note_rect(grid, nav, 0.0, n, axis))
        };
        let width = m.trace_w.max(1.0) * 1.5;
        for &[from, to, joined] in &self.written.slurs {
            let (lo, hi) = (from.min(to), from.max(to));
            let (Some(first), Some(last)) = (rect(from), rect(to)) else {
                continue;
            };
            // the boxes between its ends in time, of the first note's voice
            let voice = self.written.voice(from);
            let (start, end) = (
                self.notes[lo].start,
                self.notes[hi].start + self.notes[hi].dur,
            );
            let between: Vec<Rect> = self
                .notes
                .iter()
                .enumerate()
                .filter(|(j, n)| {
                    n.start >= start
                        && n.start < end
                        && (voice.is_none() || self.written.voice(*j) == voice)
                })
                .filter_map(|(j, _)| rect(j))
                .collect();
            if let Some(span) = crate::host::graphics::slurs::Span::over(first, last, &between) {
                crate::host::graphics::slurs::draw(
                    mesh,
                    style,
                    &span,
                    joined != 0,
                    width,
                    theme.text,
                );
            }
        }
        mesh.set_clip(kept);
    }

    /// **The pitch lines over the boxes** (`lines`): each run a line through
    /// the pitch it sounds, from the attack that starts it across every note
    /// it goes on into, so a line played without a new attack reads as one
    /// and a bend is seen where it moves the pitch. Cut at the grid.
    pub(super) fn draw_lines(
        &self,
        d: &mut Draw,
        grid: Rect,
        nav: &View,
        axis: pianoroll::Pitches,
    ) {
        if self.lines.is_empty() {
            return;
        }
        let (mesh, m, theme) = d.parts();
        let kept = mesh.clip();
        mesh.set_clip(Some(kept.map_or(grid, |k| k.intersect(grid))));
        let width = m.trace_w.max(1.0) * 1.5;
        for run in &self.lines {
            let at = |p: &[f64; 2]| {
                [
                    pianoroll::to_x(p[0], nav, grid) as f32,
                    axis.y(self.row_of(p[1]), grid),
                ]
            };
            for pair in run.windows(2) {
                mesh.line(at(&pair[0]), at(&pair[1]), width, theme.cursor);
            }
        }
        mesh.set_clip(kept);
    }

    /// **The barlines down the plane, wherever the ruler counts bars**: the
    /// page's, where the axis has them, else one every `quant` beats on a
    /// ruler in beats -- the bar a `bar:beat` label counts. A line at each
    /// bar's start, through the tempo map as the ruler places them, so the
    /// plane is barred where its ruler is and nowhere else.
    pub(super) fn draw_bars(&self, d: &mut Draw, grid: Rect, nav: &View, rate: f64) {
        let editor = &self.editor;
        let every = [[0.0, editor.quant, 1.0]];
        let runs: &[[f64; 3]] = if !editor.bars.is_empty() {
            &editor.bars
        } else if editor.ruler == Ruler::Beats && editor.quant > 0.0 {
            &every
        } else {
            return;
        };
        if rate <= 0.0 || nav.len <= 0.0 {
            return;
        }
        let units = |beat: f64| match editor.tempo_map.as_deref() {
            Some(map) => map.secs_at(beat) * rate,
            None => (beat - editor.beat_at) / editor.tempo.max(1e-9) * rate,
        };
        let beat = |units: f64| match editor.tempo_map.as_deref() {
            Some(map) => map.beats_at(units / rate),
            None => editor.beat_at + units / rate * editor.tempo,
        };
        let (b0, b1) = (beat(nav.start), beat(nav.start + nav.len));
        let (mesh, m, theme) = d.parts();
        let bars = crate::host::ruler::bars_in(runs, b0, b1);
        // a line where they stand far enough apart to read as bars
        let x_of = |b: f64| grid.x as f64 + (units(b) - nav.start) / nav.len * grid.w as f64;
        let narrowest = bars
            .windows(2)
            .map(|pair| x_of(pair[1].0) - x_of(pair[0].0))
            .fold(f64::INFINITY, f64::min);
        if narrowest < f64::from(m.tick_gap) {
            return;
        }
        for (at, _, _) in bars {
            let x = x_of(at) as f32;
            if x >= grid.x && x <= grid.x + grid.w {
                mesh.rect(Rect::new(x, grid.y, m.divider_w, grid.h), theme.frame);
            }
        }
    }

    pub(super) fn draw_axis_chrome(&self, d: &mut Draw, ctx: &Ctx, grid: Rect, nav: &View) {
        let Some(time) = ctx.time else {
            return;
        };
        let (mesh, m, theme) = d.parts();
        let to_x =
            |s: f64| (grid.x as f64 + (s - nav.start) / nav.len.max(1.0) * grid.w as f64) as f32;
        // The sweep, through the one routine every view that lets a hand draw
        // one draws it with: the same half-sample edges as a waveform's, and
        // **the pitch band this roll's own marquee swept** -- which it used to
        // throw away, drawing a full-height stripe over a selection that held a
        // few semitones of it.
        let axis = self.axis(m);
        crate::host::graphics::selection::draw_span(
            &mut Draw::new(mesh, m, theme),
            grid,
            nav,
            time.sel,
            1,
            self.editor.value_range(),
            crate::host::graphics::selection::Vertical::Pitch(axis),
        );
        // The position cursor here, and the playhead over it on the live
        // layer ([`Element::draw_live`]): two lines that mean two things, and
        // where they coincide the music's is the one that reads.
        if let Some(pos) = time.cursor
            && pos >= nav.start
            && pos <= nav.start + nav.len
        {
            mesh.rect(
                Rect::new(to_x(pos), grid.y, m.trace_w, grid.h),
                theme.cursor,
            );
        }
    }

    /// **What is under the pointer, said**: a curve's label and value, a
    /// note's pitch, velocity and channel -- in notation mode its staff and
    /// voice, how it is written and its dynamic -- or the pitch the row is,
    /// and the time. `None` off the grid and the curves.
    pub(super) fn reading_at(&self, ctx: &Ctx) -> Option<String> {
        ctx.time?;
        let m = ctx.metrics;
        let r = self.regions(ctx.rect, ctx.indent, m);
        let grid = r.grid;
        let nav = self.view(ctx.time);
        let rate = self.rate(ctx.world.sample_rate);
        let (cx, cy) = ctx.world.cursor?;
        if !ctx.rect.contains(cx, cy) {
            return None;
        }
        let curve = self.curve_readout((cx, cy), ctx.rect, ctx.indent, m, ctx.time);
        if curve.is_none() && !grid.contains(cx, cy) {
            return None;
        }
        let row = self.axis(m).pitch(cy as f32, grid);
        let s = nav.start + nav.len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
        let time = match self.editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => ruler::readout_beats(
                s,
                rate,
                self.editor.tempo,
                self.editor.beat_at,
                self.editor.quant,
                self.editor.tempo_map.as_deref(),
                nav.len / rate / grid.w.max(1.0) as f64,
            ),
            _ => ruler::readout_time(s, rate, nav.len / rate / grid.w.max(1.0) as f64),
        };
        let pitch_name = |pitch: f32| {
            if self.hz() {
                format!("{:.1} Hz", self.value_of(pitch))
            } else {
                format!(
                    "{} ({})",
                    clausters_core::scale::note_name(pitch.round() as i32),
                    pitch.round() as i32
                )
            }
        };
        let note = pianoroll::note_hit(
            grid,
            &nav,
            0.0,
            &self.notes,
            self.axis(m),
            cx as f32,
            cy as f32,
        )
        .map(|hit| hit.index);
        let value = match (curve, note) {
            (Some(curve), _) => curve,
            // A note's channel is counted from 1, the way MIDI shows one.
            (None, Some(i)) => {
                let n = &self.notes[i];
                let level = format!("vel {}  ch {}", n.velocity, n.channel + 1);
                match self.written.voice(i) {
                    // in notation mode, where and how it is written
                    Some([staff, voice]) => {
                        let mut said = format!(
                            "{}  {level}  staff {}  voice {}",
                            pitch_name(n.pitch),
                            staff + 1,
                            voice + 1
                        );
                        for more in [self.written.spelled.get(i), self.written.labels.get(i)]
                            .into_iter()
                            .flatten()
                            .filter(|t| !t.is_empty())
                        {
                            said.push_str("  ");
                            said.push_str(more);
                        }
                        said
                    }
                    None => format!("{}  {level}", pitch_name(n.pitch)),
                }
            }
            (None, None) => pitch_name(row),
        };
        Some(format!("{value}  {time}"))
    }
}
