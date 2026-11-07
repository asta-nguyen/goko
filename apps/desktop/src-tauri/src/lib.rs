pub mod activity;
pub mod agents;
pub mod bridge;
pub mod commands;
pub mod daemon_client;
pub mod error;
pub mod events;
pub mod git_view;
pub mod store;
pub mod terminal;
pub mod usage;
pub mod worktrees;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod wiring_tests;

use bridge::Bridge;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
struct ShutdownState {
    started: AtomicBool,
    completed: AtomicBool,
}

fn start_shutdown(app: &tauri::AppHandle, state: Arc<ShutdownState>) {
    if state.started.swap(true, Ordering::AcqRel) || state.completed.load(Ordering::Acquire) {
        return;
    }

    let app = app.clone();
    let bridge = app.state::<Bridge>().inner().clone();
    tauri::async_runtime::spawn(async move {
        match bridge.shutdown().await {
            Ok(()) => {
                state.completed.store(true, Ordering::Release);
                app.exit(0);
            }
            Err(error) => {
                state.started.store(false, Ordering::Release);
                app.dialog()
                    .message(format!(
                        "Goko could not stop its background processes: {error}"
                    ))
                    .title("Goko is still running")
                    .show(|_| {});
            }
        }
    });
}

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
        commands::git_view_branches,
        commands::git_view_file_list,
        commands::git_view_search,
        commands::git_view_tree,
        commands::git_view_file,
        commands::git_view_file_save,
        commands::git_view_stashes,
        commands::git_view_stash_diff,
        commands::env_file_candidates,
        commands::env_copy,
        commands::session_stop,
        commands::session_delete,
        commands::session_list,
        usage::usage_get,
        commands::terminal_write,
        commands::terminal_resize,
        commands::daemon_status,
        commands::project_add,
        commands::project_list,
        commands::project_remove,
        commands::agent_list,
        activity::activity_get,
        commands::session_meta_list,
        commands::settings_get,
        commands::settings_set,
        terminal::terminal_subscribe,
        terminal::terminal_unsubscribe,
        terminal::terminal_ack,
    ]
}

pub fn run() {
    agents::adopt_login_shell_path(&commands::default_shell(std::env::var_os("SHELL")));
    let shutdown = Arc::new(ShutdownState::default());
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let home = daemon_client::home_dir()?;
            let bridge = Bridge::new(home.clone());
            app.manage(bridge.clone());
            app.manage(store::Storage::open(&home));
            let app_handle = app.handle().clone();
            let emit: events::Emit = Arc::new(move |name, payload| {
                let _ = app_handle.emit(name, payload);
            });
            app.manage(activity::spawn_poller(
                bridge.clone(),
                home.clone(),
                Arc::clone(&emit),
            ));
            app.manage(usage::spawn_poller(home, Arc::clone(&emit)));
            events::spawn_pump(bridge, emit);
            Ok(())
        })
        .invoke_handler(handler())
        .build(tauri::generate_context!())
        .expect("error while running goko");
    app.run(move |app, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            api.prevent_close();
            start_shutdown(app, Arc::clone(&shutdown));
        }
        RunEvent::ExitRequested { api, .. } if !shutdown.completed.load(Ordering::Acquire) => {
            api.prevent_exit();
            start_shutdown(app, Arc::clone(&shutdown));
        }
        _ => {}
    });
}
