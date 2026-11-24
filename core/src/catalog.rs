use crate::files::{atomic, lock, no_link};
use crate::{local, option, records};
use chrono::Local;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

type Result<T> = std::result::Result<T, String>;
fn io(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn registry(config: &Path) -> Result<Value> {
    serde_json::from_reader(File::open(config).map_err(io)?).map_err(io)
}

fn validate_key(key: &str) -> bool {
    (1..=63).contains(&key.len())
        && key.bytes().enumerate().all(|(i, c)| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || (i != 0 && matches!(c, b'-' | b'_'))
        })
}

/// Seed only a new wiki. An interrupted first activation can retry the same
/// synthetic pages; existing work, edited pages and symlinks are preserved.
pub(crate) fn seed_example(root: &Path) -> Result<()> {
    const PAGES: [(&str, &str); 2] = [
        (
            "refund.md",
            "# Synthetic refund policy\nUnused fictional products may be returned within 14 days.\n사용하지 않은 가상 제품은 14일 안에 반품할 수 있습니다.\n",
        ),
        (
            "shipping.md",
            "# Synthetic shipping policy\nFictional parcels leave every Monday.\n",
        ),
    ];
    let dir = root.join("entities");
    let conflict = "example requires an empty new wiki";
    for entry in fs::read_dir(root).map_err(io)? {
        let entry = entry.map_err(io)?;
        if entry.path() != dir || !entry.file_type().map_err(io)?.is_dir() {
            return Err(conflict.into());
        }
    }
    if dir.exists() {
        for entry in fs::read_dir(&dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let Some((_, text)) = PAGES.iter().find(|(name, _)| entry.file_name() == *name) else {
                return Err(conflict.into());
            };
            if !entry.file_type().map_err(io)?.is_file()
                || fs::read(entry.path()).map_err(io)? != text.as_bytes()
            {
                return Err(conflict.into());
            }
        }
    } else {
        fs::create_dir(&dir).map_err(io)?;
    }
    for (name, text) in PAGES {
        let path = dir.join(name);
        if !path.exists() {
            atomic(&path, text.as_bytes(), true)?;
        }
    }
    Ok(())
}

fn change_registry(cmd: &str, config: &Path, args: &mut Vec<String>) -> Result<Value> {
    let key = option(args, "--key")?
        .or_else(|| (cmd == "init").then(|| "main".into()))
        .ok_or("add needs --key")?;
    let name = option(args, "--name")?.unwrap_or_else(|| {
        if cmd == "init" {
            "Example library".into()
        } else {
            key.clone()
        }
    });
    let demo = cmd == "init" && args.as_slice() == ["--demo"];
    if (!args.is_empty() && !demo) || !validate_key(&key) || name.trim().is_empty() {
        return Err("invalid library arguments, key or name".into());
    }
    let config = std::path::absolute(config).map_err(io)?;
    let base = config.parent().ok_or("config needs a directory")?;
    fs::create_dir_all(base).map_err(io)?;
    let state = base.join(".library-state");
    no_link(&state)?;
    fs::create_dir_all(&state).map_err(io)?;
    let lockpath = state.join("registry.lock");
    no_link(&lockpath)?;
    let _lock = lock(&lockpath)?;
    let directory = if cmd == "init" {
        "wiki".into()
    } else {
        format!("wiki-{key}")
    };
    let root = base.join(&directory);
    no_link(&root)?;
    no_link(&config)?;
    let mut row = json!({"name":name.trim(),"wiki_root":directory});
    let mut reg = if cmd == "init" {
        if config.exists() || root.exists() {
            return Err("init needs an empty registry and wiki destination".into());
        }
        row["central"] = true.into();
        json!({"version":1,"libraries":{}})
    } else {
        registry(&config)?
    };
    let central = if cmd == "add" {
        let central = local::selection(&config, None)?.0;
        let reads = &reg["libraries"][&central]["reads"];
        if let Some(existing) = reg["libraries"].get(&key) {
            if existing == &row
                && root.is_dir()
                && reads.as_array().is_some_and(|v| v.contains(&json!(key)))
            {
                return Ok(
                    json!({"created":false,"central":central,"library":key,"config":config,"wiki_root":root}),
                );
            }
            return Err("library key is already registered".into());
        }
        for existing in reg["libraries"]
            .as_object()
            .ok_or("invalid registry")?
            .values()
        {
            if let Some(s) = existing["wiki_root"].as_str() {
                let p = base.join(s).canonicalize().map_err(io)?;
                if root.starts_with(&p) || p.starts_with(&root) {
                    return Err("wiki roots overlap".into());
                }
            }
        }
        let reads = &mut reg["libraries"][&central]["reads"];
        if reads.is_null() {
            *reads = json!([]);
        }
        reads
            .as_array_mut()
            .ok_or("invalid reads list")?
            .push(json!(key));
        central
    } else {
        key.clone()
    };
    match fs::create_dir(&root) {
        Ok(()) => (),
        Err(e)
            if cmd == "add"
                && e.kind() == ErrorKind::AlreadyExists
                && root.is_dir()
                && fs::read_dir(&root).map_err(io)?.next().is_none() =>
        {
            ()
        }
        Err(e) => return Err(io(e)),
    }
    if demo {
        seed_example(&root)?;
    }
    reg["libraries"][&key] = row;
    atomic(
        &config,
        &serde_json::to_vec_pretty(&reg).map_err(io)?,
        cmd == "init",
    )?;
    local::index(&config, &key)?;
    Ok(if cmd == "init" {
        json!({"config":config,"library":key,"demo":demo})
    } else {
        json!({"config":config,"central":central,"library":key,"wiki_root":root,"created":true})
    })
}

const QUESTION: &str = "## 질문\n\n";
const HISTORY: &str = "## 변경 이력\n\n";
const STOP: &[&str] = &[
    "무엇",
    "뭐",
    "어디",
    "어떻게",
    "왜",
    "언제",
    "누가",
    "이거",
    "그거",
    "저거",
    "하나",
    "인가",
    "인지",
    "되나",
    "있나",
    "대해",
    "관련",
    "방법",
    "어떤",
    "정도",
    "지금",
];
fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || ('가'..='힣').contains(&c)
}

/// `--turns FILE:A-B` or `FILE:A..FILE:B` names the first and last transcript lines.
fn turns(text: &str) -> Result<String> {
    let point = |p: &str| -> Result<(String, u64)> {
        let (file, line) = p.rsplit_once(':').ok_or("turns need FILE:LINE")?;
        let (day, name) = file.split_once('/').ok_or("turns file is DAY/LIBRARY-ROOM.jsonl")?;
        if day.len() != 6
            || !day.chars().all(|c| c.is_ascii_digit())
            || !name.ends_with(".jsonl")
            || name.contains('/')
            || name.starts_with('.')
        {
            return Err("turns file is DAY/LIBRARY-ROOM.jsonl".into());
        }
        let line: u64 = line.parse().map_err(|_| "turns line must be a number")?;
        if line == 0 {
            return Err("turns lines start at 1".into());
        }
        Ok((file.to_owned(), line))
    };
    let (first, last) = if let Some((a, b)) = text.split_once("..") {
        (point(a)?, point(b)?)
    } else {
        let (file, range) = text.rsplit_once(':').ok_or("turns need FILE:LINE")?;
        let (a, b) = range.split_once('-').unwrap_or((range, range));
        (point(&format!("{file}:{a}"))?, point(&format!("{file}:{b}"))?)
    };
    if first.0 == last.0 && first.1 > last.1 {
        return Err("turns range ends before it starts".into());
    }
    Ok(json!([{"file":first.0,"line":first.1},{"file":last.0,"line":last.1}]).to_string())
}

/// An agent's summary of finished work becomes one database record. Only the
/// librarian files it into the wiki, with `file --id`.
fn save(config: &Path, key: Option<&str>, args: &mut Vec<String>) -> Result<Value> {
    let refs = option(args, "--refs")?.unwrap_or_default();
    let query = option(args, "--query")?.unwrap_or_default();
    let session = option(args, "--session")?.unwrap_or_default();
    let turns = match option(args, "--turns")? {
        Some(text) => turns(&text)?,
        None => String::new(),
    };
    if args.first().is_some_and(|s| s == "--") {
        args.remove(0);
    }
    let [question, answer] = args.as_slice() else {
        return Err("save needs question and answer".into());
    };
    if question.trim().is_empty() || answer.trim().is_empty() {
        return Err("question and answer must contain text".into());
    }
    if !records::enabled() {
        return Err("save keeps a record in the room database; configure one with setup".into());
    }
    let key = local::selection(config, key)?.0;
    local::verify_references(config, &key, &refs)?;
    let extra = records::Extra {
        session: &session,
        turns: &turns,
        kind: "record",
    };
    let record = records::store_with(&key, "librarian", question, &query, answer, &refs, &extra)?;
    Ok(json!({"saved":true,"library":key,"record_id":record["id"],"kind":"record"}))
}

/// Write or update the filing page for a question. `file --id` and archival
/// receipts call this; `save` does not.
pub fn file_page(
    config: &Path,
    key: Option<&str>,
    question: &str,
    answer: &str,
    refs: &str,
) -> Result<Value> {
    let mut args = vec![format!("--refs={refs}"), question.into(), answer.into()];
    write_page(config, key, &mut args)
}

fn write_page(config: &Path, key: Option<&str>, args: &mut Vec<String>) -> Result<Value> {
    let refs = option(args, "--refs")?.unwrap_or_default();
    if args.first().is_some_and(|s| s == "--") {
        args.remove(0);
    }
    let [question, answer] = args.as_slice() else {
        return Err("save needs question and answer".into());
    };
    if question.trim().is_empty() || answer.trim().is_empty() {
        return Err("question and answer must contain text".into());
    }
    let (key, root) = local::selection(config, key)?;
    local::verify_references(config, &key, &refs)?;
    let dir = root.join("queries");
    no_link(&dir)?;
    fs::create_dir_all(&dir).map_err(io)?;
    let lockpath = dir.join(".filing.lock");
    no_link(&lockpath)?;
    let _lock = lock(&lockpath)?;
    let now = Local::now();
    let day = now.format("%Y-%m-%d");
    let digest = format!("{:x}", Sha256::digest(question.as_bytes()));
    let slug: String = question
        .chars()
        .filter(|&c| word(c) || c == ' ' || c == '_')
        .take(40)
        .map(|c| if c == ' ' { '_' } else { c })
        .collect();
    let slug = if slug.is_empty() { "query" } else { &slug };
    let relative = format!(
        "queries/{}_{slug}_{}.md",
        now.format("%y%m%d"),
        &digest[..12]
    );
    let page = root.join(&relative);
    let log = root.join("log.md");
    no_link(&page)?;
    no_link(&log)?;
    let evidence = if refs.is_empty() {
        "근거 미기록".to_owned()
    } else {
        refs.clone()
    };
    let current =
        format!("{QUESTION}{question}\n\n## 답변\n\n{answer}\n\n## 검색 결과\n\n{evidence}\n");
    let previous = match fs::read_to_string(&page) {
        Ok(p) => Some(p),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(io(e)),
    };
    let mut history = String::new();
    let mut changed = true;
    if let Some(old) = previous.as_deref() {
        let (header, body) = old.split_once("\n---\n").unwrap_or(("", old));
        let parts = body.split_once(&format!("\n\n{QUESTION}"));
        let old_current = if let Some((intro, tail)) = parts {
            if !tail.starts_with(&format!("{question}\n\n## 답변\n\n")) {
                return Err("existing filename belongs to a different question".into());
            }
            if let Some((_, h)) = intro.split_once(&format!("\n\n{HISTORY}")) {
                history = h.trim_end().to_owned();
                history.push_str("\n\n");
            }
            format!("{QUESTION}{tail}")
        } else {
            old.into()
        };
        changed = old_current != current;
        let stamp = header
            .lines()
            .find_map(|l| l.strip_prefix("saved_at: "))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{} (시각 미상)",
                    header
                        .lines()
                        .find_map(|l| l.strip_prefix("created: "))
                        .unwrap_or("unknown")
                )
            });
        history.push_str(&format!("### {stamp}\n\n"));
        for line in old_current.trim_end_matches('\n').split('\n') {
            history.push_str(&format!("> {line}\n"));
        }
    }
    let mut bytes = previous.as_deref().unwrap_or("").len();
    if changed {
        let lowered = question.to_lowercase();
        let mut tags = Vec::new();
        for tag in lowered.split(|c| !word(c)) {
            if tag.chars().count() >= 2 && !STOP.contains(&tag) && !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        let tags: String = tags.join(", ").chars().take(120).collect();
        let line = answer.trim().replace('\n', " ");
        let end = line.char_indices().find_map(|(i, c)| {
            (matches!(c, '.' | '。') && line[i + c.len_utf8()..].starts_with(char::is_whitespace))
                .then_some(i + c.len_utf8())
        });
        let summary: String = line[..end.unwrap_or(line.len())]
            .chars()
            .take(200)
            .collect();
        let hist = if history.is_empty() {
            String::new()
        } else {
            format!("{HISTORY}{}\n\n", history.trim_end())
        };
        let title = serde_json::to_string(question).map_err(io)?;
        let page_text = format!(
            "---\ntitle: {title}\ntype: query\ntags: [query, {tags}]\ncreated: {day}\nsaved_at: {}\n---\n\n{summary}\n\n{hist}{current}",
            now.to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
        );
        bytes = page_text.len();
        crate::files::wiki_page(&page, page_text.as_bytes(), &root)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .map_err(io)?;
        writeln!(f, "\n## [{day}] query | {question}").map_err(io)?;
    }
    local::refresh(config, &key, Some(&page))?;
    Ok(json!({"saved":true,"path":relative,"library":key,"bytes":bytes,"learning":"not configured"}))
}

/// The archival receipt title is machine-generated. The search room owns the actual
/// observation and records its original user question separately from the public query.
pub fn file_receipt(config: &Path, key: &str, title: &str, body: &str, url: &str) -> Result<Value> {
    let mut args = vec!["--refs".into(), url.into(), title.into(), body.into()];
    if local::simple(config)?
        && !local::selection(config, Some(key))?
            .1
            .join(".wiki-pending")
            .is_dir()
        && env::var_os("LIBRARY_EMBED_URL").is_none()
    {
        return write_page(config, Some(key), &mut args);
    }
    let out = std::process::Command::new("python3")
        .env_remove("BENEDICT_CLI")
        .arg(crate::app().join("bin/library.py"))
        .arg("save")
        .arg("--config")
        .arg(config)
        .args(["--library", key, "--refs", url, "--", title, body])
        .output()
        .map_err(io)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    serde_json::from_slice(&out.stdout).map_err(io)
}

pub fn run(cmd: &str, config: &Path, key: Option<&str>, args: &[String]) -> Result<Option<Value>> {
    if cmd != "init" && !local::simple(config)? {
        return Ok(None);
    }
    let mut args = args.to_owned();
    if cmd == "save" {
        let root = local::selection(config, key)?.1;
        if root.join(".wiki-pending").is_dir() || env::var_os("LIBRARY_EMBED_URL").is_some() {
            return Ok(None);
        }
        return save(config, key, &mut args).map(Some);
    }
    change_registry(cmd, config, &mut args).map(Some)
}
