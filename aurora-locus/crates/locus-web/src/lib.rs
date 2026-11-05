//! Server-rendered web UI: the operator console, account portal, OAuth pages
//! and setup wizard. Its design is Horizon-Prism's admin UI: one stylesheet
//! of tokens, a dark sidebar beside the page when signed in, a top bar
//! before, no inline styles or scripts (so a strict CSP stands), and plain
//! sentences.
//!
//! This crate holds the look (assets, theme, templates and their view
//! models); the HTTP handlers that fill them live in `locus-http`.

pub mod assets;
pub mod console;
pub mod console_pages;
pub mod setup;
pub mod theme;
pub mod view;
