use std::process::Command;

fn storage_monitor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_storage-monitor"))
}

#[test]
fn info_json_prints_name_and_version() {
    let output = storage_monitor()
        .args(["info", "--json"])
        .output()
        .expect("binary runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(json["name"], "Storage Monitor");
    // The binary bakes in core's version; release-please bumps every crate in lockstep, so cli's own version must match.
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn info_plain_prints_one_line() {
    let output = storage_monitor().arg("info").output().expect("binary runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text,
        format!("Storage Monitor {}\n", env!("CARGO_PKG_VERSION"))
    );
}
