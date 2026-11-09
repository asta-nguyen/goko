//! The app-owned SQLite store: registered projects, session metadata, and settings.
//! Implemented in `app-core` so native frontends share `goko.db` without Tauri.

pub use app_core::store::*;
