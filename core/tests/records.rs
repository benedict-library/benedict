use serde_json::Value;
use std::process::{Command, Output};

fn call(db: &std::path::Path, project: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_doseogwan"))
        .env("LIBRARY_DB", db)
        .env("LIBRARY_PROJECT", project)
        .env_remove("LIBRARY_DB_URL")
        .args(args)
        .output()
        .unwrap()
}

fn value(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn search_then_filing_remain_distinct_and_scoped_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    let question = "원래 질문 '3건인가요?'\n다음 사람에게 근거를 남겨줘.";
    let query = "검사표";
    let first = value(call(
        &db,
        "central",
        &[
            "record",
            "--room",
            "search",
            "--question",
            question,
            "--query",
            query,
        ],
    ));
    assert_eq!(first["answer"], "");
    let answer = value(call(
        &db,
        "central",
        &[
            "record",
            "--room",
            "filing",
            "--question",
            question,
            "--query",
            query,
            "--source",
            "shelf/report.txt",
            "--answer",
            "3건",
        ],
    ));
    let history = value(call(&db, "central", &["recall", "--question", question]));
    assert_eq!(history["records"], serde_json::json!([first, answer]));
    let id = first["id"].to_string();
    assert_eq!(value(call(&db, "central", &["recall", "--id", &id])), first);
    for (project, library) in [("unrelated", "main"), ("central", "unrelated")] {
        assert!(
            !call(&db, project, &["recall", "--library", library, "--id", &id])
                .status
                .success()
        );
    }
}

#[test]
fn query_without_original_does_not_claim_to_be_a_user_utterance() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.sqlite3");
    let record = value(call(
        &db,
        "project",
        &["record", "--room", "search", "--query", "keyword"],
    ));
    assert_eq!(record["question"], "");
    assert_eq!(
        value(call(&db, "project", &["recall", "--question", "keyword"]))["records"],
        serde_json::json!([])
    );
    assert_eq!(
        value(call(&db, "project", &["recall", "--query", "keyword"]))["records"],
        serde_json::json!([record])
    );
}

#[test]
fn fresh_session_finds_prior_work_and_only_its_reading_notes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.sqlite3");
    value(call(
        &db,
        "demo",
        &["record", "--room", "search", "--query", "start"],
    ));
    let open = value(call(
        &db,
        "demo",
        &["work", "open", "Shipment", "Check original"],
    ));
    let id = open["work_id"].to_string();
    assert_eq!(open["recent_work"], serde_json::json!([]));
    let note = value(call(
        &db,
        "demo",
        &["work", "note", &id, "Use gate B", "shelf:policy.md"],
    ));
    value(call(
        &db,
        "demo",
        &[
            "record",
            "--room",
            "meeting",
            "--query",
            &format!("work/{id}"),
            "--answer",
            "different room",
        ],
    ));
    let next = value(call(
        &db,
        "demo",
        &["work", "open", "Prepare notice", "Recover choice"],
    ));
    assert_eq!(next["recent_work"][0]["id"], open["work_id"]);
    let history = value(call(&db, "demo", &["work", "show", &id]));
    assert_eq!(history["initial"]["answer"], "Check original");
    assert_eq!(history["notes"], serde_json::json!([note]));
    assert!(!call(&db, "other", &["work", "show", &id]).status.success());
    assert!(
        !call(&db, "demo", &["work", "show", &id, "--library", "other"])
            .status
            .success()
    );
}
