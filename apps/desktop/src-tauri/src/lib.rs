use storage_monitor_core::{AppInfo, app_info};

/// Returns product name and version to the UI.
#[tauri::command]
fn get_app_info() -> AppInfo {
    app_info()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_app_info])
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
