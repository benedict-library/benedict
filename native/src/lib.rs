//! Native speed-ups for library rooms, called from Python through ctypes (C ABI, no crates).
//!
//! `library_tok` matches `wiki_room_fuse.tok` exactly: lowercase, take runs of `[0-9A-Za-z]` (with
//! `underscore`, `[0-9A-Za-z_]`) or of Hangul syllables (U+AC00..=U+D7A3), and after each Hangul run of two
//! or more syllables add its two-syllable pieces. Tokens come back joined by '\n'.

mod index;
pub use index::Index;

use std::slice;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Other,
    Alnum,
    Hangul,
}

fn kind(c: char, underscore: bool) -> Kind {
    if c.is_ascii_alphanumeric() || (underscore && c == '_') {
        Kind::Alnum
    } else if ('\u{AC00}'..='\u{D7A3}').contains(&c) {
        Kind::Hangul
    } else {
        Kind::Other
    }
}

fn emit(run: &[char], k: Kind, out: &mut String) {
    if run.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.extend(run.iter());
    if k == Kind::Hangul && run.len() >= 2 {
        for pair in run.windows(2) {
            out.push('\n');
            out.push(pair[0]);
            out.push(pair[1]);
        }
    }
}

/// Tokens of `s`, joined by '\n' (empty string when there are none).
pub fn tokens(s: &str, underscore: bool) -> String {
    let low = s.to_lowercase();
    let mut out = String::with_capacity(low.len() + low.len() / 2);
    let mut run: Vec<char> = Vec::new();
    let mut cur = Kind::Other;
    for c in low.chars() {
        let k = kind(c, underscore);
        if k != cur {
            emit(&run, cur, &mut out);
            run.clear();
            cur = k;
        }
        if k != Kind::Other {
            run.push(c);
        }
    }
    emit(&run, cur, &mut out);
    out
}

/// Tokenize `len` bytes of UTF-8 at `ptr` (`underscore` nonzero: '_' joins letters and digits). Returns a buffer the caller frees with `library_free`, its length in
/// `out_len`; on invalid UTF-8 returns null and sets `out_len` to usize::MAX (the caller tokenizes in Python).
///
/// # Safety
/// `ptr` must point to `len` readable bytes and `out_len` to a writable usize.
#[no_mangle]
pub unsafe extern "C" fn library_tok(ptr: *const u8, len: usize, underscore: u8, out_len: *mut usize) -> *mut u8 {
    let bytes = if len == 0 { &[][..] } else { slice::from_raw_parts(ptr, len) };
    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            *out_len = usize::MAX;
            return std::ptr::null_mut();
        }
    };
    let mut buf = tokens(s, underscore != 0).into_bytes().into_boxed_slice();
    *out_len = buf.len();
    let p = buf.as_mut_ptr();
    std::mem::forget(buf);
    p
}

/// Free a buffer returned by `library_tok`.
///
/// # Safety
/// `ptr`/`len` must be exactly a pointer and length returned by `library_tok`.
#[no_mangle]
pub unsafe extern "C" fn library_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

#[cfg(test)]
mod tests {
    use super::tokens;

    #[test]
    fn runs_and_pairs() {
        assert_eq!(tokens("도서관 ABC12_x", false), "도서관\n도서\n서관\nabc12\nx");
        assert_eq!(tokens("도서관 ABC12_x", true), "도서관\n도서\n서관\nabc12_x");
        assert_eq!(tokens("가", false), "가");
        assert_eq!(tokens("k8s한글", false), "k8s\n한글\n한글");
        assert_eq!(tokens("…", false), "");
    }
}
