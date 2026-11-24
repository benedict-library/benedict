//! The librarian writes the index to the database and the search room answers from memory.
//! Results are the same as the earlier JSON index gave; the fixture holds those results.
mod common;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Library {
    dir: tempfile::TempDir,
    project: &'static str,
}

impl Library {
    fn new() -> Self {
        Library {
            dir: tempfile::tempdir().unwrap(),
            project: "search-room",
        }
    }
    /// The 40-page wiki and 3-file shelf of the parity fixture.
    fn fixture() -> Self {
        let library = Library::new();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ranking");
        copy(&source.join("wiki"), &library.path().join("wiki"));
        copy(&source.join("shelf"), &library.path().join("shelf"));
        fs::write(
            library.path().join("library.json"),
            json!({"version":1,"libraries":{"main":{"wiki_root":"wiki","shelf_root":"shelf","central":true}}})
                .to_string(),
        )
        .unwrap();
        library
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_benedict"));
        cmd.current_dir(self.path())
            .env("LIBRARY_PROJECT", self.project)
            .env("LIBRARY_ROOM_SOCKET", common::socket(self.path()))
            .env_remove("LIBRARY_DB")
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_DB_URL_FILE")
            .env_remove("LIBRARY_CONFIG")
            .env_remove("LIBRARY_ROOM_HOME");
        cmd
    }
    fn call(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.call(args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn room(&self) -> common::Room {
        common::Room::start(|| self.cmd())
    }
    fn database(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.path().join("library.sqlite3")).unwrap()
    }
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn paths(result: &Value) -> Vec<String> {
    result["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap().to_owned())
        .collect()
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let state = dir.join(".library-state");
    let Ok(entries) = fs::read_dir(state) else {
        return Vec::new();
    };
    entries
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with("-index.json"))
        .collect()
}

fn expected() -> Value {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ranking/expected.json");
    serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap()
}

#[test]
fn indexing_writes_the_catalog_to_the_database_and_no_json_file() {
    let library = Library::fixture();
    let indexed = library.ok(&["index"]);
    assert_eq!(indexed["documents"], 43);
    assert_eq!(indexed["catalog"], 43);
    assert!(indexed["words"].as_u64().unwrap() > 0);
    assert!(json_files(library.path()).is_empty());
    let catalog: i64 = library
        .database()
        .query_row("SELECT count(*) FROM benedict_catalog WHERE project='search-room'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(catalog, 43);
    let text_columns: i64 = library
        .database()
        .query_row(
            "SELECT count(*) FROM pragma_table_info('benedict_catalog') WHERE name IN ('text','body')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(text_columns, 0, "the catalog holds no document text");
    // Nothing changed: the version stays and nothing is written again.
    let again = library.ok(&["index"]);
    assert_eq!(again["indexed_at"], indexed["indexed_at"]);
    assert_eq!(again["changed"], 0);
}

#[test]
fn the_room_ranks_as_the_earlier_json_index_did() {
    let expected = expected();
    let library = Library::fixture();
    library.ok(&["index"]);
    let _room = library.room();
    for (query, want) in expected["synthetic"].as_object().unwrap() {
        let got = library.ok(&["search", "--top", "10", query]);
        assert_eq!(got["hits"], want["hits"], "{query}");
        assert_eq!(got.get("hint").unwrap_or(&Value::Null), &want["hint"], "{query}");
    }
    let demo = Library::new();
    demo.ok(&["init", "--demo"]);
    let _demo_room = demo.room();
    for (query, want) in expected["demo"].as_object().unwrap() {
        let got = demo.ok(&["search", "--top", "10", query]);
        assert_eq!(got["hits"], want["hits"], "{query}");
        assert_eq!(got.get("hint").unwrap_or(&Value::Null), &want["hint"], "{query}");
    }
}

#[test]
fn the_catalog_lists_up_to_three_rows_apart_from_the_hits() {
    let expected = expected();
    let library = Library::fixture();
    library.ok(&["index"]);
    let _room = library.room();
    let one = library.ok(&["search", "--top", "1", "반품"]);
    assert_eq!(one["hits"].as_array().unwrap().len(), 1);
    let catalog = one["catalog"].as_array().unwrap();
    assert_eq!(catalog.len(), 3);
    for row in catalog {
        assert!(row["title"].is_string() && row["summary"].is_string() && row["path"].is_string());
    }
    // A title that holds the query word comes first.
    let titled = catalog[0]["title"].as_str().unwrap();
    assert!(titled.contains("반품"), "{titled}");
    assert_eq!(one["hits"][0], expected["synthetic"]["반품"]["hits"][0]);
}

#[test]
fn a_changed_or_removed_document_shows_in_the_next_search() {
    let library = Library::fixture();
    library.ok(&["index"]);
    let _room = library.room();
    assert!(paths(&library.ok(&["search", "zebra"])).is_empty());
    let page = library.path().join("wiki/entities/policy-00.md");
    fs::write(&page, "# Zebra policy\n\nzebra crossings are free\n").unwrap();
    let changed = library.ok(&["index"]);
    assert_eq!(changed["changed"], 1);
    assert_eq!(paths(&library.ok(&["search", "zebra"])), ["entities/policy-00.md"]);
    fs::remove_file(&page).unwrap();
    let removed = library.ok(&["index"]);
    assert_eq!(removed["removed"], 1);
    assert_eq!(removed["documents"], 42);
    assert!(paths(&library.ok(&["search", "zebra"])).is_empty());
}

#[test]
fn without_the_room_search_fails_and_reads_no_files() {
    let library = Library::fixture();
    library.ok(&["index"]);
    let out = library.call(&["search", "반품"]);
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("search room not running"), "{error}");
    let out = library
        .cmd()
        .env_remove("LIBRARY_ROOM_SOCKET")
        .args(["search", "반품"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("search room not running"));
}

#[test]
fn a_changed_shelf_original_asks_for_a_new_index() {
    let library = Library::fixture();
    library.ok(&["index"]);
    let _room = library.room();
    assert_eq!(paths(&library.ok(&["search", "coupon"]))[0], "shelf:notes.md");
    fs::write(library.path().join("shelf/notes.md"), "# Notes\n\ncoupon changed\n").unwrap();
    let out = library.call(&["search", "coupon"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("rebuild index"));
}

#[test]
fn another_project_does_not_see_the_index() {
    let library = Library::fixture();
    library.ok(&["index"]);
    let other = Library {
        dir: tempfile::tempdir_in(library.path()).unwrap(),
        project: "another",
    };
    // The other project uses the same database and the same library key.
    let make = || {
        let mut cmd = library.cmd();
        cmd.env("LIBRARY_PROJECT", other.project)
            .env("LIBRARY_DB", library.path().join("library.sqlite3"))
            .env("LIBRARY_ROOM_SOCKET", common::socket(other.path()));
        cmd
    };
    let _room = common::Room::start(make);
    let out = make().args(["search", "반품"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("index missing"));
}

#[test]
fn the_librarian_removes_the_json_index_only_when_results_agree() {
    let library = Library::new();
    fs::create_dir_all(library.path().join("wiki")).unwrap();
    fs::write(library.path().join("wiki/alpha.md"), "# Alpha\n\nalpha beta\n").unwrap();
    fs::write(library.path().join("wiki/beta.md"), "# Beta\n\nbeta\n").unwrap();
    fs::write(
        library.path().join("library.json"),
        json!({"version":1,"libraries":{"main":{"wiki_root":"wiki","central":true}}}).to_string(),
    )
    .unwrap();
    library.ok(&["index"]);
    let root = fs::canonicalize(library.path().join("wiki")).unwrap();
    let earlier = |alpha_count: u64| {
        json!({"version":1,"root":root,"shelf_root":null,"indexed_at":"2025-01-01T00:00:00+00:00",
            "documents":{"alpha.md":"# Alpha\n\nalpha beta\n","beta.md":"# Beta\n\nbeta\n"},
            "counts":{"alpha.md":{"alpha":alpha_count,"beta":1},"beta.md":{"beta":2}},
            "lengths":{"alpha.md":alpha_count + 1,"beta.md":2}})
    };
    let file = old_json(&library);
    fs::write(&file, earlier(5).to_string()).unwrap();
    let row = librarian_row(&library);
    assert_eq!(row["steps"]["index"]["json_index"]["removed"], false, "{row}");
    assert!(row["steps"]["index"]["json_index"]["index_mismatch"].as_array().is_some_and(|v| !v.is_empty()));
    assert!(file.exists());
    fs::write(&file, earlier(2).to_string()).unwrap();
    let row = librarian_row(&library);
    assert_eq!(row["steps"]["index"]["json_index"]["removed"], true, "{row}");
    assert!(!file.exists());
}

#[test]
fn pages_changed_since_the_json_index_are_left_out_of_the_comparison() {
    // The JSON file was written before alpha.md changed, gone.md was removed and delta.md was added.
    // Those pages shift every score, so only the pages both indexes hold the same are compared.
    let library = Library::new();
    fs::create_dir_all(library.path().join("wiki")).unwrap();
    fs::write(library.path().join("wiki/alpha.md"), "# Alpha\n\nalpha beta\n").unwrap();
    fs::write(library.path().join("wiki/beta.md"), "# Beta\n\nbeta\n").unwrap();
    fs::write(library.path().join("wiki/delta.md"), "# Delta\n\ndelta beta\n").unwrap();
    fs::write(
        library.path().join("library.json"),
        json!({"version":1,"libraries":{"main":{"wiki_root":"wiki","central":true}}}).to_string(),
    )
    .unwrap();
    library.ok(&["index"]);
    let root = fs::canonicalize(library.path().join("wiki")).unwrap();
    let file = old_json(&library);
    let earlier = json!({"version":1,"root":root,"shelf_root":null,"indexed_at":"2025-01-01T00:00:00+00:00",
        "documents":{"alpha.md":"# Alpha\n\nalpha\n","beta.md":"# Beta\n\nbeta\n","gone.md":"# Gone\n\nbeta gone\n"},
        "counts":{"alpha.md":{"alpha":2},"beta.md":{"beta":2},"gone.md":{"beta":1,"gone":2}},
        "lengths":{"alpha.md":2,"beta.md":2,"gone.md":3}});
    fs::write(&file, earlier.to_string()).unwrap();
    let row = librarian_row(&library);
    let json_index = &row["steps"]["index"]["json_index"];
    assert_eq!(json_index["removed"], true, "{row}");
    assert_eq!(json_index["changed_since_json"], 3, "{row}");
    assert!(!file.exists());
}

/// The JSON index path of earlier releases, named from the registry path and key.
fn old_json(library: &Library) -> PathBuf {
    use sha2::{Digest, Sha256};
    let registry = fs::canonicalize(library.path().join("library.json")).unwrap();
    let hash = Sha256::digest(format!("{}\0main", registry.display()).as_bytes());
    library.path().join(".library-state").join(format!("{:.16}-index.json", format!("{hash:x}")))
}

/// One librarian cycle. Without a golden file it ends with status 1; its row holds the steps.
fn librarian_row(library: &Library) -> Value {
    let out = library.call(&["librarian", "--iterations", "1", "--interval", "1"]);
    let rows = String::from_utf8(out.stdout).unwrap();
    serde_json::from_str::<Value>(rows.lines().last().unwrap()).unwrap()
}
