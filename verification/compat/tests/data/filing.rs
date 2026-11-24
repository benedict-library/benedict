use super::*;

// Exercise the public filing function directly so the clock and the registry row
// are controlled without a database or a real user's wiki.
fn save(l: &Library, row: Value, q: &str, answer: &str, refs: &str, minute: &str) -> Output {
    l.bridge("from datetime import datetime, timezone\nfrom save_query import save\nrow,q,a,r,m=sys.argv[1:]\ncode,msg=save(json.loads(row),q,a,r,now=datetime(2030,1,2,9,int(m),tzinfo=timezone.utc)); print(msg); sys.exit(code)",&[&row.to_string(),q,answer,refs,minute])
}
#[test]
fn filing_preserves_wording_sources_and_each_decision() {
    let l = Library::demo();
    let row = json!({"key":"main","wiki_root":l.path("wiki")});
    let q = "표식 색은?";
    let page = "wiki/queries/300102_표식_색은.md";
    let a = "보라색. 이전 표식과 구분한다.";
    checked(save(&l, row.clone(), q, a, "", "30"), 0);
    let expected = format!(
        "---\ntitle: \"{q}\"\ntype: query\ntags: [query, 표식, 색은]\ncreated: 2030-01-02\nsaved_at: 2030-01-02T09:30:00.000000+00:00\n---\n\n보라색.\n\n## 질문\n\n{q}\n\n## 답변\n\n{a}\n\n## 검색 결과\n\n근거 미기록\n"
    );
    assert_eq!(l.read(page), expected);
    assert_eq!(
        l.read("wiki/log.md"),
        format!("\n## [2030-01-02] query | {q}\n")
    );
    for (minute, ans, src) in [("40", "흰색", "night.md"), ("50", "흰색", "revision.md")] {
        checked(save(&l, row.clone(), q, ans, src, minute), 0);
    }
    let result = l.read(page);
    for x in [
        "09:30",
        "09:40",
        "09:50",
        a,
        "night.md",
        "revision.md",
        "## 답변\n\n흰색",
    ] {
        assert!(result.contains(x));
    }
    checked(save(&l, row.clone(), q, "흰색", "revision.md", "55"), 0);
    assert_eq!(l.read(page), result);
    assert!(result.ends_with("## 검색 결과\n\nrevision.md\n"));
    l.put(
        page,
        expected
            .lines()
            .filter(|line| !line.starts_with("saved_at:"))
            .collect::<Vec<_>>()
            .join("\n")
            .replace("근거 미기록", "original.md"),
    );
    checked(save(&l, row.clone(), q, "New note", "fresh.md", "59"), 0);
    let result = l.read(page);
    for x in [
        a,
        "original.md",
        "fresh.md",
        "2030-01-02 (시각 미상)",
        "## 답변\n\nNew note",
    ] {
        assert!(result.contains(x));
    }
    let base = "x".repeat(41);
    checked(
        save(&l, row.clone(), &format!("{base} one"), "Original", "", "1"),
        0,
    );
    let path = format!("wiki/queries/300102_{}.md", "x".repeat(40));
    let old = l.read(&path);
    assert!(
        !save(&l, row, &format!("{base} two"), "Other", "", "2")
            .status
            .success()
    );
    assert_eq!(l.read(&path), old);
}
#[test]
fn filing_refuses_blocked_empty_and_missing_destinations() {
    let l = Library::demo();
    let row =
        json!({"key":"main","wiki_root":l.path("wiki"),"save_block":{"sample":"UNPUBLISHED"}});
    let rejected = checked(
        save(&l, row.clone(), "Which colour?", "UNPUBLISHED", "", "0"),
        2,
    );
    assert!(text(&rejected.stdout).contains("sample"));
    for blank in ["", " \t\n"] {
        assert!(
            text(&checked(save(&l, row.clone(), "Which colour?", blank, "", "0"), 0).stdout)
                .contains("비어")
        );
    }
    assert!(!l.path("wiki/queries").exists());
    assert!(!l.path("wiki/log.md").exists());
    for missing in [Value::Null, json!(l.path("missing/wiki"))] {
        assert!(
            !save(
                &l,
                json!({"key":"demo","wiki_root":missing}),
                "Q",
                "A",
                "",
                "0"
            )
            .status
            .success()
        );
    }
    assert!(!l.path("missing").exists());
}
#[test]
fn reports_are_filed_before_delivery() {
    let l = Library::demo();
    let row = json!({"key":"main","wiki_root":l.path("wiki"),"save_block":{"draft":"UNPUBLISHED"}});
    let run = |body: &str, channel: &str| {
        l.bridge("from report import run\nrc,lines=run(json.loads(sys.argv[1]),'Marker review',sys.argv[2],{'LIBRARY_REPORT_CMD':sys.argv[3]});print('\\n'.join(lines));sys.exit(rc)",&[&row.to_string(),body,channel])
    };
    let r = checked(run("Amber is visible.", ""), 0);
    assert!(text(&r.stdout).contains("설정 안 됨"));
    assert_eq!(
        files(&l.path("wiki/queries"))
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .count(),
        1
    );
    let exe = env!("CARGO_BIN_EXE_catalog_adapter").replace('\\', "/");
    let receiver = l
        .path("receiver.out")
        .display()
        .to_string()
        .replace('\\', "/");
    let cmd = format!("\"{exe}\" receive \"{receiver}\"");
    let r = checked(run("Amber is visible.", &cmd), 0);
    assert!(text(&r.stdout).contains("보냄"));
    assert_eq!(l.read("receiver.out"), "Marker review|Amber is visible.");
    fs::remove_file(l.path("receiver.out")).unwrap();
    assert!(!run("UNPUBLISHED", &cmd).status.success());
    assert!(!l.path("receiver.out").exists());
    let cmd = format!("\"{exe}\" unavailable");
    assert!(text(&checked(run("Updated report", &cmd), 3).stdout).contains("실패"));
}
#[test]
fn settings_are_available_without_a_user_home() {
    let l = Library::empty();
    l.put("search.env", "LIBRARY_ROOM_CMD=room-example");
    let read = "from unittest.mock import patch\nimport search\nos.environ.pop('LIBRARY_SEARCH_ENV',None)\nif sys.argv[1]: os.environ['LIBRARY_SEARCH_ENV']=sys.argv[1]\nos.environ['LIBRARY_MAIL_FROM']='reader'\nwith patch.object(Path,'home',side_effect=RuntimeError('no home')): print(json.dumps(search.settings()))";
    assert_eq!(
        value(
            l.bridge(read, &[&l.path("search.env").display().to_string()]),
            0
        )["LIBRARY_ROOM_CMD"],
        "room-example"
    );
    assert_eq!(
        value(l.bridge(read, &[""]), 0),
        json!({"LIBRARY_MAIL_FROM":"reader"})
    );
}
#[test]
fn backups_restore_exact_bytes_and_check_integrity_before_writing() {
    let l = Library::demo();
    let run = |args: &[&str]| l.py("wiki_backup.py").args(args).output().unwrap();
    let cfg = l.cfg();
    let archive = l.path("backup.tar");
    assert_eq!(
        value(
            run(&["snapshot", "--config", &cfg, "--out", "backup.tar"]),
            0
        )["verified"],
        true
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(windows)]
    {
        let acl=l.cmd("powershell.exe").args(["-NoProfile","-NonInteractive","-Command","[IO.File]::GetAccessControl($env:BENEDICT_TEST_ARCHIVE).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)"]).env("BENEDICT_TEST_ARCHIVE",&archive).output().unwrap();
        assert!(text(&checked(acl, 0).stdout).contains("D:P(A;;FA;;;OW)"));
    }
    assert_eq!(
        value(
            run(&["restore", "--archive", "backup.tar", "--target", "restored"]),
            0
        )["verified"],
        true
    );
    assert_eq!(
        fs::read(l.path("restored/entities/refund.md")).unwrap(),
        fs::read(l.path("wiki/entities/refund.md")).unwrap()
    );
    assert!(
        !run(&["restore", "--archive", "backup.tar", "--target", "restored"])
            .status
            .success()
    );
    assert!(
        !run(&["snapshot", "--config", &cfg, "--out", "backup.tar"])
            .status
            .success()
    );
    l.put("invalid.tar", "Not an archive");
    assert!(
        !run(&[
            "restore",
            "--archive",
            "invalid.tar",
            "--target",
            "bad-target"
        ])
        .status
        .success()
    );
    assert!(!l.path("bad-target").exists());
    let mut raw = fs::read(archive).unwrap();
    let pos = raw
        .windows(8)
        .position(|w| w == b"# v2 git" || w == b"# v3 git")
        .unwrap();
    raw[pos] = b'?';
    l.put("bad-bundle.tar", raw);
    assert!(
        !run(&[
            "restore",
            "--archive",
            "bad-bundle.tar",
            "--target",
            "bad-target"
        ])
        .status
        .success()
    );
    assert!(!l.path("bad-target").exists());
}

// A Docker library writes as its own account into the installer's wiki. The grant command is
// recorded here so the test does not depend on ACL support in the temporary directory.
#[cfg(unix)]
#[test]
fn bound_wiki_filing_grants_the_installer_access() {
    let l = Library::empty();
    let body = r#"
from datetime import datetime, timezone
import save_query
base = Path(sys.argv[1])
tools, grants, wiki = base / "tools", base / "grants", base / "wiki"
tools.mkdir()
(tools / "setfacl").write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$GRANT_LOG"\ncp "$4" "$GRANT_LOG.page"\n')
(tools / "setfacl").chmod(0o755)
os.environ["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
os.environ["GRANT_LOG"] = str(grants)
installer = os.geteuid() + 1
os.environ["LIBRARY_WIKI_USER"] = str(installer)
assert save_query.wiki_user(Path("/data/wiki")) == installer
assert save_query.wiki_user(wiki) is None
os.environ["LIBRARY_WIKI_USER"] = str(os.geteuid())
assert save_query.wiki_user(Path("/data/wiki")) is None
del os.environ["LIBRARY_WIKI_USER"]
assert save_query.wiki_user(Path("/data/wiki")) is None
now = datetime(2030, 1, 2, tzinfo=timezone.utc)
page = wiki / "queries" / "q.md"
save_query.write_page(page, wiki / "log.md", "q", "first", "", now)
assert not grants.exists()
save_query.wiki_user = lambda root: installer
for answer in ("second", "third"):
    grants.unlink(missing_ok=True)
    save_query.write_page(page, wiki / "log.md", "q", answer, "", now)
    option, entries, end, granted = grants.read_text().splitlines()
    assert (option, entries, end) == ("--set", f"u::rw-,u:{installer}:rw-,g::---,m::rw-,o::---", "--")
    assert Path(granted).parent == page.parent and Path(granted) != page
    assert answer in Path(str(grants) + ".page").read_text(encoding="utf-8")
    assert answer in page.read_text(encoding="utf-8")
"#;
    let base = l.path("").display().to_string();
    checked(l.bridge(body, &[&base]), 0);
}
