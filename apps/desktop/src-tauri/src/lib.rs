//! Tauri shell of Storage Monitor: app state, commands and events over the core library.

mod actions;
mod commands;
pub mod scan_manager;
pub mod views;

use storage_monitor_core::action::ActionLog;
use storage_monitor_core::system::RealSystem;
use storage_monitor_core::{AppInfo, app_info, paths};

use crate::scan_manager::ScanManager;

/// Returns product name and version to the UI.
#[tauri::command]
fn get_app_info() -> AppInfo {
    app_info()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(ScanManager::default())
        // The one door to deleting, and the record of everything that went through it.
        .manage(RealSystem)
        .manage(ActionLog::new(paths::actions_log()))
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            commands::default_root,
            commands::scan_start,
            commands::scan_status,
            commands::scan_cancel,
            commands::tree_node,
            commands::disk_usage,
            commands::top_growers,
            commands::action_preview,
            commands::action_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Whether the tests are running as root, for which permission bits decide nothing: a
/// directory staged as unreadable is read anyway, and one staged as undeletable is deleted.
/// A test that needs either says so and stops, exactly as `crates/core/tests/walker.rs` does.
/// Here rather than in one of the test modules, because two of them need it.
#[cfg(test)]
pub(crate) fn running_as_root() -> bool {
    unsafe extern "C" {
        #[link_name = "geteuid"]
        fn geteuid() -> u32;
    }
    let root = unsafe { geteuid() } == 0;
    if root {
        eprintln!("skipped: running as root");
    }
    root
}

#[cfg(test)]
mod tests {
    #[test]
    fn get_app_info_returns_core_metadata() {
        assert_eq!(super::get_app_info(), storage_monitor_core::app_info());
    }
}
