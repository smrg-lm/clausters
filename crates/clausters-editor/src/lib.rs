//! **The code editor, drawn by the GUI host**: the application (`app`), a
//! client of the host that decides what its panels show and what a verb
//! does, through the protocol any script speaks, and what it asks of the
//! program it runs in (`platform`).
//!
//! It runs in two: the desktop's binary links the host and runs the
//! session as a Python process (`native`); a page runs it beside the page's
//! host, with the session JavaScript in a worker (`page`). The application
//! is the same in both.

pub mod app;
pub mod arrangement;
pub mod config;
pub mod docs;
pub mod filedialog;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(target_arch = "wasm32")]
pub mod page;
pub mod platform;
pub mod settings;
pub mod store;
pub mod theme;
pub mod wire;
