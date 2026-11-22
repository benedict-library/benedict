//! New Docker-style installations must be able to finish a handoff with only Rust commands.
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;

fn cli(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_doseogwan"))
        .args(args)
        .env_clear()
        .env("PATH", "/no-external-programs")
        .env("LIBRARY_CONFIG", home.join("library.json"))
        .env("LIBRARY_DB", home.join("records.sqlite"))
        .output()
        .unwrap()
}
fn ok(home: &Path, args: &[&str]) -> Value {
    let out = cli(home, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn standalone_handoff_in_new_processes_without_python() {
    let home = tempdir().unwrap();
    let home = home.path();
    let raw = "What was agreed? Keep the original instruction.";
    ok(home, &["init", "--demo"]);
    ok(home, &["add", "--key", "research"]);
    let found = ok(home, &["search", "--question", raw, "refund"]);
    assert_eq!(found["hits"][0]["path"], "entities/refund.md");
    let body = ok(home, &["read", "--question", raw, "entities/refund.md"]);
    assert!(body["text"].as_str().unwrap().contains("14 days"));
    let filed = ok(
        home,
        &[
            "save",
            raw,
            "Within 14 days.",
            "--query",
            "refund",
            "--refs",
            "entities/refund.md",
        ],
    );
    let recalled = ok(home, &["recall", "--question", raw]);
    for (entry, room) in
        recalled["records"]
            .as_array()
            .unwrap()
            .iter()
            .zip(["search", "reading", "librarian"])
    {
        assert_eq!(entry["room"], room);
        assert_eq!(entry["question"], raw);
    }
    assert_eq!(recalled["records"].as_array().unwrap().len(), 3);
    let isolated = ok(
        home,
        &["recall", "--library", "research", "--question", raw],
    );
    assert!(isolated["records"].as_array().unwrap().is_empty());
    let path = filed["path"].as_str().unwrap();
    std::fs::write(
        home.join("wiki/revised.md"),
        "Approved extension to 17 days.",
    )
    .unwrap();
    ok(
        home,
        &[
            "save",
            raw,
            "Approved extension to 17 days.",
            "--refs",
            "revised.md",
        ],
    );
    let updated = ok(home, &["read", path]);
    for evidence in [
        "## 변경 이력",
        "> Within 14 days.",
        "entities/refund.md",
        "Approved extension to 17 days.",
        "revised.md",
    ] {
        assert!(
            updated["text"].as_str().unwrap().contains(evidence),
            "{evidence}"
        );
    }
}

#[test]
fn scoped_originals_are_searchable_readable_and_verifiable() {
    use sha2::{Digest, Sha256};
    use std::{fs, os::unix::fs::symlink};
    let dir = tempdir().unwrap();
    let home = dir.path();
    ok(home, &["init"]);
    ok(home, &["add", "--key", "private"]);
    let config = home.join("library.json");
    let mut registry: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    registry["libraries"]["main"]["shelf_root"] = "originals".into();
    fs::write(&config, registry.to_string()).unwrap();
    let shelf = home.join("originals");
    fs::create_dir_all(shelf.join("survey")).unwrap();
    let raw = "# Field test\r\nViolet markers are visible.\r\n";
    fs::write(shelf.join("survey/field.md"), raw).unwrap();
    fs::write(home.join("wiki-private/private.md"), "Cobalt").unwrap();
    symlink(home.join("wiki-private"), shelf.join("elsewhere")).unwrap();
    symlink(
        home.join("wiki-private/private.md"),
        shelf.join("outside.md"),
    )
    .unwrap();
    ok(home, &["index"]);
    let reference = "shelf:survey/field.md";
    assert_eq!(
        ok(home, &["search", "violet"])["hits"][0]["path"],
        reference
    );
    let original = ok(home, &["read", reference]);
    assert_eq!(original["text"], raw);
    assert_eq!(
        original["sha256"],
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    assert_eq!(
        ok(
            home,
            &["recall", "--id", &original["record_id"].to_string()]
        )["source"],
        reference
    );
    let saved = ok(
        home,
        &["save", "Which markers?", "Violet", "--refs", reference],
    );
    assert!(
        ok(home, &["read", saved["path"].as_str().unwrap()])["text"]
            .as_str()
            .unwrap()
            .contains(reference)
    );
    for bad in [
        "shelf:../wiki-private/private.md",
        "shelf:elsewhere/private.md",
        "shelf:outside.md",
        "shelf:missing.txt",
    ] {
        assert!(!cli(home, &["read", bad]).status.success());
        assert!(
            !cli(home, &["save", "What?", "Unverified", "--refs", bad])
                .status
                .success()
        );
    }
    assert!(
        !cli(
            home,
            &[
                "save",
                "Missing evidence?",
                "Unverified",
                "--refs",
                "entities/missing.md"
            ]
        )
        .status
        .success()
    );
    assert!(
        cli(
            home,
            &[
                "save",
                "External reference?",
                "Unverified locally",
                "--refs",
                "https://example.org/field.md"
            ]
        )
        .status
        .success()
    );
    let denied = cli(home, &["read", "--library", "private", reference]);
    assert!(!denied.status.success());
    assert!(!String::from_utf8_lossy(&denied.stderr).contains("survey"));
    assert!(
        ok(home, &["search", "--library", "private", "violet"])["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
