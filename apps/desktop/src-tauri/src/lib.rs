//! Tauri shell of Storage Monitor: app state, commands and events over the core library.

mod commands;
pub mod scan_manager;
pub mod views;

use storage_monitor_core::{AppInfo, app_info};

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
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            commands::default_root,
            commands::scan_start,
            commands::scan_status,
            commands::scan_cancel,
            commands::tree_node,
            commands::disk_usage,
            commands::top_growers,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn get_app_info_returns_core_metadata() {
        assert_eq!(super::get_app_info(), storage_monitor_core::app_info());
    }
}
