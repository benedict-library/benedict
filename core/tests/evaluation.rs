mod common;
use serde_json::{Value, json};
use std::{fs, process::Command};
#[test]
fn evaluate_offline_scores_readable_sources_and_writes_no_room_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path();
    let make = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_benedict"));
        command
            .current_dir(base)
            .env("LIBRARY_CONFIG", base.join("library.json"))
            .env_remove("LIBRARY_DB_URL")
            .env("LIBRARY_ROOM_SOCKET", common::socket(base))
            .env("LIBRARY_DB", base.join("evidence.sqlite"));
        command
    };
    let cmd = |args: &[&str]| make().args(args).output().unwrap();
    assert!(cmd(&["init", "--demo"]).status.success());
    let _room = common::Room::start(make);
    let q = "refund";
    fs::write(
        base.join("golden"),
        json!({"q":q,"answer_doc":"refund.md","keys":["Straße|14 days","unused"]}).to_string(),
    )
    .unwrap();
    fs::write(
        base.join("answers"),
        json!({"q":q,"answer":"STRASSE, un used"}).to_string(),
    )
    .unwrap();
    let result = cmd(&[
        "evaluate",
        "--golden",
        "golden",
        "--answers",
        "answers",
        "--out",
        "result.json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value =
        serde_json::from_reader(fs::File::open(base.join("result.json")).unwrap()).unwrap();
    assert_eq!(report["rows"][0]["rank"], 1);
    assert_eq!(report["summary"]["key_coverage_pass"], 1);
    assert_eq!(
        report,
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    );
    fs::write(
        base.join("golden"),
        json!({"q":q,"answer_doc":"missing.md"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        cmd(&["evaluate", "--golden", "golden"]).status.code(),
        Some(1)
    );
    assert_eq!(
        cmd(&["evaluate", "--golden", "golden", "--out", "golden"])
            .status
            .code(),
        Some(2)
    );
    // The database holds the index; evaluating writes no room record.
    let records: i64 = rusqlite::Connection::open(base.join("evidence.sqlite"))
        .unwrap()
        .query_row("SELECT count(*) FROM benedict_room_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(records, 0);
}
