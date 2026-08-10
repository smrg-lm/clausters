//! **What a signal element hands its claimed GPU slot** — the filling half of
//! the slot seam, whose declaring half is [`SignalElement::slot_kind`].
//!
//! A slot is claimed by the element and owned by the frame: the element cannot
//! touch a device, and the frame cannot know what a picture is built from. So
//! the element says *which* slot it wants ([`Bulk`](super::super::widget::element::Bulk)'s
//! neighbour, `Needs::slot`) and then, whenever its picture moves, hands over
//! the content — a pyramid at its own bucket, an analysis at its own window and
//! hop, or the columns its rolling transform just produced.
//!
//! Both fronts used to *derive* this from outside, twice each. The inline-data
//! path matched on the presentation to decide whether the element's own samples
//! became a pyramid or a set of analyses (`collect_timelines` natively,
//! `build_inline_timelines` in the page), and the waterfall pass reached into
//! `el.live.roll` to find the columns of the tick. Neither knows anything now:
//! one walk asks every widget [`SignalElement::fill`] and uploads what comes
//! back.
//!
//! **A load is not a fill.** What a loader resolved (a mapped file, a fetch, a
//! server buffer) is routed into the slot by the loader itself, on the declared
//! `SlotKind` — that is the bulk seam, and it is why an element whose data is
//! still out there hands back `None` here rather than an empty picture.

use super::super::widget::element::SlotFill;
use super::{Presentation, SignalElement};

impl SignalElement {
    /// The content of this element's claimed slot, or `None` when it has
    /// nothing new for it: it claimed no slot, its picture has not moved since
    /// the last fill, or the data it draws from has not arrived yet.
    ///
    /// The rolling case comes first and is the only one that repeats: a
    /// retained waterfall produces columns for as long as its bus runs, where a
    /// stored view fills once and then only when something it is built from
    /// changed.
    pub fn fill(&mut self) -> Option<SlotFill> {
        if let Some(roll) = self.live.roll.as_mut() {
            if !roll.is_dirty() {
                return None;
            }
            let (window_size, hop, sample_rate) = roll.geometry();
            let capacity = roll.capacity();
            return Some(SlotFill::Columns {
                columns: roll.take_pending(),
                window_size,
                hop,
                sample_rate,
                capacity,
            });
        }
        if !self.slot_dirty || self.slot_kind().is_none() {
            return None;
        }
        // The element's *own* samples, which is the only data it holds: a
        // resource it named is the loader's, and filling from nothing here
        // would show an empty picture until that load lands and replaces it.
        let data = self.source.data()?;
        if data.samples.is_empty() {
            return None;
        }
        let fill = if self.presentation == Presentation::Signal {
            SlotFill::Geometry(crate::waveform::WaveformData::from_interleaved(
                &data.samples,
                data.channels,
                data.base_bucket,
            ))
        } else {
            SlotFill::Texture(super::super::frame::stft_lanes(
                super::super::frame::deinterleave(&data.samples, data.channels),
                self.spectral.fft_size,
                self.spectral.hop,
                self.editor.sample_rate,
            ))
        };
        self.slot_dirty = false;
        Some(fill)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::host::BusSource;
    use crate::host::guidef::GuiNode;
    use crate::host::live::{tick_tree, update_retention};
    use crate::host::widget::element::{Live, SlotFill, SlotKind};
    use crate::host::widget::{Widget, WidgetKind};

    fn element(json: &str) -> SignalElement {
        let w = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        match w.kind {
            WidgetKind::Signal(el) => *el,
            other => panic!("not a signal: {other:?}"),
        }
    }

    /// The form follows the slot the element claimed, and the whole point of
    /// the seam is that nothing outside had to work that out: a navigable trace
    /// over inline samples fills a geometry slot with the pyramid, a
    /// spectrogram over the same samples fills a texture slot with analyses.
    #[test]
    fn inline_samples_fill_the_slot_the_element_claimed() {
        let mut trace = element(
            r#"{"id":1,"type":"signal","view":"trace","navigable":1,"data":[0.0,1.0,-1.0,0.5,0.0,-0.5,0.25,0.0]}"#,
        );
        assert!(matches!(trace.slot_kind(), Some(SlotKind::Geometry { .. })));
        let Some(SlotFill::Geometry(data)) = trace.fill() else {
            panic!("a navigable trace fills its geometry slot")
        };
        assert_eq!(data.total_samples(), 8);

        let ramp: Vec<String> = (0..1024)
            .map(|k| format!("{:.4}", (k as f32 * 0.01).sin()))
            .collect();
        let mut spectral = element(&format!(
            r#"{{"id":1,"type":"signal","view":"spectrogram","navigable":1,
                 "window_size":256,"hop":128,"data":[{}]}}"#,
            ramp.join(",")
        ));
        assert!(matches!(
            spectral.slot_kind(),
            Some(SlotKind::Texture { .. })
        ));
        let Some(SlotFill::Texture(stfts)) = spectral.fill() else {
            panic!("a spectrogram fills its texture slot")
        };
        assert_eq!(stfts.len(), 1);
        assert_eq!(stfts[0].window_size(), 256);
        assert!(stfts[0].n_frames() > 1, "one column per hop of the samples");
    }

    /// A fill is a *taking*: the second ask costs nothing, which is what keeps
    /// a tick that changed no picture at zero uploads.
    #[test]
    fn a_filled_slot_asks_for_nothing_again() {
        let mut trace =
            element(r#"{"id":1,"type":"signal","view":"trace","navigable":1,"data":[0.0,1.0]}"#);
        assert!(trace.fill().is_some());
        assert!(trace.fill().is_none());
        // ...until something it is built from moves.
        trace.slot_dirty = true;
        assert!(trace.fill().is_some());
    }

    /// An element with no slot never fills one, however much data it holds —
    /// a clip's take draws into the window's mesh.
    #[test]
    fn an_element_without_a_slot_fills_nothing() {
        let mut take = element(
            r#"{"id":1,"type":"signal","view":"trace","navigable":0,"data":[0.0,1.0,-1.0,0.5]}"#,
        );
        assert_eq!(take.slot_kind(), None);
        assert!(take.fill().is_none());
    }

    /// The rolling case, driven the way a front drives it: retain, tick, then
    /// ask the tree what to upload. The columns come out through the same door
    /// a stored picture does — which is the point, since the front used to
    /// reach into the transform to find them.
    #[test]
    fn a_retained_waterfall_hands_over_the_columns_of_the_tick() {
        struct Ramp;
        impl BusSource for Ramp {
            fn control(&self, _index: usize) -> f32 {
                0.0
            }
            fn read_bus(&self, _bus: i32, out: &mut [f32]) -> bool {
                for (k, s) in out.iter_mut().enumerate() {
                    *s = (k as f32 * 0.01).sin();
                }
                true
            }
        }
        let rate = 48_000.0;
        let mut tree = Widget::from_node(
            1,
            &GuiNode::parse(
                br#"{"id":1,"type":"signal","view":"spectrogram","bus":0,"rate":"audio",
                     "retention":0.05,"navigable":1,"window_size":256,"hop":128}"#,
            )
            .unwrap(),
            &[],
        )
        .unwrap();
        let mut histories = HashMap::new();
        update_retention(
            &tree,
            rate,
            2048,
            |_bus, out| {
                Ramp.read_bus(0, out);
                Some(2047)
            },
            &mut histories,
        );
        tick_tree(
            &mut tree,
            &Live {
                bus: Some(&Ramp),
                sample_rate: rate,
                histories: &histories,
            },
        );
        let Some(SlotFill::Columns {
            columns,
            window_size,
            hop,
            capacity,
            ..
        }) = tree.kind.fill()
        else {
            panic!("a ticked waterfall hands its columns over")
        };
        assert_eq!((window_size, hop), (256, 128));
        assert_eq!(capacity, (0.05 * rate / 128.0).ceil() as usize);
        assert!(!columns.is_empty());
        // And the next tick uploads nothing until another column lands.
        assert!(tree.kind.fill().is_none());
    }

    /// A named resource is the loader's: the element hands back nothing rather
    /// than an empty picture that would be replaced the moment the load lands.
    #[test]
    fn a_pending_load_fills_nothing() {
        let mut pending = element(
            r#"{"id":1,"type":"signal","view":"trace","navigable":1,"path":"take.f32","bulk":true}"#,
        );
        assert!(pending.slot_kind().is_some());
        assert!(pending.fill().is_none());
    }
}
