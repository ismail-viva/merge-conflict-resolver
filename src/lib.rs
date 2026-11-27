//! mcr — a keyboard-first, three-pane Git merge conflict resolver.
//!
//! The crate is layered so that each layer only knows the ones above it:
//!
//! * [`core`] — the merge engine: three-way diff, per-line resolution state,
//!   undo/redo and output rendering. No I/O.
//! * [`git`] — where merge inputs come from and where results go
//!   ([`git::MergeBackend`]), implemented for a git repository and for plain
//!   files.
//! * [`syntax`] — tree-sitter syntax highlighting into theme-independent
//!   tokens.
//! * [`config`] — configuration file, keymap and themes.
//! * [`action`] — the commands a key can trigger.
//! * [`app`] — application state and behaviour, driven by events.
//! * [`ui`] — rendering of the application state.
//! * [`tui`] — the terminal event loop.

pub mod action;
pub mod app;
pub mod config;
pub mod core;
pub mod git;
pub mod syntax;
pub mod tui;
pub mod ui;
