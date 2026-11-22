mod optional_support;
use optional_support::{Response, Server, call, result};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Output};
use tempfile::{TempDir, tempdir};

const QUESTION: &str = "When do the exhibition doors open?";
const SUMMARY: &str = "The public catalogue lists an opening time of nine for the exhibition.";
const HTML: &str = "<html><head><style>invisible-style</style><title>Exhibition</title></head><body><script>invisible-script</script><p>The exhibition opens at nine. The collection contains ninety different local maps.</p></body></html>";
const RUN: &str = r#"
import datetime, shlex, web
root = Path.cwd()
provider = shlex.quote(data['provider'])
row = {'key':'demo','wiki_root':str(root/'wiki'),'save_block':data['rules'],
       'options':{'web_shelf':str(root/'shelf') if data['shelf'] else None, 'web_block':data['web_rules']}}
if data['writer']: row['options']['web_save_cmd'] = provider+' '+data['writer']
web.save_query.load_row = lambda _:row
web.search.settings = lambda:{'LIBRARY_WEB_SEARCH_CMD': provider+' find' if data['configured'] else None}
web.MAX_BYTES = data['limit']
start = datetime.datetime.fromisoformat(data['start'])
ticks = iter([start])
def clock(): return next(ticks,start+datetime.timedelta(seconds=12))
def local_fixture_only(url):
    return None if url.startswith(data['base']+'/') else 'outside fixture'
sys.exit(web.main(['web.py','demo',data['question']], guard=local_fixture_only,clock=clock))
"#;

struct Library {
    dir: TempDir,
    server: Server,
    settings: Value,
}
impl Library {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("wiki")).unwrap();
        let server = Server::new(|path, _| match path {
            "/html" => Response::new(200, "text/html", HTML),
            "/charset" => Response::new(
                200,
                "text/html",
                b"<meta charset='iso-8859-1'><p>caf\xe9</p>".to_vec(),
            ),
            "/thin" => Response::new(200, "text/html", "<script>app()</script>"),
            "/plain" => Response::new(200, "text/plain", "public text"),
            "/pdf" => Response::new(200, "application/pdf", "%PDF-1.7 fixture"),
            "/empty" => Response::new(200, "text/plain", ""),
            "/big" => Response::new(200, "text/plain", vec![b'x'; 2048]),
            "/moved" | "/unavailable" => {
                let mut r = Response::new(302, "text/plain", "");
                r.headers.push((
                    "Location".into(),
                    if path == "/moved" {
                        "/plain"
                    } else {
                        "https://example.invalid/withdrawn"
                    }
                    .into(),
                ));
                r
            }
            _ => Response::new(404, "text/plain", "no such publication"),
        });
        let provider = Path::new(env!("CARGO_BIN_EXE_web_fixture"))
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let settings = json!({"base":server.url(),"provider":provider,"question":QUESTION,"start":"2020-01-02T09:00:00","limit":4096,
          "configured":true,"shelf":true,"writer":null,"rules":{},"web_rules":{}});
        Self {
            dir,
            server,
            settings,
        }
    }
    fn response(&self, paths: &[&str]) -> Value {
        let sources = paths
            .iter()
            .map(|p| json!({"url":format!("{}{p}",self.server.url())}))
            .collect::<Vec<_>>();
        json!({"summary":SUMMARY,"sources":sources})
    }
    fn run(&self, response: &str, provider_status: &str, expected: i32) -> Output {
        let out = call(
            self.dir.path(),
            RUN,
            &self.settings,
            &[
                ("FIXTURE_RESPONSE", response),
                ("FIXTURE_EXIT", provider_status),
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn search(&self, paths: &[&str], expected: i32) -> Output {
        self.run(&self.response(paths).to_string(), "0", expected)
    }
    fn rows(&self, file: &str) -> Vec<Value> {
        let path = self.dir.path().join(file);
        if !path.exists() {
            return Vec::new();
        }
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn records(&self) -> Vec<Value> {
        self.rows("shelf/200102.jsonl")
    }
}

#[test]
fn files_originals_and_reading_copies_with_separate_collection_times() {
    let lib = Library::new();
    lib.search(
        &[
            "/html", "/charset", "/thin", "/plain", "/pdf", "/empty", "/missing",
        ],
        0,
    );
    let records = lib.records();
    let rec = &records[0];
    assert_eq!(rec["question"], QUESTION);
    assert_eq!(rec["summary"], SUMMARY);
    assert_eq!(rec["at"], "2020-01-02T09:00:00");
    let sources = rec["sources"].as_array().unwrap();
    let html = &sources[0];
    assert_eq!(html["fetched_at"], "2020-01-02T09:00:12");
    assert_eq!(html["status"], 200);
    assert_eq!(html["bytes"], HTML.len());
    let original = Path::new(html["path"].as_str().unwrap());
    assert_eq!(fs::read(original).unwrap(), HTML.as_bytes());
    assert_eq!(
        fs::canonicalize(original.parent().unwrap()).unwrap(),
        fs::canonicalize(lib.dir.path().join("shelf/200102")).unwrap()
    );
    assert_eq!(
        original.file_stem().unwrap().to_str(),
        html["sha256"].as_str()
    );
    let text = fs::read_to_string(html["text_path"].as_str().unwrap()).unwrap();
    assert!(text.contains("Exhibition") && text.contains("opens at nine"));
    assert!(!text.contains("invisible"));
    assert!(
        fs::read_to_string(sources[1]["text_path"].as_str().unwrap())
            .unwrap()
            .contains("café")
    );
    assert_eq!(sources[2]["thin"], true);
    assert_eq!(sources[3]["text_path"], sources[3]["path"]);
    assert!(sources[4]["text_path"].is_null());
    for source in &sources[5..] {
        assert!(source.get("error").is_some());
        assert!(source.get("sha256").is_none());
    }
    let page = fs::read_to_string(rec["page"].as_str().unwrap()).unwrap();
    assert!(page.contains(SUMMARY) && page.contains("원문과 대조하기 전"));
    assert!(page.contains(html["text_path"].as_str().unwrap()) && page.contains("HTTP 404"));
    let calls = lib.rows("provider.jsonl");
    assert_eq!(calls[0], json!({"args":["find",QUESTION],"stdin":""}));
}

#[test]
fn follows_permitted_redirects_marks_failures_and_reuses_identical_bytes() {
    let mut lib = Library::new();
    lib.settings["limit"] = json!(1024);
    lib.search(&["/moved", "/unavailable", "/big"], 0);
    let rec = &lib.records()[0];
    let src = &rec["sources"][0];
    assert!(src["url"].as_str().unwrap().ends_with("/moved"));
    assert!(src["final_url"].as_str().unwrap().ends_with("/plain"));
    assert_eq!(
        src["sha256"],
        "331c5bf69585b750aa7bdc8b8d961fe8ef42f97801c45081af027a75e4bd956c"
    );
    for error in &rec["sources"].as_array().unwrap()[1..] {
        assert!(error.get("error").is_some());
        assert!(error.get("path").is_none());
    }
    let first = Path::new(src["path"].as_str().unwrap());
    let modified = fs::metadata(first).unwrap().modified().unwrap();
    lib.settings["question"] = json!("What is the public schedule?");
    lib.search(&["/plain"], 0);
    assert_eq!(lib.records()[1]["sources"][0]["path"], src["path"]);
    assert_eq!(fs::metadata(first).unwrap().modified().unwrap(), modified);
}

#[test]
fn retries_failures_and_reuses_only_successful_answers_from_today() {
    let mut lib = Library::new();
    lib.run("unavailable", "7", 3);
    for response in [
        "invalid",
        r#"{"summary":"","sources":[]}"#,
        r#"{"summary":"summary","sources":[]}"#,
    ] {
        lib.run(response, "0", 3);
    }
    assert!(!lib.dir.path().join("wiki/queries").exists());
    lib.search(&["/plain"], 0);
    lib.search(&["/plain"], 0);
    assert_eq!(lib.rows("provider.jsonl").len(), 5);
    assert_eq!(lib.records().len(), 5);
    lib.settings["start"] = json!("2020-01-03T09:00:00");
    lib.search(&["/plain"], 0);
    assert_eq!(lib.rows("provider.jsonl").len(), 6);
    assert_eq!(lib.rows("shelf/200103.jsonl").len(), 1);
}

#[test]
fn checks_disclosure_rules_and_settings_before_invoking_provider() {
    let mut lib = Library::new();
    lib.settings["web_rules"] = json!({"local draft":"unpublished"});
    lib.settings["rules"] = json!({"reserved":"unreleased"});
    for q in [
        "unpublished exhibit",
        "unreleased schedule",
        "sample@example.invalid",
        "password=fixture-value",
        "open ~/sample-notes",
        "reference to localhost",
        "postgresql://example.invalid/fixture",
        &"a".repeat(32),
        &"x".repeat(301),
    ] {
        lib.settings["question"] = json!(q);
        lib.search(&["/html"], 2);
    }
    lib.settings["question"] = json!(QUESTION);
    lib.settings["shelf"] = json!(false);
    lib.search(&["/plain"], 2);
    lib.settings["shelf"] = json!(true);
    lib.settings["configured"] = json!(false);
    lib.search(&["/plain"], 2);
    assert!(lib.rows("provider.jsonl").is_empty());
    assert!(!lib.dir.path().join("shelf").exists());
    assert!(lib.server.requests.lock().unwrap().is_empty());
}

#[test]
fn registry_writers_receive_only_question_and_answer_and_refusal_preserves_original() {
    for writer in ["file", "legacy-file"] {
        let mut lib = Library::new();
        lib.settings["writer"] = json!(writer);
        lib.search(&["/plain"], 0);
        let calls = lib.rows("provider.jsonl");
        let args = calls[1]["args"].as_array().unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[1], QUESTION);
        assert!(args[2].as_str().unwrap().contains(SUMMARY));
        assert_eq!(lib.records()[0]["page"], "queries/public-note.md");
    }
    let mut lib = Library::new();
    lib.settings["rules"] = json!({"catalogue":"catalogue"});
    lib.search(&["/plain"], 4);
    let rec = &lib.records()[0];
    assert!(rec["page"].is_null());
    assert!(Path::new(rec["sources"][0]["path"].as_str().unwrap()).is_file());
}

#[test]
fn normalizes_sources_and_confines_agent_selected_sources_to_approved_public_material() {
    let lib = Library::new();
    let mut paths = vec!["/plain", "/plain"];
    let rest: Vec<_> = (0..12).map(|i| format!("/catalog-{i}")).collect();
    paths.extend(rest.iter().map(String::as_str));
    lib.search(&paths, 0);
    let urls = lib.records()[0]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["url"].as_str().unwrap().to_owned())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(urls.len(), 8);
    let statuses = result(call(
        lib.dir.path(),
        r#"
import web, socket
from unittest.mock import patch
def resolve(host, *args, **kwargs):
    addresses = {'localhost':['127.0.0.1'], 'mixed.example.invalid':['1.1.1.1','127.0.0.1']}
    return [(2,1,6,'',(address,443)) for address in addresses.get(host,['1.1.1.1'])]
with patch.object(socket,'getaddrinfo',side_effect=resolve):
    print(json.dumps([web.why_not_fetch(u) for u in data]))
"#,
        &json!([
            "https://example.org/public",
            "file:demo.txt",
            "http://localhost/",
            "https://mixed.example.invalid/",
            "https://demo@example.org/private"
        ]),
        &[],
    ));
    assert!(statuses[0].is_null());
    assert!(
        statuses.as_array().unwrap()[1..]
            .iter()
            .all(|v| !v.is_null())
    );
}

#[test]
fn an_agent_can_archive_and_reopen_its_public_source_without_a_provider() {
    let dir = tempdir().unwrap();
    let bridge = r#"
import web, library_local
from unittest.mock import patch
config=Path.cwd()/'registry.json'
if not config.exists(): library_local.init(config)
lib=library_local.Library(config)
def receive(url,guard):
    if data['offline']: raise OSError('offline')
    return 200,url,'text/html',data['body'].encode()
with patch.object(web,'receive',side_effect=receive) as fetch:
    try: rec=web.agent_source(lib,None,data['query'],'https://example.org/catalogue')
    except ValueError as e: rec={'error':str(e)}
print(json.dumps({'receipt':rec,'calls':fetch.call_count,'hits':lib.search(data['query'])['hits']}))
"#;
    for (q, offline, calls, ok) in [
        ("password=fixture", false, 0, false),
        ("A new public page", true, 1, false),
        ("Public catalogue", false, 1, true),
    ] {
        let v = result(call(
            dir.path(),
            bridge,
            &json!({"body":HTML,"offline":offline,"query":q}),
            &[],
        ));
        assert_eq!(v["calls"], calls);
        if ok {
            let rec = &v["receipt"];
            assert_eq!(
                fs::read(rec["original"].as_str().unwrap()).unwrap(),
                HTML.as_bytes()
            );
            assert!(
                fs::read_to_string(rec["read_path"].as_str().unwrap())
                    .unwrap()
                    .contains("opens at nine")
            );
            assert_eq!(v["hits"][0]["path"], rec["path"]);
        } else {
            assert!(v["receipt"]["error"].is_string());
            assert_eq!(v["hits"], json!([]));
        }
    }
}
