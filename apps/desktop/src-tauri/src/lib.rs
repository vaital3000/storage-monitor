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

/// Every command the window can invoke, written once and expanded twice: by
/// `generate_handler!` in [`configure`], and by the test that calls each of them over the
/// IPC. The macro to expand with arrives as token trees rather than as a `path`, because a
/// captured `path` is an opaque fragment and cannot be invoked. A command joins both sides
/// in one edit, or neither.
macro_rules! every_command {
    ($($expand:tt)*) => {
        $($expand)*![
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
        ]
    };
}

/// The whole app but the runtime: the plugins, the state every command resolves against,
/// and — through [`every_command`] — the commands themselves.
///
/// [`run`] hands it the real builder; a test hands it `tauri::test::mock_builder()` and
/// invokes over the same list. That test is the only thing in this project that can catch a
/// command written, exported, reviewed — and never registered. The Vitest suite and
/// Playwright both answer from `src/mocks/ipc.ts`, so neither of them ever reaches this
/// list, and a command missing from it fails first for a user, as `command not found` in
/// the middle of a deletion. Because the handler and the test expand one macro, that test
/// also refuses to pass while any command has no expectation of its own.
///
/// The scan manager and the action log are parameters so that a test can put the snapshots
/// and the record in a temp directory. That is **not** a sandbox: [`RealSystem`] is built
/// here, so a command given a real path in a test deletes a real file. What keeps the test
/// safe is what it passes in, and it says so where it passes it.
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
        .invoke_handler(every_command!(tauri::generate_handler))
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

    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use storage_monitor_core::action::{EntryOutcome, EntryResult, Mode, Outcome};
    use storage_monitor_core::scan::NodeKind;
    use tauri::WebviewUrl;
    use tauri::test::{INVOKE_KEY, MockRuntime, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    #[test]
    fn get_app_info_returns_core_metadata() {
        assert_eq!(super::get_app_info(), storage_monitor_core::app_info());
    }

    /// The names in [`every_command`], as the IPC spells them: the last segment of each
    /// path, which is the command name Tauri registers.
    macro_rules! command_names {
        ($($path:path),* $(,)?) => {
            [$(stringify!($path)),*]
        };
    }

    /// `commands :: default_root` — however the token stream was spelled — as
    /// `default_root`.
    fn command_name(path: &str) -> &str {
        path.rsplit("::")
            .next()
            .expect("a path has a last segment")
            .trim()
    }

    /// One mock window, and a record of which commands were called through it.
    struct Ipc<'a> {
        webview: &'a tauri::WebviewWindow<MockRuntime>,
        called: RefCell<BTreeSet<&'static str>>,
    }

    impl Ipc<'_> {
        fn call(&self, cmd: &'static str, args: Value) -> Result<Value, Value> {
            self.called.borrow_mut().insert(cmd);
            tauri::test::get_ipc_response(
                self.webview,
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
            .map(|body| {
                body.deserialize::<Value>()
                    .unwrap_or_else(|err| panic!("{cmd} answered with something unreadable: {err}"))
            })
        }

        /// The answer of a command that has to succeed. An unregistered command fails here,
        /// and so does one whose state was never managed.
        fn ok(&self, cmd: &'static str, args: Value) -> Value {
            self.call(cmd, args)
                .unwrap_or_else(|err| panic!("{cmd} did not answer: {err}"))
        }

        /// The message of a command that has to refuse. Only our own messages are asserted
        /// on: Tauri's wording for an unregistered command is not a contract, and a test
        /// that reads it passes vacuously the day it changes.
        fn err(&self, cmd: &'static str, args: Value) -> String {
            match self.call(cmd, args) {
                Err(Value::String(message)) => message,
                other => panic!("{cmd} was supposed to refuse, got {other:?}"),
            }
        }
    }

    /// Every command of [`every_command`], invoked over the IPC of a mock runtime built
    /// from [`configure`] — the same list [`run`] gives the real one.
    ///
    /// What this is for is the step nothing else checks: that a command is *registered* and
    /// that the state it asks for is *managed*. Both are invisible to every other test in
    /// the project — Vitest and Playwright answer from `src/mocks/ipc.ts` — and both reach
    /// a user in the middle of a deletion. The behaviour behind each command has its own
    /// tests, so every expectation here is the smallest true thing that a reachable command
    /// answers, and every one of them is *ours*: a value, or a message this crate wrote.
    ///
    /// The last assertion is the one that keeps it honest: a command in the list with no
    /// expectation here fails the test.
    ///
    /// **Whoever adds a case here:** the path lists stay empty, and any path that is not
    /// under [`tempfile::TempDir`] stays out. [`configure`] builds a real [`RealSystem`],
    /// so `action_run` over a real path deletes a real file — a test that proves "deletion
    /// works over the IPC" by deleting something must build its own tree first, and the
    /// scan root here is a temp directory for the same reason.
    #[test]
    fn every_command_answers_over_the_ipc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blob.bin"), vec![b'x'; 4_096]).unwrap();
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
        // `main` is the label `capabilities/default.json` grants its permissions to
        // (`"windows": ["main"]`), so a command that goes through the ACL behaves here as
        // it does in the packaged app rather than in a window nothing was granted to.
        let webview =
            tauri::WebviewWindowBuilder::new(&app, "main", WebviewUrl::App("index.html".into()))
                .build()
                .expect("the webview builds");
        let ipc = Ipc {
            webview: &webview,
            called: RefCell::new(BTreeSet::new()),
        };

        assert_eq!(
            ipc.ok("get_app_info", json!({})),
            serde_json::to_value(app_info()).unwrap()
        );
        assert_eq!(
            ipc.ok("default_root", json!({}))
                .as_str()
                .map(PathBuf::from),
            paths::home_dir()
        );

        // Before any scan: the manager is idle and has no tree to answer from.
        assert_eq!(ipc.ok("scan_status", json!({}))["state"], json!("idle"));
        assert_eq!(ipc.ok("scan_cancel", json!({}))["state"], json!("idle"));
        assert_eq!(ipc.err("tree_node", json!({})), "no scan result");
        assert_eq!(ipc.ok("top_growers", json!({})), json!([]));
        let usage = ipc.ok("disk_usage", json!({ "path": dir.path() }));
        assert!(
            usage["total"].as_u64().is_some_and(|total| total > 0),
            "the volume under the temp dir has a size: {usage}"
        );

        // Two batches with nothing in them: they delete nothing and write nothing, which
        // is the whole reason they are safe to run here.
        let preview = ipc.ok("action_preview", json!({ "paths": [], "mode": "trash" }));
        assert_eq!(preview["entries"], json!([]), "{preview}");
        assert_eq!(preview["mode"], json!("trash"), "{preview}");
        let batch = ipc.ok("action_run", json!({ "paths": [], "mode": "trash" }));
        assert_eq!(batch["outcome"]["entries"], json!([]), "{batch}");
        assert_eq!(batch["recorded"], json!(true), "{batch}");

        let tail = ipc.ok("activity_log", json!({}));
        assert_eq!(
            tail["entries"],
            json!([{
                "at": tail["entries"][0]["at"],
                "path": "/h/sentinel",
                "kind": "file",
                "mode": "trash",
                "result": "removed",
                "detail": null,
                "bytes": 7,
            }]),
            "exactly the line this app's log was given, and only it: the empty batch above \
             wrote nothing, which is also why it cannot prove that `action_run` and \
             `activity_log` share one `ActionLog` — that proof is `commands.rs`, one layer \
             down, where a batch with entries in it is written and read back"
        );
        assert_eq!(tail["damaged"], json!(0), "{tail}");

        // Last, because it is the only one that leaves the app busy: a scan of the temp
        // directory, which also runs `AppHandle<MockRuntime>` as a `StatusEmitter` — the
        // impl this crate made generic, and which nothing else exercises at run time.
        let started = ipc.ok("scan_start", json!({ "root": dir.path() }));
        assert_eq!(started["root"], json!(dir.path()), "{started}");
        assert_eq!(started["state"], json!("running"), "{started}");
        let deadline = Instant::now() + Duration::from_secs(10);
        let done = loop {
            let status = ipc.ok("scan_status", json!({}));
            if status["state"] != json!("running") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the scan did not finish: {status}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(done["state"], json!("done"), "{done}");
        assert!(
            done["files"].as_u64().is_some_and(|files| files > 0),
            "the emitter carried the walk back: {done}"
        );

        let listed: BTreeSet<&str> = every_command!(command_names)
            .into_iter()
            .map(command_name)
            .collect();
        assert_eq!(
            ipc.called.into_inner(),
            listed,
            "every command in `every_command!` is called here, and nothing else is"
        );
    }

    /// The policy in `tauri.conf.json`, held to the shape that makes it worth having.
    ///
    /// This test is the only thing that fails when the policy is loosened or deleted.
    /// `just dev` does not apply it — Tauri attaches the header in the `tauri://localhost`
    /// handler, and a `devUrl` document never passes through it — CI builds the app but
    /// never launches it, and Vitest and Playwright run in a plain browser. Without this,
    /// removing the `csp` line leaves every gate green.
    ///
    /// The reasons the grants are what they are live here too, because JSON has nowhere to
    /// write them, and this is the test that fails under the hand that would change them:
    ///
    /// - **`script-src` takes neither `'unsafe-eval'` nor `'unsafe-inline'`.** That is the
    ///   whole point of the policy. Nothing in the bundle needs either: Tauri itself adds
    ///   the `sha256-` hash of the one inline script in the built `index.html`.
    /// - **`style-src` keeps `'unsafe-inline'`, and not because of Tailwind.** Tailwind v4
    ///   compiles at build time into a static stylesheet; measured, `dist/index.html` links
    ///   it and injects nothing. The real consumers are inline style *attributes* —
    ///   `DiskUsageBar.tsx`, `NodeTable.tsx` (`style={{ width }}`) and the one `cssText`
    ///   ECharts writes for its tooltip — which `style-src-attr` inherits from `style-src`.
    ///   Dropping the grant breaks the disk bar and the tooltip **in the packaged app
    ///   only**, where nothing but a run of the real thing would notice. What makes the
    ///   grant tolerable is the company it keeps: with no remote origin in `img-src`, an
    ///   injected `url()` has nowhere to beacon to, and `font-src 'self'` closes the same
    ///   door for `@font-face`.
    /// - **`img-src 'self'` alone.** The `data:`, `asset:` and `http://asset.localhost`
    ///   grants this policy was drafted with were for nothing: the app has no `<img>`, the
    ///   built CSS has no `url(`, and nothing calls `convertFileSrc`. They come back with
    ///   the feature that needs them, deliberately, rather than standing open for it.
    /// - **`object-src 'none'` and `form-action 'none'`.** `default-src` backstops
    ///   `frame-src`, `worker-src`, `media-src`, `manifest-src` and `child-src`, but
    ///   `form-action` falls back to nothing at all, and a `<form action="https://…">` with
    ///   a synthetic submit is the way out of a policy that is otherwise sealed.
    /// - **`connect-src` names no remote origin.** `ipc:` and `http://ipc.localhost` are
    ///   Tauri's own channel on macOS; nothing else may be dialled.
    /// - **`require-trusted-types-for` is left out on purpose, not forgotten.** ECharts'
    ///   tooltip writes `innerHTML`, so adopting it needs that audit first, and WebKit
    ///   ignores it on most of the macOS versions this ships to.
    #[test]
    fn the_content_security_policy_stays_closed() {
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("tauri.conf.json is JSON");
        let policy = config["app"]["security"]["csp"]
            .as_str()
            .expect("a policy, not null: the window ships with one");
        let directives: Vec<(&str, Vec<&str>)> = policy
            .split(';')
            .filter_map(|directive| {
                let mut words = directive.split_whitespace();
                Some((words.next()?, words.collect()))
            })
            .collect();
        let sources = |name: &str| {
            directives
                .iter()
                .find(|(directive, _)| *directive == name)
                .unwrap_or_else(|| panic!("no {name} in {policy}"))
                .1
                .clone()
        };

        assert_eq!(sources("default-src"), ["'self'"], "the backstop");
        assert_eq!(sources("script-src"), ["'self'"]);
        assert_eq!(sources("object-src"), ["'none'"]);
        assert_eq!(sources("form-action"), ["'none'"]);
        assert_eq!(sources("frame-ancestors"), ["'none'"]);
        assert_eq!(sources("base-uri"), ["'self'"]);
        assert_eq!(sources("img-src"), ["'self'"]);
        assert_eq!(sources("font-src"), ["'self'"]);
        assert_eq!(sources("style-src"), ["'self'", "'unsafe-inline'"]);
        assert_eq!(
            sources("connect-src"),
            ["'self'", "ipc:", "http://ipc.localhost"]
        );

        // Not only in the directives named above: a source that names a host this app does
        // not serve is a way out of the policy wherever it is written.
        for (directive, sources) in &directives {
            for source in sources {
                let host = source.split("//").nth(1).unwrap_or_default();
                assert!(
                    host.is_empty() || host == "localhost" || host.ends_with(".localhost"),
                    "{directive} names a remote origin: {source}"
                );
                assert!(
                    !source.contains('*'),
                    "{directive} names a wildcard: {source}"
                );
            }
        }
    }
}
