//! Keep each approved, nonsecret installation decision until a library exists.
//! The stable decision key lets a retry finish filing after a committed DB write.
use crate::{catalog, files, local, records, wizard};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

fn input() -> Result<Value> {
    let mut raw = String::new();
    io::stdin()
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|_| "expected installation JSON on stdin".into())
}

fn fields(v: &Value, allowed: &[&str]) -> Result<()> {
    if v.as_object()
        .is_none_or(|o| o.keys().any(|k| !allowed.contains(&k.as_str())))
    {
        return Err("unexpected installation journal field".into());
    }
    Ok(())
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn nonsecret(answer: &str) -> Result<()> {
    // Only an explicitly reviewed nonsecret free-text answer enters the journal.
    // Reject typical credentials even if the caller accidentally approved one.
    let s = answer.to_lowercase();
    let sensitive_url = answer
        .split_whitespace()
        .filter(|piece| piece.contains("://"))
        .any(|piece| {
            let piece =
                piece.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '<' | '>' | '"'));
            url::Url::parse(piece).map_or(true, |url| {
                !matches!(url.scheme(), "https" | "http")
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query_pairs().any(|(k, _)| {
                        matches!(
                            k.to_ascii_lowercase().as_str(),
                            "key"
                                | "token"
                                | "signature"
                                | "sig"
                                | "credential"
                                | "password"
                                | "access_token"
                                | "x-amz-signature"
                                | "x-goog-signature"
                        )
                    })
            })
        });
    if sensitive_url
        || [
            "-----begin ",
            "bearer ",
            "ghp_",
            "github_pat_",
            "glpat-",
            "xoxb-",
            "xoxp-",
            "sk-",
            "akia",
            "비밀번호",
            "password",
            "passwd",
            "access_token",
            "api_key",
            "api-key",
            "secret=",
            "token=",
        ]
        .iter()
        .any(|p| s.contains(p))
        || answer.contains('\0')
    {
        return Err("omit credentials; enter only a nonsecret installation choice".into());
    }
    Ok(())
}

fn digest(v: &Value) -> String {
    format!("{:x}", Sha256::digest(v.to_string().as_bytes()))
}

fn decision(v: &Value) -> Result<Value> {
    fields(v, &["step", "wiki_found", "choice", "answer", "nonsecret"])?;
    let found = v["wiki_found"]
        .as_bool()
        .ok_or("wiki_found must be true or false")?;
    let prompt = wizard::prompt(text(v, "step")?, found)?;
    let choice = text(v, "choice")?;
    let entry = prompt["choices"]
        .as_array()
        .and_then(|opts| opts.iter().find(|opt| opt["id"] == choice))
        .ok_or("unknown installation choice")?;
    let answer = if choice == "other" {
        if v["nonsecret"] != true {
            return Err("review free text for credentials and set nonsecret to true".into());
        }
        let answer = text(v, "answer")?;
        nonsecret(answer)?;
        answer
    } else {
        if v.get("answer").is_some() || v.get("nonsecret").is_some() {
            return Err("a clicked choice needs only its ID".into());
        }
        text(entry, "label")?
    };
    Ok(json!({"input":v,"prompt":prompt,"answer":answer}))
}

fn validate_buffer(buf: &Value, project: &str) -> Result<()> {
    fields(buf, &["version", "project", "decisions"])?;
    if buf["version"] != 1 || buf["project"] != project {
        return Err("installation buffer version or project does not match".into());
    }
    let entries = buf["decisions"].as_array().ok_or("missing decision list")?;
    for entry in entries {
        // The file is under the installation user's control. A modified buffer
        // cannot add a raw secret field or silently substitute a different prompt.
        if decision(&entry["input"])? != *entry {
            return Err("installation prompt differs from the recorded version; use the installer that asked it".into());
        }
    }
    Ok(())
}
pub(crate) fn selections(path: &Path, project: &str) -> Result<BTreeMap<String, String>> {
    files::no_link(path)?;
    let buf: Value = serde_json::from_reader(fs::File::open(path).map_err(|e| e.to_string())?)
        .map_err(|_| "invalid installation buffer")?;
    validate_buffer(&buf, project)?;
    let mut latest = BTreeMap::new();
    for entry in buf["decisions"].as_array().unwrap() {
        let input = &entry["input"];
        latest.insert(text(input, "step")?.into(), text(input, "choice")?.into());
    }
    Ok(latest)
}

fn append(project: &str, path: &Path) -> Result<Value> {
    let entry = decision(&input()?)?;
    files::no_link(path)?;
    let mut lockname = path.as_os_str().to_owned();
    lockname.push(".lock");
    files::no_link(Path::new(&lockname))?;
    let _guard = files::lock(Path::new(&lockname))?;
    let mut buf = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "invalid installation buffer")?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            json!({"version":1,"project":project,"decisions":[]})
        }
        Err(e) => return Err(e.to_string()),
    };
    validate_buffer(&buf, project)?;
    let decisions = buf["decisions"].as_array_mut().unwrap();
    if decisions.last() != Some(&entry) {
        decisions.push(entry);
    }
    let count = decisions.len();
    files::atomic(
        path,
        &serde_json::to_vec(&buf).map_err(|e| e.to_string())?,
        false,
    )?;
    Ok(json!({"buffered":count,"project":project}))
}

fn restore_entry(
    config: &Path,
    library: &str,
    wiki: &Path,
    key: &str,
    entry: &Value,
) -> Result<Value> {
    let prompt = &entry["prompt"];
    let question = text(prompt, "question")?;
    let answer = text(entry, "answer")?;
    let found = records::recall(library, "query", key)?;
    let source = json!({"prompt":prompt,"choice":entry["input"]["choice"]}).to_string();
    let hits = found["records"]
        .as_array()
        .ok_or("invalid database receipt")?;
    let record = match hits.as_slice() {
        [] => records::store(library, "librarian", question, key, answer, &source)?,
        [one]
            if one["room"] == "librarian"
                && one["question"] == question
                && one["answer"] == answer
                && one["source"] == source =>
        {
            one.clone()
        }
        _ => return Err("conflicting installation decision in database".into()),
    };
    let id = record["id"].as_i64().ok_or("missing decision ID")?;
    let reference = format!("doseogwan recall --library {library} --id {id}");
    let title = format!("설치 결정 · {question} ({key})");
    let suffix = format!(
        "_{:.12}.md",
        format!("{:x}", Sha256::digest(title.as_bytes()))
    );
    let dir = wiki.join("queries");
    files::no_link(&dir)?;
    if dir.exists() {
        for item in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            if item.file_name().to_string_lossy().ends_with(&suffix) {
                files::no_link(&item.path())?;
                let previous = fs::read_to_string(item.path()).map_err(|e| e.to_string())?;
                if previous.lines().any(|line| line == reference) {
                    // If the previous run stopped during refresh, finish it.
                    local::refresh(config, library, Some(&item.path()))?;
                    return Ok(
                        json!({"record_id":id,"path":Path::new("queries").join(item.file_name())}),
                    );
                }
                return Err("installation filing path already contains a different record".into());
            }
        }
    }
    let opts = prompt["choices"]
        .as_array()
        .ok_or("missing wizard choices")?;
    let mut body = format!("선택한 답: {answer}\n\n");
    for option in opts {
        let recommended = if option["recommended"] == true {
            " (추천)"
        } else {
            ""
        };
        body.push_str(&format!(
            "- {}{} — {}\n",
            text(option, "label")?,
            recommended,
            text(option, "reason")?
        ));
    }
    let receipt = catalog::file_receipt(config, library, &title, &body, &reference)?;
    Ok(json!({"record_id":id,"path":receipt["path"]}))
}

fn replay(config: &Path) -> Result<Value> {
    let buf = input()?;
    let project = records::project()?;
    validate_buffer(&buf, &project)?;
    let (library, wiki) = local::selection(config, None)?;
    let state = config
        .canonicalize()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("invalid registry")?
        .join(".library-state");
    files::no_link(&state)?;
    fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let lock = state.join("install-journal.lock");
    files::no_link(&lock)?;
    let _guard = files::lock(&lock)?;
    records::initialize()?;
    let mut receipts = Vec::new();
    let mut previous = project.clone();
    for entry in buf["decisions"].as_array().unwrap() {
        let id = digest(&json!([previous, entry]));
        let key = format!("setup/decision/{id}");
        receipts.push(restore_entry(config, &library, &wiki, &key, entry)?);
        previous = id;
    }
    Ok(json!({"project":project,"library":library,"decisions":receipts}))
}

pub fn run(args: &[String]) -> Result<()> {
    let (cmd, args) = args.split_first().ok_or("use journal append or replay")?;
    let result = match cmd.as_str() {
        "append" => {
            let flags = records::options(args, &["project", "buffer"])?;
            append(
                flags.get("project").ok_or("missing --project")?,
                Path::new(flags.get("buffer").ok_or("missing --buffer")?),
            )?
        }
        "replay" => {
            let flags = records::options(args, &["config"])?;
            let cfg = flags
                .get("config")
                .map(PathBuf::from)
                .or_else(|| env::var_os("LIBRARY_CONFIG").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("library.json"));
            replay(&cfg)?
        }
        _ => return Err("use journal append or replay".into()),
    };
    println!("{result}");
    Ok(())
}
