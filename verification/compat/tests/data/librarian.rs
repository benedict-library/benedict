use super::*;

struct Rooms(Library);
impl Rooms {
    fn new() -> Self {
        let r = Self(Library::demo());
        let sync = r
            .cmd("room_sync.py")
            .args(["--config", &r.0.cfg()])
            .output()
            .unwrap();
        checked(sync, 0);
        r
    }
    fn cmd(&self, script: &str) -> Command {
        let mut c = self.0.py(script);
        c.env("LIBRARY_ROOM_HOME", self.0.path("rooms"));
        c
    }
    fn librarian(&self, args: &[&str]) -> Command {
        let mut c = self.cmd("librarian.py");
        c.args(["--config", &self.0.cfg()]).args(args);
        c
    }
    fn run(&self, args: &[&str], code: i32) -> Value {
        value(self.librarian(args).output().unwrap(), code)
    }
    fn room(&self, verb: &str, room: &str, args: &[&str]) -> String {
        text(
            &checked(
                self.cmd("room.py")
                    .args([verb, &format!("main-{room}")])
                    .args(args)
                    .output()
                    .unwrap(),
                0,
            )
            .stdout,
        )
        .trim()
        .to_owned()
    }
    fn exec(&self, room: &str, code: &str) -> String {
        self.room("exec", room, &[code])
    }
    fn query(&self, room: &str, expr: &str) -> Value {
        serde_json::from_str(&self.exec(
            room,
            &format!("import json as _j\nprint(_j.dumps({expr}, ensure_ascii=False))"),
        ))
        .unwrap()
    }
    fn journal(&self) -> Value {
        self.query("librarian", "journal[-1]")
    }
    fn pid(&self) -> String {
        self.room("status", "reading", &[])
            .split("pid=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    }
    fn last_record(&self) -> Value {
        serde_json::from_str(
            self.0
                .read(".library-state/librarian.jsonl")
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Rooms {
    fn drop(&mut self) {
        for file in files(&self.0.path("rooms")) {
            if file.extension().is_some_and(|ex| ex == "pid") {
                let _ = self
                    .cmd("room.py")
                    .arg("stop")
                    .arg(file.file_stem().unwrap())
                    .output();
            }
        }
    }
}

#[test]
fn curate_sources_and_validate_registry_export() {
    let l = Library::demo();
    for (name, source) in [("a", "one.txt"), ("b", "two.txt")] {
        l.put(
            &format!("wiki/entities/{name}.md"),
            format!("---\ntitle: Twins\nsources: [{source}]\n---\n# Example\n"),
        );
    }
    l.put("wiki/queries/note.md", "Unreviewed sample");
    l.choose(json!({"forbidden_terms":["Unreviewed"]}));
    let result = value(
        l.bridge(
            "import librarian\nprint(json.dumps(librarian.tidy(sys.argv[1],'main')))",
            &[&l.cfg()],
        ),
        0,
    );
    assert_eq!(result["documents"], 5);
    assert_eq!(
        result["missing_sources"],
        json!(["entities/refund.md", "entities/shipping.md"])
    );
    assert_eq!(
        result["duplicate_titles"],
        json!({"Twins":["entities/a.md","entities/b.md"]})
    );
    assert_eq!(
        result["forbidden_terms"],
        json!({"Unreviewed":["queries/note.md"]})
    );
    let mut rows = json!([{"key":"central","ord":1,"central":true,"name":"Example","wiki_root":l.path("wiki"),"dirs":"entities,concepts","status":"active","options":{"reads":["annex"],"rooms":{"reading":"books"}}},{"key":"annex","ord":2,"name":"Annex","central":false,"room":"annex-room","search":"room","maxd":0.24,"status":"active","save_block":{"example":"sample"}}]);
    let export = |rows: &Value| {
        l.bridge("import registry_export\nprint(json.dumps(registry_export.write(json.loads(sys.argv[1]),sys.argv[2])))",&[&rows.to_string(),&l.cfg()])
    };
    let out = value(export(&rows), 0);
    assert_eq!(out["libraries"], json!(["central", "annex"]));
    let registry = l.registry();
    assert_eq!(
        registry["libraries"]["central"]["dirs"],
        json!(["entities", "concepts"])
    );
    assert_eq!(registry["libraries"]["central"]["reads"], json!(["annex"]));
    assert_eq!(
        registry["libraries"]["annex"]["save_block"],
        json!({"example":"sample"})
    );
    assert_eq!(
        registry["libraries"]["central"]["rooms"]["reading"],
        "books"
    );
    rows[0]["options"]["reads"] = json!(["central"]);
    assert!(!export(&rows).status.success());
    assert_eq!(l.registry(), registry);
}

#[test]
fn cycle_measures_and_files_warnings() {
    let r = Rooms::new();
    r.0.put(
        "gold",
        json!({"q":"refund","answer_doc":"entities/refund.md","src":"example"}).to_string(),
    );
    let result = r.run(&["--golden", "gold"], 0);
    assert_eq!(result["status"], "ok");
    assert!(result["steps"]["sync"]["rooms"].is_object());
    assert_eq!(result["steps"]["tidy"]["missing_sources"], 2);
    r.0.put(
        "gold",
        json!({"q":"undiscoverable","answer_doc":"entities/refund.md"}).to_string(),
    );
    assert_eq!(r.run(&["--golden", "gold"], 1)["status"], "warning");
    assert_eq!(r.journal()["status"], "warning");
}

#[test]
fn clean_exports_expired_work_and_keeps_ongoing_tasks() {
    let r = Rooms::new();
    assert_eq!(r.run(&["--clean"], 0)["clean"], "not configured");
    r.0.choose(json!({"clean":true}));
    r.exec(
        "reading",
        "reading_old_200101={'label':1}\nreading_fresh_991231=1\nscratch=2",
    );
    r.exec(
        "meeting",
        "task_old_200101=1\ntask_open=2\nlegacy_notes=3\nmail['topic']='kept'",
    );
    assert!(field(&r.run(&["--clean", "--dry-run"], 0)["result"]).contains("reading_old_200101"));
    assert!(
        r.room("vars", "reading", &[])
            .contains("reading_old_200101")
    );
    let clean = r.run(&["--clean"], 0);
    assert!(field(&clean["result"]).contains("규칙 밖 1"));
    let names = r.room("vars", "reading", &[]);
    for removed in ["reading_old_200101", "scratch"] {
        assert!(!names.contains(removed));
    }
    for kept in ["reading_fresh_991231", "docs"] {
        assert!(names.contains(kept));
    }
    let meeting = r.room("vars", "meeting", &[]);
    assert!(!meeting.contains("task_old_200101"));
    for kept in ["task_open", "legacy_notes", "mail"] {
        assert!(meeting.contains(kept));
    }
    let entry = tree(&r.0.path(".library-state/room_dump/main-reading"))
        .into_iter()
        .find(|p| p.ends_with("reading_old_200101.json"))
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(entry).unwrap()).unwrap(),
        json!({"label":1})
    );
    assert_eq!(r.journal()["kind"], "변수 정리");
    r.0.choose(json!({"clean":{"unknown":{}}}));
    checked(r.librarian(&["--clean"]).output().unwrap(), 2);
}

#[test]
fn clean_uses_registry_rules_and_protects_room_settings() {
    let r = Rooms::new();
    r.0.choose(json!({"clean":{"reading":{"keep":["docs","sync_meta"],"prefixes":["work_","reading_"],"shells":true},"meeting":{"keep":["mail"],"prefixes":["task_"],"shells":true}}}));
    r.exec("meeting", "ROOM_NOTE='sample'\nleftover=1");
    r.run(&["--clean"], 0);
    let names = r.room("vars", "reading", &[]);
    for name in ["ROOM_DOC_ROOT", "ROOM_CARD_PREFIXES", "docs"] {
        assert!(names.contains(name));
    }
    let names = r.room("vars", "meeting", &[]);
    assert!(names.contains("ROOM_NOTE") && !names.contains("leftover"));
}

#[test]
fn restart_preserves_snapshots_and_exports_large_values() {
    let r = Rooms::new();
    assert_eq!(r.run(&["--restart"], 0)["restart"], "not configured");
    r.0.choose(json!({"restart":{"rooms":["reading"]}}));
    r.exec(
        "reading",
        "reading_small_991231={'value':1}\nreading_big_991231='x'*(17*1024*1024)",
    );
    let before = r.pid();
    assert_eq!(r.run(&["--restart"], 0)["restart"], "done");
    assert_ne!(r.pid(), before);
    let names = r.room("vars", "reading", &[]);
    assert!(names.contains("reading_small_991231") && !names.contains("reading_big_991231"));
    assert_eq!(r.query("reading", "len(docs)"), 2);
    let file = tree(&r.0.path(".library-state/room_dump/main-reading"))
        .into_iter()
        .find(|p| p.ends_with("reading_big_991231.json"))
        .unwrap();
    let dumped: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    assert_eq!(field(&dumped).len(), 17 * 1024 * 1024);
    let entry = r.journal();
    assert_eq!(entry["kind"], "다시 띄우기");
    for label in ["main-reading", "변수", "→", "메모리", "reading_big_991231"] {
        assert!(field(&entry["result"]).contains(label));
    }
    r.0.choose(json!({"clean":true,"restart":true}));
    let before = r.pid();
    let result = r.run(&["--clean"], 0);
    assert_eq!(result["restart"]["restart"], "done");
    assert_eq!(result["clean"], "done");
    assert_ne!(before, r.pid());
}

#[test]
fn restart_protects_unserializable_work_and_retries_failed_starts() {
    let r = Rooms::new();
    r.0.choose(json!({"restart":{"rooms":["reading"]}}));
    r.exec(
        "reading",
        "import threading\nreading_lock_991231=threading.Lock()",
    );
    let before = r.pid();
    let out = checked(r.librarian(&["--restart"]).output().unwrap(), 2);
    assert!(text(&out.stderr).contains("reading_lock_991231"));
    assert_eq!(r.pid(), before);
    assert!(
        r.room("vars", "reading", &[])
            .contains("reading_lock_991231")
    );
    r.exec("reading", "del reading_lock_991231");
    let retry = |failures: &str| {
        r.0.bridge(r#"import librarian
from unittest.mock import patch
os.environ['LIBRARY_ROOM_HOME']=sys.argv[2]
operation=librarian.room_sync.room_command
remaining=int(sys.argv[3])
def with_transient_failure(command,room,code=None,args=()):
    global remaining
    if command=='start' and room=='main-reading' and remaining:
        remaining-=1
        raise RuntimeError('injected startup failure')
    return operation(command,room,code,args)
with patch.object(librarian.room_sync,'room_command',with_transient_failure), patch.object(librarian.time,'sleep'):
    print(json.dumps(librarian.restart(sys.argv[1])))"#,&[&r.0.cfg(),&r.0.path("rooms").display().to_string(),failures])
    };
    assert_eq!(value(retry("2"), 0)["restart"], "done");
    assert!(field(&r.journal()["result"]).contains("시작 3번째"));
    let down = retry("3");
    assert!(!down.status.success());
    assert!(text(&down.stderr).contains("the room is down"));
    let j = r.journal();
    assert_eq!(j["kind"], "다시 띄우기 실패");
    assert!(field(&j["result"]).contains("main-reading"));
}

#[test]
fn expired_mail_exports_before_removal() {
    let r = Rooms::new();
    assert_eq!(r.run(&["--reap"], 0)["reap"], "not configured");
    r.exec("meeting","import datetime\nmail['old/topic']=[{'from':'reader','at':'2000-01-01T00:00:00','text':'first'}]\nmail['fresh']=[{'from':'author','at':datetime.datetime.now().isoformat(),'text':'second'}]\nmail['unknown']=['malformed']");
    r.0.choose(json!({"mail_reap":{"ttl_days":7}}));
    assert_eq!(r.run(&["--reap"], 0)["reap"], "done");
    assert_eq!(
        r.query("meeting", "sorted(mail)"),
        json!(["fresh", "unknown"])
    );
    let exports = tree(&r.0.path(".library-state/room_dump/main-meeting"));
    assert_eq!(exports.len(), 1);
    let entry: Value = serde_json::from_slice(&fs::read(&exports[0]).unwrap()).unwrap();
    assert_eq!(entry["topic"], "old/topic");
    assert_eq!(entry["entries"][0]["text"], "first");
    assert_eq!(r.journal()["kind"], "회신함 정리");
}

#[test]
fn journal_commits_to_the_chosen_database_before_updating_the_room() {
    let r = Rooms::new();
    r.0.choose(json!({"clean":true,"journal_db":true}));
    assert!(
        text(&checked(r.librarian(&["--clean"]).output().unwrap(), 2).stderr)
            .contains("LIBRARY_JOURNAL_DSN")
    );
    let nonce =
        r.0.0
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches('.')
            .to_owned();
    let recorder = format!(
        "\"{}\"",
        env!("CARGO_BIN_EXE_journal_recorder").replace('\\', "/")
    );
    let command = |args: &[&str]| {
        let mut c = r.librarian(args);
        c.env("EXAMPLE_JOURNAL_NONCE", &nonce)
            .env("EXAMPLE_JOURNAL_LOG", r.0.path("journal.jsonl"))
            .env("LIBRARY_PSQL", &recorder)
            .env(
                "LIBRARY_JOURNAL_DSN",
                format!("postgresql://reader:{nonce}@example.invalid/archive"),
            )
            .env("EXAMPLE_LEDGER_DSN", "postgresql://example.invalid/annex");
        c
    };
    let last = || {
        serde_json::from_str::<Value>(r.0.read("journal.jsonl").lines().last().unwrap()).unwrap()
    };
    value(command(&["--clean"]).output().unwrap(), 0);
    let line = last();
    assert_eq!(line["passed_in_environment"], true);
    assert_eq!(line["passed_in_arguments"], false);
    assert_eq!(line["database"], "archive");
    assert_eq!(line["host"], "example.invalid");
    assert_eq!(line["user"], "reader");
    assert!(field(&line["sql"]).contains("INSERT INTO library.journal"));
    let b = &line["bindings"];
    assert_eq!(b["kind"], "변수 정리");
    assert_eq!(b["room"], "main-librarian");
    let timestamp = field(&b["at"]);
    assert!(
        timestamp[10..].contains('+') || timestamp[10..].contains('-') || timestamp.ends_with('Z')
    );
    assert_eq!(r.journal()["kind"], "변수 정리");
    let n = r.query("librarian", "len(journal)");
    let failed = checked(
        command(&["--clean"])
            .env("EXAMPLE_DB_UNAVAILABLE", "1")
            .output()
            .unwrap(),
        2,
    );
    assert!(text(&failed.stderr).contains("database unavailable"));
    assert_eq!(r.query("librarian", "len(journal)"), n);
    r.0.choose(
        json!({"journal_db":{"dsn_env":"EXAMPLE_LEDGER_DSN","table":"example.librarian_log"}}),
    );
    assert_eq!(
        value(command(&[]).output().unwrap(), 1)["status"],
        "warning"
    );
    let row = last();
    assert!(field(&row["sql"]).contains("INSERT INTO example.librarian_log"));
    assert_eq!(row["bindings"]["kind"], "사서 한 바퀴");
    assert_eq!(
        serde_json::from_str::<Value>(field(&row["bindings"]["result"])).unwrap()["status"],
        "warning"
    );
    let schema = fs::read_to_string(repo_root().join("journal.sql")).unwrap();
    assert!(schema.contains("CREATE TABLE IF NOT EXISTS library.journal"));
    assert!(schema.contains("BEFORE UPDATE OR DELETE"));
    for key in ["seq", "at", "room", "kind", "target", "result"] {
        assert!(
            schema
                .lines()
                .any(|l| l.split_whitespace().next() == Some(key))
        );
    }
}

#[test]
fn copies_update_independently_and_reject_oversize_data() {
    let r = Rooms::new();
    let data = json!({"units":3});
    r.0.put("sample.json", data.to_string());
    let exe = env!("CARGO_BIN_EXE_catalog_adapter").replace('\\', "/");
    let printer = format!("\"{exe}\" labels");
    r.0.choose(
        json!({"copies":{"sample_totals":{"file":"sample.json"},"labels":{"command":printer}}}),
    );
    r.run(&[], 1);
    assert_eq!(r.last_record()["steps"]["copies"]["failed"], json!([]));
    assert_eq!(r.query("search", "sample_totals"), data);
    assert_eq!(r.query("search", "labels"), json!([{"label":"one"}]));
    let copy_journal = || {
        r.query(
            "librarian",
            "[entry for entry in journal if entry.get('kind')=='사본 올리기'][-1]",
        )
    };
    let log = copy_journal();
    assert_eq!(log["target"], "main-search");
    assert!(field(&log["result"]).contains("labels 1행"));
    assert!(field(&log["result"]).contains("sample_totals"));
    let printer = format!("\"{exe}\" unavailable");
    r.0.put("sample.json", r#"{"units":4}"#);
    r.0.choose(json!({"copies":{"sample_totals":{"file":"sample.json"},"labels":{"command":printer},"missing_copy":{"file":"absent"}}}));
    checked(r.librarian(&[]).output().unwrap(), 2);
    let failure = r.last_record();
    assert_eq!(failure["status"], "failed");
    for key in ["labels", "missing_copy"] {
        assert!(field(&failure["error"]).contains(key));
    }
    assert_eq!(r.query("search", "labels"), json!([{"label":"one"}]));
    assert_eq!(r.query("search", "sample_totals"), json!({"units":4}));
    assert!(field(&copy_journal()["result"]).contains("실패"));
    r.0.put(
        "large.json",
        json!("x".repeat(17 * 1024 * 1024)).to_string(),
    );
    r.0.choose(json!({"copies":{"oversize":{"file":"large.json"}}}));
    checked(r.librarian(&[]).output().unwrap(), 2);
    assert!(field(&r.last_record()["error"]).contains("snapshot"));
    assert_eq!(r.query("search", "globals().get('oversize')"), Value::Null);
}

#[test]
fn invalid_librarian_options_are_reported_before_work_starts() {
    let l = Library::demo();
    let original = l.registry();
    let invalid = [
        json!({"restart":{"rooms":["kitchen"]}}),
        json!({"restart":{"after":"breakfast"}}),
        json!({"restart":"yes"}),
        json!({"mail_reap":{"ttl_days":0}}),
        json!({"mail_reap":{"ttl_days":true}}),
        json!({"journal_db":{"table":"invalid table"}}),
        json!({"journal_db":{"dsn_env":"lower case"}}),
        json!({"copies":["labels"]}),
        json!({"copies":{"two words":{"file":"x"}}}),
        json!({"copies":{"labels":{}}}),
        json!({"copies":{"labels":{"file":"x","command":"y"}}}),
        json!({"copies":{"labels":{"file":""}}}),
        json!({"copies":{"docs":{"file":"x"}}}),
        json!({"copies":{"ROOM_READS":{"file":"x"}}}),
        json!({"copies":{"_len":{"file":"x"}}}),
        json!({"copies":{"library_terms":{"file":"x"}},"terms":"terms.json"}),
    ];
    for opts in invalid {
        l.registry_set(&original);
        l.choose(opts);
        assert!(l.reject(&["index"]).contains("error"));
    }
}
