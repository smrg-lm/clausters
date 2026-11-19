//! **The pictures the host draws, packed into one sheet**: an image's
//! pixels, decoded by whoever read its file, kept here as the glyphs are
//! kept in [`crate::font::atlas`] -- one RGBA sheet a window copies to its
//! card and a mesh's image vertices sample ([`crate::paint::Mesh::image`]).
//!
//! A picture is packed once, under a key its caller chooses (what it was
//! read from), and found again by that key. A picture wider or taller than
//! the sheet is shrunk to fit it before it is packed: the sheet is drawn
//! from at the size the picture is shown, never finer than the screen can
//! tell apart, and a documentation page shows nothing wider than itself.
//!
//! The sheet grows by doubling its rows, and when it is full at its
//! largest it is emptied and packed again; the [`epoch`](ImageAtlas::epoch)
//! says so, so a picture kept from before is drawn again.

use std::cell::RefCell;
use std::collections::HashMap;

/// The width of the sheet, in texels, and the most rows it grows to.
pub const SIDE: u32 = 2048;

/// The rows a sheet starts with.
const FIRST_ROWS: u32 = 256;

/// A texel of empty margin around each picture, so a filtered sample at a
/// quad's edge never reaches a neighbour.
const PAD: u32 = 1;

/// Bytes per texel: red, green, blue and alpha.
pub const TEXEL: u32 = 4;

/// **Where a picture is in the sheet**: its texels (`[u0, v0, u1, v1]`, as
/// a mesh's image vertices take them) and the size it was packed at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub uv: [f32; 4],
    pub width: u32,
    pub height: u32,
}

/// The sheet: RGBA texels, a shelf packer over them, and where each picture
/// landed.
#[derive(Default)]
pub struct ImageAtlas {
    pixels: Vec<u8>,
    rows: u32,
    pen_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    map: HashMap<u64, Placed>,
    version: u64,
    epoch: u64,
}

impl ImageAtlas {
    fn new() -> Self {
        Self {
            pen_x: PAD,
            shelf_y: PAD,
            ..Self::default()
        }
    }

    /// The picture kept under `key`, if one is.
    pub fn get(&self, key: u64) -> Option<Placed> {
        self.map.get(&key).copied()
    }

    /// **Packs a picture** of `width` x `height` RGBA texels (rows top to
    /// bottom, `4 * width` bytes each) under `key`, shrunk first where it is
    /// larger than the sheet; answers where it went. `None` for pixels that
    /// are not that size.
    pub fn insert(&mut self, key: u64, width: u32, height: u32, rgba: &[u8]) -> Option<Placed> {
        if width == 0 || height == 0 || rgba.len() != (width * height * TEXEL) as usize {
            return None;
        }
        let most = SIDE - 2 * PAD;
        let (w, h, owned);
        let pixels = if width > most || height > most {
            let scale = (most as f32 / width as f32).min(most as f32 / height as f32);
            w = ((width as f32 * scale) as u32).max(1);
            h = ((height as f32 * scale) as u32).max(1);
            owned = shrink(width, height, rgba, w, h);
            &owned[..]
        } else {
            (w, h) = (width, height);
            rgba
        };
        let (ox, oy) = self.fit(w, h);
        let row = (SIDE * TEXEL) as usize;
        for y in 0..h {
            let src = (y * w * TEXEL) as usize;
            let dst = (oy + y) as usize * row + (ox * TEXEL) as usize;
            self.pixels[dst..dst + (w * TEXEL) as usize]
                .copy_from_slice(&pixels[src..src + (w * TEXEL) as usize]);
        }
        self.version += 1;
        let placed = Placed {
            uv: [ox as f32, oy as f32, (ox + w) as f32, (oy + h) as f32],
            width: w,
            height: h,
        };
        self.map.insert(key, placed);
        Some(placed)
    }

    /// Reserves a box on the current shelf, opening the next one where it
    /// does not fit, growing the sheet, or emptying it once it is full at
    /// its largest.
    fn fit(&mut self, w: u32, h: u32) -> (u32, u32) {
        if self.pen_x + w + PAD > SIDE {
            self.pen_x = PAD;
            self.shelf_y += self.shelf_h + PAD;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + PAD > SIDE {
            self.reset();
        }
        let needed = self.shelf_y + h + PAD;
        if needed > self.rows {
            let mut rows = self.rows.max(FIRST_ROWS);
            while rows < needed {
                rows *= 2;
            }
            self.rows = rows.min(SIDE);
            self.pixels.resize((SIDE * TEXEL * self.rows) as usize, 0);
        }
        let at = (self.pen_x, self.shelf_y);
        self.pen_x += w + PAD;
        self.shelf_h = self.shelf_h.max(h);
        at
    }

    /// Empties the sheet: every picture is packed again when it is next
    /// asked for.
    fn reset(&mut self) {
        self.map.clear();
        self.pixels.clear();
        self.rows = 0;
        self.pen_x = PAD;
        self.shelf_y = PAD;
        self.shelf_h = 0;
        self.version += 1;
        self.epoch += 1;
    }

    /// Bumped whenever the texels change.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Bumped whenever the sheet was emptied and packed again.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The texels, [`SIDE`] times [`TEXEL`] bytes a row.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many rows the sheet has.
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// The top of the shelf being filled: every row above it is finished.
    pub fn shelf_top(&self) -> u32 {
        self.shelf_y
    }

    /// One past the last row that holds a picture.
    pub fn used_rows(&self) -> u32 {
        (self.shelf_y + self.shelf_h + PAD).min(self.rows)
    }
}

/// `rgba` shrunk from `w` x `h` to `to_w` x `to_h`, each texel the mean of
/// the ones it covers.
fn shrink(w: u32, h: u32, rgba: &[u8], to_w: u32, to_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (to_w * to_h * TEXEL) as usize];
    for ty in 0..to_h {
        let y0 = ty * h / to_h;
        let y1 = ((ty + 1) * h / to_h).max(y0 + 1);
        for tx in 0..to_w {
            let x0 = tx * w / to_w;
            let x1 = ((tx + 1) * w / to_w).max(x0 + 1);
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = ((y * w + x) * TEXEL) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[at + c]);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let at = ((ty * to_w + tx) * TEXEL) as usize;
            for c in 0..4 {
                out[at + c] = (sum[c] / n) as u8;
            }
        }
    }
    out
}

thread_local! {
    /// The pictures the host draws: one sheet per thread, as the glyphs'.
    static IMAGES: RefCell<ImageAtlas> = RefCell::new(ImageAtlas::new());
}

/// Runs `f` with the thread's sheet.
pub fn with<R>(f: impl FnOnce(&mut ImageAtlas) -> R) -> R {
    IMAGES.with(|a| f(&mut a.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture lands where it says, its texels copied; the same key finds
    /// it, and the sheet grows rather than moving what it holds.
    #[test]
    fn a_picture_is_packed_and_found_again() {
        let mut a = ImageAtlas::new();
        let red: Vec<u8> = [255, 0, 0, 255].repeat(3 * 2);
        let placed = a.insert(7, 3, 2, &red).unwrap();
        assert_eq!((placed.width, placed.height), (3, 2));
        let [u0, v0, ..] = placed.uv;
        let at = ((v0 as u32 * SIDE + u0 as u32) * TEXEL) as usize;
        assert_eq!(&a.pixels()[at..at + 4], &[255, 0, 0, 255]);
        assert_eq!(a.get(7), Some(placed));
        let version = a.version();
        let tall = vec![9u8; (10 * 600 * TEXEL) as usize];
        a.insert(8, 10, 600, &tall).unwrap();
        assert!(a.rows() >= 600 && a.version() > version);
        assert_eq!(a.get(7), Some(placed), "growing moves nothing");
    }

    /// A picture wider than the sheet is shrunk to fit it, in proportion.
    #[test]
    fn a_picture_wider_than_the_sheet_is_shrunk() {
        let mut a = ImageAtlas::new();
        let wide = vec![128u8; (4096 * 10 * TEXEL) as usize];
        let placed = a.insert(1, 4096, 10, &wide).unwrap();
        assert!(placed.width <= SIDE - 2 && placed.height == 4, "{placed:?}");
        assert!(a.insert(2, 2, 2, &[0; 3]).is_none(), "not that many bytes");
    }
}
