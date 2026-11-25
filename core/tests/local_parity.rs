// A mounted wiki may have been indexed by an earlier Python installation. Search reads the
// index the Rust librarian writes to the database; both clients must rank the same documents.
mod common;
use serde_json::Value;
use std::{fs, path::Path, process::Command};

#[test]
fn existing_index_and_new_index_can_be_read_by_both_clients() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path();
    let legacy = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin/library.py");
    let run = |native: bool, args: &[&str]| {
        let mut command = if native {
            Command::new(env!("CARGO_BIN_EXE_benedict"))
        } else {
            let mut c = Command::new("python3");
            c.arg(&legacy);
            c
        };
        command
            .args(args)
            .current_dir(data)
            // The compatibility client searches through this build of the Rust CLI.
            .env("BENEDICT_BIN", env!("CARGO_BIN_EXE_benedict"))
            .env("LIBRARY_ROOM_SOCKET", common::socket(data))
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_DB")
            .env_remove("LIBRARY_ROOM_HOME")
            .env_remove("LIBRARY_CONFIG");
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    run(false, &["init", "--demo"]);
    let _room = common::Room::start(|| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_benedict"));
        command
            .current_dir(data)
            .env("LIBRARY_ROOM_SOCKET", common::socket(data))
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_DB")
            .env_remove("LIBRARY_ROOM_HOME")
            .env_remove("LIBRARY_CONFIG");
        command
    });
    fs::write(
        data.join("wiki/entities/unicode.md"),
        "# Straße\r\n한글 도서관\r\nΟΣ _ß İﬃ",
    )
    .unwrap();
    run(false, &["add", "--key", "river"]);
    fs::write(data.join("wiki-river/decision.md"), "# 조사\n한글 Straße").unwrap();
    // An earlier Python index stays beside the registry; the Rust index is the one searched.
    for native_index in [false, true] {
        for key in ["main", "river"] {
            run(native_index, &["index", "--library", key]);
            run(true, &["index", "--library", key]);
        }
        for query in ["REFUND", "STRASSE 한글", "οσ", "_ss", "İﬃ", "no such token"] {
            let args = ["search", "--top", "2", query];
            assert_eq!(run(false, &args), run(true, &args));
        }
        let args = ["read", "entities/unicode.md"];
        assert_eq!(run(false, &args), run(true, &args));
    }
}

#[test]
fn previous_clients_approval_covers_exactly_the_same_wiki_and_skill() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let run = |native: bool, args: &[&str], ok: bool| {
        let mut cmd = if native {
            Command::new(env!("CARGO_BIN_EXE_benedict"))
        } else {
            let mut c = Command::new("python3");
            c.arg(repo.join("bin/library.py"));
            c
        };
        let ret = cmd
            .args(args)
            .current_dir(base)
            .env("BENEDICT_APP", repo)
            .env("LIBRARY_ROOM_SOCKET", common::socket(base))
            .env_remove("LIBRARY_CONFIG")
            .env_remove("LIBRARY_DB")
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_ROOM_HOME")
            .output()
            .unwrap();
        assert_eq!(
            ret.status.success(),
            ok,
            "{args:?}: {}",
            String::from_utf8_lossy(&ret.stderr)
        );
        serde_json::from_slice::<Value>(if ok { &ret.stdout } else { &ret.stderr }).unwrap()
    };
    run(false, &["init"], true);
    let _room = common::Room::start(|| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_benedict"));
        command
            .current_dir(base)
            .env("BENEDICT_APP", repo)
            .env("LIBRARY_ROOM_SOCKET", common::socket(base))
            .env_remove("LIBRARY_CONFIG")
            .env_remove("LIBRARY_DB")
            .env_remove("LIBRARY_DB_URL")
            .env_remove("LIBRARY_ROOM_HOME");
        command
    });
    fs::write(base.join("원문.md"), "original").unwrap();
    let skill = base.join("기존 스킬");
    fs::create_dir(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\r\nname: packing\r\ndescription: Amber labels\r\n---\r\nUse the table.",
    )
    .unwrap();
    let script = skill.join("order.sh");
    fs::write(&script, "echo packing").unwrap();
    fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    for (kind, path) in [("wiki", "원문.md"), ("skill", "기존 스킬")] {
        let mut args = vec!["import", "--library", "main", kind, path];
        let py = run(false, &args, true);
        assert_eq!(run(true, &args, true), py);
        args.extend(["--approve", py["confirmation"].as_str().unwrap()]);
        run(true, &args, true);
        let result = run(true, &args, true);
        if kind == "wiki" {
            fs::write(base.join(path), "new content").unwrap();
            assert!(
                run(true, &args, false)["error"]
                    .as_str()
                    .unwrap()
                    .contains("preview")
            );
        } else {
            let companion =
                Path::new(result["shelf"].as_str().unwrap()).join("skills/packing/order.sh");
            assert_ne!(
                fs::metadata(companion).unwrap().permissions().mode() & 0o111,
                0
            );
            assert!(
                run(true, &["search", "amber"], true)["hits"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|h| h["path"] == "entities/skill_packing.md")
            );
        }
    }
}
