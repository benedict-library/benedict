use serde_json::Value;
use std::{fs, path::Path, process::Command};

#[test]
fn guarded_transport_files_a_receipt_and_keeps_user_words_local() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path();
    let original_app = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let app = base.join("app");
    fs::create_dir_all(app.join("bin")).unwrap();
    // Substitute only the final network response in the existing transport. Leave its
    // registry reader, outbound privacy filter, byte extraction and metadata intact.
    fs::write(app.join("bin/web.py"),r#"
import runpy, sys, os
from pathlib import Path
actual=Path(os.environ['REAL_TEST_APP'])/'bin/web.py'
w=runpy.run_path(str(actual))
assert len(sys.argv)==7 and sys.argv[1]=='--agent-fetch'
assert 'private handoff' not in ' '.join(sys.argv[1:])
w['shelve'].__globals__['receive']=lambda url, guard: (200, url, 'text/plain', b'official published text')
w['native_fetch'](sys.argv[2:])
"#).unwrap();
    let run = |args: &[&str], expected: bool| {
        let out = Command::new(env!("CARGO_BIN_EXE_doseogwan"))
            .args(args)
            .env("DOSEOGWAN_APP", &app)
            .env("REAL_TEST_APP", original_app)
            .env("LIBRARY_DB", base.join("records.sqlite"))
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_CONFIG")
            .env_remove("LIBRARY_ROOM_HOME")
            .current_dir(base)
            .output()
            .unwrap();
        assert_eq!(
            out.status.success(),
            expected,
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(if expected { &out.stdout } else { &out.stderr }).unwrap()
    };
    run(&["init"], true);
    let query = "official title";
    let url = "https://example.org/public";
    let raw = "What is the official title? Leave the private handoff here.";
    let result = run(&["web-source", "--question", raw, query, url], true);
    assert_eq!(
        fs::read(result["original"].as_str().unwrap()).unwrap(),
        b"official published text"
    );
    assert!(
        base.join("wiki")
            .join(result["path"].as_str().unwrap())
            .exists()
    );
    let id = result["record_id"].as_i64().unwrap().to_string();
    let r = run(&["recall", "--library", "main", "--id", &id], true);
    assert_eq!(r["query"], query);
    assert_eq!(r["question"], raw);
    assert!(
        r["source"]
            .as_str()
            .unwrap()
            .contains(result["sha256"].as_str().unwrap())
    );
    run(&["web-source", "password=private", url], false);
}
