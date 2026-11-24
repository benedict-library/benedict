use serde_json::Value;
use std::{fs, process::Command};
#[test]
fn file_preserves_original_and_rejects_unanswered_or_unrelated_rows() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = temp.path().join("library.json");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_benedict"))
            .args(args)
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_ROOM_SOCKET")
            .env_remove("LIBRARY_ROOM_HOME")
            .env_remove("LIBRARY_EMBED_URL")
            .env("LIBRARY_CONFIG", &cfg)
            .env("LIBRARY_DB", temp.path().join("evidence.sqlite3"))
            .env("LIBRARY_PROJECT", "project")
            .output()
            .unwrap()
    };
    let success = |args: &[&str]| -> Value {
        let out = run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    success(&["init"]);
    success(&["add", "--key", "research"]);
    let q = "Which batch is approved?\nKeep the reason for the next agent.";
    let search = success(&[
        "record",
        "--library",
        "research",
        "--room",
        "search",
        "--question",
        q,
        "--query",
        "batch A7",
    ]);
    let id = search["id"].to_string();
    assert!(
        !run(&["file", "--library", "research", "--id", &id])
            .status
            .success()
    );
    std::fs::create_dir(temp.path().join("wiki-research/shelf")).unwrap();
    std::fs::write(temp.path().join("wiki-research/shelf/A7.md"), "59 units").unwrap();
    let completed = success(&[
        "record",
        "--library",
        "research",
        "--room",
        "meeting",
        "--question",
        q,
        "--query",
        "batch A7",
        "--answer",
        "59 units",
        "--source",
        "shelf/A7.md",
    ]);
    let id = completed["id"].to_string();
    assert!(!run(&["file", "--id", &id]).status.success());
    assert!(
        !run(&[
            "file",
            "--library",
            "research",
            "--id",
            &id,
            "--as",
            "concept"
        ])
        .status
        .success()
    );
    let synthesis = success(&[
        "record",
        "--library",
        "research",
        "--room",
        "librarian",
        "--question",
        "Batch inventory",
        "--answer",
        "Audited count is 59 units.",
        "--source",
        "shelf/A7.md",
    ]);
    let synthesis_id = synthesis["id"].to_string();
    let publish = [
        "file",
        "--library",
        "research",
        "--id",
        &synthesis_id,
        "--as",
        "concept",
    ];
    let concept = success(&publish);
    assert_eq!(success(&publish), concept);
    let hits = success(&["search", "--library", "research", "Audited"]);
    assert_eq!(hits["hits"][0]["path"], concept["path"]);
    assert!(
        success(&[
            "read",
            "--library",
            "research",
            concept["path"].as_str().unwrap()
        ])["text"]
            .as_str()
            .unwrap()
            .contains("shelf/A7.md")
    );
    let receipt = success(&["file", "--library", "research", "--id", &id]);
    assert_eq!(receipt["source_record_id"], completed["id"]);
    let path = temp
        .path()
        .join("wiki-research")
        .join(receipt["path"].as_str().unwrap());
    let page = fs::read_to_string(path).unwrap();
    for text in [
        q,
        "59 units",
        "shelf/A7.md",
        &format!("benedict recall --library research --id {id}"),
    ] {
        assert!(page.contains(text));
    }
    // Filing adds no record; it marks the original as filed and keeps the rest.
    assert!(receipt.get("record_id").is_none());
    let mut original = success(&["recall", "--library", "research", "--id", &id]);
    assert_ne!(original["filed_at"], "");
    assert_eq!(receipt["filed_at"], original["filed_at"]);
    original["filed_at"] = "".into();
    assert_eq!(original, completed);
}
