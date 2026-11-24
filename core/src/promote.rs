//! A librarian can file a completed room observation without retyping its
//! question, answer or source. The durable room ID remains in the wiki receipt.
use crate::{catalog, files, local, records};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn run(config: &Path, key: Option<&str>, args: &[String]) -> Result<Value, String> {
    let fields = records::options(args, &["id", "as"])?;
    let id = fields.get("id").ok_or("file requires --id")?;
    let (library, root) = local::selection(config, key)?;
    let entry = records::recall(&library, "id", id)?;
    let field = |name: &str| {
        entry[name]
            .as_str()
            .ok_or_else(|| format!("invalid room evidence field {name}"))
    };
    let (question, answer) = (field("question")?, field("answer")?);
    if question.trim().is_empty() || answer.trim().is_empty() {
        return Err("filing requires a recorded user question and answer".into());
    }
    let source = field("source")?;
    let link = format!(
        "Room evidence: benedict recall --library {library} --id {}",
        entry["id"]
    );
    let refs = if source.is_empty() {
        link
    } else {
        format!("{source}\n\n{link}")
    };
    match fields.get("as").copied().unwrap_or("filing") {
        "filing" => (),
        "concept" => {
            if entry["room"] != "librarian" || source.trim().is_empty() {
                return Err("a concept needs a librarian's synthesis and its sources".into());
            }
            if !local::simple(config)?
                || root.join(".wiki-pending").is_dir()
                || std::env::var_os("LIBRARY_EMBED_URL").is_some()
            {
                return Err("concept publication requires a native registry".into());
            }
            local::verify_references(config, &library, source)?;
            let folder = root.join("concepts");
            files::no_link(&folder)?;
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            let scope = json!([entry["project"], library]).to_string();
            let scope_hash = Sha256::digest(scope.as_bytes());
            let relative = format!("concepts/room-{scope_hash:x}-{}.md", entry["id"]);
            let path = root.join(&relative);
            files::no_link(&path)?;
            let page = format!("# {question}\n\n{answer}\n\n## Sources\n\n{refs}\n");
            if let Err(err) = files::create_wiki_page(&path, page.as_bytes(), &root) {
                if std::fs::read(&path).ok().as_deref() != Some(page.as_bytes()) {
                    return Err(err);
                }
            }
            local::refresh(config, &library, Some(&path))?;
            return Ok(
                json!({"saved":true,"path":relative,"library":library,"source_record_id":entry["id"]}),
            );
        }
        _ => return Err("expected --as filing or concept".into()),
    }
    if !local::simple(config)?
        || root.join(".wiki-pending").is_dir()
        || std::env::var_os("LIBRARY_EMBED_URL").is_some()
    {
        return Err("this registry requires its policy-aware filing adapter".into());
    }
    // Filing is complete when the page exists and, for wiki sources, the golden
    // line does too. A repeated call fills only what is missing.
    let mut filed = catalog::file_page(config, Some(&library), question, answer, &refs)?;
    let docs: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|s| {
            s.ends_with(".md")
                && !s.contains("://")
                && !s.starts_with('/')
                && !s.split('/').any(|p| p == "..")
                && root.join(s).is_file()
        })
        .collect();
    if !docs.is_empty() {
        filed["golden"] = golden(&library, question, &docs, &entry["id"].to_string())?;
    }
    filed["filed_at"] = json!(records::mark_filed(&library, &entry["id"].to_string())?);
    filed["source_record_id"] = entry["id"].clone();
    Ok(filed)
}

/// One golden line per question. Another wiki document for the same question
/// extends that line instead of adding a second one.
fn golden(library: &str, question: &str, docs: &[&str], id: &str) -> Result<Value, String> {
    let dir = crate::rooms::data_dir().join("golden");
    files::no_link(&dir)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{library}.jsonl"));
    files::no_link(&path)?;
    let mut rows: Vec<Value> = match std::fs::read_to_string(&path) {
        Ok(text) => text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| format!("golden file: {e}")))
            .collect::<Result<_, _>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e.to_string()),
    };
    let mut changed = false;
    match rows.iter_mut().find(|r| r["q"] == question) {
        Some(row) => {
            let mut list: Vec<Value> = match &row["answer_doc"] {
                Value::String(s) => vec![json!(s)],
                Value::Array(a) => a.clone(),
                _ => vec![],
            };
            for doc in docs {
                if !list.iter().any(|d| d == doc) {
                    list.push(json!(doc));
                    changed = true;
                }
            }
            row["answer_doc"] = json!(list);
        }
        None => {
            rows.push(json!({"q":question,"answer_doc":docs,"src":format!("room record {id}")}));
            changed = true;
        }
    }
    if changed {
        let mut text = String::new();
        for row in &rows {
            text.push_str(&row.to_string());
            text.push('\n');
        }
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    Ok(json!({"path":format!("golden/{library}.jsonl"),"changed":changed}))
}
