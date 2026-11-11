//! The Tauri-free application core shared by the desktop app and native shells:
//! the error type, the `goko.db` store, file/Git inspection and writes, agent
//! and ACP support, GOKO_HOME resolution, and registered-project validation.

pub mod accounts;
pub mod acp;
pub mod activity;
pub mod agents;
pub mod error;
pub mod git_view;
pub mod git_write;
pub mod home;
pub mod lsp;
pub mod projects;
pub mod session_debug;
pub mod store;
pub mod subagents;
#[doc(hidden)]
pub mod test_support;
pub mod usage;
pub mod worktrees;
