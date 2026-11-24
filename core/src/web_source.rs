//! Commit a verified, agent-selected original to the shelf and retain its receipt.
//! The fetch adapter applies the same URL, redirect and privacy checks as other web clients.
use crate::{catalog, files, local, option, records};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

type Result<T> = std::result::Result<T, String>;

fn field<'a>(r: &'a Value, name: &str) -> Result<&'a str> {
    r[name]
        .as_str()
        .ok_or_else(|| format!("fetch adapter omitted {name}"))
}

fn commit_file(
    stage: &Path,
    root: &Path,
    shelf: &Path,
    path: &str,
    digest: &str,
    original: bool,
) -> Result<PathBuf> {
    let source = Path::new(path).canonicalize().map_err(|e| e.to_string())?;
    if source.parent() != Some(stage) || source.file_stem().and_then(|x| x.to_str()) != Some(digest)
    {
        return Err("fetch adapter returned an unexpected shelf path".into());
    }
    let bytes = fs::read(&source).map_err(|e| e.to_string())?;
    if original && format!("{:x}", Sha256::digest(&bytes)) != digest {
        return Err("staged original digest mismatch".into());
    }
    let target = shelf.join(source.file_name().unwrap());
    files::no_link(&target)?;
    if !target.exists() {
        let result = files::create_wiki_page(&target, &bytes, root);
        if !target.exists() {
            result?;
        }
    }
    // Another writer can archive the same original concurrently. Verify it in either case.
    if fs::read(&target).map_err(|e| e.to_string())? != bytes {
        return Err("shelf original differs from approved bytes".into());
    }
    Ok(target)
}

pub fn run(config: &Path, key: Option<&str>, args: &[String]) -> Result<Value> {
    let mut args = args.to_vec();
    let question = option(&mut args, "--question")?.unwrap_or_default();
    let [query, url] = args.as_slice() else {
        return Err("web-source needs public search terms and source URL".into());
    };
    let (key, root) = local::selection(config, key)?;
    let query = query.trim();
    let shelf = root.join(".web-shelf");
    files::no_link(&shelf)?;
    let stage = TempDir::new().map_err(|e| e.to_string())?;
    let stage_path = stage.path().canonicalize().map_err(|e| e.to_string())?;
    let fetch = Command::new("python3")
        .arg(crate::app().join("bin/web.py"))
        .arg("--agent-fetch")
        .arg(config)
        .arg(&key)
        .arg(&stage_path)
        .arg(query)
        .arg(url)
        .output()
        .map_err(|e| e.to_string())?;
    if !fetch.status.success() {
        let why = serde_json::from_slice::<Value>(&fetch.stderr)
            .ok()
            .and_then(|s| s["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| String::from_utf8_lossy(&fetch.stderr).trim().into());
        return Err(why);
    }
    let r: Value = serde_json::from_slice(&fetch.stdout).map_err(|e| e.to_string())?;
    let digest = field(&r, "sha256")?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid archived source digest".into());
    }
    fs::create_dir_all(&shelf).map_err(|e| e.to_string())?;
    let original = commit_file(&stage_path, &root, &shelf, field(&r, "path")?, digest, true)?;
    let read = match r["text_path"].as_str() {
        Some(path) if path != field(&r, "path")? => {
            commit_file(&stage_path, &root, &shelf, path, digest, false)?
        }
        _ => original.clone(),
    };
    let relative = |p: &Path| p.strip_prefix(&root).unwrap().display().to_string();
    let mut body = format!(
        "Source: {url}\nFinal URL: {}\nFetched: {}\nHTTP: {}\nSHA-256: {digest}\nOriginal: {}\nRead: {}",
        field(&r, "final_url")?,
        field(&r, "fetched_at")?,
        r["status"].as_u64().ok_or("missing HTTP status")?,
        relative(&original),
        relative(&read)
    );
    let thin = r["thin"]
        .as_bool()
        .ok_or("missing text quality indicator")?;
    if thin {
        body.push_str(
            "\nThe extracted text is short; check that it contains the requested evidence.",
        );
    }
    let mut result = catalog::file_receipt(config, &key, &format!("{query} [{url}]"), &body, url)?;
    if records::enabled() {
        let proof = json!({"url":url,"original":relative(&original),"sha256":digest,"receipt":result["path"]});
        let record = records::store(&key, "search", &question, query, &body, &proof.to_string())
            .map_err(|e| {
                format!(
                    "source and receipt saved at {}, but search-room recording failed: {e}",
                    result["path"]
                )
            })?;
        result["record_id"] = record["id"].clone();
    }
    result["original"] = json!(original);
    result["read_path"] = json!(read);
    result["sha256"] = json!(digest);
    result["thin"] = json!(thin);
    Ok(result)
}
