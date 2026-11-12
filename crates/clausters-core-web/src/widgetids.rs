//! The widget-id table: an id that names what it draws.
//!
//! The GUI namespace's other door. A leased id changes on every redraw, so
//! anything in flight across one lands on the wrong widget; a keyed id is asked
//! for by naming what it draws and keeps its number for as long as it keeps
//! being drawn. Both doors take from one occupancy map, which is what makes
//! them impossible to collide.

#[cfg(target_arch = "wasm32")]
use clausters_core::widgetids::WidgetIds;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// A client's widget-id space, the JS face of
/// [`clausters_core::widgetids::WidgetIds`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = WidgetIds)]
pub struct JsWidgetIds(WidgetIds);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = WidgetIds)]
impl JsWidgetIds {
    /// A bounded table over `[base, base + capacity)`.
    #[wasm_bindgen(constructor)]
    pub fn new(base: f64, capacity: u32) -> JsWidgetIds {
        JsWidgetIds(WidgetIds::new(base as i64, capacity as usize))
    }

    /// The table whose space never runs out.
    pub fn unbounded(base: f64) -> JsWidgetIds {
        JsWidgetIds(WidgetIds::unbounded(base as i64))
    }

    /// A fresh drawer: the owner a `begin`/`retire` cycle names. One table
    /// serves a whole host, so a drawer is a value the table hands out rather
    /// than one a caller invents.
    pub fn owner(&mut self) -> f64 {
        self.0.owner() as f64
    }

    /// An id nothing names, or `undefined` when the space is full.
    pub fn alloc(&mut self) -> Option<f64> {
        self.0.alloc().map(|id| id as f64)
    }

    /// The first of a run of `width` contiguous anonymous ids -- what a
    /// mounted bundle's widgets take -- or `undefined` when no such run is
    /// free. Each goes back through `release`.
    #[wasm_bindgen(js_name = allocBlock)]
    pub fn alloc_block(&mut self, width: u32) -> Option<f64> {
        self.0.alloc_block(width as usize).map(|id| id as f64)
    }

    /// Returns an anonymous id to the space. Ids this table never handed out
    /// are ignored, so freeing is always safe.
    pub fn release(&mut self, id: f64) {
        self.0.free(id as i64);
    }

    /// The id that draws `(structure, role, key)`, minted on first ask and the
    /// same one after that; `undefined` when the space is full.
    #[wasm_bindgen(js_name = idFor)]
    pub fn id_for(&mut self, owner: f64, structure: f64, role: &str, key: &str) -> Option<f64> {
        self.0
            .id_for(owner as i64, structure as i64, role, key)
            .map(|id| id as f64)
    }

    /// The id that draws `(structure, role, key)` **if it already has one**.
    /// No minting, and no effect on the draw cycle.
    #[wasm_bindgen(js_name = idOf)]
    pub fn id_of(&self, structure: f64, role: &str, key: &str) -> Option<f64> {
        self.0
            .id_of(structure as i64, role, key)
            .map(|id| id as f64)
    }

    /// Gives one keyed id back by name, answering the id released.
    pub fn forget(&mut self, structure: f64, role: &str, key: &str) -> Option<f64> {
        self.0
            .forget(structure as i64, role, key)
            .map(|id| id as f64)
    }

    /// Starts `owner`'s draw: every keyed id that owner asks for until its
    /// `retire` counts as still drawn.
    pub fn begin(&mut self, owner: f64) {
        self.0.begin(owner as i64);
    }

    /// Ends `owner`'s draw and takes back every keyed id of that owner's it did
    /// not ask for, answering them ascending.
    pub fn retire(&mut self, owner: f64) -> Vec<f64> {
        self.0
            .retire(owner as i64)
            .into_iter()
            .map(|id| id as f64)
            .collect()
    }

    /// How many ids are held, keyed and anonymous together.
    #[wasm_bindgen(getter, js_name = inUse)]
    pub fn in_use(&self) -> u32 {
        self.0.in_use() as u32
    }

    /// How many of them answer to a name.
    #[wasm_bindgen(getter)]
    pub fn named(&self) -> u32 {
        self.0.named() as u32
    }

    /// Whether `id` falls in this table's space.
    pub fn contains(&self, id: f64) -> bool {
        self.0.contains(id as i64)
    }

    /// Drops every name and every id: the table as it was made.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}
