// A Korean word carries its particle or ending in the text ("반품하려면", "반품은"), so a search for the
// bare word ("반품") must still find the page. A search that finds nothing says why.
mod common;
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn cmd(data: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_benedict"));
    cmd.current_dir(data)
        .env_remove("LIBRARY_DB_URL")
        .env_remove("LIBRARY_DB_URL_FILE")
        .env_remove("LIBRARY_DB")
        .env_remove("LIBRARY_ROOM_HOME")
        .env_remove("LIBRARY_CONFIG")
        .env("LIBRARY_ROOM_SOCKET", common::socket(data));
    cmd
}

fn run(data: &Path, args: &[&str]) -> Value {
    let out = cmd(data).args(args).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn paths(result: &Value) -> Vec<String> {
    result["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_bare_korean_word_finds_the_same_word_with_its_ending() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    run(data, &["init"]);
    fs::create_dir_all(data.join("wiki/entities")).unwrap();
    fs::write(
        data.join("wiki/entities/guide.md"),
        "# 고객 안내\n\n제품을 반품하려면 받은 날부터 14일 안에 신청해야 합니다.\n",
    )
    .unwrap();
    fs::write(data.join("wiki/entities/shipping.md"), "# 배송\n\n배송은 사흘 걸립니다.\n").unwrap();
    run(data, &["index"]);
    let _room = common::Room::start(|| cmd(data));
    for query in ["반품", "반품 기한", "신청", "반품하려고"] {
        assert_eq!(paths(&run(data, &["search", query])), ["entities/guide.md"], "{query}");
    }
    // A one-syllable word ("안" in "안에", "안내") and English words still match only whole words.
    assert!(paths(&run(data, &["search", "안"])).is_empty());
    assert!(paths(&run(data, &["search", "return"])).is_empty());
}

#[test]
fn the_readme_questions_find_the_example_policy() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    run(data, &["init", "--demo"]);
    run(data, &["index"]);
    let _room = common::Room::start(|| cmd(data));
    // "return" matches "returned"; "반품" matches "반품할".
    for query in ["return window", "반품 기한"] {
        let found = run(data, &["search", query]);
        assert_eq!(paths(&found).first().map(String::as_str), Some("entities/refund.md"), "{query}");
        assert!(found.get("hint").is_none(), "{query}");
    }
    // Short words still match only whole words.
    assert!(!paths(&run(data, &["search", "day"])).contains(&"entities/refund.md".to_string()));
}

#[test]
fn a_search_that_finds_nothing_says_why() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    run(data, &["init"]);
    fs::create_dir_all(data.join("wiki/entities")).unwrap();
    fs::write(data.join("wiki/entities/policy.md"), "# Policy\n\nParcels leave every Monday.\n").unwrap();
    run(data, &["index"]);
    let _room = common::Room::start(|| cmd(data));
    let korean = run(data, &["search", "반품 기한"]);
    assert!(paths(&korean).is_empty());
    let hint = korean["hint"].as_str().unwrap();
    assert!(hint.contains("no Korean"), "{hint}");
    let none = run(data, &["search", "zebra"]);
    assert!(paths(&none).is_empty());
    assert!(none["hint"].as_str().unwrap().contains("words the documents use"));
    assert!(run(data, &["search", "parcels"]).get("hint").is_none());
}
