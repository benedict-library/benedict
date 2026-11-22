use super::*;

#[test]
fn independent_libraries_and_concurrent_registration() {
    let l = Library::empty();
    l.reject(&["search", "word"]);
    l.reject(&["init", "--key", "../invalid"]);
    assert!(!l.path("library.json").exists());
    let made = l.ok(&["init", "--key", "central", "--name", "서가", "--demo"]);
    assert_eq!(made["library"], "central");
    let original = l.read("library.json");
    let row = &l.registry()["libraries"]["central"];
    assert_eq!(row["name"], "서가");
    assert_eq!(row["central"], true);
    assert_eq!(row["wiki_root"], "wiki");
    assert_eq!(l.ok(&["index"])["library"], "central");
    l.reject(&["init", "--demo"]);
    l.reject(&["add", "--key", "central"]);
    l.reject(&["add", "--key", "../other"]);
    l.put("wiki-occupied/note.md", "Preserve this");
    l.reject(&["add", "--key", "occupied"]);
    assert_eq!(l.read("wiki-occupied/note.md"), "Preserve this");
    assert_eq!(l.read("library.json"), original);
    std::thread::scope(|s| {
        for name in ["estuary", "plants"] {
            let lib = &l;
            s.spawn(move || assert_eq!(lib.ok(&["add", "--key", name])["created"], true));
        }
    });
    assert_eq!(l.ok(&["add", "--key", "estuary"])["created"], false);
    l.reject(&["add", "--key", "estuary", "--name", "Different library"]);
    let saved = l.ok(&[
        "save",
        "--library",
        "estuary",
        "Which colour?",
        "Violet was distinguishable in the photographs.",
    ]);
    let near = l.ok(&["search", "violet"]);
    let nearby = near["near"].as_array().unwrap();
    assert_eq!(nearby.len(), 2);
    assert_eq!(
        nearby.iter().find(|r| r["library"] == "plants").unwrap()["hits"],
        json!([])
    );
    assert_eq!(
        nearby.iter().find(|r| r["library"] == "estuary").unwrap()["hits"][0]["path"],
        saved["path"]
    );
    let path = field(&saved["path"]);
    assert!(field(&l.ok(&["read", "--library", "estuary", path])["text"]).contains("photographs"));
    assert_eq!(
        l.ok(&["search", "--library", "estuary", "refund"])["hits"],
        json!([])
    );
    assert_eq!(
        l.ok(&["search", "refund"])["hits"][0]["path"],
        "entities/refund.md"
    );
    assert!(field(&l.ok(&["read", "entities/refund.md"])["text"]).contains("14 days"));
    l.reject(&["search", "--library", "missing", "word"]);
    let revision = l.ok(&[
        "save",
        "--library",
        "estuary",
        "Which colour?",
        "White is easier to see at night.",
        "--refs",
        "night.md",
    ]);
    assert_eq!(revision["path"], path);
    let history = l.ok(&["search", "violet"]);
    assert!(
        history["near"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["library"] == "estuary" && entry["hits"][0]["path"] == path)
    );
    let revised = l.ok(&["read", "--library", "estuary", path]);
    for evidence in ["Violet", "White", "night.md"] {
        assert!(field(&revised["text"]).contains(evidence));
    }
}

#[test]
fn indexes_follow_the_registered_documents_and_report_missing_state() {
    let blank = Library::empty();
    blank.ok(&["init"]);
    assert_eq!(blank.ok(&["index"])["documents"], 0);
    assert_eq!(blank.ok(&["search", "nothing"])["hits"], json!([]));
    let existing = Library::empty();
    existing.put("wiki/existing.md", "keep");
    existing.reject(&["init", "--demo"]);
    assert!(!existing.path("library.json").exists());
    let l = Library::demo();
    l.put("wiki/entities/refund.md", "조사 우산");
    assert!(
        !l.ok(&["search", "refund"])["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    l.ok(&["index"]);
    assert_eq!(l.ok(&["search", "refund"])["hits"], json!([]));
    assert_eq!(
        l.ok(&["search", "우산"])["hits"][0]["path"],
        "entities/refund.md"
    );
    fs::remove_file(l.path("wiki/entities/refund.md")).unwrap();
    l.ok(&["index"]);
    assert_eq!(l.ok(&["search", "우산"])["hits"], json!([]));
    l.put("wiki/notes/fern.md", "Fern sample");
    l.ok(&["index"]);
    assert_eq!(
        l.ok(&["search", "fern"])["hits"][0]["path"],
        "notes/fern.md"
    );
    l.choose(json!({"dirs":["entities"]}));
    l.ok(&["index"]);
    assert_eq!(l.ok(&["search", "fern"])["hits"], json!([]));
    for invalid in [
        json!(["../outside"]),
        json!(["/absolute"]),
        json!("entities"),
        json!([3]),
    ] {
        l.choose(json!({"dirs":invalid}));
        l.reject(&["index"]);
    }
    l.choose(json!({"dirs":["entities"]}));
    l.put("second/wiki/unique.md", "Apricot");
    l.put(
        "second.json",
        json!({"version":1,"libraries":{"main":{"wiki_root":"second/wiki","central":true}}})
            .to_string(),
    );
    checked(
        l.py("library.py")
            .args(["index", "--config", "second.json"])
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(
        l.ok(&["search", "shipping"])["hits"][0]["path"],
        "entities/shipping.md"
    );
    for path in files(&l.path(".library-state")) {
        if path.to_string_lossy().ends_with("-index.json") {
            fs::write(path, "[]").unwrap();
        }
    }
    assert!(l.reject(&["search", "shipping"]).contains("rebuild"));
    for path in files(&l.path(".library-state")) {
        if path.to_string_lossy().ends_with("-index.json") {
            fs::remove_file(path).unwrap();
        }
    }
    assert!(l.reject(&["search", "shipping"]).contains("index"));
    l.ok(&["index"]);
    assert!(l.ok(&["search", "shipping"])["hits"][0].is_object());
}

#[test]
fn registry_shape_scope_and_read_directions() {
    let l = Library::demo();
    let mut reg = l.registry();
    let original = reg.clone();
    for invalid in [
        json!([]),
        Value::Null,
        json!({"version":1,"libraries":[]}),
        json!({"version":2,"libraries":{}}),
    ] {
        l.registry_set(&invalid);
        assert!(l.reject(&["index"]).contains("error"));
    }
    for opts in [
        json!({"save_block":[]}),
        json!({"save_block":{"x":7}}),
        json!({"save_block":{"x":"["}}),
        json!({"status":"typo"}),
        json!({"central":"true"}),
    ] {
        l.registry_set(&original);
        l.choose(opts);
        let error = l.reject(&["index"]);
        assert!(error.contains("error") && !error.contains("Traceback"));
    }
    l.put("other/note.md", "Isolated magnolia");
    reg["libraries"]["other"] = json!({"wiki_root":"other"});
    l.registry_set(&reg);
    l.ok(&["index", "--library", "other"]);
    let result = l.ok(&["search", "magnolia"]);
    assert!(result.get("near").is_none());
    assert_eq!(result["hits"], json!([]));
    assert_eq!(
        l.ok(&["search", "--library", "other", "magnolia"])["hits"][0]["path"],
        "note.md"
    );
    l.reject(&["read", "../other/note.md"]);
    l.choose(json!({"reads":["other"]}));
    assert_eq!(
        l.ok(&["search", "magnolia"])["near"][0]["hits"][0]["path"],
        "note.md"
    );
    assert!(
        l.ok(&["search", "--library", "other", "refund"])
            .get("near")
            .is_none()
    );
    for value in [json!(["ghost"]), json!(["main"]), json!("other")] {
        l.choose(json!({"reads":value}));
        l.reject(&["index"]);
    }
    reg["libraries"]["main"]["reads"] = json!(["other"]);
    reg["libraries"]["other"]["reads"] = json!(["main"]);
    l.registry_set(&reg);
    l.reject(&["index"]);
    reg["libraries"]["main"]["reads"] = json!([]);
    l.registry_set(&reg);
    l.reject(&["index"]);
    reg["libraries"]["main"]["reads"] = json!(["other"]);
    reg["libraries"]["other"] = json!({"room":"other","search":"room","maxd":0.24});
    l.registry_set(&reg);
    assert!(field(&l.ok(&["search", "refund"])["near"][0]["error"]).contains("no local wiki_root"));
    l.reject(&["index", "--library", "other"]);
    for row in [
        json!({"room":""}),
        json!({"room":"other","search":"web"}),
        json!({"room":"other","maxd":0}),
        json!({"room":"other","maxd":"0.2"}),
        json!({}),
    ] {
        reg["libraries"]["other"] = row;
        l.registry_set(&reg);
        l.reject(&["index"]);
    }
}

#[test]
fn document_paths_and_logs_remain_in_library() {
    let l = Library::demo();
    l.reject(&["read", "../library.json"]);
    l.reject(&["read", &l.cfg()]);
    link(
        &l.path("library.json"),
        &l.path("wiki/entities/alias.md"),
        false,
    );
    l.reject(&["read", "entities/alias.md"]);
    assert_eq!(l.ok(&["search", "wiki_root"])["hits"], json!([]));
    fs::create_dir(l.path("elsewhere")).unwrap();
    link(&l.path("elsewhere"), &l.path("wiki/queries"), true);
    l.reject(&["save", "Question", "One answer."]);
    assert!(files(&l.path("elsewhere")).is_empty());
    let l = Library::demo();
    l.put("outside.md", "unchanged");
    link(&l.path("outside.md"), &l.path("wiki/log.md"), false);
    l.reject(&["save", "Question", "One answer."]);
    assert_eq!(l.read("outside.md"), "unchanged");
}

#[test]
fn lasting_answers_and_concurrent_revisions() {
    let l = Library::demo();
    let s = l.ok(&["save", "Which colour?", "Violet", "--refs", "sample.md"]);
    let p = field(&s["path"]);
    assert_eq!(l.ok(&["search", "violet"])["hits"][0]["path"], p);
    for blank in ["", "   "] {
        l.reject(&["save", "Empty", blank]);
    }
    l.choose(json!({"save_block":{"example":"UNPUBLISHED"}}));
    l.reject(&["save", "UNPUBLISHED", "Text"]);
    l.ok(&[
        "save",
        "Which colour?",
        "White at night",
        "--refs",
        "night.md",
    ]);
    let old = l.ok(&["read", p]);
    let body = field(&old["text"]);
    for val in [
        "Violet",
        "sample.md",
        "night.md",
        "## 답변\n\nWhite at night",
    ] {
        assert!(body.contains(val));
    }
    l.ok(&[
        "save",
        "Which colour?",
        "White at night",
        "--refs",
        "night.md",
    ]);
    assert_eq!(l.ok(&["read", p]), old);
    let answers = [
        "observation one",
        "observation two",
        "observation three",
        "observation four",
    ];
    std::thread::scope(|s| {
        for answer in answers {
            let lib = &l;
            s.spawn(move || {
                lib.ok(&["save", "Which colour?", answer]);
            });
        }
    });
    let read = l.ok(&["read", p]);
    for answer in answers {
        assert!(field(&read["text"]).contains(answer));
    }
    let base = "A".repeat(45);
    let first = format!("{base} first?");
    let second = format!("{base} second?");
    let a = l.ok(&["save", &first, "alpha"]);
    let b = l.ok(&["save", &second, "beta"]);
    assert_ne!(a["path"], b["path"]);
    assert_eq!(l.ok(&["save", &first, "updated"])["path"], a["path"]);
    assert!(field(&l.ok(&["read", field(&b["path"])])["text"]).contains("beta"));
}

#[test]
fn local_day_and_learning_from_a_matched_search() {
    let l = Library::demo();
    let saved = value(
        l.cli(&["save", "Local day", "A short answer"])
            .env("TZ", "KST-9")
            .output()
            .unwrap(),
        0,
    );
    let expected=checked(l.cmd(python()).args(["-c","import datetime as d; print(d.datetime.now(d.timezone(d.timedelta(hours=9))).strftime('%Y-%m-%d'))"]).output().unwrap(),0);
    let stamp = text(&expected.stdout);
    let date = stamp.trim();
    let prefix = format!("queries/{}_", &date[2..].replace("-", ""));
    assert!(field(&saved["path"]).starts_with(&prefix));
    assert!(
        l.read(&format!("wiki/{}", field(&saved["path"])))
            .contains(&format!("created: {date}"))
    );
    assert_eq!(
        l.ok(&[
            "save",
            "refund rules?",
            "Fourteen days",
            "--refs",
            "refund.md"
        ])["learning"],
        "not configured"
    );
    let stamp = l
        .cmd(python())
        .args([
            "-c",
            "import time; print(time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()))",
        ])
        .output()
        .unwrap();
    let wiki_path = checked(
        l.cmd(python())
            .args([
                "-c",
                "from pathlib import Path; print(Path('wiki').resolve())",
            ])
            .output()
            .unwrap(),
        0,
    );
    let wiki = PathBuf::from(text(&wiki_path.stdout).trim());
    l.put("wiki/.wiki-pending/retrieval/a.json",json!({"q":"refund rules","det":[wiki.join("entities/refund.md")],"vec":[wiki.join("entities/shipping.md")],"ts":text(&stamp.stdout).trim()}).to_string());
    assert!(
        field(&l.ok(&["save", "refund rules", "Fourteen days"])["learning"]).starts_with("skip")
    );
    assert_eq!(
        l.ok(&[
            "save",
            "the refund rules",
            "Fourteen days",
            "--refs",
            "refund.md"
        ])["learning"],
        "w_det=0.667 w_vec=0.333 (n=1)"
    );
    let weights: Value =
        serde_json::from_str(&l.read("wiki/.wiki-pending/rrf_weights.json")).unwrap();
    assert_eq!(weights["det"], json!({"a":2.0, "b":1.0}));
    assert!(!l.path("wiki/.wiki-pending/retrieval/a.json").exists());
    assert_eq!(
        l.ok(&[
            "save",
            "refund rules",
            "Fourteen days",
            "--refs",
            "refund.md"
        ])["learning"],
        "skip(no retrieval log)"
    );
}
