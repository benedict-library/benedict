use serde_json::Value;
use std::{fs, process::Command};
#[test]
fn librarian_reindexes_scores_and_records_each_real_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_benedict"))
            .current_dir(base)
            .env("LIBRARY_CONFIG", base.join("library.json"))
            .env("LIBRARY_DB", base.join("evidence.sqlite"))
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_ROOM_SOCKET")
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run(&["init", "--demo"]).status.success());
    fs::write(
        base.join("wiki/entities/current.md"),
        "# Today's dispatch\nXylo crate from aisle seven.",
    )
    .unwrap();
    fs::write(
        base.join("golden"),
        r#"{"q":"Xylo","answer_doc":"entities/current.md"}"#,
    )
    .unwrap();
    let out = run(&[
        "librarian",
        "--golden",
        "golden",
        "--iterations",
        "2",
        "--interval",
        "0.001",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["steps"]["golden"]["summary"]["hit"], 1);
    assert!(rows[1]["steps"].get("backup").is_none());
    for row in &rows {
        let receipt = run(&["recall", "--id", &row["record_id"].to_string()]);
        let stored: Value = serde_json::from_slice(&receipt.stdout).unwrap();
        assert_eq!(stored["room"], "librarian");
        let actual: Value = serde_json::from_str(stored["answer"].as_str().unwrap()).unwrap();
        assert_eq!(actual["status"], "ok");
        assert_eq!(actual["ended"], row["ended"]);
    }
    let warning = run(&["librarian"]);
    assert_eq!(warning.status.code(), Some(1));
    let failed = run(&["librarian", "--golden", "absent", "--iterations", "2"]);
    assert_eq!(failed.status.code(), Some(2));
    assert_eq!(String::from_utf8(failed.stdout).unwrap().lines().count(), 1);
    let backup = run(&[
        "librarian",
        "--golden",
        "golden",
        "--backup-dir",
        "snapshots",
    ]);
    assert!(
        backup.status.success(),
        "{}",
        String::from_utf8_lossy(&backup.stdout)
    );
    assert_eq!(fs::read_dir(base.join("snapshots")).unwrap().count(), 1);
}
