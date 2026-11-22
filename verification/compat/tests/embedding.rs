mod optional_support;
use optional_support::{Server, call, result};
use serde_json::{Value, json};
use std::{fs, path::Path};
use tempfile::tempdir;

#[test]
fn local_provider_receives_expected_prompts_and_shortens_oversize_requests() {
    let dir = tempdir().unwrap();
    let server = Server::embeddings();
    let endpoint = server.url();
    let env = [
        ("LIBRARY_EMBED_URL", endpoint.as_str()),
        ("LIBRARY_EMBED_DIM", "2"),
        ("LIBRARY_EMBED_MODEL", "sample"),
    ];
    let v = result(call(
        dir.path(),
        r#"
import wiki_embed_client as api
plain=api.embed('catalogue','query')
os.environ['LIBRARY_EMBED_PROMPTS']='gemma'
query=api.embed('catalogue','query')
doc=api.embed('a public page','doc','Maps')
os.environ.pop('LIBRARY_EMBED_PROMPTS')
large=api.embed('a'*9000)
batch=api.embed_local_batch(['one','two'],'doc')
print(json.dumps([api.local_alive(),plain,query,doc,large,batch]))
"#,
        &Value::Null,
        &env,
    ));
    assert_eq!(v[0], true);
    for n in 1..=4 {
        assert_eq!(v[n], json!([[0.25, 0.5], "sample"]));
    }
    assert_eq!(v[5], json!([[0.25, 0.5], [0.25, 0.5]]));
    let posted = server
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(path, _)| path == "/v1/embeddings")
        .map(|(_, b)| serde_json::from_slice::<Value>(b).unwrap()["input"].clone())
        .collect::<Vec<_>>();
    assert_eq!(posted[0], "catalogue");
    assert_eq!(posted[1], "task: search result | query: catalogue");
    assert_eq!(posted[2], "title: Maps | text: a public page");
    let lengths = posted[3..6]
        .iter()
        .map(|v| v.as_str().unwrap().len())
        .collect::<Vec<_>>();
    assert_eq!(lengths, [8000, 3000, 1500]);
    assert_eq!(posted[6], json!(["one", "two"]));
}

#[test]
fn errors_and_chunk_boundaries_are_explicit() {
    let dir = tempdir().unwrap();
    let server = Server::embeddings();
    let endpoint = server.url();
    let missing = call(
        dir.path(),
        "import wiki_embed_client as api\napi.embed('map')",
        &Value::Null,
        &[],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("not configured"));
    let wrong = call(
        dir.path(),
        "import wiki_embed_client as api\napi.embed('map')",
        &Value::Null,
        &[("LIBRARY_EMBED_URL", &endpoint), ("LIBRARY_EMBED_DIM", "3")],
    );
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("dim"));
    let chunks = result(call(
        dir.path(),
        r#"
import wiki_embed_client as api
print(json.dumps([api.local_alive(),[len(c) for c in api.chunk_texts(data)],api.chunk_parent(api.chunk_key('map.md',2)),api.chunk_parent('map.md')]))
"#,
        &json!(format!("---\ntitle: map\n---\n{}", "x".repeat(2500))),
        &[],
    ));
    assert_eq!(chunks, json!([false, [1000, 1000, 500], "map.md", null]));
}

const SYNC: &str = r#"
import embed_sync, library_local
from types import SimpleNamespace
rows=data['rows']
def put(path, hash_, metadata, vector, model): rows[path]=[hash_,model]
def remove(paths):
    for p in paths: rows.pop(p,None)
store=SimpleNamespace(rows=lambda prefix:[(p,*v) for p,v in rows.items() if p.startswith(prefix)], upsert=put, delete=remove,commit=lambda:None,by_model=lambda:{})
r=embed_sync.sync(library_local.Library(Path('library.json').resolve()),None,store,**data['options'])
print(json.dumps({'summary':r,'rows':rows}))
"#;

fn sync(dir: &Path, url: &str, rows: &Value, options: Value) -> std::process::Output {
    call(
        dir,
        SYNC,
        &json!({"rows":rows,"options":options}),
        &[
            ("LIBRARY_EMBED_URL", url),
            ("LIBRARY_EMBED_DIM", "2"),
            ("LIBRARY_EMBED_MODEL", "sample"),
        ],
    )
}

#[test]
fn incremental_sync_skips_unchanged_rows_and_prunes_only_selected_library() {
    let temp = tempdir().unwrap();
    let dir = temp.path();
    let server = Server::embeddings();
    let url = server.url();
    result(call(
        dir,
        "import library_local\nlibrary_local.init(Path('library.json').resolve());print('null')",
        &Value::Null,
        &[],
    ));
    let cfg = dir.join("library.json");
    let mut registry: Value = serde_json::from_slice(&fs::read(&cfg).unwrap()).unwrap();
    registry["libraries"]["main"]["index_block"] = json!({"draft":"UNPUBLISHED-FIXTURE"});
    fs::write(cfg, registry.to_string()).unwrap();
    let wiki = dir.join("wiki/entities");
    fs::create_dir_all(&wiki).unwrap();
    let note = "This is a synthetic catalogue entry about a map on public display. ".repeat(8);
    for n in ["map", "book"] {
        fs::write(wiki.join(format!("{n}.md")), &note).unwrap();
    }
    fs::write(wiki.join("short.md"), "short").unwrap();
    let seeded = result(sync(dir, &url, &json!({}), json!({})));
    let mut rows = seeded["rows"].clone();
    assert_eq!(seeded["summary"]["embedded"], 2);
    assert_eq!(rows.as_object().unwrap().len(), 2);
    assert_eq!(
        result(sync(dir, &url, &rows, json!({})))["summary"]["skipped"],
        2
    );
    let key = rows
        .as_object()
        .unwrap()
        .keys()
        .find(|k| k.ends_with("book.md"))
        .unwrap()
        .clone();
    rows[&key][1] = json!("retired-model");
    fs::write(wiki.join("map.md"), format!("{note} updated")).unwrap();
    let refreshed = result(sync(dir, &url, &rows, json!({})));
    assert_eq!(refreshed["summary"]["embedded"], 2);
    assert_eq!(refreshed["summary"]["reembedded_for_model"], 1);
    fs::write(wiki.join("new.md"), &note).unwrap();
    fs::write(
        wiki.join("unpublished.md"),
        format!("UNPUBLISHED-FIXTURE {note}"),
    )
    .unwrap();
    fs::write(wiki.join("later.md"), &note).unwrap();
    rows = result(sync(
        dir,
        &url,
        &refreshed["rows"],
        json!({"path":"wiki/entities/new.md"}),
    ))["rows"]
        .clone();
    assert_eq!(rows.as_object().unwrap().len(), 3);
    {
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&wiki, dir.join("linked-collection")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&wiki, dir.join("linked-collection")).unwrap();
        assert_eq!(
            result(sync(
                dir,
                &url,
                &rows,
                json!({"path":"linked-collection/later.md"})
            ))["summary"]["embedded"],
            1
        );
    }
    assert!(
        !sync(dir, &url, &rows, json!({"path":"missing.md"}))
            .status
            .success()
    );
    let chunks = result(sync(dir, &url, &json!({}), json!({"chunks":true})));
    assert!(chunks["summary"]["chunks_written"].as_u64().unwrap() >= 4);
    assert!(
        chunks["rows"]
            .as_object()
            .unwrap()
            .keys()
            .all(|p| p.contains("::c"))
    );
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(_, b)| !String::from_utf8_lossy(b).contains("UNPUBLISHED-FIXTURE"))
    );
    fs::remove_file(wiki.join("book.md")).unwrap();
    rows["unrelated-library/page.md"] = json!(["unrelated", "sample"]);
    rows[format!("{key}::c0")] = json!(["old", "sample"]);
    assert_eq!(
        result(sync(dir, &url, &rows, json!({})))["summary"]["removed"],
        0
    );
    let cleaned = result(sync(dir, &url, &rows, json!({"prune":true})));
    assert_eq!(cleaned["summary"]["removed"], 2);
    assert!(cleaned["rows"].get(&key).is_none());
    assert!(cleaned["rows"].get("unrelated-library/page.md").is_some());
    assert!(
        !sync(
            dir,
            &url,
            &rows,
            json!({"prune":true,"path":"wiki/entities/map.md"})
        )
        .status
        .success()
    );
    let output = call(
        dir,
        "import embed_sync\nsys.argv=['embed_sync','--config','library.json'];sys.exit(embed_sync.main())",
        &Value::Null,
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not configured"));
}

#[cfg(unix)]
#[test]
fn embedding_client_reads_the_selected_unix_socket() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temp.path().join("embed.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("GET /health "));
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
        }
        let body = br#"{"status":"ok"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    let url = format!("unix://{}", socket.display());
    let data = result(call(
        temp.path(),
        "import wiki_embed_client as api\nprint(json.dumps(api.local_alive()))",
        &Value::Null,
        &[("LIBRARY_EMBED_URL", &url)],
    ));
    assert_eq!(data, true);
    thread.join().unwrap();
}
