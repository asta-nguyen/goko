pub mod bridge;
pub mod commands;
pub mod daemon_client;
pub mod error;
pub mod events;
pub mod terminal;

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
        commands::session_stop,
        commands::session_list,
        commands::terminal_write,
        commands::terminal_resize,
        commands::daemon_status,
        terminal::terminal_subscribe,
        terminal::terminal_unsubscribe,
    ]
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let bridge = Bridge::new(daemon_client::home_dir()?);
            app.manage(bridge.clone());
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
