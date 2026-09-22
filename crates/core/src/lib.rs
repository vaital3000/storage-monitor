//! Core library of Storage Monitor.
//!
//! Everything the desktop app and the CLI share: the parallel directory scanner and its
//! arena tree ([`scan`]), persisted snapshots and the growth deltas between them
//! ([`snapshot`]), volume usage ([`disk`]) and the application's data locations
//! ([`paths`]). Deleting, running programs and the clock go through the [`system`] port,
//! so the code above it can be tested without touching the real machine, and [`action`]
//! holds what a deletion is and the rules that decide what may be touched at all.
//! [`module`] is the contract a cleanup module implements: it finds and judges items, and
//! plans the steps that remove them, which the core alone executes (ADR 0008).

use serde::{Deserialize, Serialize};

pub mod action;
pub mod cleanup;
pub mod disk;
pub mod module;
pub mod paths;
pub mod scan;
pub mod snapshot;
pub mod system;

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
