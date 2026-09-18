use std::fs;
use std::io::Write;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_storage-monitor"))
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, size) in [
        ("a/one.bin", 30_000usize),
        ("a/two.bin", 10_000),
        ("b.bin", 5_000),
    ] {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path)
            .unwrap()
            .write_all(&vec![0u8; size])
            .unwrap();
    }
    dir
}

#[test]
fn scan_json_reports_the_tree_and_stats() {
    let dir = fixture();
    let out = bin()
        .args([
            "scan",
            dir.path().to_str().unwrap(),
            "--json",
            "--depth",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["root"], dir.path().to_str().unwrap());
    assert_eq!(json["stats"]["files"], 3);
    assert_eq!(json["tree"]["fileCount"], 3);
    let children = json["tree"]["children"].as_array().unwrap();
    assert_eq!(children[0]["name"], "a");
    assert_eq!(children[0]["kind"], "dir");
    assert!(
        children[0]["children"].as_array().unwrap().is_empty(),
        "depth 1 stops here"
    );
    assert_eq!(children[0]["truncated"], true);
    assert!(json["disk"]["total"].as_u64().unwrap() > 0);
    assert!(json["snapshot"].is_null());
}

#[test]
fn scan_save_writes_a_snapshot_and_reports_growers_on_the_second_run() {
    let dir = fixture();
    let data = tempfile::tempdir().unwrap();
    let run = |extra: &[&str]| {
        bin()
            .env("STORAGE_MONITOR_DATA_DIR", data.path())
            .args([
                "scan",
                dir.path().to_str().unwrap(),
                "--json",
                "--save",
                "--threshold",
                "1",
            ])
            .args(extra)
            .output()
            .unwrap()
    };
    let first = run(&[]);
    assert!(first.status.success());
    let json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert!(json["snapshot"].as_str().unwrap().ends_with(".snap"));
    assert!(json["topGrowers"].as_array().unwrap().is_empty());

    fs::File::create(dir.path().join("a/three.bin"))
        .unwrap()
        .write_all(&vec![0u8; 40_000])
        .unwrap();
    let second = run(&[]);
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let growers = json["topGrowers"].as_array().unwrap();
    assert!(!growers.is_empty());
    assert_eq!(growers[0]["path"], dir.path().join("a").to_str().unwrap());
    assert_eq!(
        fs::read_dir(data.path().join("snapshots")).unwrap().count(),
        4,
        "two .snap + two .json"
    );
}

#[test]
fn scan_text_output_mentions_the_root_and_a_child() {
    let dir = fixture();
    let out = bin()
        .args(["scan", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(dir.path().to_str().unwrap()));
    assert!(text.contains("a/") || text.contains(" a"), "text: {text}");
}

#[test]
fn scan_of_a_missing_root_fails_with_exit_code_1() {
    let out = bin()
        .args(["scan", "/definitely/missing", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("error"));
}
