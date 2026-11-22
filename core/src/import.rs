//! An import starts with an inventory. Approval is bound to that exact inventory,
//! destination and file versions. Recheck open file handles before publishing.
use crate::{files, local};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    fmt::Display,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

type Result<T> = std::result::Result<T, String>;
fn err(e: impl Display) -> String {
    e.to_string()
}

fn inventory(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(err)? {
        let entry = entry.map_err(err)?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let ty = entry.file_type().map_err(err)?;
        if ty.is_symlink() {
            return Err(format!(
                "nested symlink requires a separate selection: {}",
                path.display()
            ));
        }
        if ty.is_dir() {
            inventory(root, &path, files)?;
        } else if ty.is_file() {
            files.push(path.strip_prefix(root).map_err(err)?.into());
        } else {
            return Err(format!("not a regular file: {}", path.display()));
        }
    }
    Ok(())
}

fn version(meta: &fs::Metadata) -> [i128; 4] {
    [
        meta.dev().into(),
        meta.ino().into(),
        meta.size().into(),
        meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128,
    ]
}

// Match historical json.dumps(..., sort_keys=True): approvals remain usable across
// native and earlier clients. The plan contains strings/arrays/objects only.
fn plan_json(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let quoted = serde_json::to_string(s).unwrap();
            quoted
                .chars()
                .map(|c| {
                    if c < '\u{7f}' {
                        c.to_string()
                    } else {
                        c.encode_utf16(&mut [0; 2])
                            .iter()
                            .map(|x| format!("\\u{x:04x}"))
                            .collect()
                    }
                })
                .collect()
        }
        Value::Array(v) => format!(
            "[{}]",
            v.iter().map(plan_json).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", plan_json(&json!(k)), plan_json(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => v.to_string(),
    }
}

fn stage(
    base: &Path,
    names: &[PathBuf],
    versions: &[[i128; 4]],
    dest: &Path,
    skill: bool,
) -> Result<()> {
    for (n, v) in names.iter().zip(versions) {
        let src = base.join(n);
        if src.canonicalize().map_err(err)? != src {
            return Err("source path changed; preview again".into());
        }
        let mut f = File::open(&src).map_err(err)?;
        let meta = f.metadata().map_err(err)?;
        if !meta.is_file() || version(&meta) != *v {
            return Err("source changed; preview again".into());
        }
        let dst = dest.join(n);
        fs::create_dir_all(dst.parent().unwrap()).map_err(err)?;
        let mut out = File::create_new(&dst).map_err(err)?;
        let mut sha = Sha256::new();
        let mut buf = [0u8; 16384];
        loop {
            let len = f.read(&mut buf).map_err(err)?;
            if len == 0 {
                break;
            }
            sha.update(&buf[..len]);
            out.write_all(&buf[..len]).map_err(err)?;
        }
        if version(&f.metadata().map_err(err)?) != *v {
            return Err("source changed while reading; preview again".into());
        }
        if skill {
            let mode = if meta.mode() & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            out.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(err)?;
        }
        drop(out);
        let mut copy = File::open(&dst).map_err(err)?;
        let mut copied = Sha256::new();
        io::copy(&mut copy, &mut copied).map_err(err)?;
        if sha.finalize() != copied.finalize() {
            return Err("staged copy differs from source".into());
        }
    }
    Ok(())
}

fn execute(command: &mut Command) -> Result<Vec<u8>> {
    let out = command.output().map_err(err)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(500)
            .collect());
    }
    Ok(out.stdout)
}

fn wiki(config: &Path, key: &str, root: &Path, name: &Path, staged: &Path) -> Result<Value> {
    let relative = Path::new("imports").join(name);
    if name.to_string_lossy().starts_with('.') {
        return Err("hidden wiki documents cannot be indexed".into());
    }
    let registry: Value = serde_json::from_reader(File::open(config).map_err(err)?).map_err(err)?;
    if let Some(dirs) = registry["libraries"][key]["dirs"]
        .as_array()
        .filter(|d| !d.is_empty())
    {
        if !dirs
            .iter()
            .filter_map(Value::as_str)
            .any(|d| d == "." || relative.starts_with(d))
        {
            return Err("imports directory is outside this library’s indexed directories".into());
        }
    }
    let data = fs::read(staged).map_err(err)?;
    std::str::from_utf8(&data).map_err(err)?;
    let dir = root.join("imports");
    files::no_link(&dir)?;
    fs::create_dir_all(&dir).map_err(err)?;
    let target = root.join(&relative);
    files::no_link(&target)?;
    if target.exists() {
        if fs::read(&target).map_err(err)? != data {
            return Err("existing document differs; preserved".into());
        }
    } else {
        files::atomic(&target, &data, true)?;
    }
    local::refresh(config, key, Some(&target))?;
    Ok(json!({"status":"imported","library":key,"path":relative}))
}

// Read the two scalar fields used by the catalog; preserve the original skill bytes.
fn skill_metadata(text: &str) -> Result<(String, String)> {
    let normalized = text.replace("\r\n", "\n");
    let (head, body) = normalized
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .ok_or("SKILL.md needs a closed YAML frontmatter block")?;
    let lines: Vec<_> = head.lines().collect();
    let (mut name, mut description) = (String::new(), String::new());
    for (i, line) in lines.iter().enumerate() {
        if let Some((key @ ("name" | "description"), value)) = line.split_once(':') {
            let found = if key == "name" {
                &mut name
            } else {
                &mut description
            };
            let value = value.trim();
            if matches!(value, "" | ">" | ">-" | "|" | "|-") {
                *found = lines[i + 1..]
                    .iter()
                    .take_while(|s| s.starts_with([' ', '\t']))
                    .map(|s| s.trim())
                    .collect::<Vec<_>>()
                    .join(" ");
            } else {
                *found = if value.len() >= 2
                    && ((value.starts_with('"') && value.ends_with('"'))
                        || (value.starts_with('\'') && value.ends_with('\'')))
                {
                    value[1..value.len() - 1]
                        .replace("\\\"", "\"")
                        .replace("''", "'")
                } else {
                    value.into()
                };
            }
        }
    }
    if description.is_empty() || body.trim().is_empty() {
        return Err("SKILL.md requires description and body".into());
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("invalid skill name".into());
    }
    Ok((name, description))
}

// Publish only the committed Git source and preserve incompatible card history.
fn publish(config: &Path, key: &str, repo: &Path, name: &str, dry: bool) -> Result<Value> {
    let git = |args: &[&str]| execute(Command::new("git").arg("-C").arg(repo).args(args));
    let revision = |spec: &str| -> Result<String> {
        Ok(String::from_utf8(git(&["rev-parse", "--verify", spec])?)
            .map_err(err)?
            .trim()
            .to_owned())
    };
    let commit = revision("HEAD^{commit}")?;
    let path = format!("skills/{name}/SKILL.md");
    let tree = revision(&format!("{commit}:skills/{name}"))?;
    let bytes = git(&["show", &format!("{commit}:{path}")])?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let text = std::str::from_utf8(&bytes).map_err(err)?;
    let (identity, description) = skill_metadata(text)?;
    if identity != name
        || !git(&[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "skills",
        ])?
        .is_empty()
    {
        return Err("skill source differs from its committed identity".into());
    }
    let secret = regex::Regex::new(r#"(?i)(?:password|passwd|secret|token|api[_-]?key)\s*[=:]\s*['"]?([A-Za-z0-9_-]{12,})|sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{20,}"#).map_err(err)?;
    let env_name = regex::Regex::new(r"^[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+$").map_err(err)?;
    if secret
        .captures_iter(text)
        .any(|m| !m.get(1).is_some_and(|v| env_name.is_match(v.as_str())))
    {
        return Err("secret-like content rejected".into());
    }
    let provenance = json!({"commit":commit,"path":path,"tree_sha":tree,"source_sha256":sha});
    let root = local::selection(config, Some(key))?.1;
    let folder = root.join("entities");
    let target = folder.join(format!("skill_{name}.md"));
    files::no_link(&folder)?;
    files::no_link(&target)?;
    let marker = "<!-- library-skill-card -->";
    let prefix = "<!-- library-skill-source ";
    let mut result = json!({"library":key,"commit":commit,"dry_run":dry,"published":[],"unchanged":[],"stale_release_skipped":[]});
    let old = match fs::read_to_string(&target) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(err(e)),
    };
    if let Some(old) = &old {
        let prior = old
            .lines()
            .find_map(|line| line.strip_prefix(prefix)?.strip_suffix(" -->"))
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .filter(|_| old.lines().any(|line| line == marker))
            .ok_or("unmanaged destination preserved")?;
        let prior_commit = prior["commit"]
            .as_str()
            .filter(|s| (40..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("invalid published source commit")?;
        if prior_commit != commit {
            revision(&format!("{prior_commit}^{{commit}}"))?;
            if git(&["merge-base", "--is-ancestor", &commit, prior_commit]).is_ok() {
                result["stale_release_skipped"] = json!([name]);
                return Ok(result);
            }
            git(&["merge-base", "--is-ancestor", prior_commit, &commit])
                .map_err(|_| "divergent skill revision preserved")?;
        }
        if prior["tree_sha"] == tree && prior["source_sha256"] == sha {
            result["unchanged"] = json!([name]);
            return Ok(result);
        }
    }
    if !dry {
        fs::create_dir_all(&folder).map_err(err)?;
        let title = json!(format!("{name} — {description}"));
        let text = text.replace("\r\n", "\n");
        let card = format!(
            "---\ntitle: {title}\ntype: entity\ntags: [skill, catalog]\n---\n{marker}\n{prefix}{provenance} -->\n\n# {name}\n\n{description}\n\n- Commit: `{commit}`\n- SHA-256: `{sha}`\n- Skill tree: `{tree}`\n- Original: `{path}` at the commit above (local Git source)\n\n## Committed SKILL.md\n\n{text}"
        );
        if let Some(previous) = old {
            if fs::read_to_string(&target).map_err(err)? != previous {
                return Err("destination changed during publication".into());
            }
            files::wiki_page(&target, card.as_bytes(), &root)?;
        } else {
            files::create_wiki_page(&target, card.as_bytes(), &root)?;
        }
    }
    result["published"] = json!([name]);
    Ok(result)
}

fn skill(config: &Path, key: &str, snapshot: &Path, state: &Path) -> Result<Value> {
    let (name, _) = skill_metadata(&fs::read_to_string(snapshot.join("SKILL.md")).map_err(err)?)?;
    if key.is_empty()
        || key == "."
        || key == ".."
        || Path::new(key).file_name().and_then(|v| v.to_str()) != Some(key)
    {
        return Err("invalid destination key for an import shelf".into());
    }
    let shelves = state.join("import-shelves");
    files::no_link(&shelves)?;
    let shelves = shelves.join(key);
    files::no_link(&shelves)?;
    fs::create_dir_all(&shelves).map_err(err)?;
    let repo = shelves.join(&name);
    files::no_link(&repo)?;
    let tmp = TempDir::new_in(&shelves).map_err(err)?;
    let candidate = tmp.path().join("repo");
    let skills = candidate.join("skills");
    fs::create_dir_all(&skills).map_err(err)?;
    // Use the approved copy. The staging folder may be on a different device.
    fs::create_dir(skills.join(&name)).map_err(err)?;
    let mut entries = Vec::new();
    inventory(snapshot, snapshot, &mut entries)?;
    for path in entries {
        let dst = skills.join(&name).join(&path);
        fs::create_dir_all(dst.parent().unwrap()).map_err(err)?;
        fs::copy(snapshot.join(path), dst).map_err(err)?;
    }
    let git =
        |path: &Path, args: &[&str]| execute(Command::new("git").arg("-C").arg(path).args(args));
    git(&candidate, &["init", "-q"])?;
    git(&candidate, &["config", "core.autocrlf", "false"])?;
    git(&candidate, &["add", "-f", "--", "skills"])?;
    git(
        &candidate,
        &[
            "-c",
            "user.name=Library import",
            "-c",
            "user.email=library@localhost",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Import approved skill",
        ],
    )?;
    if repo.exists() {
        if git(&repo, &["rev-parse", "HEAD:skills"])?
            != git(&candidate, &["rev-parse", "HEAD:skills"])?
        {
            return Err(
                "an imported skill already has different contents; existing source preserved"
                    .into(),
            );
        }
    } else {
        publish(config, key, &candidate, &name, true)?;
        fs::rename(&candidate, &repo).map_err(err)?;
    }
    let mut result = publish(config, key, &repo, &name, false)?;
    local::refresh(config, key, None)?;
    result["status"] = json!("imported");
    result["shelf"] = json!(repo);
    Ok(result)
}

pub fn run(config: &Path, key: &str, args: &[String]) -> Result<Option<Value>> {
    if !local::simple(config)?
        || (env::var_os("LIBRARY_EMBED_URL").is_some() && env::var_os("LIBRARY_PG_DSN").is_some())
    {
        return Ok(None);
    }
    let mut args = args.to_vec();
    let approve = crate::option(&mut args, "--approve")?;
    if args.len() != 2 || !matches!(args[0].as_str(), "wiki" | "skill") {
        return Err("import needs wiki|skill and a source path".into());
    }
    let kind = &args[0];
    let selected = Path::new(&args[1]);
    let source = if let Ok(suffix) = selected.strip_prefix("~") {
        PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join(suffix)
    } else {
        selected.to_owned()
    }
    .canonicalize()
    .map_err(err)?;
    let config = config.canonicalize().map_err(err)?;
    let (_, root) = local::selection(&config, Some(key))?;
    let mut names = Vec::new();
    let base = if kind == "wiki" {
        if !source.is_file() || source.extension().is_none_or(|x| x != "md") {
            return Err("select one Markdown wiki file".into());
        }
        names.push(PathBuf::from(source.file_name().unwrap()));
        source.parent().unwrap()
    } else {
        if !source.is_dir() || !source.join("SKILL.md").is_file() {
            return Err("select a skill directory containing SKILL.md".into());
        }
        inventory(&source, &source, &mut names)?;
        names.sort();
        source.as_path()
    };
    let mut plan = json!({"kind":kind,"source":source,"library":key,"wiki":root,"registry":config,"files":names});
    let versions: Vec<_> = names
        .iter()
        .map(|n| fs::metadata(base.join(n)).map(|m| version(&m)))
        .collect::<io::Result<_>>()
        .map_err(err)?;
    let token = format!(
        "{:x}",
        Sha256::digest(format!("[{}, {:?}]", plan_json(&plan), versions).as_bytes())
    );
    match approve {
        None => {
            plan["confirmation"] = json!(token);
            plan["status"] = json!("awaiting approval");
            return Ok(Some(plan));
        }
        Some(v) if v != token => {
            return Err(
                "source or destination differs from the approved preview; preview again".into(),
            );
        }
        _ => {}
    }
    let snapshot = TempDir::new().map_err(err)?;
    stage(base, &names, &versions, snapshot.path(), kind == "skill")?;
    let state = config.parent().unwrap().join(".library-state");
    files::no_link(&state)?;
    fs::create_dir_all(&state).map_err(err)?;
    let lock = state.join("import.lock");
    files::no_link(&lock)?;
    let _lock = files::lock(&lock)?;
    let result = if kind == "wiki" {
        wiki(
            &config,
            key,
            &root,
            &names[0],
            &snapshot.path().join(&names[0]),
        )?
    } else {
        skill(&config, key, snapshot.path(), &state)?
    };
    Ok(Some(result))
}
