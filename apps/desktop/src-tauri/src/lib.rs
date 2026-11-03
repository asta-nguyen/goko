pub mod agents;
pub mod bridge;
pub mod commands;
pub mod daemon_client;
pub mod error;
pub mod events;
pub mod git_view;
pub mod store;
pub mod terminal;
pub mod worktrees;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod wiring_tests;

use bridge::Bridge;
use std::sync::Arc;
use tauri::{Emitter, Manager};

/// Every command the webview may call.
fn handler<R: tauri::Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        commands::session_create,
        commands::worktree_list,
        commands::worktree_branches,
        commands::worktree_create,
        commands::worktree_remove,
        commands::git_view_changes,
        commands::git_view_diff,
        commands::git_view_history,
        commands::git_view_tree,
        commands::git_view_file,
        commands::git_view_stashes,
        commands::git_view_stash_diff,
        commands::env_file_candidates,
        commands::env_copy,
        commands::session_stop,
        commands::session_delete,
        commands::session_list,
        commands::terminal_write,
        commands::terminal_resize,
        commands::daemon_status,
        commands::project_add,
        commands::project_list,
        commands::project_remove,
        commands::agent_list,
        commands::session_meta_list,
        commands::settings_get,
        commands::settings_set,
        terminal::terminal_subscribe,
        terminal::terminal_unsubscribe,
    ]
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let home = daemon_client::home_dir()?;
            let bridge = Bridge::new(home.clone());
            app.manage(bridge.clone());
            app.manage(store::Storage::open(&home));
            let app = app.handle().clone();
            events::spawn_pump(
                bridge,
                Arc::new(move |name, payload| {
                    let _ = app.emit(name, payload);
                }),
            );
            Ok(())
        })
        .invoke_handler(handler())
        .run(tauri::generate_context!())
        .expect("error while running goko");
}
