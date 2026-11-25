//! Local Markdown operations share the persisted version-one index with existing installations.
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use crate::index_store::{self, IndexEntry, IndexState, IndexedDoc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::time::Instant;
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

pub(crate) struct Registry {
    pub(crate) path: PathBuf,
    pub(crate) rows: Map<String, Value>,
}

impl Registry {
    pub(crate) fn open(config: &Path) -> Result<Self> {
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
    pub(crate) fn simple(&self) -> Result<bool> {
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

    pub(crate) fn root(&self, key: &str) -> Result<PathBuf> {
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

    pub(crate) fn shelf_root(&self, key: &str) -> Result<Option<PathBuf>> {
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

    /// The JSON index file of earlier releases; its lock file still serializes indexing.
    pub(crate) fn index_path(&self, key: &str) -> PathBuf {
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

    /// The wiki and shelf roots an index version is made for; the shelf root is empty without a shelf.
    pub(crate) fn roots(&self, key: &str) -> Result<(String, String)> {
        let root = self.root(key)?;
        let root = root.to_str().ok_or("wiki_root is not UTF-8")?.to_owned();
        let shelf = match self.shelf_root(key)? {
            Some(path) => path.to_str().ok_or("shelf_root is not UTF-8")?.to_owned(),
            None => String::new(),
        };
        Ok((root, shelf))
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

pub(crate) fn words(text: &str) -> impl Iterator<Item = String> {
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

/// Receives each document as (path, text), one at a time.
type Visit<'a> = dyn FnMut(String, String) -> Result<()> + 'a;

fn discover(root: &Path, dir: &Path, seen: &mut HashSet<String>, visit: &mut Visit<'_>) -> Result<()> {
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
            discover(root, &path, seen, visit)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if relative != "log.md" && !seen.contains(&relative) {
                let Ok(target) = fs::canonicalize(&path) else {
                    continue;
                };
                if target.starts_with(root) && target.is_file() {
                    let bytes = fs::read(target).map_err(|e| e.to_string())?;
                    seen.insert(relative.clone());
                    visit(relative, normalize_newlines(&String::from_utf8_lossy(&bytes)))?;
                }
            }
        }
    }
    Ok(())
}

/// Each document of the library, wiki pages then shelf originals, read one at a time.
fn each_document(reg: &Registry, key: &str, root: &Path, visit: &mut Visit<'_>) -> Result<()> {
    let mut seen = HashSet::new();
    if let Some(dirs) = reg.rows[key]["dirs"].as_array().filter(|l| !l.is_empty()) {
        for dir in dirs.iter().filter_map(Value::as_str) {
            let dir = Path::new(dir);
            if !dir
                .components()
                .any(|c| matches!(c, Component::Normal(s) if s.to_string_lossy().starts_with('.')))
            {
                discover(root, &root.join(dir), &mut seen, visit)?;
            }
        }
    } else {
        discover(root, root, &mut seen, visit)?;
    }
    if let Some(shelf) = reg.shelf_root(key)? {
        crate::shelf::each(&shelf, visit)?;
    }
    Ok(())
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
        let mut paths = Vec::new();
        each_document(&registry, &key, &registry.root(&key)?, &mut |path, _| {
            paths.push(path);
            Ok(())
        })?;
        paths.sort();
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

/// The catalog row, length and word counts of one document.
fn indexed(path: String, text: &str, hash: String) -> IndexedDoc {
    let mut counter = BTreeMap::new();
    for word in words(text) {
        *counter.entry(word).or_insert(0u64) += 1;
    }
    let title = text
        .lines()
        .find(|s| s.starts_with("# "))
        .map_or(path.as_str(), |s| s.trim_start_matches(['#', ' ']))
        .replace('\0', "");
    IndexedDoc {
        title,
        summary: summary(text).replace('\0', ""),
        hash,
        length: counter.values().sum(),
        terms: counter.into_iter().collect(),
        path,
    }
}

/// What a document is about: its front matter title and description, otherwise its first line.
fn summary(text: &str) -> String {
    let mut body = text;
    let mut parts = Vec::new();
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---")
    {
        for line in rest[..end].lines() {
            for key in ["title:", "description:"] {
                if let Some(value) = line.strip_prefix(key) {
                    let value = value.trim().trim_matches(['"', '\'']).trim();
                    if !value.is_empty() {
                        parts.push(value);
                    }
                }
            }
        }
        body = rest[end + 4..].split_once('\n').map_or("", |(_, after)| after);
    }
    let summary = if parts.is_empty() {
        body.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_owned()
    } else {
        parts.join(" — ")
    };
    summary.chars().take(300).collect()
}

/// Changed documents kept in memory while the library is read. Beyond this many (a large change),
/// documents are written while they are read, as on a first index.
const HELD: usize = 2_000;
const MANY: &str = "more changed documents than are held";

/// Index the library into the database. Only documents whose text changed are written again,
/// documents that are gone are removed, and the new version is set in the same transaction.
/// The documents are read before the transaction starts, so the room can record searches meanwhile.
fn build_index(reg: &Registry, key: &str) -> Result<Value> {
    let started = Instant::now();
    let lock_path = reg.index_path(key).with_extension("lock");
    let parent = lock_path.parent().unwrap();
    crate::files::no_link(parent)?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    crate::files::no_link(&lock_path)?;
    let _lock = crate::files::lock(&lock_path)?;
    let root = reg.root(key)?;
    let (root_text, shelf_text) = reg.roots(key)?;
    let previous = index_store::state(key)?;
    let reset = previous
        .as_ref()
        .is_none_or(|s| s.root != root_text || s.shelf_root != shelf_text);
    let mut known = if reset {
        HashMap::new()
    } else {
        index_store::hashes(key)?
    };
    let (mut documents, mut changed, mut removed) = (0u64, 0u64, 0u64);
    let new_state = |documents| IndexState {
        root: root_text.clone(),
        shelf_root: shelf_text.clone(),
        indexed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        documents,
        vectors_at: String::new(),
    };
    // Read every document first and keep only the changed ones.
    let mut held = Vec::new();
    let mut left = known.clone();
    let read_first = !reset
        && match each_document(reg, key, &root, &mut |path, text| {
            documents += 1;
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            if left.remove(&path).as_deref() != Some(hash.as_str()) {
                if held.len() == HELD {
                    return Err(MANY.into());
                }
                held.push(indexed(path, &text, hash));
            }
            Ok(())
        }) {
            Ok(()) => true,
            Err(e) if e == MANY => false,
            Err(e) => return Err(e),
        };
    let written = if read_first {
        changed = held.len() as u64;
        removed = left.len() as u64;
        if changed + removed == 0 {
            None
        } else {
            index_store::write(key, false, &mut |sink| {
                for doc in held.drain(..) {
                    sink(IndexEntry::Document(doc))?;
                }
                for (path, _) in left.drain() {
                    sink(IndexEntry::Removed(path))?;
                }
                Ok(Some(new_state(documents)))
            })?
        }
    } else {
        drop((held, left));
        documents = 0;
        index_store::write(key, reset, &mut |sink| {
            each_document(reg, key, &root, &mut |path, text| {
                documents += 1;
                let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
                if known.remove(&path).as_deref() != Some(hash.as_str()) {
                    changed += 1;
                    sink(IndexEntry::Document(indexed(path, &text, hash)))?;
                }
                Ok(())
            })?;
            for (path, _) in known.drain() {
                removed += 1;
                sink(IndexEntry::Removed(path))?;
            }
            Ok((reset || changed > 0 || removed > 0).then(|| new_state(documents)))
        })?
    };
    let state = match written {
        Some(state) => state,
        None => previous.ok_or("index state missing; rebuild index")?,
    };
    let (catalog, vocabulary) = index_store::counts(key)?;
    Ok(json!({"library":key,"documents":state.documents,"engine":"lexical-bm25","persistent":true,
        "indexed_at":state.indexed_at,"changed":changed,"removed":removed,"catalog":catalog,
        "words":vocabulary,"seconds":(started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0}))
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

/// One library's hits, from the search room that holds its index in memory. Without the room
/// nothing is searched: reading the files here instead would hide that the room is not running.
fn search_one(reg: &Registry, key: &str, query: &str, top: usize) -> Result<Value> {
    if words(query).next().is_none() {
        return Err("query must contain a word".into());
    }
    let socket =
        crate::rooms::socket_path().ok_or("search room not running: set LIBRARY_ROOM_SOCKET")?;
    crate::rooms::request(
        &socket,
        json!({"op":"search","project":crate::records::project()?,"config":reg.path,
               "library":key,"query":query,"top":top}),
    )
    .map_err(|e| match e.strip_prefix(crate::rooms::NOT_RUNNING) {
        Some(rest) => format!("search room not running{rest}"),
        None => e,
    })
}

pub(crate) fn hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

/// A word in the text often carries an ending: "반품하려면" and "반품은" for "반품", "returned" for "return". The search
/// counts every indexed word that begins with the returned part. For a query word of two or more Hangul syllables it is
/// the longest leading part of at least two syllables that begins some indexed word; for another word of four or more
/// characters it is the word itself. Shorter words match only whole words. `first_from` gives the first indexed word
/// at or after a part, in sorted order.
pub(crate) fn stem<'a>(word: &str, first_from: &dyn Fn(&str) -> Option<&'a str>) -> Option<&'a str> {
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
        let next = first_from(part)?;
        next.starts_with(part).then(|| &next[..end])
    })
}

pub(crate) fn read(reg: &Registry, key: &str, path: &str, pages: Option<&str>) -> Result<Value> {
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
    let mut web = false;
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
            ("search", "--web") if val.is_none() => web = true,
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
                    output["text"]
                        .as_str()
                        .or_else(|| output["note"].as_str())
                        .ok_or("missing text")?,
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
    // Semantic search that was configured but failed still prints the word results, and says so.
    let mut code = i32::from(output["semantic"]["status"] == "failed");
    if web {
        let (step, rc) = web_step(name, pos[0])?;
        output["web"] = step;
        code = rc;
    }
    println!("{output}");
    Ok(Some(code))
}

/// The web step after the inside results. LIBRARY_WEB_CMD is a shell command line; it receives the library key and
/// the search terms as two more arguments and nothing else. Without it nothing leaves the library and the step says
/// how an agent keeps a public source it found itself.
fn web_step(key: &str, query: &str) -> Result<(Value, i32)> {
    use std::io::{Read, Seek};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let Some(cmd) = std::env::var("LIBRARY_WEB_CMD")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        let next = format!(
            "LIBRARY_WEB_CMD is not set, so nothing left the library. Find public sources with your own web search \
             tool, then keep each one with: benedict web-source --library {key} PUBLIC_QUERY SOURCE_URL"
        );
        return Ok((json!({"library":key,"configured":false,"next":next}), 1));
    };
    let limit = match std::env::var("LIBRARY_WEB_TIMEOUT") {
        Ok(s) => s
            .trim()
            .parse::<u64>()
            .map_err(|_| "LIBRARY_WEB_TIMEOUT must be whole seconds")?,
        Err(_) => 540,
    };
    let io = |e: std::io::Error| format!("web step: {e}");
    let mut out = tempfile::tempfile().map_err(io)?;
    let mut err = tempfile::tempfile().map_err(io)?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{cmd} \"$@\""))
        .arg("sh")
        .arg(key)
        .arg(query)
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(io)?)
        .stderr(err.try_clone().map_err(io)?)
        .spawn()
        .map_err(io)?;
    let deadline = Instant::now() + Duration::from_secs(limit);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(io)? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = |file: &mut fs::File| -> Result<String> {
        let mut bytes = Vec::new();
        file.rewind().map_err(io)?;
        file.read_to_end(&mut bytes).map_err(io)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let (stdout, stderr) = (text(&mut out)?, text(&mut err)?);
    let code = status.map_or(124, |s| s.code().unwrap_or(1));
    let mut step = json!({"library":key,"configured":true,"exit_code":code,"output":stdout});
    if status.is_none() {
        step["error"] = json!(format!("web step stopped after {limit} seconds"));
    } else if !stderr.trim().is_empty() {
        step["error"] = json!(stderr.trim());
    }
    Ok((step, code))
}
