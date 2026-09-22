//! Tauri shell of Storage Monitor: app state, commands and events over the core library.

mod actions;
mod cleanup;
mod commands;
pub mod module_manager;
pub mod scan_manager;
pub mod views;

use std::sync::Arc;

use storage_monitor_core::action::ActionLog;
use storage_monitor_core::system::RealSystem;
use storage_monitor_core::{AppInfo, app_info, paths};
use storage_monitor_modules::registry;
use tauri::Runtime;

use crate::actions::BatchLock;
use crate::module_manager::ModuleManager;
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
            commands::modules_list,
            commands::modules_refresh,
            commands::cleanup_items,
            commands::cleanup_preview,
            commands::cleanup_run,
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
/// The scan manager, the action log and the module manager are parameters so that a test
/// can put the snapshots and the record in a temp directory and hand over a registry of its
/// own. That is **not** a sandbox: [`RealSystem`] is built here for the Explorer's batches,
/// so a command given a real path in a test deletes a real file. What keeps the test safe is
/// what it passes in, and it says so where it passes it.
fn configure<R: Runtime>(
    builder: tauri::Builder<R>,
    manager: ScanManager,
    log: ActionLog,
    modules: ModuleManager,
) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_opener::init())
        .manage(manager)
        .manage(modules)
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
        ModuleManager::new(
            registry(),
            Arc::new(RealSystem),
            paths::home_dir(),
            paths::data_dir(),
        ),
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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use storage_monitor_core::action::{EntryOutcome, EntryResult, Mode, Outcome};
    use storage_monitor_core::scan::NodeKind;

    use crate::scan_manager::{DONE_EVENT, PROGRESS_EVENT};
    use tauri::test::{INVOKE_KEY, MockRuntime, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;
    use tauri::{Listener, WebviewUrl};

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
        // The scan at the end of this test walks this directory and has to find a file in
        // it: `blob.bin` is what makes `done["files"] > 0` true. Deleting it does not fail
        // here, it fails there.
        std::fs::write(dir.path().join("blob.bin"), vec![b'x'; 4_096]).unwrap();
        let log = ActionLog::new(dir.path().join("actions.jsonl"));
        let at = chrono::Utc::now();
        // A line only this log has, so a tail that comes back cannot have been read from
        // the user's own record — which is also what the temp dir is for.
        log.append(&Outcome {
            entries: vec![EntryOutcome {
                path: "/h/sentinel".into(),
                kind: NodeKind::File,
                result: EntryResult::Removed { bytes: 7 },
            }],
            freed_bytes: 7,
            at,
            mode: Mode::Trash,
        })
        .expect("the log is written");

        // An empty registry: the commands over it are registered and resolve their state,
        // and there is nothing a batch could clean.
        let modules = ModuleManager::new(
            Vec::new(),
            Arc::new(RealSystem),
            Some(dir.path().to_path_buf()),
            dir.path().join("data"),
        );
        let app = configure(
            mock_builder(),
            ScanManager::with_snapshots_dir(dir.path().join("snapshots")),
            log,
            modules,
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
                "at": serde_json::to_value(at).unwrap(),
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

        // The cleanup commands, over a registry with nothing in it: the same smallest true
        // answers, and a batch of no requests, which runs nothing and records nothing.
        assert_eq!(ipc.ok("modules_list", json!({})), json!([]));
        assert_eq!(ipc.ok("modules_refresh", json!({})), json!([]));
        assert_eq!(
            ipc.ok("cleanup_items", json!({})),
            json!({ "items": [], "total": 0 })
        );
        let previews = ipc.ok("cleanup_preview", json!({ "requests": [] }));
        assert_eq!(previews["trash"]["mode"], json!("trash"), "{previews}");
        assert_eq!(
            previews["permanent"]["mode"],
            json!("permanent"),
            "{previews}"
        );
        assert_eq!(previews["trash"]["entries"], json!([]), "{previews}");
        let cleaned = ipc.ok("cleanup_run", json!({ "requests": [], "mode": "trash" }));
        assert_eq!(cleaned["outcome"]["entries"], json!([]), "{cleaned}");
        assert_eq!(cleaned["recorded"], json!(true), "{cleaned}");
        assert_eq!(cleaned["treeStale"], json!(false), "{cleaned}");

        // Last, because it is the only one that leaves the app busy: a scan of the temp
        // directory, which also runs `AppHandle<MockRuntime>` as a `StatusEmitter` — the
        // impl this crate made generic, and which nothing else exercises at run time.
        // Recorded as they are emitted: the worker thread calls a Rust listener directly,
        // so nothing here waits on an event loop.
        let events: Arc<Mutex<Vec<(&str, String)>>> = Arc::default();
        for event in [PROGRESS_EVENT, DONE_EVENT] {
            let events = Arc::clone(&events);
            // The payload is recorded, never parsed here: this runs on the scan's worker
            // thread, where a panic would poison a lock `ScanManager::lock` recovers from
            // and leave the poll below to finish normally — a test that passes while its
            // listener died. Parsing happens on the test thread, where failing is failing.
            webview.listen(event, move |received| {
                events
                    .lock()
                    .expect("the recorder")
                    .push((event, received.payload().to_owned()));
            });
        }

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
            "the walk reached the manager: {done}"
        );

        // And the window was told. `run` emits `scan:done` while it holds the manager's
        // lock, and the poll above needed that lock to see `done` at all, so the emit has
        // already returned by now: no sleep, no flake. An emitter whose body does nothing
        // passes every other test in this crate — every manager test carries a recorder of
        // its own — and costs the real window its live progress.
        //
        // That one lock carries the delivery as well as the ordering. Tauri's `emit_filter`
        // takes its handler map with `try_lock` and, on contention, puts the emit in a
        // pending queue that only a later emit *which dispatched a handler* flushes — so a
        // deferred last emit strands, and `scan:done` is the last one. Emitting under the
        // manager lock is what keeps two of ours from contending for that map. An emit
        // added outside that lock breaks the ordering and the delivery together.
        //
        // Parsed here, on the test thread, and copied out of the lock: nothing below needs
        // the recorder, and a lock held across assertions is a hang waiting for the next
        // emit to be added.
        let seen: Vec<(&str, Value)> = events
            .lock()
            .expect("the recorder")
            .iter()
            .map(|(name, payload)| {
                let status = serde_json::from_str(payload)
                    .unwrap_or_else(|err| panic!("{name} carried no status: {err} in {payload}"));
                (*name, status)
            })
            .collect();
        let of = |event| seen.iter().filter(move |(name, _)| *name == event);
        let finished: Vec<&Value> = of(DONE_EVENT).map(|(_, status)| status).collect();
        assert_eq!(
            finished.len(),
            1,
            "one scan:done reached the window: {seen:?}"
        );
        assert_eq!(finished[0]["state"], json!("done"), "{:?}", finished[0]);
        assert_eq!(
            finished[0]["files"], done["files"],
            "and it carried the same walk the command reports"
        );
        // `scan:progress` is not asserted to *arrive*: the ticker sleeps before its first
        // emit and stops as soon as the scan is over, so a scan of a temp directory this
        // small is normally finished first, and demanding one would be a timing race. What
        // every one that does arrive has to be is a running status — the window draws it.
        for (name, status) in of(PROGRESS_EVENT) {
            assert_eq!(status["state"], json!("running"), "{name} {status}");
        }

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
    /// The evidence behind each grant — the measurements, the two positive controls the
    /// policy was verified with, and why `require-trusted-types-for` is left out — belongs
    /// in `docs/adr/0006-content-security-policy.md`. What stays here is the part that
    /// cannot be guessed from the line someone is about to edit:
    ///
    /// - **`style-src`'s `'unsafe-inline'` is for style *attributes*, not for Tailwind**,
    ///   which compiles at build time and injects nothing. `DiskUsageBar`, `NodeTable`'s
    ///   `style={{ width }}` and ECharts' tooltip `cssText` are the consumers, through
    ///   `style-src-attr`, and they break in the packaged app only.
    /// - **`form-action` falls back to nothing.** `default-src` backstops the directives
    ///   that are absent here; this one has no fallback, so removing the line opens a
    ///   `<form action="https://…">` out of an otherwise sealed policy.
    /// - **`img-src` is narrow because the grants it dropped were dead**, measured: no
    ///   `<img>`, no `url(` in the built CSS, no `convertFileSrc`. `data:` and `asset:`
    ///   come back with the feature that needs them, not before.
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

        // The directive names themselves, as a closed set. Every assertion below reads the
        // sources of a directive it names, and a list checked item by item cannot notice an
        // addition — which is the whole reason `every_command!` carries a completeness
        // assertion. Here the addition is the attack: `script-src-elem` does not add to
        // `script-src`, it *replaces* it for `<script>` elements, so one more directive
        // defeats the assertion under it while every line here still passes. Same shape for
        // `style-src-attr` over `style-src`, and `sandbox`, `frame-src` and `worker-src`
        // answer for themselves. A directive nobody decided on fails here, by name.
        let named: BTreeSet<&str> = directives.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            named,
            BTreeSet::from([
                "base-uri",
                "connect-src",
                "default-src",
                "font-src",
                "form-action",
                "frame-ancestors",
                "img-src",
                "object-src",
                "script-src",
                "style-src",
            ]),
            "the policy has a directive nobody decided on, or is missing one: {policy}"
        );

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

        // And every source of every directive, whatever the equalities above allow: a
        // quoted keyword, Tauri's own `ipc:`, or a host under `localhost`. A CSP source
        // needs no scheme to name a host — `evil.com` and `evil.com:443` are valid sources,
        // and a bare `https:` is a valid scheme source — so nothing here may be recognised
        // by looking for `//`, which is what the first version of this did.
        let local = |source: &str| {
            if source.starts_with('\'') && source.ends_with('\'') {
                return true; // 'self', 'none', 'unsafe-inline', a hash, a nonce
            }
            if source == "ipc:" {
                return true; // the other half of Tauri's channel
            }
            let host = source
                .rsplit_once("//")
                .map_or(source, |(_scheme, rest)| rest)
                .split(['/', ':'])
                .next()
                .unwrap_or_default();
            host == "localhost" || host.ends_with(".localhost")
        };
        for (directive, sources) in &directives {
            for source in sources {
                assert!(
                    local(source),
                    "{directive} names something this app does not serve: {source}"
                );
                assert!(
                    !source.contains('*'),
                    "{directive} names a wildcard: {source}"
                );
            }
        }
    }
}
