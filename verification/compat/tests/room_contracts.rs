mod rooms;
use rooms::Library;
use serde_json::{Value, json};
use std::fs;

#[test]
fn meeting_mail_accepts_files_and_input_and_preserves_old_entries() {
    let lib = Library::new();
    lib.room(&["start", "main-meeting"], "");
    let mail = |args: &[&str], input: &str| {
        let mut argv = vec!["--room", "main-meeting"];
        argv.extend_from_slice(args);
        lib.cmd("mail", &argv, input)
    };
    assert_eq!(mail(&["list"], "").ok().trim(), "회신함 비어 있음");
    assert!(
        mail(&["put", "review"], "first 'quoted'\n")
            .ok()
            .contains("1 건")
    );
    let note = lib.write("second.md", "Second note");
    assert!(mail(&["put", "review", &note], "").ok().contains("2 건"));
    lib.exec("main-meeting", "mail['legacy'] = ['unstructured message']");
    let list = mail(&["list"], "").ok();
    assert!(list.contains("review : 2 건"));
    assert!(list.contains("legacy : 1 건 (마지막 미상 )"));
    let data = mail(&["get", "review"], "").ok();
    assert!(data.contains("reader\nfirst 'quoted'\n[2]") && data.contains("Second note\n-- 2 건"));
    assert!(
        mail(&["get", "review", "--clear"], "")
            .ok()
            .contains("비움")
    );
    assert!(mail(&["get", "review"], "").ok().contains("-- 0 건"));
    for result in [
        mail(&["put", "review"], ""),
        mail(&["get"], ""),
        lib.cmd("mail", &["list"], ""),
    ] {
        assert_eq!(result.status, Some(2));
    }
}

#[test]
fn meeting_materials_have_explicit_summaries_and_retrievable_text_or_structured_results() {
    let lib = Library::new();
    lib.room(&["start", "main-meeting"], "");
    let task = |args: &[&str], input: &str| {
        let mut argv = vec!["--room", "main-meeting"];
        argv.extend_from_slice(args);
        lib.cmd("task", &argv, input)
    };
    let text = format!(
        "# Garden\n\nPlan for spring.\nNew bulbs.\n{}",
        "Measurements available.\n".repeat(400)
    );
    let source = lib.write("plan.md", &text);
    assert!(
        task(
            &[
                "add",
                "garden",
                &source,
                "--summary",
                "Plan for spring.\nReviewed this month."
            ],
            ""
        )
        .ok()
        .contains("task_garden plan")
    );
    let note=lib.write("note.md","Summary first.\nSummary second.\n\n# Report title\n\nBody with garden words.\nNext line.\n");
    assert!(
        task(
            &[
                "add",
                "garden",
                &note,
                "--summary",
                "A report.",
                "--key",
                "n"
            ],
            ""
        )
        .ok()
        .contains("2 건")
    );
    let listing = task(&["list", "garden"], "").ok();
    for needle in [
        "[plan]",
        "Plan for spring.\nReviewed this month.",
        "[n]",
        "A report.",
        "결과 자리: 없음",
    ] {
        assert!(listing.contains(needle), "{listing}");
    }
    assert!(!listing.contains("Measurements available"));
    assert_eq!(
        task(&["get", "garden", "plan", "--lines", "2-3"], "").ok(),
        "2: \n3: Plan for spring.\n-- 404 줄 중 2 - 3\n"
    );
    let found = task(&["get", "garden", "n", "--q", "first garden"], "").ok();
    assert!(
        found.contains("Summary first.")
            && found.contains("…")
            && found.contains("Body with garden words.")
    );
    assert!(
        task(&["put", "garden", "result"], "Seven days\n")
            .ok()
            .contains("results result")
    );
    assert_eq!(task(&["get", "garden", "result"], "").ok(), "Seven days\n");
    assert!(
        task(&["get", "garden", "result", "--q", "days"], "")
            .ok()
            .contains("1: Seven days")
    );
    assert!(
        task(&["get", "garden", "absent"], "")
            .failed()
            .out
            .contains("결과 자리: result")
    );
    let empty = lib.write("empty.md", "  \n");
    for args in [
        vec!["add", "garden", &note],
        vec!["add", "garden", &note, "--summary", " "],
        vec!["add", "garden", &empty, "--summary", "sample"],
        vec!["get", "garden", "n", "--lines", "3-1"],
        vec!["put", "garden", "empty"],
        vec!["get"],
    ] {
        assert_eq!(task(&args, "").status, Some(2));
    }
    assert_eq!(task(&["list", "absent"], "").status, Some(1));
    assert_eq!(lib.cmd("task", &["list", "garden"], "").status, Some(2));
    lib.exec("main-meeting",r#"task_old = {'materials': {'old.md': 'paragraph one\nparagraph two', 'outline': '# Outline\n\n| a | b |\n- first point\n- second point', 'header': '---\ntitle: Choices\nsources: ["a",\n"b"]\n---\nLead one.\nLead two.', 'loose': '---\ntitle: Loose\nsources:\n  - example\n# Heading\nBody one.\nBody two.'}, 'results': {'analysis': {'shade': 'blue', 'count': 2}}}
task_minutes = {'목적': 'Meeting', '안건': 'First\nSecond', '재료 변수': ['attachment']}
task_attachment = {'원문': 'a\nb\nc'}"#);
    let legacy = task(&["list", "old"], "").ok();
    for needle in [
        "(옛 형식)",
        "paragraph one\nparagraph two",
        "Outline\nfirst point\nsecond point",
        "Choices\nLead one.\nLead two.",
        "Loose\nBody one.\nBody two.",
    ] {
        assert!(legacy.contains(needle), "{legacy}");
    }
    assert_eq!(
        task(&["get", "old", "old.md"], "").ok(),
        "paragraph one\nparagraph two\n"
    );
    assert!(
        task(&["get", "old", "analysis", "--q", "count"], "")
            .ok()
            .contains("2: count: 2")
    );
    assert!(task(&["list", "minutes"], "").ok().contains("[안건]"));
    assert_eq!(
        task(&["get", "minutes", "안건"], "").ok(),
        "First\nSecond\n"
    );
    assert!(
        task(&["get", "attachment", "원문", "--lines", "2-3"], "")
            .ok()
            .contains("2: b\n3: c")
    );
}

#[test]
fn startup_reports_the_error_and_preserves_its_log() {
    let lib = Library::new();
    fs::create_dir_all(lib.home.join("rooms/main.sock")).unwrap();
    let error = lib.cmd("room", &["start", "main"], "").failed();
    assert!(
        error.err.contains("failed to open main") && error.err.contains("Error"),
        "{}",
        error.err
    );
    assert!(
        fs::read_to_string(lib.path("rooms/main.log"))
            .unwrap()
            .contains("Error")
    );
}

#[test]
fn four_workrooms_resume_independent_snapshots_and_filter_sensitive_variables() {
    let lib = Library::new();
    for name in ["reading", "meeting", "search", "librarian"] {
        lib.room(&["start", name], "");
        assert!(lib.room(&["start", name], "").contains("open"));
        lib.exec(name, &format!("message = {name:?}\napi_token = None"));
        assert!(lib.room(&["vars", name], "").contains("message: str"));
        assert!(lib.room(&["status", name], "").contains("idle"));
    }
    for name in ["reading", "meeting", "search", "librarian"] {
        assert_eq!(lib.exec(name, "print(message)").trim(), name);
        lib.room(&["stop", name], "");
        let manifest: Value = serde_json::from_slice(
            &fs::read(lib.path(&format!("rooms/{name}.snap.json"))).unwrap(),
        )
        .unwrap();
        let keys = manifest["saved"].as_array().unwrap();
        assert!(keys.contains(&json!("message")) && !keys.contains(&json!("api_token")));
        lib.room(&["start", name], "");
        assert_eq!(lib.exec(name, "print(message)").trim(), name);
    }
    lib.cmd(
        "room",
        &["exec", "reading", "--timeout", ".1"],
        "while True: pass",
    )
    .failed();
    lib.cmd("room", &["start", "../invalid"], "").failed();
    assert!(!lib.home.join("invalid.pid").exists());
    assert_eq!(lib.exec("reading", "print(message)").trim(), "reading");
    let mut isolated = Library::new();
    isolated.env.remove("LIBRARY_ROOM_HOME");
    isolated.cmd("room", &["list"], "").failed();
    assert!(!isolated.home.join(".kernel_rooms").exists());
}

#[test]
fn a_route_forwards_commands_and_the_socket_route_uses_the_same_remote_state() {
    let local = Library::new();
    let remote = Library::new();
    remote.room(&["start", "branch"], "");
    let executable = benedict_compat::python()
        .to_string_lossy()
        .replace('\\', "/");
    let entry = benedict_compat::repo_root().join("bin/room.py");
    let route=local.write("forward.py",&format!("import os, runpy\nos.environ['LIBRARY_ROOM_HOME'] = {}\nrunpy.run_path({},run_name='__main__')\n",json!(remote.path("rooms")),json!(entry)));
    local.write(
        "rooms/branch.route",
        &format!("\"{executable}\" \"{}\"\n", route.replace('\\', "/")),
    );
    local.exec("branch", "value=7");
    assert_eq!(remote.exec("branch", "print(value)").trim(), "7");
    assert_eq!(local.exec("branch", "print(value+1)").trim(), "8");
    local.room(&["start", "branch"], "");
    assert!(!local.home.join("rooms/branch.pid").exists());
    local.write(
        "rooms/branch.route",
        &format!(
            "\"{executable}\" -c \"import sys; sys.exit(1)\"\nsocket {}\n",
            remote.path("rooms")
        ),
    );
    assert_eq!(local.cmd("room", &["start", "branch"], "").status, Some(1));
    assert_eq!(local.exec("branch", "print(value*2)").trim(), "14");
    assert!(local.room(&["vars", "branch"], "").contains("value: int"));
    let list = local.room(&["list"], "");
    assert_eq!(
        list.lines()
            .filter(|l| l.starts_with("branch") && l.contains("route"))
            .count(),
        1
    );
    local.room(&["stop", "branch"], "");
    assert!(remote.room(&["list"], "").contains("off(snapshot)"));
    assert_eq!(local.cmd("room", &["exec", "branch"], "1").status, Some(2));
}

#[test]
fn library_record_mode_keeps_short_cards_in_rooms_and_full_text_in_the_index() {
    let lib = Library::new();
    lib.init();
    let text = format!(
        "---\ntitle: Garden study\n---\nThe garden was planted in spring. {}",
        "Daffodils bloom. ".repeat(200)
    );
    let page = lib.write("wiki/entities/garden.md", &text);
    let mut reg = lib.registry();
    let read_body = || {
        serde_json::from_str::<String>(&lib.exec(
            "main-reading",
            &format!("import json\nprint(json.dumps(docs[{}]))", json!(page)),
        ))
        .unwrap()
    };
    lib.sync(&[]);
    assert_eq!(read_body(), text);
    reg["libraries"]["main"]["records"] = json!({"chars":60});
    lib.set_registry(&reg);
    lib.sync(&["--full"]);
    let card = read_body();
    assert!(
        card.starts_with("---\ntitle: Garden study")
            && card.contains("garden was planted")
            && card.contains("서지 레코드")
            && card.len() < 360
    );
    let short = lib
        .python(
            &[
                "-c",
                "import library_local; print(library_local.record_text('short note',60),end='')",
            ],
            "",
        )
        .ok();
    assert_eq!(short, "short note");
    assert!(lib.exec("main-search", "print(len(_len))").contains('3'));
    // The index adapter consumes the unshortened version of the original.
    let returned=lib.python(&["-c","from library_local import Library; import sys; from pathlib import Path; print(Library(Path(sys.argv[1])).index_body('main',Path(sys.argv[2])),end='')",&lib.path("library.json"),&page],"").ok();
    assert_eq!(returned, text);
    for setting in [
        json!({"chars":0}),
        json!({"chars":"bad"}),
        json!({"size":5}),
        json!("yes"),
    ] {
        reg["libraries"]["main"]["records"] = setting;
        lib.set_registry(&reg);
        assert!(
            lib.cmd("room_sync", &["--config", &lib.path("library.json")], "")
                .failed()
                .err
                .contains("records")
        );
    }
}

#[test]
fn updates_reindex_changes_and_deletions_and_rebuild_after_a_restart() {
    let lib = Library::new();
    lib.init();
    assert_eq!(lib.sync(&[])["sent"], 2);
    assert_eq!(lib.sync(&[])["sent"], 0);
    lib.exec("main-meeting", "mail['sample'] = ['received']");
    lib.write(
        "wiki/entities/shipping.md",
        "# Shipping\nFriday was selected.\n",
    );
    lib.write("wiki/entities/spring.md", "# Spring\nGarden preparation.\n");
    let change = lib.sync(&[]);
    for (key, expected) in [
        ("sent", json!(2)),
        ("deleted", json!(0)),
        ("reading_full", json!(false)),
        ("search_full", json!(false)),
    ] {
        assert_eq!(change[key], expected);
    }
    assert!(
        lib.exec(
            "main-reading",
            "print([v for k,v in docs.items() if k.endswith('shipping.md')][0])"
        )
        .contains("Friday")
    );
    assert_eq!(
        lib.exec("main-search", "print(len(_len),sync_meta['n'])")
            .trim(),
        "3 3"
    );
    lib.room(&["stop", "main-search"], "");
    lib.room(&["start", "main-search"], "");
    let rebuilt = lib.sync(&[]);
    assert_eq!(rebuilt["search_full"], true);
    assert_eq!(rebuilt["reading_full"], false);
    assert_eq!(rebuilt["sent"], 0);
    fs::remove_file(lib.path("wiki/entities/spring.md")).unwrap();
    assert_eq!(lib.sync(&[])["deleted"], 1);
    assert!(
        lib.exec("main-meeting", "print(mail['sample'])")
            .contains("received")
    );
    assert_eq!(lib.sync(&["--full"])["reading_full"], true);
    assert!(
        lib.cmd(
            "room_sync",
            &[
                "--config",
                &lib.path("library.json"),
                "--path",
                &lib.path("elsewhere.md")
            ],
            ""
        )
        .failed()
        .err
        .contains("not in library")
    );
    let saved: Value = serde_json::from_str(
        &lib.cmd(
            "library",
            &[
                "save",
                "--config",
                &lib.path("library.json"),
                "Seasonal report",
                &"The field notes will be shared on Monday. ".repeat(5),
            ],
            "",
        )
        .ok(),
    )
    .unwrap();
    let path = saved["path"].as_str().unwrap();
    assert_eq!(lib.exec("main-reading",&format!("from pathlib import Path\nprint(any(Path(p).as_posix().endswith({}) for p in docs))",json!(path))).trim(),"True");
}

#[test]
fn a_private_local_channel_authenticates_clients_and_round_trips_utf8() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpStream,
        thread,
        time::{Duration, Instant},
    };
    let lib = Library::new();
    let path = lib.path("channel.sock");
    let server = "import room_channel,sys\nlistener=room_channel.Listener(sys.argv[1],tcp=True)\nlistener.sock.settimeout(10)\ntry:\n for _ in range(2):\n  client=listener.accept()\n  if client is not None:\n   sock,reader=client\n   with sock,reader: sock.sendall(reader.readline())\nfinally: listener.close()";
    thread::scope(|scope| {
        let handle = scope.spawn(|| lib.python(&["-c", server, &path], ""));
        let start = Instant::now();
        let fields = loop {
            if let Ok(text) = fs::read_to_string(&path)
                && text.split_whitespace().count() == 2
            {
                break text;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "listener never published an endpoint"
            );
            thread::sleep(Duration::from_millis(20));
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let port = fields
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let mut rejected = TcpStream::connect(("127.0.0.1", port)).unwrap();
        rejected
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        rejected.write_all(b"invalid-test-value\n").unwrap();
        let mut received = String::new();
        assert_eq!(
            BufReader::new(rejected).read_line(&mut received).unwrap(),
            0
        );
        let source = "import room_channel,sys\nwith room_channel.connect(sys.argv[1],5,tcp=True) as s:\n s.sendall('Round trip.\\n'.encode()); print(s.makefile('rb').readline().decode(),end='')";
        assert_eq!(lib.python(&["-c", source, &path], "").ok(), "Round trip.\n");
        handle.join().unwrap().ok();
    });
    assert!(!lib.home.join("channel.sock").exists());
    let missing = lib
        .python(
            &[
                "-c",
                "import room_channel,sys; room_channel.connect(sys.argv[1],1,tcp=True)",
                &path,
            ],
            "",
        )
        .failed();
    assert!(missing.err.contains("FileNotFoundError"));
}

#[cfg(unix)]
#[test]
fn a_long_private_path_keeps_its_unix_socket_inside_the_room() {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let mut lib = Library::new();
    let directory = lib.home.join("연구".repeat(26));
    fs::create_dir(&directory).unwrap();
    lib.env
        .insert("LIBRARY_ROOM_HOME".into(), directory.display().to_string());
    lib.room(&["start", "reading"], "");
    let socket = directory.join("reading.sock");
    let meta = fs::metadata(&socket).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        lib.exec("reading", "print('Received.')").trim(),
        "Received."
    );
    lib.room(&["stop", "reading"], "");
    assert!(!socket.exists());
    assert_eq!(lib.cmd("room", &["status", "reading"], "").status, Some(2));
}

#[test]
fn reading_opens_the_original_and_persists_it_but_only_shows_the_requested_lines() {
    let lib = Library::new();
    lib.init();
    let original = format!(
        "---\ntitle: Trial\n---\n# Garden\n{} preceding\nLavender {} MATCH\n{} following\n# Next\n",
        "A".repeat(500),
        "B".repeat(601),
        "C".repeat(500)
    );
    lib.write("wiki/entities/trial.md", &original);
    let mut reg = lib.registry();
    reg["libraries"]["main"]["records"] = true.into();
    reg["libraries"]["main"]["reads"] = json!(["branch", "box"]);
    reg["libraries"]["branch"] = json!({"name":"Fiction branch","wiki_root":"branch-wiki"});
    reg["libraries"]["box"] = json!({"name":"Box","search":"box","room":"box"});
    lib.write(
        "branch-wiki/entities/branch.md",
        "# Branch\nA violet flower.\n",
    );
    lib.set_registry(&reg);
    lib.sync(&[]);
    lib.sync(&["--library", "branch"]);
    let read = |args: &[&str]| {
        let mut argv = vec!["--var", "garden notes"];
        argv.extend_from_slice(args);
        lib.cmd("read", &argv, "")
    };
    let out = read(&["--q", "lavender", "entities/trial.md"]).ok();
    assert!(out.contains(&format!("6: Lavender {} MATCH", "B".repeat(601))));
    assert!(
        out.contains("--lines 5-5")
            && !out.contains("preceding")
            && !out.contains("7: CCCCCC following")
    );
    let row = read(&["--lines", "5-5", "entities/trial.md"]).ok();
    assert!(
        row.contains(&format!("5: {} preceding", "A".repeat(500))) && !row.contains("6: Lavender")
    );
    let overview = read(&["entities/trial.md"]).ok();
    assert!(
        overview.contains("2: title: Trial")
            && overview.contains("8: # Next")
            && overview.contains("--lines")
    );
    let vars = lib.room(&["vars", "main-reading"], "");
    assert!(vars.contains("reading_garden_notes_") && !vars.contains("_zz"));
    assert!(
        !lib.room(&["vars", "main-search"], "")
            .contains("reading_garden_notes_")
    );
    let stored=lib.exec("main-reading","import json\nk=next(k for k in globals() if k.startswith('reading_garden_notes_'))\nprint(json.dumps(next(iter(globals()[k].values()))))");
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored["text"], original);
    assert_eq!(stored["sha256"].as_str().unwrap().len(), 64);
    assert!(
        read(&["branch-wiki/entities/branch.md"])
            .ok()
            .contains("violet flower")
    );
    assert!(
        read(&["--library", "branch", "entities/branch.md"])
            .ok()
            .contains("violet flower")
    );
    let attachment = lib.write("field.md", "# Field note\nSeven plots.\n");
    assert!(read(&[&attachment]).failed().err.contains("--attach"));
    assert!(
        read(&["--attach", &attachment])
            .ok()
            .contains("Seven plots.")
    );
    assert!(
        read(&["entities/missing.md"])
            .ok()
            .contains("FileNotFoundError")
    );
    for key in ["box", "absent"] {
        read(&["--library", key, "entities/trial.md"]).failed();
    }
    assert!(
        lib.cmd("read", &["--var", "///", "entities/trial.md"], "")
            .failed()
            .err
            .contains("--var")
    );
    let var_name=lib.python(&["-c","import datetime, read; print(read.var_name('field book/reading',datetime.date(2024,1,2)))"],"").ok();
    assert_eq!(var_name.trim(), "reading_field_book_reading_240102");
    // A transferred attachment remains usable if the receiver cannot reopen the sender's path.
    let delivered=lib.python(&["-c","import read; names=['/sample/unmounted.md']; print(read.read_code(names,'reading_copy',[],(1,1),local={names[0]:'Shared field notes'}))"],"").ok();
    assert!(
        lib.exec("main-reading", &delivered)
            .contains("1: Shared field notes")
    );
    assert_eq!(
        lib.exec(
            "main-reading",
            "print(reading_copy['/sample/unmounted.md']['text'])"
        )
        .trim(),
        "Shared field notes"
    );
}

#[test]
fn catalog_cards_and_library_links_follow_the_owning_library_configuration() {
    let lib = Library::new();
    lib.init();
    lib.write(
        "wiki/entities/skill_botany.md",
        "# Botanical notes\nSteps for botanical drawings.\n",
    );
    lib.write("branch/garden.md", "# Sample\nBotanical drawings.\n");
    let mut registry = lib.registry();
    registry["libraries"]["branch"] =
        json!({"name":"Branch","wiki_root":"branch","maxd":0.3,"ord":2,"librarian":"generic"});
    registry["libraries"]["remote"] = json!({"name":"Remote","room":"remote-room","search":"box"});
    registry["libraries"]["main"]["reads"] = json!(["branch", "remote"]);
    registry["libraries"]["main"]["dirs"] = json!(["entities", "notes"]);
    registry["libraries"]["main"]["golden"] = json!("sample_v1");
    registry["libraries"]["main"]["ord"] = json!(1);
    lib.set_registry(&registry);
    lib.sync(&[]);
    let search = || {
        lib.cmd("search", &["--library", "main", "botanical drawings"], "")
            .ok()
    };
    assert!(!search().contains("# 서지 레코드"));
    registry["libraries"]["main"]["cards"] = json!(["entities/skill_"]);
    lib.set_registry(&registry);
    lib.sync(&[]);
    let output = search();
    assert!(
        output.contains("# 서지 레코드 1. entities/skill_botany.md") && output.contains("낱말"),
        "{output}"
    );
    let rows = |room| {
        serde_json::from_str::<Value>(
            &lib.exec(room, "import json\nprint(json.dumps(library_registry))"),
        )
        .unwrap()
    };
    let visible = rows("main-search");
    assert_eq!(visible[0]["key"], "main");
    assert_eq!(visible[0]["central"], true);
    assert_eq!(visible[0]["dirs"], "entities,notes");
    assert_eq!(visible[0]["golden"], "sample_v1");
    assert_eq!(visible[1]["room"], "branch-search");
    assert_eq!(visible[1]["maxd"], 0.3);
    assert_eq!(visible[1]["librarian"], "generic");
    assert_eq!(visible[1]["wiki_root"], lib.path("branch"));
    assert_eq!(visible[2]["room"], "remote-room");
    assert_eq!(visible[2]["search"], "box");
    assert!(visible[2].get("wiki_root").is_none());
    lib.sync(&["--library", "branch"]);
    let branch = rows("branch-search");
    assert_eq!(branch.as_array().unwrap().len(), 1);
    assert_eq!(branch[0]["key"], "branch");
}

#[test]
fn indexing_policy_preserves_the_original_while_excluding_its_body_from_room_memory() {
    let mut lib = Library::new();
    lib.init();
    let mut reg = lib.registry();
    reg["libraries"]["main"]["index_block"] = json!({"sample":"OMIT_FROM_INDEX"});
    reg["libraries"]["main"]["rooms"] = json!({"search":"catalog","reading":"study"});
    lib.write(
        "wiki/entities/sample.md",
        "---\ntitle: Public sample\n---\nOMIT_FROM_INDEX\n",
    );
    lib.set_registry(&reg);
    let synced = lib.sync(&[]);
    assert_eq!(
        synced["rooms"],
        json!({"search":"catalog","reading":"study","meeting":"main-meeting","librarian":"main-librarian"})
    );
    let body = lib.exec(
        "study",
        "print(next(v for k,v in docs.items() if k.endswith('sample.md')))",
    );
    assert!(body.contains("Public sample") && !body.contains("OMIT_FROM_INDEX"));
    lib.env
        .insert("LIBRARY_REGISTRY_ROOM".into(), "catalog".into());
    let refused = lib
        .cmd("read", &["--var", "restricted", "entities/sample.md"], "")
        .ok();
    assert!(refused.contains("index policy excludes") && !refused.contains("OMIT_FROM_INDEX"));
    assert_eq!(
        lib.exec(
            "study",
            "print(next(v for k,v in globals().items() if k.startswith('reading_restricted_')))"
        )
        .trim(),
        "{}"
    );
    let cfg = lib.path("library.json");
    let visible: Value = serde_json::from_str(
        &lib.cmd(
            "library",
            &["search", "--config", &cfg, "OMIT_FROM_INDEX"],
            "",
        )
        .ok(),
    )
    .unwrap();
    assert_eq!(visible["hits"], json!([]));
    assert!(
        lib.cmd(
            "library",
            &["read", "--config", &cfg, "entities/sample.md"],
            ""
        )
        .ok()
        .contains("OMIT_FROM_INDEX")
    );
    for (key, invalid) in [
        ("rooms", json!({"unknown":"x"})),
        ("rooms", json!({"search":""})),
        ("rooms", json!([])),
        ("index_block", json!({"sample":"["})),
        ("sections", json!({"unknown":{}})),
        ("sections", json!({"today":{"dirs":"queries"}})),
        ("sections", json!({"decisions":{}})),
        ("terms", json!(3)),
    ] {
        let mut invalid_reg = reg.clone();
        invalid_reg["libraries"]["main"][key] = invalid;
        lib.set_registry(&invalid_reg);
        assert_eq!(
            lib.cmd("room_sync", &["--config", &cfg], "").status,
            Some(2),
            "{key}"
        );
    }
}

#[test]
fn side_sections_are_optional_can_be_named_and_accept_an_older_room_module() {
    let lib = Library::new();
    lib.init();
    lib.sync(&[]);
    let all = || lib.cmd("search", &["garden"], "").ok();
    let output = all();
    for section in ["오늘 들어온 글", "회의실 변수", "결정 기록"] {
        assert!(output.contains(&format!("## {section}\n(설정 안 됨)")));
    }
    assert!(output.contains("용어 사본 없음"));
    let today = lib
        .python(
            &[
                "-c",
                "import datetime; print(datetime.date.today().strftime('%y%m%d'))",
            ],
            "",
        )
        .ok()
        .trim()
        .to_owned();
    let note = format!("{today}_garden.md");
    lib.write(&format!("wiki/queries/{note}"), "# Garden\nSpring.\n");
    let mut reg = lib.registry();
    reg["libraries"]["main"]["sections"] =
        json!({"today":{"dirs":["queries"]},"meeting_vars":{"title":"Meeting notes"}});
    reg["libraries"]["main"]["terms"] = json!("terms.json");
    lib.write("terms.json",&json!([{"표준어":"garden","별칭":["garden"],"절":"Botany","실체":"Plants grown together."}]).to_string());
    lib.set_registry(&reg);
    lib.sync(&[]);
    lib.exec("main-meeting", "task_garden_note = {'title':'field notes'}");
    let output = all();
    assert!(
        output.contains(&note)
            && output.contains("## Meeting notes\ntask_garden_note")
            && output.contains("Plants grown together."),
        "{output}"
    );
    assert!(
        !lib.room(&["vars", "main-search"], "")
            .contains("rrf_weights")
    );
    lib.write(
        "wiki/.wiki-pending/rrf_weights.json",
        r#"{"det":{"a":3,"b":1},"vec":{"a":1,"b":1},"n":2}"#,
    );
    lib.sync(&[]);
    assert_eq!(
        lib.exec("main-search", "print(rrf_weights['n'])").trim(),
        "2"
    );
    lib.exec("main-search","import os, wiki_room_fuse as _m\n_m._room_mtime = os.path.getmtime(_m.__file__)\n_m.entry_sections = lambda q,qv: '## Earlier section\\n'\ndel _m");
    assert!(all().contains("## Earlier section"));
}

#[test]
fn cached_daily_lists_refresh_when_the_directory_changes_and_parameterized_decisions_show_nulls() {
    let lib = Library::new();
    lib.room(&["start", "display"], "");
    lib.write("today/240101_garden_a.md", "a");
    lib.write("today/231231_garden_old.md", "old");
    let dir = lib.path("today");
    lib.exec(
        "display",
        &format!(
            "import wiki_room_fuse as fuse\nfolder = {}\ncfg={{'dirs':[folder]}}",
            json!(dir)
        ),
    );
    let show = "print(fuse.entry_today('garden',cfg,ymd='240101'))";
    assert_eq!(lib.exec("display", show).trim(), "240101_garden_a.md");
    let saved=lib.exec("display","from unittest.mock import patch\nwith patch.object(fuse._os,'listdir',side_effect=RuntimeError('unexpected rescan')):\n print(fuse.entry_today('garden',cfg,ymd='240101'))");
    assert_eq!(saved.trim(), "240101_garden_a.md");
    lib.write("today/240101_garden_b.md", "b");
    lib.exec(
        "display",
        "import os\ns=os.stat(folder)\nos.utime(folder,ns=(s.st_atime_ns,s.st_mtime_ns+100000000))",
    );
    let refreshed = lib.exec("display", show);
    assert!(
        refreshed.contains("240101_garden_a")
            && refreshed.contains("240101_garden_b")
            && !refreshed.contains("231231")
    );
    let data=lib.exec("display",r#"import json
from unittest.mock import MagicMock
db=MagicMock()
cursor=db.cursor.return_value.__enter__.return_value
cursor.fetchall.return_value=[('alpha', None, 'June')] * 4
options={'sql':'select sample', 'k':2}
result=fuse.entry_decisions([0.1,0.2], options, connect=lambda:db)
print(json.dumps([result,cursor.execute.call_args.args,db.close.called,fuse.entry_decisions(None,options)]))"#);
    let response: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(response[0], "alpha |  | June\nalpha |  | June");
    assert_eq!(
        response[1],
        json!(["select sample",{"vec":"[0.100000,0.200000]","k":2}])
    );
    assert_eq!(response[2], true);
    assert!(response[3].as_str().unwrap().contains("건너뜀"));
    let warning=lib.exec("display","from unittest.mock import patch\nwith patch.object(fuse,'entry_decisions',side_effect=RuntimeError('line one\\nline two')):\n print(fuse.entry_sections('garden',[0.1],{'decisions':options}))");
    assert!(warning.contains("건너뜀: RuntimeError: line one line two"));
}

#[cfg(unix)]
#[test]
fn a_room_with_denied_local_access_reports_the_right_error() {
    use std::os::unix::fs::PermissionsExt;
    let lib = Library::new();
    if lib
        .python(&["-c", "import os; print(os.geteuid())"], "")
        .ok()
        .trim()
        == "0"
    {
        return;
    }
    let blocked = lib.path("closed");
    fs::create_dir(&blocked).unwrap();
    lib.write("rooms/meeting.route", &format!("false\nsocket {blocked}\n"));
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0)).unwrap();
    let error = lib.cmd("room", &["status", "meeting"], "");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(error.status, Some(3));
    assert!(error.err.contains("room access denied"), "{}", error.err);
}
