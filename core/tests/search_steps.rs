// One search command covers what an agent needs before it answers: the central library and the libraries it may
// read, answered by the search room from each library's index, and, only when asked with --web, the configured web
// step after the inside results. A library without an index is not searched in its files; the error says so.
mod common;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn benedict(data: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_benedict"));
    command
        .args(args)
        .current_dir(data)
        .env("LIBRARY_ROOM_SOCKET", common::socket(data));
    for name in [
        "LIBRARY_DB_URL",
        "LIBRARY_DB_URL_FILE",
        "LIBRARY_DB",
        "LIBRARY_ROOM_HOME",
        "LIBRARY_CONFIG",
        "LIBRARY_WEB_CMD",
        "LIBRARY_WEB_TIMEOUT",
    ] {
        command.env_remove(name);
    }
    command
}

fn result(out: Output, code: i32) -> Value {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn ok(data: &Path, args: &[&str]) -> Value {
    result(benedict(data, args).output().unwrap(), 0)
}

fn write(data: &Path, rel: &str, body: &str) {
    let path = data.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn paths(found: &Value) -> Vec<&str> {
    found["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect()
}

fn near<'a>(found: &'a Value, key: &str) -> &'a Value {
    found["near"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["library"] == key)
        .unwrap_or_else(|| panic!("no near entry for {key}: {found}"))
}

/// A central library that may read two others, and one more library it may not read.
fn libraries(data: &Path) {
    write(
        data,
        "library.json",
        &json!({"version":1,"libraries":{
            "main":{"name":"Main","wiki_root":"wiki","central":true,"dirs":["entities"],"reads":["branch","annex"]},
            "branch":{"name":"Branch","wiki_root":"wiki-branch"},
            "annex":{"name":"Annex","wiki_root":"wiki-annex"},
            "private":{"name":"Private","wiki_root":"wiki-private"}
        }})
        .to_string(),
    );
    write(data, "wiki/entities/garden.md", "# Garden\nMain garden policy.\n");
    write(data, "wiki/drafts/garden.md", "# Draft\nA garden draft outside dirs.\n");
    write(data, "wiki-branch/notes/garden.md", "# Branch garden\nPlanting the garden in spring.\n");
    write(data, "wiki-annex/roads.md", "# Roads\nPaving.\n");
    write(data, "wiki-private/garden.md", "# Private garden\nNot for the central library.\n");
}

fn index_all(data: &Path) {
    for key in ["main", "branch", "annex", "private"] {
        ok(data, &["index", "--library", key]);
    }
}

#[test]
fn without_a_library_the_search_reads_the_central_library_and_only_the_libraries_it_may_read() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    libraries(data);
    index_all(data);
    let _room = common::Room::start(|| benedict(data, &[]));
    let found = ok(data, &["search", "garden"]);
    assert_eq!(found["library"], "main");
    assert_eq!(paths(&found), ["entities/garden.md"]);
    let rows: Vec<_> = found["near"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["library"].as_str().unwrap())
        .collect();
    assert_eq!(rows, ["branch", "annex"]);
    assert_eq!(paths(near(&found, "branch")), ["notes/garden.md"]);
    assert_eq!(near(&found, "branch")["name"], "Branch");
    assert!(paths(near(&found, "annex")).is_empty());
    assert!(found.get("fallback").is_none());
    let one = ok(data, &["search", "--library", "private", "garden"]);
    assert_eq!(paths(&one), ["garden.md"]);
    assert!(one.get("near").is_none());
}

#[test]
fn a_library_without_an_index_is_not_searched_and_the_error_says_how_to_build_one() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    libraries(data);
    let _room = common::Room::start(|| benedict(data, &[]));
    // No index yet: nothing reads the wiki files instead.
    let out = benedict(data, &["search", "garden"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("run index or librarian"), "{error}");
    // A library the central one may read and that has no index says so in its row.
    ok(data, &["index", "--library", "main"]);
    let found = ok(data, &["search", "garden"]);
    assert_eq!(paths(&found), ["entities/garden.md"]);
    let why = near(&found, "branch")["error"].as_str().unwrap();
    assert!(why.contains("index missing"), "{why}");
    // After the librarian builds every index, every library answers.
    index_all(data);
    let rebuilt = ok(data, &["search", "garden"]);
    assert!(rebuilt["indexed_at"].is_string());
    assert_eq!(paths(near(&rebuilt, "branch")), ["notes/garden.md"]);
    assert!(near(&rebuilt, "branch").get("error").is_none());
    // A question without words, or an unknown library, is still refused.
    assert_eq!(benedict(data, &["search", "--", "!!"]).output().unwrap().status.code(), Some(2));
    assert_eq!(
        benedict(data, &["search", "--library", "ghost", "garden"]).output().unwrap().status.code(),
        Some(2)
    );
}

#[cfg(unix)]
#[test]
fn the_web_step_runs_only_when_asked_after_the_inside_results_and_receives_only_library_and_query() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    libraries(data);
    index_all(data);
    let _room = common::Room::start(|| benedict(data, &[]));
    // Not configured: the inside results stay, the step says how to keep a public source, exit 1.
    let unset = result(benedict(data, &["search", "--web", "garden"]).output().unwrap(), 1);
    assert_eq!(paths(&unset), ["entities/garden.md"]);
    assert_eq!(unset["web"]["configured"], false);
    assert!(unset["web"]["next"].as_str().unwrap().contains("web-source"));
    write(
        data,
        "web.sh",
        "printf '%s\\n' \"$@\" > \"$SAMPLE_WEB_LOG\"\necho 'Sample external finding'\n[ -z \"$SAMPLE_FAIL_WEB\" ] || { echo 'means failed' >&2; exit 3; }\n",
    );
    let log = data.join("web.log");
    let web = |args: &[&str], fail: bool| {
        let mut command = benedict(data, args);
        command
            .env("LIBRARY_WEB_CMD", format!("sh '{}'", data.join("web.sh").display()))
            .env("SAMPLE_WEB_LOG", &log);
        if fail {
            command.env("SAMPLE_FAIL_WEB", "1");
        }
        command.output().unwrap()
    };
    // Without --web nothing goes out.
    result(web(&["search", "garden"], false), 0);
    assert!(!log.exists());
    for (key, args) in [
        ("main", vec!["search", "--web", "--question", "the asker's own words", "garden"]),
        ("branch", vec!["search", "--library", "branch", "--web", "garden"]),
    ] {
        let found = result(web(&args, false), 0);
        assert!(!paths(&found).is_empty());
        assert_eq!(found["web"]["library"], key);
        assert_eq!(found["web"]["exit_code"], 0);
        assert!(found["web"]["output"].as_str().unwrap().contains("Sample external finding"));
        assert_eq!(fs::read_to_string(&log).unwrap(), format!("{key}\ngarden\n"));
    }
    // A failing web step keeps the inside results and returns its exit code.
    let failed = result(web(&["search", "--web", "garden"], true), 3);
    assert_eq!(paths(&failed), ["entities/garden.md"]);
    assert_eq!(failed["web"]["exit_code"], 3);
    assert!(failed["web"]["error"].as_str().unwrap().contains("means failed"));
}
