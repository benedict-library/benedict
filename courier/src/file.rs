//! File work through the courier: the reason travels with the change, and the agent sees one short line.
//!
//!   courier --why "<reason>" --write <path> [--replace] <<'EOF'   (new content on stdin)
//!   courier --why "<reason>" --edit <path> <<'EOF'                (one or more blocks on stdin)
//!   <<<<<<< old
//!   exact text that appears once
//!   =======
//!   its replacement
//!   >>>>>>> new
//!
//! `--write` refuses to overwrite an existing file unless `--replace` is given. `--edit` changes nothing unless every
//! old text appears exactly once. The file keeps its permissions.

pub const OLD: &str = "<<<<<<< old";
pub const MID: &str = "=======";
pub const NEW: &str = ">>>>>>> new";

/// The edit blocks in stdin, as (old, new) pairs.
pub fn blocks(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut lines = text.split('\n').peekable();
    while let Some(l) = lines.next() {
        if l.trim_end() != OLD {
            if l.trim().is_empty() {
                continue;
            }
            return Err(format!("expected '{OLD}', got '{l}'"));
        }
        let (mut old, mut new) = (Vec::new(), Vec::new());
        let mut in_new = false;
        let mut closed = false;
        for l in lines.by_ref() {
            match l.trim_end() {
                MID if !in_new => in_new = true,
                NEW if in_new => {
                    closed = true;
                    break;
                }
                _ if in_new => new.push(l),
                _ => old.push(l),
            }
        }
        if !closed {
            return Err(format!("a block is not closed with '{NEW}'"));
        }
        if old.is_empty() {
            return Err("an old text is empty".into());
        }
        out.push((old.join("\n"), new.join("\n")));
    }
    if out.is_empty() {
        return Err(format!("no blocks — each block is '{OLD}' / old / '{MID}' / new / '{NEW}'"));
    }
    Ok(out)
}

/// Apply every block, or none: each old text must appear exactly once in the original.
pub fn apply(original: &str, edits: &[(String, String)]) -> Result<String, String> {
    let mut spans = Vec::with_capacity(edits.len());
    for (i, (old, new)) in edits.iter().enumerate() {
        if old.is_empty() { return Err(format!("block {}: old text is empty", i + 1)); }
        let position = original.find(old).ok_or_else(|| format!("block {}: old text not found", i + 1))?;
        // Search from the next character so overlapping matches also count.
        let next = position + old.chars().next().unwrap().len_utf8();
        if original[next..].contains(old) {
            let count = original.char_indices().filter(|(i, _)| original[*i..].starts_with(old)).count();
            return Err(format!("block {}: old text appears {count} times — add surrounding lines", i + 1));
        }
        spans.push((position, position + old.len(), new));
    }
    spans.sort_by_key(|span| span.0);
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("edit blocks overlap in the original".into());
    }
    let mut out = original.to_string();
    for (start, end, new) in spans.into_iter().rev() {
        out.replace_range(start..end, new);
    }
    Ok(out)
}

pub fn line_count(s: &str) -> usize {
    s.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_blocks() {
        let b = blocks("<<<<<<< old\na\n=======\nb\n>>>>>>> new\n\n<<<<<<< old\nc\nd\n=======\n>>>>>>> new\n").unwrap();
        assert_eq!(b, vec![("a".into(), "b".into()), ("c\nd".into(), "".into())]);
    }

    #[test]
    fn parse_errors() {
        assert!(blocks("").is_err());
        assert!(blocks("hello").is_err());
        assert!(blocks("<<<<<<< old\na\n=======\nb\n").is_err());
        assert!(blocks("<<<<<<< old\n=======\nb\n>>>>>>> new").is_err());
    }

    #[test]
    fn edits_always_refer_to_original_positions() {
        let edits = vec![("left".into(),"right".into()),("right".into(),"third".into())];
        assert_eq!(apply("left right", &edits).unwrap(), "right third");
        assert!(apply("abcd", &[("abc".into(),"x".into()),("bcd".into(),"y".into())]).is_err());
    }

    #[test]
    fn apply_all_or_nothing() {
        let e = vec![("one".to_string(), "1".to_string()), ("three".to_string(), "3".to_string())];
        assert_eq!(apply("one two three", &e).unwrap(), "1 two 3");
        assert!(apply("one two", &e).unwrap_err().contains("block 2: old text not found"));
        assert!(apply("one one three", &e).unwrap_err().contains("appears 2 times"));
    }
}
