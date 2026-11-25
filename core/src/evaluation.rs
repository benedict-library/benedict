//! Evaluate retrieval against known documents and optionally grade supplied
//! answer text by literal key coverage. Evaluation never generates an answer.
use crate::{files, local, option};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
    time::Instant,
};

type Result<T> = std::result::Result<T, String>;
fn rows(path: &Path) -> Result<Vec<Value>> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rows = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(line)
            .map_err(|_| format!("{}: line {}: invalid JSON", path.display(), i + 1))?;
        if !row.is_object() {
            return Err(format!(
                "{}: line {}: expected object",
                path.display(),
                i + 1
            ));
        }
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(format!("{}: empty evaluation data", path.display()));
    }
    Ok(rows)
}

fn targets(value: &Value, corpus: &[String]) -> Result<BTreeSet<String>> {
    let values = if value.is_string() {
        std::slice::from_ref(value)
    } else {
        value
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or("answer_doc must be a document string or nonempty string list")?
            .as_slice()
    };
    let mut wanted = BTreeSet::new();
    for item in values {
        let name = item.as_str().ok_or("invalid answer_doc")?;
        let path = Path::new(name);
        if path.is_absolute()
            || name.contains('\\')
            || path.components().any(|p| p == Component::ParentDir)
            || path.extension().and_then(|p| p.to_str()) != Some("md")
        {
            return Err("answer_doc must contain relative Markdown paths or basenames".into());
        }
        if name.contains('/') {
            let normal = path
                .components()
                .filter_map(|p| {
                    if let Component::Normal(n) = p {
                        n.to_str()
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            wanted.insert(normal);
        } else {
            let mut matching = corpus
                .iter()
                .filter(|s| Path::new(s).file_name() == path.file_name());
            let found = matching.next().map(String::as_str).unwrap_or(name);
            if matching.next().is_some() {
                return Err(format!(
                    "ambiguous answer_doc basename: {name}; use a relative path"
                ));
            }
            wanted.insert(found.to_owned());
        }
    }
    Ok(wanted)
}

fn compact(text: &str) -> String {
    local::casefold(text)
        .chars()
        .filter(|c| !c.is_whitespace() && !('\u{1c}'..='\u{1f}').contains(c))
        .collect()
}
fn missing_keys(answer: &str, keys: &Value) -> Result<Vec<String>> {
    let keys = keys
        .as_array()
        .filter(|ks| !ks.is_empty())
        .ok_or("offline grading requires nonempty keys")?;
    let text = compact(answer);
    let mut missing = Vec::new();
    for key in keys {
        let key = key.as_str().ok_or("invalid answer key")?;
        if key.split('|').any(|s| compact(s).is_empty()) {
            return Err("empty answer-key alternative".into());
        }
        if !key.split('|').any(|s| text.contains(&compact(s))) {
            missing.push(key.to_owned());
        }
    }
    Ok(missing)
}

pub fn run(config: &Path, key: Option<&str>, args: &[String]) -> Result<Option<(Value, i32)>> {
    if !local::simple(config)? {
        return Ok(None);
    }
    let mut args = args.to_vec();
    let golden = option(&mut args, "--golden")?.ok_or("evaluate requires --golden")?;
    let k: usize = option(&mut args, "--k")?
        .unwrap_or_else(|| "8".into())
        .parse()
        .map_err(|_| "invalid --k")?;
    if !(1..=100).contains(&k) {
        return Err("k must be between 1 and 100".into());
    }
    let answers = option(&mut args, "--answers")?;
    let out = option(&mut args, "--out")?;
    if !args.is_empty() {
        return Err("unexpected evaluation arguments".into());
    }
    let golden = Path::new(&golden);
    let answers = answers.as_deref().map(Path::new);
    if let Some(out) = out.as_deref() {
        let path = Path::new(out);
        if path.exists() {
            let dest = fs::canonicalize(path).map_err(|e| e.to_string())?;
            if [Some(config), Some(golden), answers]
                .into_iter()
                .flatten()
                .any(|p| fs::canonicalize(p).is_ok_and(|p| p == dest))
            {
                return Err("out must not overwrite an input".into());
            }
        }
    }
    let questions = rows(golden)?;
    let mut known = BTreeSet::new();
    for row in &questions {
        let q = row["q"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("golden requires unique nonempty q strings")?;
        if !known.insert(q) {
            return Err("duplicate golden question".into());
        }
    }
    let mut supplied = BTreeMap::new();
    if let Some(answers) = answers {
        for row in rows(answers)? {
            let q = row["q"]
                .as_str()
                .filter(|q| known.contains(q))
                .ok_or("unexpected answer question")?;
            let a = row["answer"]
                .as_str()
                .ok_or("answers require string answer")?;
            if supplied.insert(q.to_owned(), a.to_owned()).is_some() {
                return Err("duplicate answer question".into());
            }
        }
        if supplied.len() != known.len() {
            return Err("answers must cover every golden question exactly once".into());
        }
    }
    let corpus = local::EvaluationCorpus::open(config, key)?;
    let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let mut result = Vec::new();
    let mut hits = 0;
    let mut passed_keys = 0;
    let mut elapsed = 0.0;
    // The search room reports the engine of each search; semantic search can fall back to words alone.
    let mut engines = BTreeSet::new();
    let mut semantic = Value::Null;
    for g in questions {
        let q = g["q"].as_str().ok_or("invalid question")?;
        let started = Instant::now();
        let wanted = targets(&g["answer_doc"], &corpus.paths)?;
        let found = corpus.search(q, 100)?;
        engines.insert(found["engine"].as_str().unwrap_or("lexical-bm25").to_owned());
        if semantic.is_null() || found["semantic"]["status"] != "ok" {
            semantic = found["semantic"].clone();
        }
        let paths: Vec<_> = found["hits"]
            .as_array()
            .ok_or("invalid search hits")?
            .iter()
            .map(|h| h["path"].as_str().ok_or("invalid hit path"))
            .collect::<std::result::Result<_, _>>()?;
        let rank = paths
            .iter()
            .position(|p| wanted.contains(*p))
            .map(|r| r + 1);
        let top = &paths[..paths.len().min(k)];
        for path in top {
            corpus.read(path)?;
        }
        let secs = (started.elapsed().as_secs_f64() * 1_000_000.0).round() / 1_000_000.0;
        elapsed += secs;
        hits += usize::from(rank.is_some_and(|r| r <= k));
        let mut row = json!({"ts":stamp,"q":q,"answer_doc":g["answer_doc"],"rank":rank,"secs":secs,"rc":0,"top":top,
            "engine":found["engine"]});
        for field in ["src", "made_by"] {
            if let Some(value) = g.get(field) {
                row[field] = value.clone()
            }
        }
        if let Some(answer) = supplied.get(q) {
            let missing = missing_keys(answer, &g["keys"])?;
            passed_keys += usize::from(missing.is_empty());
            row["key_coverage_pass"] = json!(missing.is_empty());
            row["missing_keys"] = json!(missing);
            row["answer"] = json!(answer);
            row["keys"] = g["keys"].clone();
        }
        result.push(row);
    }
    let n = result.len();
    let graded = answers.is_some();
    let passed = hits == n && (!graded || passed_keys == n);
    let mut summary = json!({"n":n,"hit":hits,"k":k,"avg_secs":elapsed/n as f64,
        "quality_per_sec": if elapsed==0.0{None}else{Some(hits as f64/elapsed)},
        "answer_metric":if graded{"key-coverage-not-semantic-correctness"}else{"not-evaluated"}});
    if graded {
        summary["key_coverage_pass"] = json!(passed_keys);
    }
    let engine = match engines.len() {
        0 => "lexical-bm25".to_owned(),
        1 => engines.into_iter().next().unwrap_or_default(),
        _ => "mixed".to_owned(),
    };
    let report = json!({"library":corpus.key,"engine":engine,"semantic":semantic,"golden":golden.file_name().and_then(|s|s.to_str()),
        "passed":passed,"rows":result,"summary":summary});
    if let Some(out) = out {
        let absolute = std::path::absolute(out).map_err(|e| e.to_string())?;
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?
        }
        let mut bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        files::atomic(&absolute, &bytes, false)?;
    }
    Ok(Some((report, if passed { 0 } else { 1 })))
}
