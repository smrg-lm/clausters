//! What a draw function takes: the batch it emits into and the two role
//! tables it reads, as one context.
//!
//! It sits at the top of the drawing base, over the batch ([`Mesh`]), the
//! size roles ([`Metrics`]) and the color roles ([`Theme`]), because it is the
//! only thing that holds all three: the batch is drawn into by the face, and
//! the size roles are measured by the face, so neither can be the module that
//! names the other.

use crate::metrics::Metrics;
use crate::paint::Mesh;
use crate::theme::Theme;

/// What a paint site draws with: the batch it emits into, plus the **size
/// roles** and the **color roles** it reads them from.
///
/// The three are one context because no paint site ever has fewer: a rectangle
/// needs a color role, its inset needs a size role, and both land in the same
/// batch. Carrying them as three parameters made every draw function three
/// wider than its own subject, which is what the `too_many_arguments` allows on
/// them were really saying.
///
/// Both tables are **passed, never global**: each window resolves its own
/// metrics at its own `ui_scale` (see [`metrics`](crate::metrics)), so the
/// context is built per frame by the renderer and handed down.
///
/// A draw function takes `&mut Draw` and either passes it on to the sub-draws
/// it delegates to, or -- a leaf that paints itself -- opens with
/// [`parts`](Self::parts) and works with the three directly.
pub struct Draw<'a> {
    /// The batch this site emits its triangles into.
    pub mesh: &'a mut Mesh,
    /// The size roles, resolved for the window being painted.
    pub m: &'a Metrics,
    /// The color roles.
    pub theme: &'a Theme,
}

impl<'a> Draw<'a> {
    pub fn new(mesh: &'a mut Mesh, m: &'a Metrics, theme: &'a Theme) -> Self {
        Self { mesh, m, theme }
    }

    /// The context's three parts, for a leaf that paints with them directly.
    pub fn parts(&mut self) -> (&mut Mesh, &Metrics, &Theme) {
        (self.mesh, self.m, self.theme)
    }
}
