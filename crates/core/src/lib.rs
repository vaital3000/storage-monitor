//! Core library of Storage Monitor.
//!
//! Phase 0 exposes only application metadata. The scanner, the module
//! contract and the action engine arrive in later phases; see
//! `docs/plans/2026-09-17-storage-monitor-design.md`.

use serde::{Deserialize, Serialize};

pub mod disk;
pub mod paths;
pub mod scan;
pub mod snapshot;

/// Human-readable product name.
pub const APP_NAME: &str = "Storage Monitor";

/// Application metadata shared by the desktop app and the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

/// Returns the product name and the crate version compiled into the binary.
pub fn app_info() -> AppInfo {
    AppInfo {
        name: APP_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_name_and_crate_version() {
        let info = app_info();
        assert_eq!(info.name, "Storage Monitor");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn app_info_serializes_to_json() {
        let json = serde_json::to_value(app_info()).unwrap();
        assert_eq!(json["name"], "Storage Monitor");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }
}
