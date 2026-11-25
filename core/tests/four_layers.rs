//! Room memory, shelf transcript, database records and filings are separate layers.
//! Events reach the day's transcript before memory, records reach the database only
//! when an agent saves them, and only the librarian files them into the wiki.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Library {
    dir: TempDir,
    server: Option<Child>,
}

impl Library {
    fn new() -> Self {
        let library = Library {
            dir: tempfile::tempdir().unwrap(),
            server: None,
        };
        library.ok_without_room(&["init", "--demo"]);
        library
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn base(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benedict"));
        cmd.current_dir(self.path())
            .env("LIBRARY_CONFIG", self.path().join("library.json"))
            .env("LIBRARY_DB", self.path().join("records.sqlite"))
            .env("LIBRARY_PROJECT", "four-layers")
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_DB_URL_FILE")
            .env_remove("LIBRARY_EMBED_URL")
            .env_remove("LIBRARY_ROOM_HOME")
            .env_remove("LIBRARY_ROOM_SOCKET")
            .env_remove("LIBRARY_TRANSCRIPT_DAYS");
        cmd
    }
    fn with_room(&self) -> Command {
        let mut cmd = self.base();
        cmd.env("LIBRARY_ROOM_SOCKET", self.path().join("room.sock"));
        cmd
    }
    fn call(&self, args: &[&str]) -> Output {
        self.with_room().args(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        value(args, self.call(args))
    }
    fn ok_without_room(&self, args: &[&str]) -> Value {
        value(args, self.base().args(args).output().unwrap())
    }
    fn start(&mut self) {
        self.server = Some(self.with_room().args(["room", "serve"]).spawn().unwrap());
        let start = Instant::now();
        while !self.call(&["room", "ping"]).status.success() {
            assert!(start.elapsed() < Duration::from_secs(5), "room did not start");
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    fn transcripts(&self) -> PathBuf {
        self.path().join("shelf/transcripts")
    }
    fn rows(&self) -> i64 {
        let db = self.path().join("records.sqlite");
        if !db.exists() {
            return 0;
        }
        rusqlite::Connection::open(db)
            .unwrap()
            .query_row("SELECT count(*) FROM benedict_room_records", [], |r| {
                r.get(0)
            })
            .unwrap_or(0)
    }
    fn librarian(&self, cmd: &mut Command) -> (Vec<Value>, Option<i32>) {
        let out = cmd
            .args(["librarian", "--iterations", "2", "--interval", "0.001"])
            .output()
            .unwrap();
        let rows = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (rows, out.status.code())
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}

fn value(args: &[&str], out: Output) -> Value {
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn day(days_ago: u64) -> String {
    (chrono::Local::now().date_naive() - chrono::Days::new(days_ago))
        .format("%y%m%d")
        .to_string()
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn filings(library: &Library) -> usize {
    match fs::read_dir(library.path().join("wiki/queries")) {
        Ok(entries) => entries
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "md")
            })
            .count(),
        Err(_) => 0,
    }
}

#[test]
fn room_events_reach_the_transcript_and_memory_but_not_the_database() {
    let mut library = Library::new();
    library.start();
    let file = format!("{}/main-reading.jsonl", day(0));
    let events = [
        ("user", "Which batch was approved?"),
        ("agent", "Batch A7."),
        ("tool", "inspection sheet: 59 units"),
    ];
    let mut receipts = vec![];
    for (i, (role, text)) in events.iter().enumerate() {
        let request = format!("request-{i}");
        let receipt = library.ok(&[
            "room", "append", "--room", "reading", "--session", "s1", "--request", &request,
            "--role", role, "--text", text,
        ]);
        assert_eq!(receipt["turn"], json!({"file": file, "line": i + 1}));
        assert_eq!(receipt["duplicate"], false);
        receipts.push(receipt);
    }
    // A retried request returns its earlier line instead of writing it twice.
    let retry = library.ok(&[
        "room", "append", "--room", "reading", "--session", "s1", "--request", "request-1",
        "--role", "agent", "--text", "Batch A7.",
    ]);
    assert_eq!(retry["turn"], receipts[1]["turn"]);
    assert_eq!(retry["duplicate"], true);
    let written = lines(&library.transcripts().join(&file));
    assert_eq!(written.len(), 3);
    for (line, (role, text)) in written.iter().zip(events) {
        assert_eq!(
            (line["role"].as_str(), line["text"].as_str()),
            (Some(role), Some(text))
        );
        assert_eq!(line["session"], "s1");
    }
    assert_eq!(library.rows(), 0);
    let memory = library.ok(&["room", "memory", "--room", "reading"]);
    assert_eq!(memory["records"].as_array().unwrap().len(), 3);
    assert_eq!(memory["records"][2]["text"], "inspection sheet: 59 units");

    // Without the room service an event is refused, not written elsewhere.
    for args in [
        &["room", "append", "--room", "reading", "--role", "user", "--text", "lost?"][..],
        &["room", "task", "--question", "Count?", "--source", "sheet"][..],
    ] {
        let refused = library.base().args(args).output().unwrap();
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("room service not running"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
    }
    assert_eq!(library.rows(), 0);

    // Meeting tasks, replies and dispatched requests keep their database contract
    // and also leave their originals on the shelf.
    let task = library.ok(&["room", "task", "--question", "Count?", "--source", "sheet A7"]);
    let id = task["id"].to_string();
    let reply = library.ok(&[
        "room", "reply", "--id", &id, "--answer", "59 units", "--source", "sheet A7",
    ]);
    let opened = library.ok(&["room", "open", "--id", &id]);
    assert_eq!(opened["task"], task);
    assert_eq!(opened["replies"], json!([reply]));
    let request = library.ok(&["room", "dispatch", "--room", "search", "--question", "Measure"]);
    let responses = library.ok(&["room", "responses", "--id", &request["id"].to_string()]);
    assert_eq!(responses["replies"], json!([]));
    assert_eq!(library.rows(), 3);
    let meeting = lines(
        &library
            .transcripts()
            .join(format!("{}/main-meeting.jsonl", day(0))),
    );
    assert_eq!(meeting.len(), 3);
    assert_eq!(meeting[0]["question"], "Count?");
    assert_eq!(meeting[1]["answer"], "59 units");
    assert_eq!(meeting[2]["role"], "open");
    let search = lines(
        &library
            .transcripts()
            .join(format!("{}/main-search.jsonl", day(0))),
    );
    assert_eq!(search.len(), 2);
    assert_eq!(search[1]["role"], "responses");
}

#[test]
fn save_keeps_one_database_record_and_files_nothing() {
    let library = Library::new();
    let before = library.rows();
    let saved = library.ok_without_room(&[
        "save",
        "--session",
        "s1",
        "--turns",
        "261124/main-reading.jsonl:1-3",
        "--refs",
        "entities/refund.md",
        "When may a fictional product be returned?",
        "Within 14 days if unused.",
    ]);
    assert_eq!(library.rows(), before + 1);
    assert!(saved.get("path").is_none());
    let record = library.ok_without_room(&["recall", "--id", &saved["record_id"].to_string()]);
    assert_eq!(record["kind"], "record");
    assert_eq!(record["session"], "s1");
    assert_eq!(record["filed_at"], "");
    assert_eq!(
        serde_json::from_str::<Value>(record["turns"].as_str().unwrap()).unwrap(),
        json!([
            {"file": "261124/main-reading.jsonl", "line": 1},
            {"file": "261124/main-reading.jsonl", "line": 3}
        ])
    );
    // A conversation that crosses midnight names both day files.
    let crossing = library.ok_without_room(&[
        "save",
        "--turns",
        "261124/main-reading.jsonl:40..261125/main-reading.jsonl:2",
        "Which batch?",
        "A7.",
    ]);
    let crossing =
        library.ok_without_room(&["recall", "--id", &crossing["record_id"].to_string()]);
    assert_eq!(
        serde_json::from_str::<Value>(crossing["turns"].as_str().unwrap()).unwrap(),
        json!([
            {"file": "261124/main-reading.jsonl", "line": 40},
            {"file": "261125/main-reading.jsonl", "line": 2}
        ])
    );
    for bad in [
        "../main-reading.jsonl:1",
        "261124/main-reading.jsonl:0",
        "261124/main-reading.jsonl:3-1",
    ] {
        assert!(
            !library
                .base()
                .args(["save", "--turns", bad, "Q?", "A."])
                .output()
                .unwrap()
                .status
                .success(),
            "{bad}"
        );
    }
    assert_eq!(filings(&library), 0);
}

#[test]
fn filing_by_id_writes_one_page_and_one_golden_line_for_wiki_sources() {
    let library = Library::new();
    let question = "When may a fictional product be returned?";
    let saved = library.ok_without_room(&[
        "save",
        "--refs",
        "entities/refund.md\nhttps://example.org/policy",
        question,
        "Within 14 days if unused.",
    ]);
    let id = saved["record_id"].to_string();
    let filed = library.ok_without_room(&["file", "--id", &id]);
    let page = fs::read_to_string(
        library
            .path()
            .join("wiki")
            .join(filed["path"].as_str().unwrap()),
    )
    .unwrap();
    assert!(page.contains(question));
    let golden = library.path().join("golden/main.jsonl");
    assert_eq!(
        lines(&golden),
        vec![json!({"q": question, "answer_doc": ["entities/refund.md"], "src": format!("room record {id}")})]
    );
    let again = library.ok_without_room(&["file", "--id", &id]);
    assert_eq!(again["path"], filed["path"]);
    assert_eq!(filings(&library), 1);
    assert_eq!(lines(&golden).len(), 1);
    let record = library.ok_without_room(&["recall", "--id", &id]);
    assert_ne!(record["filed_at"], "");

    // Without a wiki source the page alone completes the filing.
    let web = library.ok_without_room(&[
        "save",
        "--refs",
        "https://example.org/policy",
        "Is there a web policy?",
        "Yes.",
    ]);
    let web_id = web["record_id"].to_string();
    library.ok_without_room(&["file", "--id", &web_id]);
    assert_ne!(
        library.ok_without_room(&["recall", "--id", &web_id])["filed_at"],
        ""
    );
    assert_eq!(lines(&golden).len(), 1);

    // The same question answered from another wiki document extends its line.
    let more = library.ok_without_room(&[
        "save",
        "--refs",
        "entities/shipping.md",
        question,
        "Within 14 days; parcels leave Monday.",
    ]);
    library.ok_without_room(&["file", "--id", &more["record_id"].to_string()]);
    let golden_lines = lines(&golden);
    assert_eq!(golden_lines.len(), 1);
    assert_eq!(
        golden_lines[0]["answer_doc"],
        json!(["entities/refund.md", "entities/shipping.md"])
    );
    assert_eq!(filings(&library), 2);
}

#[test]
fn librarian_removes_transcript_days_only_past_retention() {
    let mut library = Library::new();
    library.start();
    for days_ago in [0, 1, 2] {
        let folder = library.transcripts().join(day(days_ago));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("main-reading.jsonl"), "{}\n").unwrap();
    }
    let (rows, _) = library.librarian(&mut library.with_room());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["steps"]["transcripts"]["retention"], "keep");
    assert_eq!(rows[0]["steps"]["transcripts"]["folders"], 3);
    for days_ago in [0, 1, 2] {
        assert!(library.transcripts().join(day(days_ago)).is_dir());
    }
    let mut kept_one_day = library.with_room();
    kept_one_day.env("LIBRARY_TRANSCRIPT_DAYS", "1");
    let (rows, _) = library.librarian(&mut kept_one_day);
    assert_eq!(rows[0]["steps"]["transcripts"]["removed"], json!([day(2)]));
    assert_eq!(rows[0]["steps"]["transcripts"]["folders"], 2);
    assert!(library.transcripts().join(day(0)).is_dir());
    assert!(library.transcripts().join(day(1)).is_dir());
    assert!(!library.transcripts().join(day(2)).exists());
    let mut invalid = library.with_room();
    invalid.env("LIBRARY_TRANSCRIPT_DAYS", "soon");
    let (rows, code) = library.librarian(&mut invalid);
    assert_eq!(
        (rows[0]["status"].as_str(), code),
        (Some("failed"), Some(2))
    );
}

#[test]
fn an_event_that_cannot_reach_the_transcript_is_refused() {
    let mut library = Library::new();
    fs::create_dir_all(library.path().join("shelf")).unwrap();
    fs::write(library.transcripts(), "not a folder").unwrap();
    library.start();
    for args in [
        &["room", "append", "--room", "reading", "--role", "user", "--text", "Hello"][..],
        &["record", "--room", "reading", "--question", "Hello"][..],
        &["room", "task", "--question", "Count?", "--source", "sheet"][..],
    ] {
        let out = library.call(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("transcript"));
    }
    assert_eq!(library.rows(), 0);
}

#[test]
fn librarian_skips_or_flags_the_golden_file_without_stopping() {
    let mut library = Library::new();
    let (rows, code) = library.librarian(&mut library.base());
    assert_eq!((rows.len(), code), (2, Some(1)));
    assert_eq!(rows[0]["steps"]["golden"]["status"], "not_configured");
    let golden = library.path().join("golden/main.jsonl");
    fs::create_dir_all(golden.parent().unwrap()).unwrap();
    fs::write(
        &golden,
        "{\"q\":\"refund\",\"answer_doc\":\"entities/refund.md\"}\n{not json}\n",
    )
    .unwrap();
    let (rows, code) = library.librarian(&mut library.base());
    assert_eq!((rows.len(), code), (2, Some(1)));
    assert_eq!(rows[1]["steps"]["golden"]["status"], "invalid");
    assert_eq!(rows[1]["steps"]["golden"]["line"], 2);
    fs::write(
        &golden,
        "{\"q\":\"refund\",\"answer_doc\":\"entities/refund.md\"}\n",
    )
    .unwrap();
    // Scoring searches, and search goes through the room.
    library.start();
    let (rows, code) = library.librarian(&mut library.with_room());
    assert_eq!((rows.len(), code), (2, Some(0)));
    assert_eq!(rows[0]["steps"]["golden"]["summary"]["hit"], 1);
}

#[test]
fn librarian_journal_counts_records_waiting_for_filing() {
    let library = Library::new();
    let first = library.ok_without_room(&[
        "save",
        "--refs",
        "entities/refund.md",
        "Refund window?",
        "14 days.",
    ]);
    library.ok_without_room(&[
        "save",
        "--refs",
        "entities/shipping.md",
        "Shipping day?",
        "Monday.",
    ]);
    // Other records are not waiting for the librarian.
    library.ok_without_room(&[
        "record",
        "--room",
        "search",
        "--question",
        "Refund window?",
        "--query",
        "refund",
    ]);
    let (rows, _) = library.librarian(&mut library.base());
    assert_eq!(rows[0]["steps"]["records"]["unfiled"], 2);
    library.ok_without_room(&["file", "--id", &first["record_id"].to_string()]);
    let (rows, _) = library.librarian(&mut library.base());
    assert_eq!(rows[0]["steps"]["records"]["unfiled"], 1);
}
