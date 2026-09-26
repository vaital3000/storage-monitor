use std::path::Path;
use std::process::{Command, Output};

/// The binary with its data dir in `data`, so no test touches the real one — the demo module
/// seeds its sandbox there on its first discovery.
fn run(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_storage-monitor"))
        .env("STORAGE_MONITOR_DATA_DIR", data)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// Tests build in debug, where the registry holds the demo module.

#[test]
fn modules_list_names_the_demo_module_as_available() {
    let data = tempfile::tempdir().unwrap();
    let out = run(data.path(), &["modules", "list"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("demo\tDemo\tavailable\n"), "{text}");

    let out = run(data.path(), &["modules", "list", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let demo = &json.as_array().unwrap()[0];
    assert_eq!(demo["id"], "demo");
    assert_eq!(demo["available"], true);
    assert!(demo["reason"].is_null());
    assert_eq!(demo["tools"], serde_json::json!(["rm", "touch"]));
}

#[test]
fn modules_run_demo_prints_its_items_with_verdicts() {
    let data = tempfile::tempdir().unwrap();
    let out = run(data.path(), &["modules", "run", "demo", "--json"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["module"], "demo");
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(items[0]["title"], "fresh.object", "largest first");
    assert!(
        data.path().join("demo/.seeded").is_file(),
        "seeded in the data dir"
    );

    let out = run(data.path(), &["modules", "run", "demo"]);
    let text = stdout(&out);
    let keepsake = text
        .lines()
        .position(|line| line.ends_with("keepsake"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(text.lines().nth(keepsake).unwrap().starts_with("keep "));
    assert_eq!(
        text.lines().nth(keepsake + 1).unwrap().trim(),
        "It holds a KEEP file"
    );
    assert!(
        text.lines()
            .any(|line| line.contains("~") && line.ends_with("old.object")),
        "an estimated size is marked: {text}"
    );
}

#[test]
fn modules_run_an_unknown_id_fails_with_the_known_ids() {
    let data = tempfile::tempdir().unwrap();
    let out = run(data.path(), &["modules", "run", "docker"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("unknown module docker; this build has: demo"),
        "{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty());
}
