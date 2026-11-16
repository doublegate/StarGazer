//! StarGazer: a terminal browser for your GitHub stars.
//!
//! Module map:
//! - [`cache`]: repo model, on-disk cache (atomic, versioned)
//! - [`github`]: API client (GraphQL with a token, REST anonymously)
//! - [`sync`]: full vs incremental sync orchestration
//! - [`categories`]: rule-based categorization
//! - [`app`]: UI state and input handling (no I/O)
//! - [`ui`]: rendering; [`theme`]: colors and glyphs; [`text`]: width-safe text
//! - [`tui`]: terminal lifecycle and event loop
//! - [`config`], [`export`], [`clipboard`]

pub mod app;
pub mod awesome;
pub mod cache;
pub mod categories;
pub mod clipboard;
pub mod config;
pub mod export;
pub mod github;
pub mod search;
pub mod sync;
pub mod text;
pub mod theme;
pub mod tui;
pub mod ui;
