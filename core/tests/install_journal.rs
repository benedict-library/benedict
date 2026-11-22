//! Installation choices survive the gap before the first library exists.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn call(dir: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_doseogwan"))
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("LIBRARY_PROJECT", "research")
        .env("LIBRARY_DB", dir.join("events.sqlite"))
        .env("LIBRARY_CONFIG", dir.join("library.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}
fn success(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
const APPEND: &[&str] = &[
    "setup",
    "journal",
    "append",
    "--project",
    "research",
    "--buffer",
    "pending.json",
];
const REPLAY: &[&str] = &["setup", "journal", "replay"];
#[test]
fn buffered_decisions_are_filed_once_after_parallel_retries() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let pick = json!({"step":"start","wiki_found":true,"choice":"wiki"}).to_string();
    success(call(dir, APPEND, pick.as_bytes()));
    success(call(dir, APPEND, pick.as_bytes()));
    let free=json!({"step":"memory","wiki_found":true,"choice":"other","answer":"256MiB로 시작합니다.","nonsecret":true}).to_string();
    success(call(dir, APPEND, free.as_bytes()));
    assert!(!dir.join("events.sqlite").exists());
    let buf = fs::read(dir.join("pending.json")).unwrap();
    success(call(dir, &["init"], &[]));
    let (a, b) = std::thread::scope(|s| {
        let child = s.spawn(|| success(call(dir, REPLAY, &buf)));
        let first = success(call(dir, REPLAY, &buf));
        (first, child.join().unwrap())
    });
    assert_eq!(a, b);
    let receipts = a["decisions"].as_array().unwrap();
    assert_eq!(receipts.len(), 2);
    let page =
        fs::read_to_string(dir.join("wiki").join(receipts[0]["path"].as_str().unwrap())).unwrap();
    for term in [
        "어떤 자료로 시작할까요?",
        "내 위키 연결하기",
        "예제 자료로 시작하기",
        "기타 (직접 입력)",
        "(추천)",
        "doseogwan recall",
    ] {
        assert!(page.contains(term), "{term}");
    }
    let record = success(call(
        dir,
        &["recall", "--question", "어떤 자료로 시작할까요?"],
        &[],
    ));
    let rows = record["records"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["room"], "librarian");
    assert_eq!(rows[0]["answer"], "내 위키 연결하기");
    let memory = success(call(
        dir,
        &["recall", "--id", &receipts[1]["record_id"].to_string()],
        &[],
    ));
    assert_eq!(memory["answer"], "256MiB로 시작합니다.");
    let mut other: Value = serde_json::from_slice(&buf).unwrap();
    other["project"] = "someone-else".into();
    assert!(
        !call(dir, REPLAY, other.to_string().as_bytes())
            .status
            .success()
    );
}

#[test]
fn rejects_secret_and_unreviewed_free_text_before_creating_buffer() {
    let dir = tempfile::tempdir().unwrap();
    for input in [
        json!({"step":"database","wiki_found":false,"choice":"other","answer":"postgres://db","nonsecret":true}),
        json!({"step":"database","wiki_found":false,"choice":"other","answer":"unreviewed"}),
        json!({"step":"database","wiki_found":false,"choice":"sqlite","password":"should never persist"}),
    ] {
        assert!(
            !call(dir.path(), APPEND, input.to_string().as_bytes())
                .status
                .success()
        );
    }
    assert!(!dir.path().join("pending.json").exists());
}

#[test]
fn a_reviewed_public_url_is_allowed_but_a_credential_in_a_url_is_not() {
    let dir = tempfile::tempdir().unwrap();
    for answer in [
        "https://user:example@example.org/guide",
        "https://example.org/guide?sig=example",
    ] {
        let v = json!({"step":"start","wiki_found":false,"choice":"other","answer":answer,"nonsecret":true});
        assert!(
            !call(dir.path(), APPEND, v.to_string().as_bytes())
                .status
                .success()
        );
    }
    let v = json!({"step":"start","wiki_found":false,"choice":"other","answer":"https://example.org/guide","nonsecret":true});
    success(call(dir.path(), APPEND, v.to_string().as_bytes()));
}
