use super::*;

#[test]
fn measurement_reports_sources_rank_and_key_coverage() {
    let l = Library::demo();
    let run = |args: &[&str], code| {
        let out = l
            .py("evaluate.py")
            .args(["--config", &l.cfg(), "--golden", "golden"])
            .args(args)
            .output()
            .unwrap();
        let out = checked(out, code);
        serde_json::from_slice::<Value>(if code == 2 { &out.stderr } else { &out.stdout }).unwrap()
    };
    let mut row = json!({"q":"refund","answer_doc":"entities/refund.md","keys":["14 days|two weeks","unused"],"src":"example","made_by":"reader"});
    l.put("golden", row.to_string());
    let report = run(&[], 0);
    assert_eq!(report["summary"]["hit"], 1);
    let result = &report["rows"][0];
    assert_eq!(result["rank"], 1);
    assert_eq!(result["src"], "example");
    assert_eq!(result["made_by"], "reader");
    assert!(result["secs"].as_f64().unwrap() >= 0.);
    row["answer_doc"] = json!(["absent.md", "entities/refund.md"]);
    l.put("golden", row.to_string());
    let output = run(&["--out", "result.json"], 0);
    assert_eq!(
        output,
        serde_json::from_str::<Value>(&l.read("result.json")).unwrap()
    );
    l.put(
        "responses",
        json!({"q":"refund","answer":"Unused items: two   weeks"}).to_string(),
    );
    let scored = run(&["--answers", "responses"], 0);
    assert_eq!(scored["rows"][0]["key_coverage_pass"], true);
    assert_eq!(
        scored["summary"]["answer_metric"],
        "key-coverage-not-semantic-correctness"
    );
    l.put(
        "responses",
        json!({"q":"refund","answer":"Unused"}).to_string(),
    );
    assert_eq!(
        run(&["--answers", "responses"], 1)["rows"][0]["missing_keys"],
        json!(["14 days|two weeks"])
    );
    for answers in [
        "",
        r#"{"q":"unknown","answer":"text"}"#,
        r#"{"q":"refund","answer":1}"#,
        "{\"q\":\"refund\",\"answer\":\"a\"}\n{\"q\":\"refund\",\"answer\":\"b\"}",
    ] {
        l.put("responses", answers);
        assert!(run(&["--answers", "responses"], 2)["error"].is_string());
    }
    row.as_object_mut().unwrap().remove("keys");
    l.put("golden", row.to_string());
    l.put("responses", r#"{"q":"refund","answer":"yes"}"#);
    run(&["--answers", "responses"], 2);
    row["answer_doc"] = json!("missing.md");
    l.put("golden", row.to_string());
    assert_eq!(run(&[], 1)["summary"]["hit"], 0);
    for invalid in [
        "",
        "invalid",
        "[]",
        "{}",
        r#"{"q":"refund","answer_doc":[]}"#,
        r#"{"q":"!!!","answer_doc":"entities/refund.md"}"#,
    ] {
        l.put("golden", invalid);
        run(&[], 2);
    }
    l.put("wiki/extra/refund.md", "Refund duplicate");
    l.put("wiki/refund.md", "Refund root policy");
    l.ok(&["index"]);
    row["answer_doc"] = json!("refund.md");
    l.put("golden", row.to_string());
    assert!(field(&run(&[], 2)["error"]).contains("ambiguous"));
    for path in ["entities/refund.md", "./refund.md"] {
        row["answer_doc"] = json!(path);
        l.put("golden", row.to_string());
        run(&[], 0);
    }
    run(&["--out", "golden"], 2);
    assert_eq!(l.read("golden"), row.to_string());
    run(&["--k", "0"], 2);
    row["answer_doc"] = json!("../outside.md");
    l.put("golden", row.to_string());
    run(&[], 2);
    row["answer_doc"] = json!("entities/refund.md");
    l.put("golden", row.to_string());
    for p in files(&l.path(".library-state")) {
        if p.to_string_lossy().ends_with("-index.json") {
            fs::remove_file(p).unwrap();
        }
    }
    assert!(field(&run(&[], 2)["error"]).contains("index"));
}
