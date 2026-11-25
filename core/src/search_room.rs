//! The search room. It loads each library's index from the database into memory once (catalog,
//! word counts and lengths, no document text) and answers searches from it. When the librarian
//! writes a new version, the room keeps answering from the old one until the new one is loaded.
use crate::index_store::{self, IndexRow, IndexState};
use crate::local::{Registry, hangul, read, stem, words};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::BufRead;
use std::ops::Bound;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;

const INVALID: &str = "invalid index; rebuild index";
const MISSING: &str = "prebuilt index missing; run index or librarian before search";
const MISMATCH: &str = "index does not match selected library; rebuild index";
/// Loading runs on its own thread. A search waits at most this long for it, so the room's other
/// requests are not held up; a small index is ready within it.
const LOAD_WAIT: Duration = Duration::from_millis(300);

/// One library's index in memory.
pub(crate) struct Index {
    pub state: IndexState,
    paths: Vec<String>,
    titles: Vec<String>,
    summaries: Vec<String>,
    hashes: Vec<String>,
    lengths: Vec<u64>,
    postings: BTreeMap<String, Vec<(u32, u32)>>,
    title_words: Vec<Vec<String>>,
    summary_words: Vec<BTreeMap<String, u32>>,
    korean: bool,
    /// Unit-length vectors of document parts, when the librarian has embedded them.
    vectors: Vec<(u32, Vec<f32>)>,
}

/// Reciprocal rank fusion constant, as in the original RRF paper and common hybrid search.
const RRF_K: f64 = 60.0;
/// Each ranking contributes this many candidates before fusion.
const CANDIDATES: usize = 50;

/// One query word: its stem, and the documents that hold it with the summed count.
struct Found<'a> {
    word: &'a str,
    stem: Option<&'a str>,
    docs: HashMap<u32, u64>,
}

#[derive(Default)]
struct Builder {
    catalog: Vec<(String, String, String, String)>,
    ids: HashMap<String, u32>,
    sorted: bool,
    lengths: Vec<Option<u64>>,
    postings: BTreeMap<String, Vec<(u32, u32)>>,
    vectors: Vec<(u32, Vec<f32>)>,
}

impl Builder {
    fn sort(&mut self) {
        if !self.sorted {
            self.catalog.sort_by(|a, b| a.0.cmp(&b.0));
            self.ids = (0..)
                .zip(&self.catalog)
                .map(|(id, row)| (row.0.clone(), id))
                .collect();
            self.lengths = vec![None; self.catalog.len()];
            self.sorted = true;
        }
    }

    fn id(&mut self, path: &str) -> Result<u32> {
        self.sort();
        self.ids.get(path).copied().ok_or_else(|| INVALID.into())
    }

    fn row(&mut self, row: IndexRow) -> Result<()> {
        match row {
            IndexRow::Catalog { path, title, summary, hash } => {
                if self.sorted {
                    return Err(INVALID.into());
                }
                self.catalog.push((path, title, summary, hash));
            }
            IndexRow::Length { path, length } => {
                let id = self.id(&path)?;
                self.lengths[id as usize] = Some(length);
            }
            IndexRow::Term { path, word, freq } => {
                let id = self.id(&path)?;
                let freq = u32::try_from(freq).map_err(|_| INVALID)?;
                match self.postings.get_mut(word.as_str()) {
                    Some(docs) => docs.push((id, freq)),
                    None => {
                        self.postings.insert(word, vec![(id, freq)]);
                    }
                }
            }
            IndexRow::Vector { path, vec } => {
                // Vectors of a document the index no longer holds wait for the librarian's next cycle.
                if let Some(&id) = self.ids.get(&path) {
                    self.vectors.push((id, vec));
                }
            }
        }
        Ok(())
    }

    fn finish(mut self, state: IndexState) -> Result<Index> {
        self.sort();
        if state.documents != self.catalog.len() as u64 {
            return Err(INVALID.into());
        }
        let lengths = self
            .lengths
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(INVALID)?;
        let mut paths = Vec::new();
        let mut titles = Vec::new();
        let mut summaries = Vec::new();
        let mut hashes = Vec::new();
        for (path, title, summary, hash) in self.catalog {
            paths.push(path);
            titles.push(title);
            summaries.push(summary);
            hashes.push(hash);
        }
        let mut index = Index::from_parts(state, paths, [titles, summaries, hashes], lengths, self.postings);
        index.vectors = self.vectors;
        Ok(index)
    }
}

impl Index {
    fn from_parts(
        state: IndexState,
        paths: Vec<String>,
        [titles, summaries, hashes]: [Vec<String>; 3],
        lengths: Vec<u64>,
        postings: BTreeMap<String, Vec<(u32, u32)>>,
    ) -> Index {
        let title_words = titles.iter().map(|t| words(t).collect()).collect();
        let summary_words = summaries
            .iter()
            .map(|s| {
                let mut counter = BTreeMap::new();
                for word in words(s) {
                    *counter.entry(word).or_insert(0) += 1;
                }
                counter
            })
            .collect();
        let korean = postings.keys().any(|w| w.chars().any(hangul));
        Index {
            state,
            paths,
            titles,
            summaries,
            hashes,
            lengths,
            postings,
            title_words,
            summary_words,
            korean,
            vectors: Vec::new(),
        }
    }

    /// The library's current index from the database; None before its first index.
    pub(crate) fn load(key: &str) -> Result<Option<Index>> {
        let mut builder = Builder::default();
        let state = index_store::load(key, &mut |row| builder.row(row))?;
        state.map(|state| builder.finish(state)).transpose()
    }

    fn first_from(&self, part: &str) -> Option<&str> {
        self.postings
            .range::<str, _>((Bound::Included(part), Bound::Unbounded))
            .next()
            .map(|(word, _)| word.as_str())
    }

    /// For each query word, every document that holds it, or a word that begins with its stem.
    fn found<'a>(&'a self, words: &'a BTreeSet<String>) -> Vec<Found<'a>> {
        words
            .iter()
            .map(|word| {
                let stem = stem(word, &|part| self.first_from(part));
                let mut docs = HashMap::new();
                match stem {
                    Some(stem) => {
                        for (_, postings) in self
                            .postings
                            .range::<str, _>((Bound::Included(stem), Bound::Unbounded))
                            .take_while(|(w, _)| w.starts_with(stem))
                        {
                            for &(doc, freq) in postings {
                                *docs.entry(doc).or_insert(0) += u64::from(freq);
                            }
                        }
                    }
                    None => {
                        for &(doc, freq) in self.postings.get(word.as_str()).into_iter().flatten() {
                            docs.insert(doc, u64::from(freq));
                        }
                    }
                }
                Found { word, stem, docs }
            })
            .collect()
    }

    /// BM25 over the documents, highest score first, then by path.
    fn rank(&self, found: &[Found<'_>], top: usize) -> Vec<(f64, u32)> {
        let n = self.paths.len() as f64;
        let total: f64 = self.lengths.iter().map(|&l| l as f64).sum();
        let average = if total == 0.0 { 1.0 } else { total / n };
        let candidates: BTreeSet<u32> = found.iter().flat_map(|f| f.docs.keys().copied()).collect();
        let mut ranked: Vec<(f64, u32)> = candidates
            .into_iter()
            .map(|doc| {
                let len = self.lengths[doc as usize] as f64;
                let mut score = 0.0;
                for f in found {
                    if let Some(&freq) = f.docs.get(&doc).filter(|&&c| c > 0) {
                        let df = f.docs.len() as f64;
                        let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                        let freq = freq as f64;
                        score += idf * freq * 2.5 / (freq + 1.5 * (0.25 + 0.75 * len / average));
                    }
                }
                (score, doc)
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then(self.paths[a.1 as usize].cmp(&self.paths[b.1 as usize]))
        });
        ranked.truncate(top);
        ranked
    }

    /// Up to three catalog rows: most query words in the title first, then BM25 over the summaries, then path.
    fn catalog(&self, found: &[Found<'_>]) -> Vec<usize> {
        let matches =
            |f: &Found<'_>, word: &str| word == f.word || f.stem.is_some_and(|s| word.starts_with(s));
        let counts: Vec<Vec<u64>> = found
            .iter()
            .map(|f| {
                self.summary_words
                    .iter()
                    .map(|counter| {
                        counter
                            .iter()
                            .filter(|(word, _)| matches(f, word))
                            .map(|(_, &c)| u64::from(c))
                            .sum()
                    })
                    .collect()
            })
            .collect();
        let df: Vec<f64> = counts
            .iter()
            .map(|column| column.iter().filter(|&&c| c > 0).count() as f64)
            .collect();
        let n = self.paths.len() as f64;
        let lengths: Vec<f64> = self
            .summary_words
            .iter()
            .map(|c| c.values().map(|&v| f64::from(v)).sum())
            .collect();
        let total: f64 = lengths.iter().sum();
        let average = if total == 0.0 { 1.0 } else { total / n };
        let mut rows = Vec::new();
        for doc in 0..self.paths.len() {
            let titled = found
                .iter()
                .filter(|f| self.title_words[doc].iter().any(|w| matches(f, w)))
                .count();
            let mut score = 0.0;
            for (column, &df) in counts.iter().zip(&df) {
                let freq = column[doc] as f64;
                if freq > 0.0 {
                    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                    score += idf * freq * 2.5 / (freq + 1.5 * (0.25 + 0.75 * lengths[doc] / average));
                }
            }
            if titled > 0 || score > 0.0 {
                rows.push((titled, score, doc));
            }
        }
        rows.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.total_cmp(&a.1))
                .then(self.paths[a.2].cmp(&self.paths[b.2]))
        });
        rows.into_iter().take(3).map(|r| r.2).collect()
    }

    /// A document's text now. A shelf original that changed since the index is an error, as before;
    /// a wiki page shows its current text.
    fn original(&self, reg: &Registry, key: &str, shelf: Option<&Path>, doc: usize) -> Result<String> {
        let path = &self.paths[doc];
        if let (Some(root), Some(name)) = (shelf, path.strip_prefix("shelf:")) {
            let changed = "shelf original changed or unavailable; rebuild index";
            let text = crate::shelf::read(root, name).map_err(|_| changed)?.0;
            if format!("{:x}", Sha256::digest(text.as_bytes())) != self.hashes[doc] {
                return Err(changed.into());
            }
            return Ok(text);
        }
        Ok(read(reg, key, path, None)
            .ok()
            .and_then(|v| v["text"].as_str().map(str::to_owned))
            .unwrap_or_default())
    }

    /// BM25 alone, or BM25 and the vector ranking fused with RRF when the library has vectors.
    /// No absolute similarity threshold: only the order of each ranking counts.
    fn ranked(&self, query: &str, found: &[Found<'_>], top: usize) -> (Vec<(f64, u32)>, &'static str, Value) {
        use crate::embed;
        let Some(url) = embed::url() else {
            return (self.rank(found, top), "lexical-bm25", json!({"status":"not_configured"}));
        };
        let covered: BTreeSet<u32> = self.vectors.iter().map(|(doc, _)| *doc).collect();
        if covered.is_empty() {
            return (self.rank(found, top), "lexical-bm25", json!({"status":"partial","documents":0}));
        }
        let status = if covered.len() == self.paths.len() { "ok" } else { "partial" };
        let failed = |e: String| json!({"status":"failed","error":e,"documents":covered.len()});
        let question = match embed::embed(&url, &[embed::query_text(query)], embed::QUERY_WAIT) {
            Ok(mut v) => v.remove(0),
            Err(e) => return (self.rank(found, top), "lexical-bm25", failed(e)),
        };
        if question.len() != self.vectors[0].1.len() {
            return (self.rank(found, top), "lexical-bm25", failed("query and document vectors differ in length".into()));
        }
        // SIMPLIFY: a linear scan; a vector index waits until a library is too large for it.
        let mut best: HashMap<u32, f32> = HashMap::new();
        for (doc, v) in &self.vectors {
            let s: f32 = v.iter().zip(&question).map(|(a, b)| a * b).sum();
            let e = best.entry(*doc).or_insert(f32::MIN);
            *e = e.max(s);
        }
        let mut by_vector: Vec<(f32, u32)> = best.into_iter().map(|(doc, s)| (s, doc)).collect();
        by_vector.sort_by(|a, b| b.0.total_cmp(&a.0).then(self.paths[a.1 as usize].cmp(&self.paths[b.1 as usize])));
        by_vector.truncate(CANDIDATES);
        let mut fused: HashMap<u32, f64> = HashMap::new();
        for (rank, (_, doc)) in self.rank(found, CANDIDATES.max(top)).iter().enumerate() {
            *fused.entry(*doc).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
        for (rank, (_, doc)) in by_vector.iter().enumerate() {
            *fused.entry(*doc).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
        let mut ranked: Vec<(f64, u32)> = fused.into_iter().map(|(doc, s)| (s, doc)).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(self.paths[a.1 as usize].cmp(&self.paths[b.1 as usize])));
        ranked.truncate(top);
        (ranked, "hybrid-rrf", json!({"status":status,"documents":covered.len()}))
    }

    fn search(&self, reg: &Registry, key: &str, query: &str, words: &BTreeSet<String>, top: usize) -> Result<Value> {
        let found = self.found(words);
        let shelf = reg.shelf_root(key)?;
        let (ranked, engine, semantic) = self.ranked(query, &found, top);
        let mut hits = Vec::new();
        for (score, doc) in ranked {
            let doc = doc as usize;
            let text = self.original(reg, key, shelf.as_deref(), doc)?;
            hits.push(json!({"path":self.paths[doc],"title":self.titles[doc],
                "score":(score * 1e6).round_ties_even() / 1e6,
                "excerpt":text.chars().take(500).collect::<String>()}));
        }
        let catalog: Vec<Value> = self
            .catalog(&found)
            .into_iter()
            .map(|doc| json!({"path":self.paths[doc],"title":self.titles[doc],"summary":self.summaries[doc]}))
            .collect();
        let empty = hits.is_empty();
        let mut result = json!({"library":key,"engine":engine,"semantic":semantic,
            "indexed_at":self.state.indexed_at,"catalog":catalog,"hits":hits});
        if empty {
            let korean_query = words.iter().any(|w| w.chars().any(hangul));
            result["hint"] = json!(if korean_query && !self.korean {
                "The library's documents contain no Korean words, and search matches words, not meanings. Search again with words in the documents' language."
            } else {
                "No document contains these words. Search matches words, not meanings, so try the words the documents use, or fewer words."
            });
        }
        Ok(result)
    }

    /// The paths and rounded scores of the first ten hits for a question.
    fn top_paths(&self, question: &str) -> Vec<(String, f64)> {
        let words: BTreeSet<String> = words(question).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let found = self.found(&words);
        self.rank(&found, 10)
            .into_iter()
            .map(|(score, doc)| {
                (self.paths[doc as usize].clone(), (score * 1e6).round_ties_even() / 1e6)
            })
            .collect()
    }

    /// Up to `n` questions: the titles of documents spread over the library.
    fn questions(&self, n: usize) -> Vec<String> {
        let mut questions: Vec<String> = Vec::new();
        for i in 0..n {
            let Some(title) = self.titles.get(i * self.titles.len() / n) else {
                continue;
            };
            if words(title).next().is_some() && !questions.contains(title) {
                questions.push(title.clone());
            }
        }
        questions
    }

    /// The same index without some documents, so two versions can be compared on the documents
    /// both hold the same: a page that changed in between shifts every score.
    fn without(self, drop: &HashSet<String>) -> Index {
        if drop.is_empty() {
            return self;
        }
        let mut ids = vec![None; self.paths.len()];
        let mut paths = Vec::new();
        let mut parts: [Vec<String>; 3] = Default::default();
        let mut lengths = Vec::new();
        for (doc, path) in self.paths.into_iter().enumerate() {
            if drop.contains(&path) {
                continue;
            }
            ids[doc] = Some(paths.len() as u32);
            paths.push(path);
            parts[0].push(self.titles[doc].clone());
            parts[1].push(self.summaries[doc].clone());
            parts[2].push(self.hashes[doc].clone());
            lengths.push(self.lengths[doc]);
        }
        let mut postings = BTreeMap::new();
        for (word, docs) in self.postings {
            let docs: Vec<(u32, u32)> =
                docs.into_iter().filter_map(|(doc, freq)| ids[doc as usize].map(|id| (id, freq))).collect();
            if !docs.is_empty() {
                postings.insert(word, docs);
            }
        }
        let state = IndexState { documents: paths.len() as u64, ..self.state };
        Index::from_parts(state, paths, parts, lengths, postings)
    }
}

#[derive(Default)]
struct Slot {
    current: Option<Arc<Index>>,
    loading: Option<(String, JoinHandle<Result<Option<Index>>>)>,
}

/// A loaded index is current while neither the index nor the library's vectors changed.
fn version(state: &IndexState) -> String {
    format!("{}|{}", state.indexed_at, state.vectors_at)
}

fn start_load(key: &str) -> JoinHandle<Result<Option<Index>>> {
    let key = key.to_owned();
    std::thread::spawn(move || Index::load(&key))
}

/// The room's indexes, by library.
#[derive(Default)]
pub(crate) struct SearchRoom {
    slots: HashMap<String, Slot>,
}

impl SearchRoom {
    /// Start loading every library that has an index, so the first search need not wait.
    pub(crate) fn preload(&mut self, config: &Path) {
        let Ok(reg) = Registry::open(config) else {
            return;
        };
        if reg.simple() != Ok(true) {
            return;
        }
        for key in reg.rows.keys() {
            if let Ok(Some(state)) = index_store::state(key) {
                self.slots.entry(key.clone()).or_default().loading = Some((version(&state), start_load(key)));
            }
        }
    }

    pub(crate) fn search(&mut self, config: &Path, key: &str, query: &str, top: usize) -> Result<Value> {
        let reg = Registry::open(config)?;
        if !reg.simple()? {
            return Err("registry requires the policy-aware search".into());
        }
        let words: BTreeSet<String> = words(query).collect();
        if words.is_empty() {
            return Err("query must contain a word".into());
        }
        let roots = reg.roots(key)?;
        let matching = |state: &IndexState| state.root == roots.0 && state.shelf_root == roots.1;
        let state = index_store::state(key)?.ok_or(MISSING)?;
        if !matching(&state) {
            return Err(MISMATCH.into());
        }
        let slot = self.slots.entry(key.to_owned()).or_default();
        let wanted = version(&state);
        let fresh = |slot: &Slot| slot.current.as_ref().is_some_and(|index| version(&index.state) == wanted);
        if !fresh(slot) && slot.loading.as_ref().is_none_or(|(at, _)| *at != wanted) {
            slot.loading = Some((wanted.clone(), start_load(key)));
        }
        // Take a finished load; wait a little for one in progress, then answer from what is loaded.
        let deadline = Instant::now() + LOAD_WAIT;
        let mut failure = None;
        loop {
            if slot.loading.as_ref().is_some_and(|(_, handle)| handle.is_finished()) {
                let (_, handle) = slot.loading.take().unwrap();
                match handle.join() {
                    Ok(Ok(Some(index))) => slot.current = Some(Arc::new(index)),
                    Ok(Ok(None)) => failure = Some(MISSING.to_string()),
                    Ok(Err(e)) => failure = Some(e),
                    Err(_) => failure = Some("index load failed".to_string()),
                }
            }
            if fresh(slot) || slot.loading.is_none() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let index = match (&slot.current, failure) {
            (Some(index), _) => Arc::clone(index),
            (None, Some(e)) => return Err(e),
            (None, None) => {
                return Err("the search room is still loading the index; search again in a few seconds".into());
            }
        };
        if !matching(&index.state) {
            return Err(MISMATCH.into());
        }
        index.search(&reg, key, query, &words, top)
    }
}

/// Earlier releases kept the index in a JSON file beside the registry. Once the database holds the
/// index, the librarian compares five questions on both, over the pages both indexes hold the same
/// (pages added, removed or changed since the file was written are left out of both). When all
/// agree the file is removed; otherwise it stays and the journal lists the questions that differ.
pub fn retire_json(config: &Path, key: &str) -> Result<Option<Value>> {
    let reg = Registry::open(config)?;
    if !reg.simple()? {
        return Ok(None);
    }
    let file = reg.index_path(key);
    if !file.is_file() || index_store::state(key)?.is_none() {
        return Ok(None);
    }
    let unreadable = |e: String| {
        Ok(Some(json!({"compared":0,"removed":false,
            "index_mismatch":[format!("old index unreadable: {e}")]})))
    };
    let earlier = match old_hashes(&file) {
        Ok(hashes) => hashes,
        Err(e) => return unreadable(e),
    };
    let current = index_store::hashes(key)?;
    let changed: HashSet<String> = earlier
        .iter()
        .filter(|(path, hash)| current.get(*path) != Some(*hash))
        .map(|(path, _)| path.clone())
        .chain(current.keys().filter(|path| !earlier.contains_key(*path)).cloned())
        .collect();
    drop((earlier, current));
    let Some(new) = Index::load(key)? else {
        return Ok(None);
    };
    let new = new.without(&changed);
    let questions = new.questions(5);
    let fresh: Vec<_> = questions.iter().map(|q| new.top_paths(q)).collect();
    drop(new);
    if questions.is_empty() {
        return Ok(Some(json!({"compared":0,"removed":false,"changed_since_json":changed.len(),
            "index_mismatch":["no page is the same in both indexes"]})));
    }
    let old = match old_index(&file) {
        Ok(index) => index.without(&changed),
        Err(e) => return unreadable(e),
    };
    let differ: Vec<&String> = questions
        .iter()
        .zip(&fresh)
        .filter(|(q, f)| old.top_paths(q) != **f)
        .map(|(q, _)| q)
        .collect();
    if !differ.is_empty() {
        return Ok(Some(json!({"compared":questions.len(),"removed":false,
            "changed_since_json":changed.len(),"index_mismatch":differ})));
    }
    fs::remove_file(&file).map_err(|e| e.to_string())?;
    Ok(Some(json!({"compared":questions.len(),"removed":true,"changed_since_json":changed.len()})))
}

/// The hash of each document's text in the JSON file of earlier releases, as the database keeps it.
fn old_hashes(file: &Path) -> Result<HashMap<String, String>> {
    let input = fs::File::open(file).map_err(|e| e.to_string())?;
    let mut json = Json {
        input: std::io::BufReader::with_capacity(1 << 16, input),
    };
    let mut hashes = HashMap::new();
    json.object(&mut |json, key| match key.as_str() {
        "documents" => json.object(&mut |json, path| {
            let text = json.text_or_null()?.unwrap_or_default();
            hashes.insert(path, format!("{:x}", Sha256::digest(text.as_bytes())));
            Ok(())
        }),
        _ => json.skip(),
    })?;
    Ok(hashes)
}

/// Reads the JSON index file one value at a time, so its document text never sits in memory.
struct Json<R: BufRead> {
    input: R,
}

const BAD_JSON: &str = "invalid JSON";

impl<R: BufRead> Json<R> {
    fn peek(&mut self) -> Result<Option<u8>> {
        Ok(self.input.fill_buf().map_err(|e| e.to_string())?.first().copied())
    }
    fn bump(&mut self) -> Result<u8> {
        let byte = self.peek()?.ok_or(BAD_JSON)?;
        self.input.consume(1);
        Ok(byte)
    }
    fn space(&mut self) -> Result<()> {
        while self.peek()?.is_some_and(|b| b.is_ascii_whitespace()) {
            self.input.consume(1);
        }
        Ok(())
    }
    fn expect(&mut self, want: u8) -> Result<()> {
        self.space()?;
        if self.bump()? == want {
            Ok(())
        } else {
            Err(BAD_JSON.into())
        }
    }
    fn hex4(&mut self) -> Result<u32> {
        let mut value = 0;
        for _ in 0..4 {
            value = value * 16 + char::from(self.bump()?).to_digit(16).ok_or(BAD_JSON)?;
        }
        Ok(value)
    }
    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.bump()? {
                b'"' => break,
                b'\\' => match self.bump()? {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let mut code = self.hex4()?;
                        if (0xD800..0xDC00).contains(&code) {
                            if self.bump()? != b'\\' || self.bump()? != b'u' {
                                return Err(BAD_JSON.into());
                            }
                            let low = self.hex4()?;
                            if !(0xDC00..0xE000).contains(&low) {
                                return Err(BAD_JSON.into());
                            }
                            code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        }
                        let c = char::from_u32(code).ok_or(BAD_JSON)?;
                        out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                    }
                    _ => return Err(BAD_JSON.into()),
                },
                byte => out.push(byte),
            }
        }
        String::from_utf8(out).map_err(|_| BAD_JSON.into())
    }
    fn number(&mut self) -> Result<u64> {
        self.space()?;
        let mut value: u64 = 0;
        let mut digits = 0;
        while let Some(b) = self.peek()?.filter(u8::is_ascii_digit) {
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(b - b'0')))
                .ok_or(BAD_JSON)?;
            digits += 1;
            self.input.consume(1);
        }
        if digits == 0 {
            Err(BAD_JSON.into())
        } else {
            Ok(value)
        }
    }
    fn text_or_null(&mut self) -> Result<Option<String>> {
        self.space()?;
        if self.peek()? == Some(b'"') {
            return self.string().map(Some);
        }
        self.skip()?;
        Ok(None)
    }
    /// Skip one value of any kind.
    fn skip(&mut self) -> Result<()> {
        self.space()?;
        match self.peek()?.ok_or(BAD_JSON)? {
            b'"' => {
                self.string()?;
            }
            b'{' => self.object(&mut |json, _| json.skip())?,
            b'[' => {
                self.bump()?;
                self.space()?;
                if self.peek()? == Some(b']') {
                    self.bump()?;
                    return Ok(());
                }
                loop {
                    self.skip()?;
                    self.space()?;
                    match self.bump()? {
                        b',' => continue,
                        b']' => break,
                        _ => return Err(BAD_JSON.into()),
                    }
                }
            }
            _ => {
                while self
                    .peek()?
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    self.input.consume(1);
                }
            }
        }
        Ok(())
    }
    fn object(&mut self, each: &mut dyn FnMut(&mut Self, String) -> Result<()>) -> Result<()> {
        self.expect(b'{')?;
        self.space()?;
        if self.peek()? == Some(b'}') {
            self.bump()?;
            return Ok(());
        }
        loop {
            let key = self.string()?;
            self.expect(b':')?;
            each(self, key)?;
            self.space()?;
            match self.bump()? {
                b',' => continue,
                b'}' => return Ok(()),
                _ => return Err(BAD_JSON.into()),
            }
        }
    }
}

/// An index from the JSON file of earlier releases, without document text.
fn old_index(file: &Path) -> Result<Index> {
    fn id(ids: &mut HashMap<String, u32>, paths: &mut Vec<String>, path: String) -> u32 {
        if let Some(&id) = ids.get(&path) {
            return id;
        }
        let id = paths.len() as u32;
        ids.insert(path.clone(), id);
        paths.push(path);
        id
    }
    let input = fs::File::open(file).map_err(|e| e.to_string())?;
    let mut json = Json {
        input: std::io::BufReader::with_capacity(1 << 16, input),
    };
    let mut ids = HashMap::new();
    let mut paths = Vec::new();
    let mut lengths = HashMap::new();
    let mut postings: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let (mut root, mut shelf, mut indexed_at, mut version) = (None, None, None, 0);
    json.object(&mut |json, key| match key.as_str() {
        "documents" => json.object(&mut |json, path| {
            id(&mut ids, &mut paths, path);
            json.skip()
        }),
        "counts" => json.object(&mut |json, path| {
            let doc = id(&mut ids, &mut paths, path);
            json.object(&mut |json, word| {
                let freq = u32::try_from(json.number()?).map_err(|_| BAD_JSON)?;
                postings.entry(word).or_default().push((doc, freq));
                Ok(())
            })
        }),
        "lengths" => json.object(&mut |json, path| {
            let doc = id(&mut ids, &mut paths, path);
            lengths.insert(doc, json.number()?);
            Ok(())
        }),
        "root" => json.text_or_null().map(|v| root = v),
        "shelf_root" => json.text_or_null().map(|v| shelf = v),
        "indexed_at" => json.text_or_null().map(|v| indexed_at = v),
        "version" => json.number().map(|v| version = v),
        _ => json.skip(),
    })?;
    if version != 1 || lengths.len() != paths.len() {
        return Err(INVALID.into());
    }
    let lengths = (0..paths.len() as u32).map(|doc| lengths[&doc]).collect();
    let empty = vec![String::new(); paths.len()];
    let state = IndexState {
        root: root.unwrap_or_default(),
        shelf_root: shelf.unwrap_or_default(),
        indexed_at: indexed_at.unwrap_or_default(),
        documents: paths.len() as u64,
        vectors_at: String::new(),
    };
    Ok(Index::from_parts(
        state,
        paths,
        [empty.clone(), empty.clone(), empty],
        lengths,
        postings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_reader_handles_escapes_nesting_and_null() {
        // The escapes are joined from pieces: a backslash, then u and four hex digits.
        let u = |hex: &str| format!("\\u{hex}");
        let text = format!(r#"{{"a":"x\"{}{}{}","b":[1,{{"c":null}}],"d":null,"e":12}}"#, u("00e9"), u("d83d"), u("de00"));
        let mut json = Json { input: text.as_bytes() };
        let mut seen = Vec::new();
        json.object(&mut |json, key| {
            match key.as_str() {
                "a" => seen.push(json.string()?),
                "d" => seen.push(format!("{:?}", json.text_or_null()?)),
                "e" => seen.push(json.number()?.to_string()),
                _ => json.skip()?,
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, ["x\"é😀", "None", "12"]);
    }
}
