//! A room's lexical index held in Rust: for each word its postings (document, count), for each document its
//! length and words. BM25 scores are the same sums `rooms/room_query.py` makes over its Python dictionaries, in the
//! same word order, so the ranking does not change; the memory is a few bytes a pair instead of a dictionary entry.

use std::collections::HashMap;
use std::slice;

#[derive(Default)]
pub struct Index {
    terms: HashMap<Box<str>, u32>,
    df: Vec<u32>,
    postings: Vec<Vec<(u32, u32)>>,
    doc_id: HashMap<Box<str>, u32>,
    paths: Vec<Box<str>>,
    lens: Vec<u32>,
    doc_terms: Vec<Box<[u32]>>,
    free: Vec<u32>,
}

impl Index {
    fn term(&mut self, w: &str) -> u32 {
        if let Some(&t) = self.terms.get(w) {
            return t;
        }
        let t = self.df.len() as u32;
        self.terms.insert(w.into(), t);
        self.df.push(0);
        self.postings.push(Vec::new());
        t
    }

    /// Take a document out; false when it was not there.
    pub fn remove(&mut self, path: &str) -> bool {
        let Some(d) = self.doc_id.remove(path) else { return false };
        let words = std::mem::take(&mut self.doc_terms[d as usize]);
        for &t in words.iter() {
            let p = &mut self.postings[t as usize];
            if let Some(i) = p.iter().position(|x| x.0 == d) {
                p.swap_remove(i);
            }
            self.df[t as usize] -= 1;
        }
        self.paths[d as usize] = "".into();
        self.lens[d as usize] = 0;
        self.free.push(d);
        true
    }

    /// Put a document (replacing any earlier one under the same path): its word counts and its length.
    pub fn put(&mut self, path: &str, counts: &[(&str, u32)], len: u32) {
        self.remove(path);
        let d = match self.free.pop() {
            Some(d) => d,
            None => {
                self.paths.push("".into());
                self.lens.push(0);
                self.doc_terms.push(Box::new([]));
                (self.paths.len() - 1) as u32
            }
        };
        let mut words = Vec::with_capacity(counts.len());
        for &(w, c) in counts {
            let t = self.term(w);
            self.postings[t as usize].push((d, c));
            self.df[t as usize] += 1;
            words.push(t);
        }
        self.paths[d as usize] = path.into();
        self.lens[d as usize] = len;
        self.doc_terms[d as usize] = words.into_boxed_slice();
        self.doc_id.insert(path.into(), d);
    }

    pub fn df(&self, w: &str) -> u32 {
        self.terms.get(w).map_or(0, |&t| self.df[t as usize])
    }

    pub fn count(&self) -> usize {
        self.doc_id.len()
    }

    pub fn has(&self, path: &str) -> bool {
        self.doc_id.contains_key(path)
    }

    /// BM25 over `words` in the given order (room_query.py: idf = ln(1 + (N - df + .5)/(df + .5)), length capped at
    /// three times the average). Documents with a positive score, in no particular order.
    pub fn bm25(&self, words: &[&str], nd: f64, k1: f64, b: f64, avg: f64) -> Vec<(u32, f64)> {
        let mut score: HashMap<u32, f64> = HashMap::new();
        let mut order: Vec<u32> = Vec::new();
        for w in words {
            let Some(&t) = self.terms.get(*w) else { continue };
            let df = self.df[t as usize] as f64;
            if df == 0.0 {
                continue;
            }
            let idf = (1.0 + (nd - df + 0.5) / (df + 0.5)).ln();
            for &(d, f) in &self.postings[t as usize] {
                let f = f as f64;
                let l = (self.lens[d as usize] as f64).min(3.0 * avg);
                let s = idf * f * (k1 + 1.0) / (f + k1 * (1.0 - b + b * l / avg));
                match score.get_mut(&d) {
                    Some(v) => *v += s,
                    None => {
                        score.insert(d, s);
                        order.push(d);
                    }
                }
            }
        }
        order.into_iter().map(|d| (d, score[&d])).collect()
    }

    fn path(&self, d: u32) -> &str {
        &self.paths[d as usize]
    }

    fn alive_paths(&self) -> impl Iterator<Item = &str> {
        self.doc_id.keys().map(|k| &**k)
    }
}

fn text<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    let bytes = if len == 0 { &[][..] } else { unsafe { slice::from_raw_parts(ptr, len) } };
    std::str::from_utf8(bytes).ok()
}

fn give(s: String, out_len: *mut usize) -> *mut u8 {
    let mut buf = s.into_bytes().into_boxed_slice();
    unsafe { *out_len = buf.len() };
    let p = buf.as_mut_ptr();
    std::mem::forget(buf);
    p
}

#[no_mangle]
pub extern "C" fn library_idx_new() -> *mut Index {
    Box::into_raw(Box::default())
}

/// # Safety
/// `ix` must come from `library_idx_new` and not be used again.
#[no_mangle]
pub unsafe extern "C" fn library_idx_drop(ix: *mut Index) {
    if !ix.is_null() {
        drop(Box::from_raw(ix));
    }
}

/// Put one document: `data` is "word\tcount\n…" UTF-8. Returns 0, or -1 for text that is not UTF-8 or a bad line.
///
/// # Safety
/// `ix` from `library_idx_new`; the pointers name readable bytes of the given lengths.
#[no_mangle]
pub unsafe extern "C" fn library_idx_put(ix: *mut Index, path: *const u8, path_len: usize, data: *const u8, data_len: usize, len: u32) -> i32 {
    let (Some(p), Some(d)) = (text(path, path_len), text(data, data_len)) else { return -1 };
    let mut counts = Vec::new();
    for line in d.split('\n').filter(|l| !l.is_empty()) {
        let Some((w, c)) = line.rsplit_once('\t') else { return -1 };
        let Ok(c) = c.parse::<u32>() else { return -1 };
        counts.push((w, c));
    }
    (*ix).put(p, &counts, len);
    0
}

/// # Safety
/// As `library_idx_put`.
#[no_mangle]
pub unsafe extern "C" fn library_idx_remove(ix: *mut Index, path: *const u8, path_len: usize) -> i32 {
    match text(path, path_len) {
        Some(p) => (*ix).remove(p) as i32,
        None => -1,
    }
}

/// # Safety
/// As `library_idx_put`.
#[no_mangle]
pub unsafe extern "C" fn library_idx_df(ix: *const Index, w: *const u8, w_len: usize) -> u32 {
    text(w, w_len).map_or(0, |w| (*ix).df(w))
}

/// # Safety
/// As `library_idx_put`.
#[no_mangle]
pub unsafe extern "C" fn library_idx_has(ix: *const Index, path: *const u8, path_len: usize) -> u8 {
    text(path, path_len).map_or(0, |p| (*ix).has(p) as u8)
}

/// # Safety
/// `ix` from `library_idx_new`.
#[no_mangle]
pub unsafe extern "C" fn library_idx_count(ix: *const Index) -> usize {
    (*ix).count()
}

/// Paths of the documents in the index, joined by '\n' (free with `library_free`).
///
/// # Safety
/// `ix` from `library_idx_new`; `out_len` writable.
#[no_mangle]
pub unsafe extern "C" fn library_idx_paths(ix: *const Index, out_len: *mut usize) -> *mut u8 {
    give((*ix).alive_paths().collect::<Vec<_>>().join("\n"), out_len)
}

/// BM25 of the '\n'-joined `words` (in that order): "path\tscore\n…" (free with `library_free`).
///
/// # Safety
/// As `library_idx_put`; `out_len` writable.
#[no_mangle]
pub unsafe extern "C" fn library_idx_bm25(ix: *const Index, words: *const u8, words_len: usize, nd: f64, k1: f64, b: f64, avg: f64,
                                          out_len: *mut usize) -> *mut u8 {
    let ws: Vec<&str> = text(words, words_len).map_or(Vec::new(), |s| s.split('\n').filter(|w| !w.is_empty()).collect());
    let ix = &*ix;
    let mut out = String::new();
    for (d, s) in ix.bm25(&ws, nd, k1, b, avg) {
        out.push_str(ix.path(d));
        out.push('\t');
        out.push_str(&format!("{s:?}"));
        out.push('\n');
    }
    give(out, out_len)
}

#[cfg(test)]
mod tests {
    use super::Index;

    #[test]
    fn put_remove_df_and_reuse() {
        let mut ix = Index::default();
        ix.put("a", &[("도서", 2), ("x", 1)], 3);
        ix.put("b", &[("도서", 1)], 1);
        assert_eq!((ix.df("도서"), ix.df("x"), ix.count()), (2, 1, 2));
        ix.put("a", &[("y", 1)], 1); // replace
        assert_eq!((ix.df("도서"), ix.df("x"), ix.df("y")), (1, 0, 1));
        assert!(ix.remove("b") && !ix.remove("b"));
        assert_eq!((ix.df("도서"), ix.count()), (0, 1));
        ix.put("c", &[("도서", 1)], 1); // takes the freed slot
        assert_eq!(ix.count(), 2);
    }

    #[test]
    fn bm25_matches_the_python_formula() {
        let mut ix = Index::default();
        ix.put("a", &[("w", 2)], 10);
        ix.put("b", &[("w", 1), ("v", 1)], 40);
        let (nd, k1, b, avg) = (2.0f64, 1.5, 0.75, 10.0);
        let idf = (1.0 + (nd - 2.0 + 0.5) / (2.0 + 0.5)).ln();
        let want_a = idf * 2.0 * (k1 + 1.0) / (2.0 + k1 * (1.0 - b + b * 10.0 / avg));
        let want_b = idf * 1.0 * (k1 + 1.0) / (1.0 + k1 * (1.0 - b + b * 30.0 / avg)); // length capped at 3×avg
        let got: std::collections::HashMap<u32, f64> = ix.bm25(&["w", "none"], nd, k1, b, avg).into_iter().collect();
        assert_eq!(got[&0], want_a);
        assert_eq!(got[&1], want_b);
    }
}
