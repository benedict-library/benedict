mod optional_support;
use benedict_compat::repo_root;
use optional_support::{call, result};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command};
use tempfile::tempdir;

const TERMS: [&str; 5] = ["maps", "atlas", "도서관", "catalogue", "absent"];
const QUERY: &str = r#"
import importlib, wiki_room_fuse as f
g={}
f.index_apply(g,data['docs'],[])
if data['reload']: f=importlib.reload(f)
f.index_apply(g,data['changes'],data['removed'])
ix=g['_tf']; q=data['query']
print(json.dumps({'kind':type(ix).__name__, 'engine':f.TOK_ENGINE, 'paths':sorted(ix),
    'df':{w:g['_df'].get(w,0) for w in q}, 'lengths':g['_len'], 'average':g['_avg'],
    'counts':ix if isinstance(ix,dict) else None,
    'scores':None if isinstance(ix,dict) else ix.bm25(q,len(ix),1.5,0.75,g['_avg'])}))
"#;

fn reference_scores(py: &Value) -> BTreeMap<String, f64> {
    let mut scores = BTreeMap::new();
    let n = py["paths"].as_array().unwrap().len() as f64;
    let mean = py["average"].as_f64().unwrap();
    for term in TERMS {
        let df = py["df"][term].as_u64().unwrap() as f64;
        for (path, counts) in py["counts"].as_object().unwrap() {
            let count = counts[term].as_u64().unwrap_or(0) as f64;
            if count == 0.0 {
                continue;
            }
            let length = py["lengths"][path].as_f64().unwrap().min(3.0 * mean);
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            *scores.entry(path.clone()).or_default() +=
                idf * count * 2.5 / (count + 1.5 * (0.25 + 0.75 * length / mean));
        }
    }
    scores
}

#[test]
fn optional_native_modes_and_unicode_match_the_reference_after_module_reload() {
    let tmp = tempdir().unwrap();
    // Build the actual shared library for this host rather than skipping the ABI tests.
    let dir = tmp.path();
    let crate_path = repo_root().join("native/Cargo.toml");
    let build = Command::new(env!("CARGO"))
        .args([
            "build",
            "--release",
            "--offline",
            "--locked",
            "--manifest-path",
        ])
        .arg(&crate_path)
        .arg("--target-dir")
        .arg(dir)
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let shared = dir.join("release").join(format!(
        "{}library_native{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(
        shared.is_file(),
        "missing native library at {}",
        shared.display()
    );
    let samples = json!([
        "도서관 ABC12_x",
        "가",
        "k8s한글",
        "…",
        "",
        "İSTANBUL Kelvin K 찾기",
        "ΣΑΣ 끝ς"
    ]);
    let words = r#"
import wiki_room_fuse as f
samples=data+['a'+chr(0xd800)+'b']
print(json.dumps({'engine':f.TOK_ENGINE,'tokens':[f.tok(s,u) for s in samples for u in (False,True)]}))
"#;
    let without = dir.join("missing-library");
    let missing = without.to_str().unwrap();
    let py = result(call(dir, words, &samples, &[("LIBRARY_NATIVE", "off")]));
    let auto = result(call(
        dir,
        words,
        &samples,
        &[("LIBRARY_NATIVE", "auto"), ("LIBRARY_NATIVE_LIB", missing)],
    ));
    assert_eq!(py, auto);
    let required = call(
        dir,
        words,
        &samples,
        &[("LIBRARY_NATIVE", "on"), ("LIBRARY_NATIVE_LIB", missing)],
    );
    assert!(!required.status.success());
    assert!(String::from_utf8_lossy(&required.stderr).contains("LIBRARY_NATIVE=on"));
    let rs = result(call(
        dir,
        words,
        &samples,
        &[
            ("LIBRARY_NATIVE", "on"),
            ("LIBRARY_NATIVE_LIB", shared.to_str().unwrap()),
        ],
    ));
    assert_eq!(py["engine"], "python");
    assert_eq!(rs["engine"], "rust");
    assert_eq!(py["tokens"], rs["tokens"]);
    let mut corpus = BTreeMap::new();
    for n in 0..256 {
        let body = format!(
            "---\ntitle: Catalogue {n}\ntags: [atlas]\n---\n{} {}",
            TERMS[n % 4],
            TERMS[(n / 4) % 4].repeat(n % 7 + 1)
        );
        corpus.insert(format!("entities/item-{n}.md"), body);
    }
    let mut task = json!({"docs":corpus,"query":TERMS,"changes":{},"removed":[],"reload":false});
    verify_index(dir, &shared, &task);
    task["changes"] =
        json!({"entities/item-1.md":"도서관 atlas maps","entities/new.md":"catalogue atlas"});
    task["removed"] = json!(["entities/item-3.md", "entities/item-5.md"]);
    task["reload"] = json!(true);
    verify_index(dir, &shared, &task);
}

fn verify_index(dir: &Path, shared: &Path, task: &Value) {
    let invoke = |mode| {
        result(call(
            dir,
            QUERY,
            task,
            &[
                ("LIBRARY_NATIVE", "on"),
                ("LIBRARY_NATIVE_LIB", shared.to_str().unwrap()),
                ("LIBRARY_NATIVE_INDEX", mode),
            ],
        ))
    };
    let py = invoke("off");
    let rs = invoke("on");
    assert_eq!(py["kind"], "dict");
    assert_eq!(rs["kind"], "NativeIndex");
    for key in ["paths", "df", "average", "lengths"] {
        assert_eq!(py[key], rs[key], "{key}");
    }
    let expected = reference_scores(&py);
    assert_eq!(expected.len(), rs["scores"].as_object().unwrap().len());
    for (path, score) in expected {
        let actual = rs["scores"][&path].as_f64().unwrap();
        assert!(
            (score - actual).abs() < 1e-12,
            "{path}: {score} vs {actual}"
        );
    }
}
