//! Tauri shell of Storage Monitor: app state, commands and events over the core library.

mod actions;
mod commands;
pub mod scan_manager;
pub mod views;

use std::sync::Arc;

use storage_monitor_core::action::ActionLog;
use storage_monitor_core::system::RealSystem;
use storage_monitor_core::{AppInfo, app_info, paths};
use tauri::Runtime;

use crate::actions::BatchLock;
use crate::scan_manager::ScanManager;

/// Returns product name and version to the UI.
#[tauri::command]
fn get_app_info() -> AppInfo {
    app_info()
}

/// The whole app but the runtime: the plugins, the state every command resolves against,
/// and the list of commands the window can invoke.
///
/// [`run`] hands it the real builder; a test hands it [`tauri::test::mock_builder`] and
/// invokes over the same list. That test is the only thing in this project that can catch a
/// command written, exported, reviewed — and never registered here. The Vitest suite and
/// Playwright both answer from `src/mocks/ipc.ts`, so neither of them ever reaches this
/// list, and a command missing from it fails first for a user, as `command not found` in
/// the middle of a deletion.
///
/// The two pieces of state that address the filesystem are parameters rather than built
/// here, so that a test puts them in a temp directory instead of over the user's own
/// snapshots and deletion record.
fn configure<R: Runtime>(
    builder: tauri::Builder<R>,
    manager: ScanManager,
    log: ActionLog,
) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_opener::init())
        .manage(manager)
        // The one door to deleting, the record of everything that went through it, and the
        // queue that keeps two batches from racing for the tree.
        .manage(RealSystem)
        .manage(log)
        .manage(Arc::new(BatchLock::default()))
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
            commands::activity_log,
        ])
}

pub fn run() {
    configure(
        tauri::Builder::default(),
        ScanManager::default(),
        ActionLog::new(paths::actions_log()),
    )
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
    use super::*;

    use serde_json::{Value, json};
    use storage_monitor_core::action::{EntryOutcome, EntryResult, Mode, Outcome};
    use storage_monitor_core::scan::NodeKind;
    use tauri::test::{INVOKE_KEY, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    #[test]
    fn get_app_info_returns_core_metadata() {
        assert_eq!(super::get_app_info(), storage_monitor_core::app_info());
    }

    /// Every command that deletes or reports a deletion, invoked over the IPC of a mock
    /// runtime built from [`configure`] — the same list [`run`] gives the real one.
    ///
    /// What this is for is the step nothing else checks: that the command is *registered*
    /// and that the state it asks for is *managed*. A command absent from the handler comes
    /// back as `command not found`, and one whose state was never managed fails on the
    /// argument; both are invisible to every other test in the project, and both reach a
    /// user in the middle of a deletion. The behaviour behind each command has its own
    /// tests, so every call here is deliberately the emptiest one that still has to answer:
    /// two batches with no paths in them, which delete nothing and write nothing.
    #[test]
    fn every_deletion_command_is_reachable_over_the_ipc() {
        let dir = tempfile::tempdir().unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        // A line only this log has, so a tail that comes back cannot have been read from
        // the user's own record — which is also what the temp dir is for.
        log.append(&Outcome {
            entries: vec![EntryOutcome {
                path: "/h/sentinel".into(),
                kind: NodeKind::File,
                result: EntryResult::Removed { bytes: 7 },
            }],
            freed_bytes: 7,
            at: chrono::Utc::now(),
            mode: Mode::Trash,
        })
        .expect("the log is written");

        let app = configure(
            mock_builder(),
            ScanManager::with_snapshots_dir(dir.path().join("snapshots")),
            log,
        )
        .build(mock_context(noop_assets()))
        .expect("the app builds");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("the webview builds");

        let call = |cmd: &str, args: Value| -> Value {
            tauri::test::get_ipc_response(
                &webview,
                InvokeRequest {
                    cmd: cmd.to_owned(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "tauri://localhost".parse().unwrap(),
                    body: args.into(),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.to_owned(),
                },
            )
            .unwrap_or_else(|err| panic!("{cmd} did not answer: {err}"))
            .deserialize::<Value>()
            .unwrap_or_else(|err| panic!("{cmd} answered with something that is not JSON: {err}"))
        };

        let preview = call("action_preview", json!({ "paths": [], "mode": "trash" }));
        assert_eq!(preview["entries"], json!([]), "{preview}");
        assert_eq!(preview["mode"], json!("trash"), "{preview}");

        let batch = call("action_run", json!({ "paths": [], "mode": "trash" }));
        assert_eq!(batch["outcome"]["entries"], json!([]), "{batch}");
        assert_eq!(batch["recorded"], json!(true), "{batch}");

        let tail = call("activity_log", json!({}));
        assert_eq!(
            tail["entries"][0]["path"],
            json!("/h/sentinel"),
            "the answer comes from the log this app was given: {tail}"
        );
        assert_eq!(tail["damaged"], json!(0), "{tail}");
    }
}
