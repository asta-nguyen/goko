//! The typed error every command returns to the caller as `{code, message, hint}`.
//! Implemented in `app-core` so native frontends share it without Tauri.

pub use app_core::error::*;
