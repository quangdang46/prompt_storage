//! pst core library — the ONLY entry point frontends use.
//!
//! Architecture rule (plan §3): `commands/*` and `tui/*` contain presentation
//! logic only. Anything that touches the database, resolves an id, or renders
//! a template lives under these core modules so the CLI, TUI, and integration
//! tests all exercise identical code paths.

pub mod argv;
pub mod commands;
pub mod model;
pub mod render;
pub mod storage;
