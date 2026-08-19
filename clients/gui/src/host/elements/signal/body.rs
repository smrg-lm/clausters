//! **What a signal element is when it is placed on somebody else's axis**: the
//! drag a navigable view hands to the container under it, and the picture it
//! draws as a clip's body.
//!
//! Both answers say the same thing from two sides — *the axis is not mine*. A
//! view placed on a navigation group lets a plain drag sweep the group's
//! selection and Shift pan its window, because those gestures belong to the
//! axis rather than to the picture drawn on it; and a take inside a clip draws
//! with **no chrome at all** — no ruler, no gutter, no navigation — against the
//! clip's own local span, because the clip is what says where in time it sits.

use super::SignalElement;
use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::widget::element::{TextureLook, TimeSpace};
use crate::host::widget::{GestureMap, GestureStep};

impl SignalElement {
    /// The drag table a navigable view wants: a plain drag selects on the
    /// container's axis, Shift pans it. A view nobody navigates declares
    /// nothing and takes the generic table.
    pub fn gesture_map(&self) -> Option<GestureMap> {
        use GestureStep::*;
        self.caps
            .navigable
            .then(|| GestureMap::of_plans(&[Select], &[Pan], &[Select], &[Select]))
    }

    /// The look of a time-frequency body, which the frame draws for the
    /// element because it samples a texture: it goes to the GPU pass keyed by
    /// the clip's id, against the clip's own axis. Every other presentation
    /// draws itself ([`Self::draw_body`]).
    pub fn texture_body(&self) -> Option<TextureLook> {
        self.is_texture_view().then_some(TextureLook {
            db_floor: self.spectral.db_floor,
            db_ceil: self.spectral.db_ceil,
            freq_scale: self.spectral.freq_scale,
            colormap: self.spectral.colormap,
        })
    }

    /// The take, drawn into a clip's rectangle against the clip's local axis:
    /// the summarized trace between the element's own value bounds. An element
    /// whose data has not arrived draws nothing, and the clip's frame stands
    /// alone until it does.
    pub fn draw_body(&self, d: &mut Draw, rect: Rect, time: &TimeSpace) {
        let Some(data) = self.source.data() else {
            return;
        };
        let (min, max) = self.domain();
        crate::host::graphics::track::draw_take(
            d,
            rect,
            &time.view,
            &time.window,
            time.span,
            &data.trace(),
            min,
            max,
            self.measures,
            self.display.overlay,
            self.editor.sample_rate,
        );
    }
}
