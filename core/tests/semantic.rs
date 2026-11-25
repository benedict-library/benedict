//! Semantic search: the librarian embeds documents through an embedding server, the search room
//! fuses the word ranking with the vector ranking, and a missing server leaves the word results.
mod common;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
};

/// A stand-in embedding server: texts about animals point one way, every other text another.
fn embedder() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            let data: Vec<Value> = request["input"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, text)| {
                    let text = text.as_str().unwrap().to_lowercase();
                    let animal = ["zebra", "horse", "stripes"].iter().any(|w| text.contains(w));
                    json!({"index": i, "embedding": if animal { [1.0, 0.0, 0.1] } else { [0.0, 1.0, 0.1] }})
                })
                .collect();
            let reply = json!({"data": data}).to_string();
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}", reply.len());
        }
    });
    url
}

/// An address where nothing listens.
fn gone() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

struct Library {
    dir: tempfile::TempDir,
}

impl Library {
    fn new() -> Self {
        let library = Library { dir: tempfile::tempdir().unwrap() };
        let wiki = library.path().join("wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("stripes.md"), "# Stripes\n\nA horse with black and white stripes.\n").unwrap();
        fs::write(wiki.join("refunds.md"), "# Refunds\n\nRefunds take five days.\n").unwrap();
        fs::write(wiki.join("shipping.md"), "# Shipping\n\nShipping takes two days.\n").unwrap();
        fs::write(
            library.path().join("library.json"),
            json!({"version":1,"libraries":{"main":{"wiki_root":"wiki","central":true}}}).to_string(),
        )
        .unwrap();
        library
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn cmd(&self, url: Option<&str>, model: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benedict"));
        cmd.current_dir(self.path())
            .env("LIBRARY_PROJECT", "semantic")
            .env("LIBRARY_ROOM_SOCKET", common::socket(self.path()))
            .env("LIBRARY_EMBED_MODEL", model)
            .env_remove("LIBRARY_DB")
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_DB_URL_FILE")
            .env_remove("LIBRARY_CONFIG");
        match url {
            Some(url) => cmd.env("LIBRARY_EMBED_URL", url),
            None => cmd.env_remove("LIBRARY_EMBED_URL"),
        };
        cmd
    }
    fn run(&self, url: Option<&str>, args: &[&str]) -> Output {
        self.cmd(url, "fake-1").args(args).output().unwrap()
    }
    fn librarian(&self, url: Option<&str>, model: &str) -> Value {
        let out = self
            .cmd(url, model)
            .args(["librarian", "--iterations", "1", "--interval", "1"])
            .output()
            .unwrap();
        let rows = String::from_utf8(out.stdout).unwrap();
        serde_json::from_str(rows.lines().last().unwrap()).unwrap()
    }
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)))
}

fn paths(result: &Value) -> Vec<&str> {
    result["hits"].as_array().unwrap().iter().map(|h| h["path"].as_str().unwrap()).collect()
}

#[test]
fn vectors_find_a_page_that_shares_no_word_with_the_question() {
    let url = embedder();
    let library = Library::new();
    let row = library.librarian(Some(&url), "fake-1");
    let vectors = &row["steps"]["vectors"];
    assert_eq!(vectors["status"], "ok", "{row}");
    assert_eq!(vectors["documents"], 3);
    let _room = common::Room::start(|| library.cmd(Some(&url), "fake-1"));
    let out = library.run(Some(&url), &["search", "zebra"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let result = json_of(&out);
    assert_eq!(result["engine"], "hybrid-rrf");
    assert_eq!(result["semantic"]["status"], "ok");
    assert_eq!(paths(&result)[0], "stripes.md");
    // The golden-set evaluation records the engine that answered.
    fs::write(library.path().join("golden.jsonl"), json!({"q":"zebra","answer_doc":"stripes.md"}).to_string()).unwrap();
    let out = library.run(Some(&url), &["evaluate", "--golden", "golden.jsonl"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report = json_of(&out);
    assert_eq!(report["engine"], "hybrid-rrf");
    assert_eq!(report["semantic"]["status"], "ok");
    assert_eq!(report["rows"][0]["rank"], 1);
}

#[test]
fn without_an_embedding_server_search_uses_words_and_says_so() {
    let library = Library::new();
    library.run(None, &["index"]);
    let _room = common::Room::start(|| library.cmd(None, "fake-1"));
    let out = library.run(None, &["search", "refunds"]);
    assert!(out.status.success());
    let result = json_of(&out);
    assert_eq!(result["engine"], "lexical-bm25");
    assert_eq!(result["semantic"]["status"], "not_configured");
    assert_eq!(paths(&result), ["refunds.md"]);
}

#[test]
fn a_server_that_stops_leaves_the_word_results_and_exit_status_1() {
    let url = embedder();
    let library = Library::new();
    library.librarian(Some(&url), "fake-1");
    let down = gone();
    let _room = common::Room::start(|| library.cmd(Some(&down), "fake-1"));
    let out = library.run(Some(&down), &["search", "refunds"]);
    assert_eq!(out.status.code(), Some(1));
    let result = json_of(&out);
    assert_eq!(result["semantic"]["status"], "failed");
    assert_eq!(result["engine"], "lexical-bm25");
    assert_eq!(paths(&result), ["refunds.md"]);
}

#[test]
fn the_librarian_records_a_failed_server_and_finishes_the_cycle() {
    let library = Library::new();
    let down = gone();
    // The cycle journals through the room, so it runs with one.
    let _room = common::Room::start(|| library.cmd(Some(&down), "fake-1"));
    let row = library.librarian(Some(&down), "fake-1");
    assert_eq!(row["steps"]["vectors"]["status"], "failed", "{row}");
    assert!(row["steps"]["vectors"]["error"].is_string());
    assert_ne!(row["status"], "failed", "{row}");
    assert!(row["steps"]["transcripts"].is_object(), "the cycle went on: {row}");
}

#[test]
fn only_changed_pages_are_embedded_again_and_a_new_model_embeds_all() {
    let url = embedder();
    let library = Library::new();
    assert_eq!(library.librarian(Some(&url), "fake-1")["steps"]["vectors"]["documents"], 3);
    assert_eq!(library.librarian(Some(&url), "fake-1")["steps"]["vectors"]["documents"], 0);
    fs::write(library.path().join("wiki/refunds.md"), "# Refunds\n\nRefunds take seven days.\n").unwrap();
    fs::remove_file(library.path().join("wiki/shipping.md")).unwrap();
    let row = library.librarian(Some(&url), "fake-1");
    assert_eq!(row["steps"]["vectors"]["documents"], 1, "{row}");
    assert_eq!(row["steps"]["vectors"]["removed"], 1, "{row}");
    assert_eq!(library.librarian(Some(&url), "fake-2")["steps"]["vectors"]["documents"], 2);
}
