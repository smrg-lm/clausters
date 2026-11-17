//! **What a documentation element holds**: Markdown documents read once into
//! blocks ([`markdown`]), laid out at a width into what is drawn and where a
//! hand lands ([`layout`]), and searched ([`index`]).
//!
//! No window, no device, no wire: a document is tested here as text, its
//! layout as positions.

pub mod index;
pub mod layout;
pub mod markdown;

#[cfg(test)]
mod tests;
