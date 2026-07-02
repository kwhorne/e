//! `e-core` — the language- and document-agnostic core of the `e` editor.
//!
//! Inspired by `lapce-core`, but kept intentionally small. This crate owns
//! everything that is independent of the GUI: file IO, language detection,
//! and (later) syntax highlighting.

pub mod buffer;
pub mod diff;
pub mod git;
pub mod language;
pub mod markdown;
pub mod syntax;
