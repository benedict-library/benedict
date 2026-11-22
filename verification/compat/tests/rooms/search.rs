use super::Library;
use serde_json::{Value, json};
use std::fs;
fn fixture(mode: &str) -> String {
    format!(
        "\"{}\" {mode}",
        env!("CARGO_BIN_EXE_search_fixture").replace('\\', "/")
    )
}
fn search_setup() -> Library {
    let mut l = Library::new();
    l.write("branch-wiki/index.md", "Garden notes\nRoads\n");
    l.write(
        "branch-wiki/entities/flowers.md",
        "# Botany\nGarden planting.\n",
    );
    l.write("memory/garden.md", "garden policy\n");
    let rows = json!([
        {"key":"main","central":true,"ord":1,"name":"Central","room":"main-search","search":"room","wiki_root":"/sample/main-search"},
        {"key":"branch","ord":2,"name":"Botany","room":"branch","search":"room","wiki_root":l.path("branch-wiki"),"dirs":"entities", "maxd":0.2},
        {"key":"far","ord":3,"name":"Far","room":"far","search":"room","maxd":0.2},
        {"key":"box","ord":4,"name":"Box","search":"box","maxd":0.2},
        {"key":"altbox","ord":5,"name":"Alternate","search":"box","box_cmd":format!("{} alternate",fixture("box"))},
        {"key":"paper","ord":6,"name":"Paper","wiki_root":l.path("branch-wiki")}
    ]);
    for (k, v) in [
        ("SAMPLE_REGISTRY", rows.to_string()),
        ("LIBRARY_ROOM_CMD", fixture("room")),
        ("LIBRARY_BOX_CMD", fixture("box")),
        ("LIBRARY_SOURCE_LOG", l.path("sources.log")),
        ("LIBRARY_MEMORY_DIR", l.path("memory")),
        ("SAMPLE_WEB_LOG", l.path("web.jsonl")),
    ] {
        l.env.insert(k.into(), v);
    }
    l
}
#[test]
fn local_room_box_and_federated_search_have_distinct_results_and_honor_the_distance_limit() {
    let mut l = search_setup();
    let args = |key| ["--library", key, "garden policy"];
    let result = l.cmd("search", &args("branch"), "");
    assert!(result.err.is_empty());
    assert!(result.ok().contains("/sample/branch/entities/flowers.md"));
    assert!(
        l.cmd("search", &args("box"), "")
            .ok()
            .contains("Box answer (garden policy)")
    );
    assert!(
        l.cmd("search", &args("altbox"), "")
            .ok()
            .contains("Box answer (alternate)")
    );
    assert!(
        l.cmd("search", &args("paper"), "")
            .ok()
            .contains("flowers.md")
    );
    assert!(
        l.cmd("search", &args("absent"), "")
            .failed()
            .err
            .contains("absent")
    );
    let all = l.cmd("search", &["garden policy"], "").ok();
    for item in [
        "## 검색실 — Central",
        "entities/flowers.md",
        "오늘 들어온 글",
        "Botany (최근접 0.100)",
        "Box (최근접 0.150)",
        "Far 0.400",
        "Planting notes.",
        "garden.md",
        "## 다음:",
    ] {
        assert!(all.contains(item), "{item}: {all}");
    }
    let log = fs::read_to_string(l.path("sources.log")).unwrap();
    for item in [
        "검색실(branch)",
        "컨테이너(box)",
        "위키 직접(paper)",
        "검색실(main)",
    ] {
        assert!(log.contains(item), "{log}");
    }
    l.env
        .insert("SAMPLE_STALE_ROOM".into(), "main-search".into());
    let result = l.cmd("search", &["garden policy"], "");
    assert!(result.err.contains("미정합"));
    assert!(result.ok().contains("Botany (최근접 0.100)"));
    l.env.remove("SAMPLE_REGISTRY");
    assert!(
        l.cmd("search", &["garden"], "")
            .failed()
            .err
            .contains("library_registry")
    );
}

#[test]
fn stale_rooms_search_the_mounted_wiki_then_the_room_host_and_report_missing_sources() {
    let mut l = search_setup();
    let args = ["--library", "branch", "garden"];
    l.env.insert("SAMPLE_STALE_ROOM".into(), "branch".into());
    let r = l.cmd("search", &args, "");
    assert!(r.err.contains("미정합"));
    let out = r.ok();
    assert!(out.contains("index.md:Garden notes"));
    assert!(out.contains("flowers.md:2:Garden planting."));
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let private = l.write("unlisted/special.md", "Garden omitted\n");
        fs::remove_file(l.path("branch-wiki/index.md")).unwrap();
        symlink(&private, l.path("branch-wiki/index.md")).unwrap();
        symlink(&private, l.path("branch-wiki/entities/link.md")).unwrap();
        assert!(!l.cmd("search", &args, "").ok().contains("Garden omitted"));
    }
    l.env.insert("SAMPLE_STALE_ROOM".into(), "far".into());
    assert!(
        l.cmd("search", &["--library", "far", "garden"], "")
            .ok()
            .contains("wiki_root 없음")
    );
    l.env.insert("SAMPLE_STALE_ROOM".into(), "branch".into());
    let mut rows: Value = serde_json::from_str(&l.env["SAMPLE_REGISTRY"]).unwrap();
    rows[1]["wiki_root"] = json!(l.path("missing-wiki"));
    l.env.insert("SAMPLE_REGISTRY".into(), rows.to_string());
    assert!(l.cmd("search", &args, "").ok().contains("missing-wiki"));
    l.env
        .insert("LIBRARY_WIKI_EXEC".into(), fixture("wiki-host"));
    l.env
        .insert("LIBRARY_ROOM_EXEC".into(), fixture("room-host"));
    assert!(
        l.cmd("search", &args, "")
            .ok()
            .contains("garden on room host")
    );
}

#[test]
fn web_is_opt_in_sends_only_question_and_library_and_preserves_internal_results_on_failure() {
    let mut l = search_setup();
    let no_adapter = l.cmd("search", &["--web", "garden"], "").failed();
    assert_eq!(no_adapter.status, Some(1));
    assert!(no_adapter.out.contains("LIBRARY_WEB_CMD") && no_adapter.out.contains("검색실"));
    l.env.insert("LIBRARY_WEB_CMD".into(), fixture("web"));
    for arg in [vec!["garden"], vec!["--library", "branch", "garden"]] {
        l.cmd("search", &arg, "").ok();
    }
    assert!(!l.home.join("web.jsonl").exists());
    for (key, args) in [
        ("main", vec!["--web", "garden"]),
        ("branch", vec!["--library", "branch", "--web", "garden"]),
    ] {
        let out = l.cmd("search", &args, "").ok();
        assert!(out.contains("Sample external finding"));
        assert!(out.find("flowers.md").unwrap() < out.find("## 웹 검색").unwrap());
        let log = fs::read_to_string(l.path("web.jsonl")).unwrap();
        let last: Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
        assert_eq!(last, json!({"args":[key,"garden"],"stdin":""}));
    }
    l.env.insert("SAMPLE_FAIL_WEB".into(), "1".into());
    let failed = l.cmd("search", &["--web", "garden"], "").failed();
    assert_eq!(failed.status, Some(3));
    assert!(failed.out.contains("검색실") && failed.err.contains("rc=3"));
}

#[test]
fn search_display_uses_library_relative_paths_preserves_distinct_titles_and_explains_limits() {
    let l = search_setup();
    let probe = r#"import json,search
lines=['# Search','# Card','entities/a.md','entities/b.md','entities/c.md']
paths=['queries/240101_garden.md — garden','branch/wiki/queries/240101_garden_eb87b52ea60b.md — garden?','queries/240101_garden.md — garden notes','entities/a.md — roses','# Card']
row={'name':'Botany','maxd':0.2,'wiki_root':'/sample/libs/branch-wiki/'}
result='# Index · 최근접 0.150\n/sample/libs/branch-wiki/entities/garden.md:1:title: Plants\n/elsewhere/x.md:1:title: More\n'
print(json.dumps([search.top_hits(lines,None),search.top_hits(lines,2),search.top_hits(lines,4),[search.tidy(p) for p in paths],[search.short_base(p) for p in ['/sample/libs/branch-wiki','/sample/libs/branch/wiki/','']],search.near_section(row,'branch',result),search.near_section(row,'branch',result,max_d=0.1),search.near_section(row,'branch',None)]))"#;
    let results: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(results[0], results[2]);
    assert_eq!(results[1].as_array().unwrap().len(), 5);
    assert_eq!(results[1][3], "entities/b.md");
    assert!(results[1][4].as_str().unwrap().contains("1건 더"));
    assert_eq!(
        results[3],
        json!([
            "queries/240101_garden.md",
            "branch/wiki/queries/240101_garden_eb87b52ea60b.md",
            "queries/240101_garden.md — garden notes",
            "entities/a.md — roses",
            "# Card"
        ])
    );
    assert_eq!(results[4], json!(["/sample/libs/", "/sample/libs/", ""]));
    assert_eq!(results[5][1], "branch-wiki/entities/garden.md — Plants");
    assert_eq!(results[5][2], "/elsewhere/x.md — More");
    assert!(results[6][0].as_str().unwrap().contains("가까운 자료 없음"));
    assert!(results[7][0].as_str().unwrap().contains("검색 못 함"));
}

#[test]
fn side_lines_are_suppressed_only_within_the_same_session_and_settings_prefer_the_environment() {
    let mut l = search_setup();
    let ask = |l: &Library| l.cmd("search", &["garden policy"], "").ok();
    assert_eq!(ask(&l), ask(&l));
    l.env.insert("LIBRARY_SESSION".into(), "first".into());
    l.env.insert("LIBRARY_SEEN_DIR".into(), l.path("seen"));
    let first = ask(&l);
    let again = ask(&l);
    assert!(
        first.contains("## 오늘 들어온 글\ngarden.md")
            && !again.contains("## 오늘 들어온 글\ngarden.md")
    );
    assert!(again.contains("이 세션에서 이미 본") && again.contains("entities/flowers.md"));
    l.env.insert("LIBRARY_SESSION".into(), "second".into());
    assert_eq!(ask(&l), first);
    l.write(
        "search.env",
        "LIBRARY_REGISTRY_ROOM=file-value\nLIBRARY_MEMORY_DIR=chosen-folder\n",
    );
    l.env.remove("LIBRARY_MEMORY_DIR");
    let probe = "import search,json; d=search.settings(); print(json.dumps([d['LIBRARY_REGISTRY_ROOM'],d['LIBRARY_MEMORY_DIR']]))";
    let settings: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(settings, json!(["main-search", "chosen-folder"]));
}

#[test]
fn room_calls_apply_a_local_timeout_or_bound_the_remote_call_and_preserve_quoting() {
    let l = search_setup();
    let probe = r#"import json, search
from unittest.mock import patch
cfg={'LIBRARY_ROOM_CMD':'python room.py exec'}
with patch.object(search,'run',return_value=(0,'','')) as called:
 search.room_exec(cfg,'study','pass',7,9)
 search.room_exec(dict(cfg, LIBRARY_ROOM_EXEC='transport host'),'study','pass',7,9)
 print(json.dumps([c.args for c in called.call_args_list]))
"#;
    let calls: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(
        calls[0],
        json!([["python", "room.py", "exec", "study"], "pass", 7])
    );
    assert_eq!(
        calls[1],
        json!([
            ["transport", "host", "timeout 7 python room.py exec study"],
            "pass",
            9
        ])
    );
    let probe = "import search,json,shlex; parts=['python','-c',\"print('a b')\"]; routed=search.on('transport host',parts); print(json.dumps([routed[:2],shlex.split(routed[-1]),search.on('',parts)]))";
    let words: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(
        words,
        json!([
            ["transport", "host"],
            ["python", "-c", "print('a b')"],
            ["python", "-c", "print('a b')"]
        ])
    );
}

#[test]
fn generated_room_code_handles_quotes_and_leaves_no_working_names_in_the_next_request() {
    let l = search_setup();
    let probe = r#"import ast,json,search,wiki_room_fuse
cfg={'LIBRARY_ROOM_LIB':'one,two','LIBRARY_ROOM_DSN_FILE':'fixture.env'}
code=search.query_code(cfg,"garden 'spring'\nJune",3,1200,entry=True,same_name_max=20)
plain=search.query_code(cfg,'garden',3,0)
ast.parse(code); ast.parse(plain)
print(json.dumps([code,plain]))"#;
    let output: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    let full = output[0].as_str().unwrap();
    let plain = output[1].as_str().unwrap();
    assert!(full.starts_with("_SEARCH_KEEP = True\n_ENTRY = True\n"));
    for token in [
        "__QUERY__",
        "__TOPN__",
        "__MAXAGE__",
        "__REFSALPHA__",
        "__ROOMLIB__",
        "__DSNFILE__",
    ] {
        assert!(!full.contains(token));
    }
    for token in ["['one', 'two']", "open('fixture.env')", "같은 이름 문서"] {
        assert!(full.contains(token));
    }
    for token in ["_ENTRY = True", "같은 이름 문서", "_read('excerpts'"] {
        assert!(!plain.contains(token));
    }
    let probe = r#"import search,json,wiki_room_fuse
ns={'_fz':wiki_room_fuse,'_SEARCH_KEEP':True,'top':[('/sample/garden_introduction.md',1.0)],'words':['garden'],'_df':{'garden':2},'docs':{'/sample/garden_introduction.md':'','/sample/garden_appendix.md':'','/sample/plan.md':''},'_qw':{'old'}}
exec(search.query_tail(0,5),ns)
print(json.dumps([k for k in ['_SEARCH_KEEP','top','words','_qw','_key','_shown','_names','_rel'] if k in ns]))"#;
    let tail = l.python(&["-c", probe], "").ok();
    assert!(
        tail.contains("같은 이름 문서(garden)")
            && tail.contains("garden_appendix.md")
            && tail.lines().last() == Some("[]"),
        "{tail}"
    );
}
