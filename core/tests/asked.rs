//! A search shows earlier saved answers beside its hits: the same question first, then ones sharing words.
mod common;
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_benedict"));
    command
        .env_clear()
        .env("PATH", "/no-external-programs")
        .env("LIBRARY_CONFIG", home.join("library.json"))
        .env("LIBRARY_DB", home.join("records.sqlite"))
        .env("LIBRARY_ROOM_SOCKET", common::socket(home));
    command
}
fn ok(home: &Path, args: &[&str]) -> Value {
    let out: Output = command(home).args(args).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn search_lists_earlier_answers_beside_the_hits() {
    let home = tempdir().unwrap();
    let home = home.path();
    ok(home, &["init", "--demo"]);
    let _room = common::Room::start(|| command(home));
    ok(home, &["add", "--key", "research"]);
    let question = "When can a fictional product be returned?";
    // Nothing saved yet: the list is there and empty.
    let first = ok(home, &["search", "--question", question, "refund"]);
    assert_eq!(first["asked"], serde_json::json!([]));
    ok(home, &["save", question, "Within 14 days.", "--query", "refund", "--refs", "entities/refund.md"]);
    ok(home, &["save", "--library", "research", question, "Elsewhere: 30 days.", "--query", "refund"]);

    // The same question finds its saved answer first; the ranked hits keep their places.
    let same = ok(home, &["search", "--question", question, "refund"]);
    assert_eq!(same["hits"][0]["path"], "entities/refund.md");
    let asked = same["asked"].as_array().unwrap();
    assert_eq!(asked.len(), 1, "another library's answer stays there: {asked:?}");
    assert_eq!(asked[0]["same"], true);
    assert_eq!(asked[0]["answer"], "Within 14 days.");
    assert_eq!(asked[0]["source"], "entities/refund.md");
    assert_eq!(asked[0]["filed"], false);

    // Spacing and letter case do not make a question different.
    let spaced = ok(home, &["search", "--question", "  when can a FICTIONAL product   be returned? ", "refund"]);
    assert_eq!(spaced["asked"][0]["same"], true);

    // A different question that shares words still sees the answer, marked as not the same.
    let near = ok(home, &["search", "--question", "How long may a fictional product wait for a refund?", "refund window"]);
    assert_eq!(near["asked"][0]["same"], false);
    assert_eq!(near["asked"][0]["answer"], "Within 14 days.");

    // An unrelated question shows none.
    let other = ok(home, &["search", "--question", "Who approved the budget?", "budget"]);
    assert_eq!(other["asked"], serde_json::json!([]));
}

