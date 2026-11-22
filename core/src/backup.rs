//! Markdown-only Git snapshots in the version 1 manifest + Git bundle tar format.
use rustix::fs::{CWD, RenameFlags, renameat_with};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};
use tempfile::{NamedTempFile, tempdir, tempdir_in};

type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn checked(program: &mut Command) -> Result<Output> {
    let output = program.output().map_err(error)?;
    if !output.status.success() {
        return Err(format!(
            "command failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        ));
    }
    Ok(output)
}

fn git(root: &Path) -> Command {
    let mut git = Command::new("timeout");
    git.args(["30", "git", "-C"])
        .arg(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_AUTHOR_NAME", "Library snapshot")
        .env("GIT_AUTHOR_EMAIL", "snapshot@example.invalid")
        .env("GIT_COMMITTER_NAME", "Library snapshot")
        .env("GIT_COMMITTER_EMAIL", "snapshot@example.invalid");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        git.env_remove(key);
    }
    git
}

fn text(cmd: &mut Command) -> Result<String> {
    String::from_utf8(checked(cmd)?.stdout).map_err(error)
}

fn hash(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    io::copy(&mut File::open(path).map_err(error)?, &mut digest).map_err(error)?;
    Ok(format!("{:x}", digest.finalize()))
}

fn relative_md(path: &Path) -> bool {
    path.extension().is_some_and(|v| v == "md")
        && path.components().all(
            |c| matches!(c, Component::Normal(part) if !part.to_string_lossy().starts_with('.')),
        )
}

fn documents(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in dir.read_dir().map_err(error)? {
        let entry = entry.map_err(error)?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type().map_err(error)?;
        if kind.is_dir() {
            documents(root, &path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if !kind.is_file() {
                return Err("snapshot documents must be regular files".into());
            }
            let rel = path
                .strip_prefix(root)
                .map_err(error)?
                .to_str()
                .ok_or("non-UTF-8 document path")?;
            files.insert(rel.into(), hash(&path)?);
        }
    }
    Ok(())
}

fn output_path(path: &Path, wiki: Option<&Path>) -> Result<PathBuf> {
    let path = std::path::absolute(path).map_err(error)?;
    if path.symlink_metadata().is_ok() {
        return Err(format!("target already exists: {}", path.display()));
    }
    let mut nearest = path.parent().ok_or("target needs a parent")?;
    while !nearest.exists() {
        nearest = nearest
            .parent()
            .ok_or("target needs an existing ancestor")?;
    }
    // Resolve the existing part first so creating output directories cannot modify the source wiki.
    let resolved = nearest
        .canonicalize()
        .map_err(error)?
        .join(path.strip_prefix(nearest).map_err(error)?);
    if resolved.components().any(|c| c == Component::ParentDir)
        || wiki.is_some_and(|w| resolved.starts_with(w))
    {
        return Err("use a new destination outside the wiki".into());
    }
    Ok(resolved)
}

// Check tree modes and names before any checkout, and bind every source hash to that exact Git commit.
fn verify_tree(root: &Path, commit: &str, expected: &Value) -> Result<()> {
    let mut actual = BTreeMap::new();
    for entry in text(git(root).args(["ls-tree", "-rz", commit]))?.split_terminator('\0') {
        let (meta, name) = entry.split_once('\t').ok_or("invalid Git tree")?;
        if !(meta.starts_with("100644 blob ") || meta.starts_with("100755 blob "))
            || !relative_md(Path::new(name))
        {
            return Err("snapshot must contain regular Markdown documents".into());
        }
        let id = meta
            .split_ascii_whitespace()
            .last()
            .ok_or("missing Git blob ID")?;
        let blob = checked(git(root).args(["cat-file", "blob", id]))?.stdout;
        actual.insert(name.to_string(), format!("{:x}", Sha256::digest(blob)));
    }
    if actual.is_empty() || json!(actual) != *expected {
        return Err("snapshot documents differ from Git commit".into());
    }
    Ok(())
}

pub(crate) fn snapshot(config: &Path, key: Option<&str>, archive: &Path) -> Result<Value> {
    let (_, root) = crate::local::selection(config, key)?;
    let root = root.canonicalize().map_err(error)?;
    let archive = output_path(archive, Some(&root))?;
    let git_dir = root.join(".git");
    crate::files::no_link(&git_dir)?;
    if !git_dir.exists() {
        checked(git(&root).args(["init", "-q"]))?;
    }
    let top = text(git(&root).args(["rev-parse", "--show-toplevel"]))?;
    if Path::new(top.trim_end_matches('\n'))
        .canonicalize()
        .map_err(error)?
        != root
    {
        return Err("wiki must be its own git repository".into());
    }
    let mut files = BTreeMap::new();
    documents(&root, &root, &mut files)?;
    if files.is_empty() {
        return Err("empty wiki has no snapshot content".into());
    }
    let tracked = text(git(&root).args(["ls-files", "-z"]))?;
    if tracked
        .split_terminator('\0')
        .any(|name| !files.contains_key(name))
    {
        return Err(
            "repository includes non-Markdown or removed tracked files; resolve explicitly".into(),
        );
    }
    for name in files.keys() {
        match git(&root)
            .env_remove("GIT_LITERAL_PATHSPECS")
            .args(["check-ignore", "-q", "--"])
            .arg(format!("./{name}"))
            .status()
            .map_err(error)?
            .code()
        {
            Some(0) => {
                return Err(format!(
                    "ignored Markdown document needs an explicit decision: {name}"
                ));
            }
            Some(1) => (),
            _ => return Err("could not check Git ignore rules".into()),
        }
        // Store the exact source bytes, including CRLF; avoid local clean filters transforming a backup.
        let file = root.join(name);
        let blob = text(
            git(&root)
                .args(["hash-object", "-w", "--no-filters", "--"])
                .arg(&file),
        )?;
        use std::os::unix::fs::PermissionsExt;
        let mode = if fs::metadata(&file).map_err(error)?.permissions().mode() & 0o111 != 0 {
            "100755"
        } else {
            "100644"
        };
        checked(git(&root).args([
            "update-index",
            "--add",
            "--cacheinfo",
            mode,
            blob.trim(),
            name,
        ]))?;
    }
    let diff = git(&root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(error)?;
    match diff.code() {
        Some(0) => (),
        Some(1) => {
            checked(git(&root).args(["commit", "-q", "-m", "Library Markdown snapshot"]))?;
        }
        _ => return Err("could not compare the Git index".into()),
    }
    let commit = text(git(&root).args(["rev-parse", "HEAD"]))?
        .trim()
        .to_string();
    verify_tree(&root, &commit, &json!(files))?;
    let tmp = tempdir().map_err(error)?;
    let bundle = tmp.path().join("wiki.bundle");
    checked(
        git(&root)
            .args(["bundle", "create"])
            .arg(&bundle)
            .arg("HEAD"),
    )?;
    checked(git(&root).args(["bundle", "verify"]).arg(&bundle))?;
    let bundled_head = text(
        git(&root)
            .args(["bundle", "list-heads"])
            .arg(&bundle)
            .arg("HEAD"),
    )?;
    if bundled_head.trim() != format!("{commit} HEAD") {
        return Err("wiki changed during snapshot; retry".into());
    }
    let manifest =
        json!({"version":1,"commit":commit,"files":files,"bundle_sha256":hash(&bundle)?});
    fs::write(tmp.path().join("manifest.json"), manifest.to_string()).map_err(error)?;
    let parent = archive.parent().unwrap();
    fs::create_dir_all(parent).map_err(error)?;
    let out = NamedTempFile::new_in(parent).map_err(error)?;
    checked(
        Command::new("tar")
            .current_dir(tmp.path())
            .args(["-cf", "-", "--", "manifest.json", "wiki.bundle"])
            .env_remove("TAR_OPTIONS")
            .stdout(Stdio::from(out.reopen().map_err(error)?)),
    )?;
    out.as_file().sync_all().map_err(error)?;
    out.persist_noclobber(&archive).map_err(error)?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(error)?;
    Ok(json!({"archive":archive,"commit":commit,"files":files.len(),"verified":true}))
}

fn tar(archive: &Path) -> Command {
    let mut cmd = Command::new("tar");
    cmd.env_remove("TAR_OPTIONS")
        .env("LC_ALL", "C")
        .arg("--file")
        .arg(archive);
    cmd
}

fn restore(archive: &Path, target: &Path) -> Result<Value> {
    let target = output_path(target, None)?;
    let tmp = tempdir().map_err(error)?;
    // Copy once; checks and reads below see the same archive even when the caller changes its input.
    let input = tmp.path().join("input.tar");
    fs::copy(archive, &input).map_err(error)?;
    let listing = text(tar(&input).args([
        "--list",
        "--verbose",
        "--numeric-owner",
        "--quoting-style=escape",
    ]))?;
    let mut expected = vec!["manifest.json", "wiki.bundle"];
    for line in listing.lines() {
        if !line.starts_with('-') {
            return Err("snapshot members must be regular files".into());
        }
        // GNU tar prints mode, owner, size, date and time before each escaped member name.
        let cols: Vec<_> = line.split_ascii_whitespace().collect();
        if cols.len() != 6 {
            return Err("invalid backup member".into());
        }
        let index = expected
            .iter()
            .position(|name| *name == cols[5])
            .ok_or("unexpected or duplicate backup member")?;
        expected.remove(index);
    }
    if !expected.is_empty() {
        return Err("missing backup members".into());
    }
    let manifest_bytes = checked(tar(&input).args(["-xO", "--", "manifest.json"]))?.stdout;
    let manifest: Value = serde_json::from_slice(&manifest_bytes).map_err(error)?;
    let bundle = tmp.path().join("wiki.bundle");
    checked(
        tar(&input)
            .args(["-xO", "--", "wiki.bundle"])
            .stdout(Stdio::from(File::create(&bundle).map_err(error)?)),
    )?;
    if manifest["version"] != 1 || manifest["bundle_sha256"] != hash(&bundle)? {
        return Err("backup hash mismatch".into());
    }
    checked(git(tmp.path()).args(["init", "-q"]))?;
    checked(git(tmp.path()).args(["bundle", "verify"]).arg(&bundle))?;
    let parent = target.parent().unwrap();
    fs::create_dir_all(parent).map_err(error)?;
    let stage = tempdir_in(parent).map_err(error)?;
    let checkout = stage.path().join("wiki");
    checked(
        git(tmp.path())
            .args(["clone", "--quiet", "--no-checkout", "--"])
            .arg(&bundle)
            .arg(&checkout),
    )?;
    let commit = text(git(&checkout).args(["rev-parse", "HEAD"]))?
        .trim()
        .to_string();
    if manifest["commit"] != commit {
        return Err("snapshot commit mismatch".into());
    }
    verify_tree(&checkout, &commit, &manifest["files"])?;
    checked(git(&checkout).args(["checkout", "--quiet", "HEAD", "--", "."]))?;
    let mut files = BTreeMap::new();
    documents(&checkout, &checkout, &mut files)?;
    if files.is_empty() || json!(files) != manifest["files"] {
        return Err("restored document hash mismatch".into());
    }
    checked(git(&checkout).args(["remote", "remove", "origin"]))?;
    renameat_with(CWD, &checkout, CWD, &target, RenameFlags::NOREPLACE).map_err(error)?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(error)?;
    Ok(json!({"target":target,"commit":commit,"files":files.len(),"verified":true}))
}

pub fn run(args: &[String]) -> Result<()> {
    if args.is_empty() || args.iter().any(|v| v == "-h" || v == "--help") {
        println!(
            "backup snapshot [--config REGISTRY] [--library KEY] --out ARCHIVE\nbackup restore --archive ARCHIVE --target NEW_DIRECTORY"
        );
        return Ok(());
    }
    let mut opts = args[1..].to_vec();
    let report = match args[0].as_str() {
        "snapshot" => {
            let config = crate::option(&mut opts, "--config")?.unwrap_or_else(|| {
                env::var("LIBRARY_CONFIG").unwrap_or_else(|_| "library.json".into())
            });
            let key = crate::option(&mut opts, "--library")?;
            let archive = crate::option(&mut opts, "--out")?.ok_or("missing --out")?;
            if !opts.is_empty() {
                return Err("unexpected snapshot arguments".into());
            }
            snapshot(Path::new(&config), key.as_deref(), Path::new(&archive))?
        }
        "restore" => {
            let archive = crate::option(&mut opts, "--archive")?.ok_or("missing --archive")?;
            let target = crate::option(&mut opts, "--target")?.ok_or("missing --target")?;
            if !opts.is_empty() {
                return Err("unexpected restore arguments".into());
            }
            restore(Path::new(&archive), Path::new(&target))?
        }
        _ => return Err("expected backup snapshot or restore".into()),
    };
    println!("{report}");
    Ok(())
}
