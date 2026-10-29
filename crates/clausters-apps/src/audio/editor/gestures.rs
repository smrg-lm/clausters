//! **A gesture onto the take**, and the edits it comes to: a stroke, a copy,
//! a cut, a paste, a mix and a reverse, each a list of parts and the renders
//! that make the files it reads; and what a hand does that is not an edit.

use super::*;
use clausters_core::channel_matrix::ChannelMatrix;

/// What a paste or a mix carries in: the parts it puts in, and the steps
/// that make the file they read when the clipboard held samples.
type Carried = (parts::Tree, Vec<Step>);

/// What a gesture made: its label, the list after it and the steps that make
/// its new takes -- or nothing at all.
type Edited = Option<(String, parts::Tree, Vec<Step>)>;

/// **A history entry's half**: the take kept under `take`
/// ([`shared::Shared::keep_take`]), which is the file's content revision `revision`.
/// The entry holds the take's number as it holds the files the take reads,
/// so the take is let go of when the entry is ([`AudioEditor::release`]).
fn take_payload(take: u64, revision: u64) -> Value {
    json!({"intent": parts::PARTS, "take": take, "contentRevision": revision})
}

impl AudioEditor {
    /// One gesture onto the take. Answers the reason and the corrections the
    /// acknowledgement carries.
    pub(super) fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if tag == "show_envelope" {
            return self.show_envelope();
        }
        let channel = values.first().and_then(Value::as_u64);
        match tag {
            "unfold" => return self.unfold_channel(channel),
            "fold_channel" => return self.fold_channel(channel),
            "solo_channel" => return self.solo_channel(channel),
            "show_all_channels" => return self.show_all_channels(),
            _ => {}
        }
        if tag == "points" && self.window_state.envelope_widget.map(i64::from) == Some(widget) {
            return self.set_envelope_event(values, out);
        }
        if matches!(tag, "apply_envelope" | "apply") {
            return self.apply_envelope_event(out);
        }
        if tag == "layer" {
            return match values
                .first()
                .and_then(Value::as_str)
                .map(|name| self.edit_layer(name))
            {
                Some(Ok(corrections)) => (None, corrections),
                Some(Err(reason)) => (Some(reason), Vec::new()),
                None => (Some("layer needs a body name".into()), Vec::new()),
            };
        }
        if tag == "layer_take" {
            return match self.edit_layer("take") {
                Ok(corrections) => (None, corrections),
                Err(reason) => (Some(reason), Vec::new()),
            };
        }
        if matches!(tag, "view_wave" | "view_spectrogram" | "view_both") {
            return match self.layer_verb(tag, &Value::Null) {
                Ok(view) => {
                    out.layers = view.layers;
                    out.restructure = view.restructure;
                    (None, self.layer_corrections())
                }
                Err(reason) => (Some(reason), Vec::new()),
            };
        }
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(widget, tag, values, out);
            // a selection let go moved the position cursor to its start, and
            // the window draws it there with the play cursor
            let released =
                tag == "selection" && values.last().and_then(Value::as_bool) == Some(true);
            if released && self.file.presentation.selection.is_some() {
                return (None, self.band_corrections());
            }
            return (None, Vec::new());
        }
        if matches!(tag, "copy" | "copy_cut") {
            if tag == "copy_cut" {
                match self.cut("cut", values) {
                    Ok(Some(_)) => {}
                    Ok(None) => return (None, Vec::new()),
                    Err(reason) => return (Some(reason), Vec::new()),
                }
            }
            return match self.copy_span(values, out) {
                Ok(()) => {
                    if tag == "copy_cut" && out.clipboard.is_some() {
                        out.clipboard_cut = Some(FinishCut {
                            file: self.focused_file(),
                            before_revision: self.file.revision,
                            values: values.to_vec(),
                            seq: 0,
                        });
                    }
                    (None, Vec::new())
                }
                Err(why) => (Some(why), Vec::new()),
            };
        }
        let edited = match tag {
            "draw" | "sample" => self.stroke(tag, values),
            "cut" | "delete" => self.cut(tag, values),
            "paste" => self.paste(values),
            "mix" => self.mix(values),
            "reverse" => self.reverse(values),
            _ => return (None, Vec::new()),
        };
        match edited {
            Ok(Some((label, list, steps))) => {
                let before = self.file.contents.clone();
                self.file.contents.parts = list;
                out.record = Some(self.record(&label, &before));
                out.planned_steps = steps.clone();
                out.steps = Some(steps_json(&steps));
                out.changed = true;
                out.version += 1;
                // **The picture is read again**: the list it draws is replaced
                // whole, and only the answer can say so. A caller sends the
                // answer once the steps have landed.
                let mut corrections = self.reread();
                // **A cut or a paste leaves nothing selected**: the frames a
                // selection named have moved, so it would stand over other
                // samples. The position cursor stays where a cut landed, and
                // goes to the end of what a paste put in -- shown whole, the
                // view opened out to it where it ran past what is seen.
                if matches!(tag, "cut" | "delete" | "paste") {
                    let at = values.first().map_or(0.0, number).round().max(0.0) as u64;
                    if tag == "paste" {
                        let was = before.parts.frames();
                        let pasted = self.length().saturating_sub(was);
                        // a paste past the end went in at the end
                        let at = at.min(was);
                        corrections.extend(self.land_at(at + pasted, out));
                        corrections.extend(self.show_span(at, at + pasted, was));
                    } else {
                        corrections.extend(self.land_at(at, out));
                    }
                }
                (None, corrections)
            }
            Ok(None) => (None, Vec::new()),
            Err(why) => (Some(why), self.reread()),
        }
    }

    /// The entry a turn leaves: the list after, the list before, and the takes
    /// each reads.
    pub(super) fn record(&mut self, label: &str, before: &parts::Take) -> Record {
        let previous = self.file.revision;
        self.file.next_revision += 1;
        self.file.revision = self.file.next_revision;
        self.file.dirty = self.file.revision != self.file.saved_revision;
        let after = self.window_state.shared.keep_take(&self.file.contents);
        let was = self.window_state.shared.keep_take(before);
        let forward = take_payload(after, self.file.revision);
        let backward = take_payload(was, previous);
        let ids = |list: &parts::Tree, take: u64| {
            let mut ids: Vec<u64> = list.sources().iter().map(|s| s.0).collect();
            ids.push(take);
            ids
        };
        Record {
            label: label.into(),
            legs: vec![Leg {
                forward: json!({ "edit": forward }),
                backward,
                key: String::new(),
                holds_forward: ids(&self.file.contents.parts, after),
                holds_backward: ids(&before.parts, was),
            }],
        }
    }

    /// **A pencil stroke is a new take the size of the stroke**: a render of
    /// the frames it was drawn over, the drawn channel written in and the
    /// others what the list holds there, spliced over those frames.
    pub(super) fn stroke(&mut self, tag: &str, values: &[Value]) -> Result<Edited, String> {
        let taken = samples::intake(tag, values);
        let Some(payload) = taken.payloads.first() else {
            return match taken.refusal {
                Some(why) => Err(why),
                None => Ok(None),
            };
        };
        let start = number(payload.get("start").unwrap_or(&Value::Null)).max(0.0) as u64;
        let channel = number(payload.get("channel").unwrap_or(&Value::Null)).max(0.0) as u32;
        let mut drawn: Vec<f32> = payload
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().map(|x| number(x) as f32).collect())
            .unwrap_or_default();
        let total = self.length();
        if start >= total || channel >= self.file.channels {
            return Err("the stroke is outside the take".into());
        }
        drawn.truncate((total - start) as usize);
        let frames = drawn.len() as u64;
        if frames == 0 {
            return Ok(None);
        }
        let over = self.render_list(start, start + frames)?;
        let out = self.render_output("stroke", "draw")?;
        let step = renders::stroke(&over, channel, &drawn, &out);
        let id = self.rendered(&out, frames, self.file.rate);
        let list = self.file.contents.parts.replace(
            start,
            start + frames,
            &self.tree_of(&[part(id, 0, frames)]),
            &self.rates_of(),
        )?;
        Ok(Some((taken.label, list, vec![step])))
    }

    /// **A copy is the span's parts, by location**: a clipboard of segments,
    /// each naming its file, so a copy of an hour is a few parts and moves no
    /// samples, into this window or any other.
    pub(super) fn copy_span(&mut self, values: &[Value], out: &mut Outcome) -> Result<(), String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        let (start, len) = (at(0), at(1).min(self.length().saturating_sub(at(0))));
        if len == 0 {
            return Ok(());
        }
        let span = self
            .file
            .contents
            .parts
            .span(start, start + len, &self.rates_of())?;
        let list = self.file_list(&span);
        let files = list.files.values().map(|f| f.path.to_string()).collect();
        let clip = Clipboard::held_segments(
            self.file.channels,
            span.frames(),
            self.file.rate,
            HeldCopy {
                id: self.window_state.shared.keep_copy(list),
                files,
            },
        );
        out.clipboard = Some(clip.to_json());
        Ok(())
    }

    /// **A cut takes the span out of the list**, and moves no samples. A
    /// delete is the same removal with nothing put on the clipboard: what
    /// reaches here as a cut was copied first (`copy_cut`), so the two differ
    /// only in the history's label (`tag`).
    pub(super) fn cut(&mut self, tag: &str, values: &[Value]) -> Result<Edited, String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        let (start, len) = (at(0), at(1));
        if len == 0 {
            return Ok(None);
        }
        let list = self
            .file
            .contents
            .parts
            .remove(start, start + len, &self.rates_of())?;
        // a take may be cut down to nothing: an empty file is a header
        Ok(Some((tag.into(), list, Vec::new())))
    }

    /// **What a paste or a mix carries**, as parts of a list this editor can
    /// read: a clipboard of segments by its files, which nothing has to read
    /// to put in; a block the host carried, rendered into a file of its own.
    /// A block in a server's buffer is refused: the editor's engine does not
    /// reach that server. The steps make what has to be made, and the frames
    /// are the block's at the take's rate.
    pub(super) fn carried(&mut self, values: &[Value]) -> Result<Option<Carried>, String> {
        let doc = values.get(2).map(text).unwrap_or_default();
        let Ok(clip) = serde_json::from_str::<Clipboard>(&doc) else {
            return Err("the clipboard did not travel with the paste".into());
        };
        match clip.content {
            Content::Segments {
                channels,
                held: Some(held),
                ..
            } => {
                let copy = self
                    .window_state
                    .shared
                    .copy(held.id)
                    .ok_or("the copy is no longer held: copy it again")?;
                for (id, file) in copy.files.iter() {
                    let peaks = match &file.peaks {
                        Some(peaks) => peaks.to_string(),
                        None => sources::peaks_beside(&file.path),
                    };
                    let added = self.add_source(FileSource {
                        path: file.path.to_string(),
                        peaks,
                        frames: file.frames,
                        channels: file.channels,
                        rate: file.rate,
                        // a file another editor wrote is that editor's to delete
                        render: false,
                    });
                    debug_assert_eq!(added, *id, "a file has one id wherever it is named");
                }
                // **The copy's parts are shared** where they are counted as
                // this take counts them, which is at the same rate: a copy
                // pasted any number of times is then the same nodes.
                let tree = if copy.rate == self.file.rate {
                    copy.tree
                } else {
                    self.tree_of(&copy.tree.to_vec())
                };
                self.fit_width(tree, channels)
            }
            Content::Segments {
                channels, parts, ..
            } => {
                let mut list = Vec::with_capacity(parts.len());
                for segment in parts {
                    let peaks = self
                        .file
                        .sources
                        .get(&sources::id_of(&segment.path))
                        .map(|s| s.peaks.clone())
                        .unwrap_or_else(|| sources::peaks_beside(&segment.path));
                    let id = self.add_source(FileSource {
                        path: segment.path.clone(),
                        peaks,
                        frames: segment.source_frames,
                        channels: segment.source_channels,
                        rate: segment.source_rate,
                        // a file another editor wrote is that editor's to delete
                        render: false,
                    });
                    let mut piece = part(id, segment.start, segment.start + segment.frames);
                    piece.fade_in = segment.fade_in;
                    piece.fade_out = segment.fade_out;
                    piece.fade_in_shape = segment.fade_in_shape;
                    piece.fade_out_shape = segment.fade_out_shape;
                    piece.channels = segment.channels.clone();
                    list.push(piece);
                }
                let tree = self.tree_of(&list);
                self.fit_width(tree, channels)
            }
            Content::Buffer { .. } => Err(
                "a block in a server's buffer does not paste into a file: copy it from a file"
                    .into(),
            ),
            Content::Samples { .. } => {
                let (_, samples, rate) = self.block(values)?;
                let frames = samples.len() as u64 / u64::from(self.file.channels);
                if frames == 0 {
                    return Ok(None);
                }
                let out = self.render_output("pasted", "paste")?;
                let step = renders::block_file(&samples, self.file.channels, rate, &out);
                let id = self.rendered(&out, frames, rate);
                Ok(Some((self.tree_of(&[part(id, 0, frames)]), vec![step])))
            }
            other => Err(format!(
                "a take takes samples, and the clipboard holds {}",
                other.kind()
            )),
        }
    }

    /// **Parts `channels` wide made the take's width**, by the paste's rule
    /// ([`ChannelMatrix::fit`]): a routing is written into each part's map,
    /// over the map it had, so nothing is read; a mix is rendered into a
    /// file of the take's width, since a part's picture is composed one
    /// channel of its file's peaks at a time.
    fn fit_width(&mut self, tree: parts::Tree, channels: u32) -> Result<Option<Carried>, String> {
        let ours = self.file.channels;
        if tree.is_empty() {
            return Ok(None);
        }
        if channels == ours {
            return Ok(Some((tree, Vec::new())));
        }
        let matrix = ChannelMatrix::fit(channels as usize, ours as usize, None);
        let parts = tree.to_vec();
        let Some(route) = matrix.as_routing() else {
            let frames = tree.frames();
            let list = sources::list(&parts, &self.file.sources, channels, self.file.rate);
            let out = self.render_output("fitted", "paste")?;
            let step = renders::convert(&list, &matrix, &out);
            let id = self.rendered(&out, frames, self.file.rate);
            return Ok(Some((self.tree_of(&[part(id, 0, frames)]), vec![step])));
        };
        let mapped: Vec<Part> = parts
            .into_iter()
            .map(|mut piece| {
                let file = self
                    .file
                    .sources
                    .get(&piece.source.source)
                    .map_or(1, |s| s.channels as usize);
                let theirs = clausters_core::segments::channel_map(
                    piece.channels.as_deref(),
                    file,
                    channels as usize,
                );
                piece.channels = Some(
                    route
                        .iter()
                        .map(|&c| {
                            usize::try_from(c)
                                .ok()
                                .and_then(|c| theirs.get(c).copied())
                                .unwrap_or(-1)
                        })
                        .collect(),
                );
                piece
            })
            .collect();
        Ok(Some((self.tree_of(&mapped), Vec::new())))
    }

    /// **A paste puts what the clipboard carries in at the position**: a
    /// copy of segments as its parts, and a block as a file of its own, at
    /// its own rate -- the list reads it at the ratio, so nothing is
    /// converted on the way in.
    pub(super) fn paste(&mut self, values: &[Value]) -> Result<Edited, String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let Some((carried, steps)) = self.carried(values)? else {
            return Ok(None);
        };
        let list = self
            .file
            .contents
            .parts
            .insert(position, &carried, &self.rates_of())?;
        Ok(Some(("paste".into(), list, steps)))
    }

    /// **A mix adds what the clipboard carries onto the frames it lands
    /// on**: a render of those frames plus it, as long as the frames under
    /// it (a block that runs past the take is mixed as far as the take
    /// goes), spliced over them. A sum is taken sample for sample, so the
    /// block has to be at the take's rate.
    pub(super) fn mix(&mut self, values: &[Value]) -> Result<Edited, String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let total = self.length();
        if position >= total {
            return Err("the mix lands past the end of the take".into());
        }
        let doc = values.get(2).map(text).unwrap_or_default();
        let clip = serde_json::from_str::<Clipboard>(&doc)
            .map_err(|_| "the clipboard did not travel with the mix".to_string())?;
        if let Some(rate) = clip.sample_rate()
            && rate != self.file.rate
        {
            return Err(format!(
                "a block at {rate} Hz does not mix into a take at {} Hz",
                self.file.rate
            ));
        }
        let out = self.render_output("mixed", "mix")?;
        let (frames, made) = if let Content::Samples { .. } = clip.content {
            let (_, mut block, _) = self.block(values)?;
            let width = u64::from(self.file.channels);
            let frames = (block.len() as u64 / width).min(total - position);
            block.truncate((frames * width) as usize);
            if frames == 0 {
                return Ok(None);
            }
            let onto = self.render_list(position, position + frames)?;
            (frames, vec![renders::mix_block(&onto, &block, &out)])
        } else {
            let Some((carried, mut made)) = self.carried(values)? else {
                return Ok(None);
            };
            let rates = self.rates_of();
            let frames = carried.frames().min(total - position);
            if frames == 0 {
                return Ok(None);
            }
            let added = sources::list(
                &carried.span(0, frames, &rates)?.to_vec(),
                &self.file.sources,
                self.file.channels,
                self.file.rate,
            );
            let onto = self.render_list(position, position + frames)?;
            // what the block needed made goes first
            made.push(renders::mix_lists(&onto, &added, &out));
            (frames, made)
        };
        let id = self.rendered(&out, frames, self.file.rate);
        let list = self.file.contents.parts.replace(
            position,
            position + frames,
            &self.tree_of(&[part(id, 0, frames)]),
            &self.rates_of(),
        )?;
        Ok(Some(("mix".into(), list, made)))
    }

    /// **A span reversed**: a render of the frames played backwards, over
    /// them.
    pub(super) fn reverse(&mut self, values: &[Value]) -> Result<Edited, String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        // From the menu it names nothing, and is the selection's.
        let (start, len) = match (values.is_empty(), self.file.presentation.selection) {
            (true, Some((a, b))) => (a, b - a),
            (true, None) => return Err("select the span to reverse".into()),
            _ => (at(0), at(1)),
        };
        let len = len.min(self.length().saturating_sub(start));
        if len == 0 {
            return Ok(None);
        }
        // **In pieces, from the end**: each piece of the span reversed into a
        // file of its own, and the pieces in the reverse order of the span's.
        // A render reads what it reverses into memory, so a piece is what is
        // held at once, whatever the span's length -- the whole span of a take
        // of hours was, and the machine ran out of memory.
        let (mut steps, mut pieces) = (Vec::new(), Vec::new());
        let mut end = start + len;
        while end > start {
            let from = end.saturating_sub(REVERSE_PIECE).max(start);
            let span = self.render_list(from, end)?;
            let out = self.render_output(&format!("reversed-{}", pieces.len()), "reverse")?;
            steps.push(renders::reverse(&span, &out));
            let id = self.rendered(&out, end - from, self.file.rate);
            pieces.push(part(id, 0, end - from));
            end = from;
        }
        let list = self.file.contents.parts.replace(
            start,
            start + len,
            &self.tree_of(&pieces),
            &self.rates_of(),
        )?;
        Ok(Some(("reverse".into(), list, steps)))
    }

    /// **The block a paste or a mix carries**: where it goes, its samples
    /// interleaved at the take's width and the rate they were taken at; or
    /// why it does not fit this take.
    pub(super) fn block(&self, values: &[Value]) -> Result<(u64, Vec<f32>, f64), String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let doc = values.get(2).map(text).unwrap_or_default();
        let Ok(clip) = serde_json::from_str::<Clipboard>(&doc) else {
            return Err("the clipboard did not travel with the paste".into());
        };
        let Content::Samples {
            channels,
            frames,
            sample_rate,
            blob,
        } = clip.content
        else {
            return Err(format!(
                "a take takes samples, and the clipboard holds {}",
                clip.content.kind()
            ));
        };
        if channels == 0 {
            return Err("the clipboard's block has no channels".into());
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("the clipboard sample rate must be finite and positive".into());
        }
        let rate = sample_rate;
        let samples: Vec<f32> = match values.get(3 + blob) {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_f64()
                        .filter(|v| v.is_finite() && (*v as f32).is_finite())
                        .map(|v| v as f32)
                })
                .collect::<Option<Vec<_>>>()
                .ok_or("the clipboard contains invalid samples")?,
            _ => return Err("the clipboard's samples did not travel with it".into()),
        };
        let width = u64::from(channels);
        if frames.checked_mul(width) != Some(samples.len() as u64) {
            return Err("the clipboard sample payload does not match its shape".into());
        }
        if channels == self.file.channels {
            return Ok((position, samples, rate));
        }
        // **A block of another width goes by the paste's rule**, which in
        // memory is exact whether it routes or mixes.
        let matrix = ChannelMatrix::fit(channels as usize, self.file.channels as usize, None);
        let mut fitted = vec![0.0; frames as usize * matrix.outputs()];
        for (from, to) in samples
            .chunks_exact(matrix.inputs())
            .zip(fitted.chunks_exact_mut(matrix.outputs()))
        {
            matrix.apply(from, to);
        }
        Ok((position, fitted, rate))
    }

    /// **Frames `from..to` shown whole**: where they run past what the view
    /// shows (the take's `was` frames when it has said nothing of its view),
    /// it opens out until they are inside; a span already seen moves nothing.
    pub(super) fn show_span(&mut self, from: u64, to: u64, was: u64) -> Vec<Correction> {
        let Some(widget) = self.window_state.widget else {
            return Vec::new();
        };
        let (start, len) = self
            .file
            .presentation
            .viewport
            .unwrap_or((0.0, was.max(1) as f64));
        let (from, to) = (from as f64, to as f64);
        if from >= start && to <= start + len {
            return Vec::new();
        }
        let begin = start.min(from);
        let len = (start + len).max(to) - begin;
        self.file.presentation.viewport = Some((begin, len));
        vec![Correction {
            widget: i64::from(widget),
            props: json!({"view_start": begin, "view_len": len}),
        }]
    }

    /// A tag that says what the view is looking at rather than what changed.
    pub(super) fn observe(&mut self, widget: i64, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            "view" if values.len() >= 2 => {
                let start = number(&values[0]);
                let len = number(&values[1]);
                if start.is_finite() && len.is_finite() && len > 0.0 {
                    self.file.presentation.viewport = Some((start.max(0.0), len));
                }
            }
            "view_y" if values.len() >= 2 => {
                let start = number(&values[0]);
                let len = number(&values[1]);
                if start.is_finite() && len.is_finite() && len > 0.0 {
                    let measure = self.vertical_measure(widget).to_owned();
                    self.file
                        .presentation
                        .vertical
                        .insert(measure, (start, len));
                }
            }
            "locate" if !values.is_empty() => {
                let frame = number(&values[0]);
                let cursor = frame.max(0.0).round() as u64;
                self.file.presentation.cursor = Some(cursor);
                out.cue = self.file.presentation.cursor;
                out.locate = Some(self.secs_at(frame));
            }
            "selection" => {
                let frame = |i: usize| values.get(i).map_or(0.0, number).max(0.0);
                let (from, len) = (frame(0), frame(1));
                self.file.presentation.selection =
                    (len > 0.0).then(|| (from.round() as u64, (from + len).round() as u64));
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.secs_at(number(v)));
                out.selection = Some(json!({ "start": at(0), "len": at(1) }));
                let released = values.last().and_then(Value::as_bool).unwrap_or(false);
                // **A selection let go puts the cursor at its start**, and the
                // music with it, whether it plays or not; a loop in progress
                // repeats over the new span.
                if released && let Some((start, _)) = self.file.presentation.selection {
                    self.file.presentation.cursor = Some(start);
                    out.cue = Some(start);
                    out.locate = Some(self.secs_at(start as f64));
                }
                out.follow = self.follow(released);
            }
            _ => {}
        }
    }
}
