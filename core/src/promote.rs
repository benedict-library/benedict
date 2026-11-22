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
        "Room evidence: doseogwan recall --library {library} --id {}",
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
    let save_args = [
        format!("--refs={refs}"),
        format!("--query={}", field("query")?),
        question.to_owned(),
        answer.to_owned(),
    ];
    let mut filed = catalog::run("save", config, Some(&library), &save_args)?
        .ok_or("this registry requires its policy-aware filing adapter")?;
    filed["source_record_id"] = entry["id"].clone();
    Ok(filed)
}
