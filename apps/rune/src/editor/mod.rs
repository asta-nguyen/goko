//! The native editor workspace: real registered projects/checkouts, Git
//! sections and file editing, all backed by `app-core` and rune-kit
//! components. Draft guards, stale-result protection and lifecycle handling
//! live in `state`/`draft`/`navigation`/`quit_guard`.

pub mod acp;
pub mod backend;
pub mod blame;
pub mod changes;
pub mod code;
pub mod comment;
pub mod commit;
pub mod commit_detail;
pub mod draft;
pub mod files;
pub mod find;
pub mod go_to_definition;
pub mod highlight;
pub mod history;
pub mod history_graph;
pub mod history_layout;
pub mod indent;
pub mod keyboard;
pub mod lsp;
pub mod media;
pub mod navigation;
pub mod pane_layout;
pub mod patch;
pub mod quick_open;
pub mod quit_guard;
pub mod search;
pub mod stashes;
pub mod state;
pub mod syntax;
pub mod trash;
pub mod workspace;
pub mod worktree_form;
