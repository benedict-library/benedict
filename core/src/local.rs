//! Local Markdown operations share the persisted version-one index with existing installations.
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

type Result<T> = std::result::Result<T, String>;

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn relative(path: &Path) -> bool {
    !path.is_absolute() && !path.components().any(|p| p == Component::ParentDir)
}

struct Registry {
    path: PathBuf,
    rows: Map<String, Value>,
}

impl Registry {
    fn open(config: &Path) -> Result<Self> {
        let path = fs::canonicalize(config).map_err(|e| format!("{}: {e}", config.display()))?;
        let mut data = read_json(&path)?;
        let invalid = "registry requires version 1 and nonempty libraries object";
        if data["version"] != 1 {
            return Err(invalid.into());
        }
        let rows = data
            .get_mut("libraries")
            .and_then(Value::as_object_mut)
            .filter(|rows| !rows.is_empty())
            .ok_or(invalid)?;
        let rows = std::mem::take(rows);
        Ok(Self { path, rows })
    }

    // Room-backed registries and policy fields use the existing policy-aware implementation.
    fn simple(&self) -> Result<bool> {
        for (key, row) in &self.rows {
            let obj = row
                .as_object()
                .ok_or_else(|| format!("invalid library row: {key}"))?;
            if obj.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "wiki_root" | "shelf_root" | "central" | "name" | "status" | "reads" | "dirs"
                )
            }) {
                return Ok(false);
            }
            if row["wiki_root"].as_str().unwrap_or("").is_empty()
                || (obj.contains_key("central") && !row["central"].is_boolean())
                || (obj.contains_key("status")
                    && !matches!(row["status"].as_str(), Some("active" | "closed")))
            {
                return Err(format!("invalid library row: {key}"));
            }
            if let Some(dirs) = obj.get("dirs") {
                let dirs = dirs
                    .as_array()
                    .ok_or_else(|| format!("invalid dirs: {key}"))?;
                if !dirs.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|s| !s.is_empty() && relative(Path::new(s)))
                }) {
                    return Err(format!(
                        "dirs must be relative paths inside wiki_root: {key}"
                    ));
                }
            }
            if let Some(reads) = obj.get("reads") {
                let reads = reads
                    .as_array()
                    .ok_or_else(|| format!("invalid reads: {key}"))?;
                for r in reads {
                    let other = r
                        .as_str()
                        .and_then(|r| self.rows.get(r))
                        .ok_or_else(|| format!("unknown library in reads: {key}"))?;
                    if r == key.as_str()
                        || other["central"] == true
                        || other["reads"]
                            .as_array()
                            .is_some_and(|back| back.iter().any(|v| v == key.as_str()))
                    {
                        return Err(format!(
                            "reads must go one way between distinct, non-central targets: {key}"
                        ));
                    }
                }
            }
        }
        Ok(true)
    }

    fn key<'a>(&'a self, name: Option<&'a str>) -> Result<&'a str> {
        if let Some(key) = name {
            return Ok(key);
        }
        let mut keys = self.rows.iter().filter(|(_, row)| row["central"] == true);
        match (keys.next(), keys.next()) {
            (Some((key, _)), None) => Ok(key),
            _ => Err("select --library, or configure exactly one central library".into()),
        }
    }

    fn root(&self, key: &str) -> Result<PathBuf> {
        let row = self
            .rows
            .get(key)
            .ok_or_else(|| format!("unknown library: {key}"))?;
        if row["status"] == "closed" {
            return Err(format!("library is not active: {key}"));
        }
        let root = self
            .path
            .parent()
            .unwrap()
            .join(row["wiki_root"].as_str().ok_or("missing wiki_root")?);
        let root = fs::canonicalize(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err("wiki_root is not a directory".into());
        }
        Ok(root)
    }

    fn shelf_root(&self, key: &str) -> Result<Option<PathBuf>> {
        self.rows[key]
            .get("shelf_root")
            .map(|v| {
                let path = v
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("invalid shelf_root")?;
                Ok(self.path.parent().unwrap().join(path))
            })
            .transpose()
    }

    fn index_path(&self, key: &str) -> PathBuf {
        // Compatibility clients index wiki pages only. Keep their index separate
        // from a native index that holds shelf originals too.
        let kind = if self.rows[key].get("shelf_root").is_some() {
            "\0shelf"
        } else {
            ""
        };
        let hash = Sha256::digest(format!("{}\0{}{kind}", self.path.display(), key).as_bytes());
        self.path
            .parent()
            .unwrap()
            .join(".library-state")
            .join(format!("{:.16}-index.json", format!("{hash:x}")))
    }
}

pub fn simple(config: &Path) -> Result<bool> {
    Registry::open(config)?.simple()
}

pub fn selection(config: &Path, key: Option<&str>) -> Result<(String, PathBuf)> {
    let registry = Registry::open(config)?;
    registry.simple()?;
    let key = registry.key(key)?;
    Ok((key.to_string(), registry.root(key)?))
}

struct Alphabet {
    ranges: Vec<(u32, u32)>,
    folds: BTreeMap<char, String>,
}

fn alphabet() -> &'static Alphabet {
    // The historical on-disk index uses Python 3.11 str.casefold and re's Unicode \w.
    // Room search uses a separate bigram tokenizer; its tokens cannot query this index.
    static ALPHABET: OnceLock<Alphabet> = OnceLock::new();
    ALPHABET.get_or_init(|| {
        let mut ranges = Vec::new();
        let mut folds = BTreeMap::new();
        let hex = |s| u32::from_str_radix(s, 16).expect("bundled Unicode table");
        for line in include_str!("lexical_unicode.tsv")
            .lines()
            .filter(|s| !s.starts_with('#'))
        {
            let mut fields = line.split('\t');
            let kind = fields.next().unwrap();
            let cp = hex(fields.next().unwrap());
            let rest = fields.next().unwrap();
            match kind {
                "A" => ranges.push((cp, hex(rest))),
                "F" => {
                    folds.insert(
                        char::from_u32(cp).unwrap(),
                        rest.split(' ')
                            .map(|c| char::from_u32(hex(c)).unwrap())
                            .collect(),
                    );
                }
                _ => unreachable!("bundled Unicode table"),
            }
        }
        Alphabet { ranges, folds }
    })
}

pub fn casefold(text: &str) -> String {
    let alphabet = alphabet();
    let mut folded = String::with_capacity(text.len());
    for c in text.chars() {
        if let Some(v) = alphabet.folds.get(&c) {
            folded.push_str(v);
        } else {
            folded.push(c);
        }
    }
    folded
}

fn words(text: &str) -> impl Iterator<Item = String> {
    let alphabet = alphabet();
    let folded = casefold(text);
    let mut output = Vec::new();
    let mut token = String::new();
    for c in folded.chars() {
        let n = u32::from(c);
        let i = alphabet.ranges.partition_point(|(_, hi)| *hi < n);
        if c == '_' || alphabet.ranges.get(i).is_some_and(|(lo, _)| *lo <= n) {
            token.push(c);
        } else if !token.is_empty() {
            output.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        output.push(token);
    }
    output.into_iter()
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn discover(root: &Path, dir: &Path, docs: &mut BTreeMap<String, String>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let resolved = fs::canonicalize(dir).map_err(|e| e.to_string())?;
    if !resolved.starts_with(root) {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            discover(root, &path, docs)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if relative != "log.md" && !docs.contains_key(&relative) {
                let Ok(target) = fs::canonicalize(&path) else {
                    continue;
                };
                if target.starts_with(root) && target.is_file() {
                    let bytes = fs::read(target).map_err(|e| e.to_string())?;
                    docs.insert(
                        relative,
                        normalize_newlines(&String::from_utf8_lossy(&bytes)),
                    );
                }
            }
        }
    }
    Ok(())
}

fn documents(reg: &Registry, key: &str, root: &Path) -> Result<BTreeMap<String, String>> {
    let mut docs = BTreeMap::new();
    if let Some(dirs) = reg.rows[key]["dirs"].as_array().filter(|l| !l.is_empty()) {
        for dir in dirs.iter().filter_map(Value::as_str) {
            let dir = Path::new(dir);
            if !dir
                .components()
                .any(|c| matches!(c, Component::Normal(s) if s.to_string_lossy().starts_with('.')))
            {
                discover(&root, &root.join(dir), &mut docs)?;
            }
        }
    } else {
        discover(&root, &root, &mut docs)?;
    }
    if let Some(shelf) = reg.shelf_root(key)? {
        crate::shelf::index(&shelf, &mut docs)?;
    }
    Ok(docs)
}

/// Evaluation opens real wiki files, even if a document changed since the last index.
pub struct EvaluationCorpus {
    registry: Registry,
    pub key: String,
    pub paths: Vec<String>,
}

impl EvaluationCorpus {
    pub fn open(config: &Path, key: Option<&str>) -> Result<Self> {
        let registry = Registry::open(config)?;
        if !registry.simple()? {
            return Err("registry requires the policy-aware evaluator".into());
        }
        let key = registry.key(key)?.to_owned();
        let paths = documents(&registry, &key, &registry.root(&key)?)?
            .into_keys()
            .collect();
        Ok(Self {
            registry,
            key,
            paths,
        })
    }
    pub fn search(&self, query: &str, top: usize) -> Result<Value> {
        search_one(&self.registry, &self.key, query, top)
    }
    pub fn read(&self, path: &str) -> Result<Value> {
        read(&self.registry, &self.key, path, None)
    }
}

fn build_index(reg: &Registry, key: &str) -> Result<Value> {
    use tempfile::NamedTempFile;
    let path = reg.index_path(key);
    let parent = path.parent().unwrap();
    crate::files::no_link(parent)?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock_path = path.with_extension("lock");
    crate::files::no_link(&lock_path)?;
    let _lock = crate::files::lock(&lock_path)?;
    let root = reg.root(key)?;
    let docs = documents(reg, key, &root)?;
    let mut counts = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    for (path, body) in &docs {
        let mut counter = BTreeMap::new();
        for word in words(body) {
            *counter.entry(word).or_insert(0u64) += 1;
        }
        lengths.insert(path, counter.values().sum::<u64>());
        counts.insert(path, counter);
    }
    let indexed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false);
    let record = json!({"version":1,"root":root,"shelf_root":reg.shelf_root(key)?,"indexed_at":indexed_at,"documents":docs,"counts":counts,"lengths":lengths});
    let mut tmp = NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut tmp, &record).map_err(|e| e.to_string())?;
    tmp.flush()
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(
        json!({"library":key,"documents":docs.len(),"engine":"lexical-bm25","persistent":true,"indexed_at":indexed_at}),
    )
}

pub fn index(config: &Path, key: &str) -> Result<Value> {
    let reg = Registry::open(config)?;
    if !reg.simple()? {
        return Err("registry requires the policy-aware indexer".into());
    }
    build_index(&reg, key)
}

pub fn refresh(config: &Path, key: &str, path: Option<&Path>) -> Result<()> {
    index(config, key)?;
    if std::env::var_os("LIBRARY_ROOM_HOME").is_some_and(|s| !s.is_empty()) {
        let mut cmd = std::process::Command::new("python3");
        cmd.arg(crate::app().join("bin/room_sync.py"))
            .arg("--config")
            .arg(config)
            .args(["--library", key]);
        if let Some(path) = path {
            cmd.arg("--path").arg(path);
        }
        let output = cmd.output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "index written but room sync failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    Ok(())
}

fn search_one(reg: &Registry, key: &str, query: &str, top: usize) -> Result<Value> {
    let root = reg.root(key)?;
    let words: BTreeSet<_> = words(query).collect();
    if words.is_empty() {
        return Err("query must contain a word".into());
    }
    let path = reg.index_path(key);
    if !path.is_file() {
        return Err("prebuilt index missing; run index or librarian before search".into());
    }
    let record = read_json(&path)?;
    if record["shelf_root"] != json!(reg.shelf_root(key)?)
        || record["version"] != 1
        || record["root"] != root.to_str().ok_or("wiki_root is not UTF-8")?
    {
        return Err("index does not match selected library; rebuild index".into());
    }
    let invalid = "invalid index; rebuild index";
    let docs = record["documents"].as_object().ok_or(invalid)?;
    let counts = record["counts"].as_object().ok_or(invalid)?;
    let lengths = record["lengths"].as_object().ok_or(invalid)?;
    if docs.len() != counts.len() || docs.len() != lengths.len() {
        return Err(invalid.into());
    }
    let indexed_at = record["indexed_at"].as_str().ok_or(invalid)?;
    let shelf = reg.shelf_root(key)?;
    let vocabulary: BTreeSet<&str> = counts
        .values()
        .filter_map(Value::as_object)
        .flat_map(|c| c.keys().map(String::as_str))
        .collect();
    let stems: BTreeMap<&String, &str> = words
        .iter()
        .filter_map(|w| stem(w, &vocabulary).map(|s| (w, s)))
        .collect();
    let mut df = BTreeMap::new();
    let mut total = 0.0;
    let mut candidates = Vec::new();
    for (path, body) in docs {
        let text = body.as_str().ok_or(invalid)?;
        let counter = counts.get(path).and_then(Value::as_object).ok_or(invalid)?;
        let length = lengths.get(path).and_then(Value::as_u64).ok_or(invalid)? as f64;
        total += length;
        let mut matches = Vec::new();
        for word in &words {
            let found = match stems.get(word) {
                Some(stem) => prefixed(counter, stem).ok_or(invalid)?,
                None => match counter.get(word) {
                    Some(freq) => Some(freq.as_u64().ok_or(invalid)?),
                    None => None,
                },
            };
            if let Some(freq) = found {
                *df.entry(word).or_insert(0) += 1;
                if freq > 0 {
                    matches.push((word, freq as f64));
                }
            }
        }
        if !matches.is_empty() {
            if let (Some(root), Some(name)) = (&shelf, path.strip_prefix("shelf:")) {
                let changed = "shelf original changed or unavailable; rebuild index";
                if crate::shelf::read(root, name).map_err(|_| changed)?.0 != text {
                    return Err(changed.into());
                }
            }
            candidates.push((path, text, length, matches));
        }
    }
    let average = if total == 0.0 {
        1.0
    } else {
        total / docs.len() as f64
    };
    let mut ranked = Vec::new();
    for (path, text, len, hits) in candidates {
        let mut score = 0.0;
        for (word, n) in hits {
            let df = f64::from(df[word]);
            let idf = (1.0 + (docs.len() as f64 - df + 0.5) / (df + 0.5)).ln();
            score += idf * n * 2.5 / (n + 1.5 * (0.25 + 0.75 * len / average));
        }
        ranked.push((score, path, text));
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
    let hits: Vec<_> = ranked.into_iter().take(top).map(|(score,path,text)| {
        let title = text.lines().find(|s| s.starts_with("# ")).map(|s| s.trim_start_matches(['#',' '])).unwrap_or(path);
        json!({"path":path,"title":title,"score":(score*1e6).round_ties_even()/1e6,"excerpt":text.chars().take(500).collect::<String>()})
    }).collect();
    let mut result =
        json!({"library":key,"engine":"lexical-bm25","indexed_at":indexed_at,"hits":hits});
    if hits_empty(&result) {
        let korean_query = words.iter().any(|w| w.chars().any(hangul));
        let korean_documents = vocabulary.iter().any(|w| w.chars().any(hangul));
        result["hint"] = json!(if korean_query && !korean_documents {
            "The library's documents contain no Korean words, and search matches words, not meanings. Search again with words in the documents' language."
        } else {
            "No document contains these words. Search matches words, not meanings, so try the words the documents use, or fewer words."
        });
    }
    Ok(result)
}

fn hits_empty(result: &Value) -> bool {
    result["hits"].as_array().is_some_and(Vec::is_empty)
}

fn hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

/// A word in the text often carries an ending: "반품하려면" and "반품은" for "반품", "returned" for "return". The search
/// counts every indexed word that begins with the returned part. For a query word of two or more Hangul syllables it is
/// the longest leading part of at least two syllables that begins some indexed word; for another word of four or more
/// characters it is the word itself. Shorter words match only whole words.
fn stem<'a>(word: &str, vocabulary: &BTreeSet<&'a str>) -> Option<&'a str> {
    let length = word.chars().count();
    let ends: Vec<usize> = if length >= 2 && word.chars().all(hangul) {
        word.char_indices()
            .map(|(i, _)| i)
            .skip(2)
            .chain([word.len()])
            .collect()
    } else if length >= 4 {
        vec![word.len()]
    } else {
        return None;
    };
    ends.iter().rev().find_map(|&end| {
        let part = &word[..end];
        let from = (std::ops::Bound::Included(part), std::ops::Bound::Unbounded);
        let next: &'a str = *vocabulary.range::<str, _>(from).next()?;
        next.starts_with(part).then(|| &next[..end])
    })
}

/// The summed count of the document's words that begin with `stem`; `None` inside when it has none, and `None`
/// outside when a count is not a number.
fn prefixed(counter: &serde_json::Map<String, Value>, stem: &str) -> Option<Option<u64>> {
    let mut sum = None;
    for (_, freq) in counter.iter().filter(|(w, _)| w.starts_with(stem)) {
        *sum.get_or_insert(0) += freq.as_u64()?;
    }
    Some(sum)
}

fn read(reg: &Registry, key: &str, path: &str, pages: Option<&str>) -> Result<Value> {
    let root = reg.root(key)?;
    if let Some(name) = path.strip_prefix("shelf:") {
        let shelf = reg
            .shelf_root(key)?
            .ok_or("no shelf configured for the selected library")?;
        return Ok(match crate::shelf::open_original(&shelf, name)? {
            crate::shelf::Original::Text(text, sha256) => {
                json!({"library":key, "path":path, "text":text, "sha256":sha256})
            }
            crate::shelf::Original::Binary(bytes, sha256) => {
                let converted = match pages {
                    Some(p) => crate::shelf::convert(&bytes, name, p)?.map(|text| (p, text)),
                    None => None,
                };
                match converted {
                    Some((p, text)) => json!({
                        "library":key, "path":path, "pages":p, "text":text, "sha256":sha256
                    }),
                    None => json!({
                        "library":key, "path":path, "binary":true, "bytes":bytes.len(), "sha256":sha256,
                        "note": if std::env::var_os("BENEDICT_KORDOC").is_some() {
                            format!("Not a text original. Read part of it with --pages, e.g. --pages 1-3 (at most {} pages).", crate::shelf::MAX_PAGES)
                        } else {
                            "Not a text original. Open this file from the shelf directly.".to_string()
                        }
                    }),
                }
            }
        });
    }
    let path = Path::new(path);
    if !relative(path) {
        return Err("path must be relative and remain inside the selected library".into());
    }
    let target = fs::canonicalize(root.join(path)).map_err(|e| e.to_string())?;
    let rel = target
        .strip_prefix(&root)
        .map_err(|_| "symlink escapes selected library")?;
    if target.extension().is_none_or(|s| s != "md") {
        return Err("read accepts Markdown documents only".into());
    }
    let text = fs::read_to_string(&target).map_err(|e| e.to_string())?;
    Ok(
        json!({"library":key,"path":rel,"text":normalize_newlines(&text),"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}),
    )
}

/// Validate native paths while preserving external URLs, identifiers, and legacy free-form references.
pub fn verify_references(config: &Path, key: &str, references: &str) -> Result<()> {
    let reg = Registry::open(config)?;
    for line in references.lines() {
        let line = line.trim();
        // Comma + space is the legacy list separator. Keep a filename that
        // contains a comma as one item when the complete path exists.
        let candidates: Vec<_> = if line.contains(", ") && read(&reg, key, line, None).is_err() {
            line.split(", ").map(str::trim).collect()
        } else {
            vec![line]
        };
        for value in candidates {
            if value.starts_with("shelf:")
                || (value.ends_with(".md")
                    && !value.contains("://")
                    && !Path::new(value).is_absolute())
            {
                read(&reg, key, value, None).map_err(|e| format!("invalid local evidence: {e}"))?;
            }
        }
    }
    Ok(())
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<i32>> {
    if !matches!(cmd, "method" | "index" | "search" | "read") {
        return Ok(None);
    }
    let mut config = "library.json";
    let mut key = None;
    let mut top = 5;
    let mut lang = "en";
    let mut question = "";
    let mut pages = None;
    let mut pos = Vec::new();
    let mut literal = false;
    let mut args = args.iter();
    while let Some(s) = args.next() {
        if literal {
            pos.push(s.as_str());
            continue;
        }
        if s == "--" {
            literal = true;
            continue;
        }
        if s == "--help" || s == "-h" {
            return Ok(None);
        }
        let (opt, val) = s
            .split_once('=')
            .map_or((s.as_str(), None), |(k, v)| (k, Some(v)));
        if !s.starts_with('-') {
            pos.push(s.as_str());
            continue;
        }
        let mut value = || {
            val.or_else(|| args.next().map(String::as_str))
                .ok_or_else(|| format!("{opt} requires a value"))
        };
        match (cmd, opt) {
            ("method", "--lang") => lang = value()?,
            ("index" | "search" | "read", "--config") => config = value()?,
            ("index" | "search" | "read", "--library") => key = Some(value()?),
            ("search" | "read", "--question") => question = value()?,
            ("read", "--pages") => pages = Some(value()?),
            ("search", "--top") => {
                top = value()?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=100).contains(n))
                    .ok_or("top must be between 1 and 100")?
            }
            _ => return Err(format!("unrecognized option: {opt}")),
        }
    }
    if cmd == "method" {
        if !pos.is_empty() {
            return Err("method takes no positional arguments".into());
        }
        print!(
            "{}",
            match lang {
                "en" => include_str!("../../docs/method.md"),
                "ko" => include_str!("../../docs/method.ko.md"),
                _ => return Err("lang must be en or ko".into()),
            }
        );
        return Ok(Some(0));
    }
    if pos.len() != usize::from(cmd != "index") {
        return Err(format!("wrong number of arguments for {cmd}"));
    }
    let reg = Registry::open(Path::new(config))?;
    if !reg.simple()? {
        return Ok(None);
    }
    let name = reg.key(key)?;
    let mut output = match cmd {
        "read" => read(&reg, name, pos[0], pages)?,
        "index" => build_index(&reg, name)?,
        _ => search_one(&reg, name, pos[0], top)?,
    };
    if cmd == "search" && key.is_none() {
        if let Some(reads) = reg.rows[name]["reads"].as_array().filter(|v| !v.is_empty()) {
            let mut near = Vec::new();
            for other in reads.iter().filter_map(Value::as_str) {
                let mut row = json!({"library":other,"name":reg.rows[other].get("name").unwrap_or(&json!(other))});
                match search_one(&reg, other, pos[0], 3) {
                    Ok(h) => row["hits"] = h["hits"].clone(),
                    Err(e) => row["error"] = json!(e),
                }
                near.push(row);
            }
            output["near"] = Value::Array(near);
        }
    }
    if cmd != "index" {
        // Mask before the result is recorded or printed, so neither keeps the values.
        crate::redact::value(&mut output);
    }
    if crate::records::enabled() {
        match cmd {
            "read" => {
                output["record_id"] = crate::records::store(
                    name,
                    "reading",
                    question,
                    "",
                    output["text"].as_str().ok_or("missing text")?,
                    output["path"].as_str().ok_or("missing source")?,
                )?["id"]
                    .clone()
            }
            "search" => {
                let mut paths = Vec::new();
                for lib in
                    std::iter::once(&output).chain(output["near"].as_array().into_iter().flatten())
                {
                    for hit in lib["hits"].as_array().into_iter().flatten() {
                        paths.push(json!({"library":lib["library"],"path":hit["path"]}));
                    }
                }
                output["record_id"] = crate::records::store(
                    name,
                    "search",
                    question,
                    pos[0],
                    "",
                    &serde_json::to_string(&paths).map_err(|e| e.to_string())?,
                )?["id"]
                    .clone();
            }
            _ => {}
        }
    }
    println!("{output}");
    Ok(Some(0))
}
