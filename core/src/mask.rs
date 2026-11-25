//! `benedict mask`: mask personal data in a text or JSON Lines file (or standard
//! input) with the same rules as search and read results, so transcripts and logs
//! can be kept or analysed without it. JSON Lines are masked string by string, so
//! keys, their order and every other value stay byte for byte as written.

use crate::redact;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, BufWriter, Write};

type Counts = BTreeMap<&'static str, usize>;

fn add(counts: &mut Counts, more: Counts) {
    for (k, v) in more {
        *counts.entry(k).or_insert(0) += v;
    }
}

/// Mask the content of every string literal in one JSON line.
fn json_line(line: &str, counts: &mut Counts) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut pos = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i] != b'"' {
            i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        let end = (i + 1).min(bytes.len());
        let literal = &line[start..end];
        if let Ok(text) = serde_json::from_str::<String>(literal) {
            let (masked, found) = redact::mask_counted(&text);
            if masked != text {
                add(counts, found);
                out.push_str(&line[pos..start]);
                out.push_str(&serde_json::to_string(&masked).unwrap_or_else(|_| literal.into()));
                pos = end;
            }
        }
        i = end;
    }
    out.push_str(&line[pos..]);
    out
}

/// Mask `input` into `out` line by line and return the line count and what was masked.
pub fn stream(
    input: impl BufRead,
    out: &mut impl Write,
    jsonl: bool,
) -> io::Result<(usize, Counts)> {
    let mut counts = Counts::new();
    let mut lines = 0;
    for raw in input.split(b'\n') {
        let raw = raw?;
        let line = String::from_utf8_lossy(&raw);
        let masked = if jsonl && serde_json::from_str::<serde_json::Value>(&line).is_ok() {
            json_line(&line, &mut counts)
        } else {
            let (masked, found) = redact::mask_counted(&line);
            add(&mut counts, found);
            masked
        };
        out.write_all(masked.as_bytes())?;
        out.write_all(b"\n")?;
        lines += 1;
    }
    Ok((lines, counts))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut jsonl = false;
    let mut files = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--jsonl" => jsonl = true,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option for mask: {other}"));
            }
            other => files.push(other),
        }
    }
    if files.len() > 1 {
        return Err("mask reads one FILE or standard input".into());
    }
    let input: Box<dyn BufRead> = match files.first() {
        Some(&path) if path != "-" => Box::new(BufReader::new(
            std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?,
        )),
        _ => Box::new(BufReader::new(io::stdin().lock())),
    };
    let mut out = BufWriter::new(io::stdout().lock());
    let (lines, counts) = stream(input, &mut out, jsonl).map_err(|e| format!("mask: {e}"))?;
    out.flush().map_err(|e| format!("mask: {e}"))?;
    eprintln!("{}", json!({"lines": lines, "masked": counts}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(input: &str, jsonl: bool) -> (String, Counts) {
        let mut out = Vec::new();
        let (_, counts) = stream(input.as_bytes(), &mut out, jsonl).unwrap();
        (String::from_utf8(out).unwrap(), counts)
    }

    #[test]
    fn text_lines_are_masked_and_kept_apart() {
        let (out, counts) = masked("휴대폰 010-1234-5678\n메일 hong@example.com\n", false);
        assert_eq!(out, "휴대폰 010-****-5678\n메일 [이메일]\n");
        assert_eq!(counts.get("phone"), Some(&1));
        assert_eq!(counts.get("email"), Some(&1));
    }

    #[test]
    fn json_lines_keep_keys_order_and_other_values() {
        let line = r#"{"z":1,"type":"user","message":{"text":"연락처\n010-1234-5678 홍길동 대표","n":2.50},"a":"ok"}"#;
        let (out, counts) = masked(&format!("{line}\n"), true);
        assert_eq!(
            out,
            "{\"z\":1,\"type\":\"user\",\"message\":{\"text\":\"연락처\\n010-****-5678 홍*동 대표\",\"n\":2.50},\"a\":\"ok\"}\n"
        );
        assert_eq!(counts.get("phone"), Some(&1));
        assert_eq!(counts.get("name"), Some(&1));
        let value: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(value["message"]["n"], json!(2.5));
    }

    #[test]
    fn escaped_quotes_and_unchanged_lines_pass_through() {
        let line = r#"{"q":"he said \"010-1234-5678\"","id":"900101"}"#;
        let (out, _) = masked(&format!("{line}\n"), true);
        assert_eq!(
            out,
            "{\"q\":\"he said \\\"010-****-5678\\\"\",\"id\":\"900101\"}\n"
        );
        let plain = "{\"ok\":true}\nnot json 010-1234-5678\n";
        let (out, _) = masked(plain, true);
        assert_eq!(out, "{\"ok\":true}\nnot json 010-****-5678\n");
    }
}
