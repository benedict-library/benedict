//! Produce an isolated Docker Compose installation and activate it inside the runtime container.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;

use std::path::{Component, Path};

fn options(args: &[String]) -> Result<BTreeMap<&str, &str>, String> {
    let mut values = BTreeMap::new();
    for pair in args.chunks(2) {
        let key = pair[0].as_str();
        if !matches!(
            key,
            "--project"
                | "--memory"
                | "--database"
                | "--database-secret-file"
                | "--start"
                | "--wiki"
                | "--shelf"
                | "--output"
                | "--answers"
        ) || pair.len() != 2
            || pair[1].is_empty()
        {
            return Err(format!(
                "expected setup option and value; see setup --help ({key})"
            ));
        }
        if values.insert(key, pair[1].as_str()).is_some() {
            return Err(format!("duplicate option {key}"));
        }
    }
    Ok(values)
}

fn memory_bytes(value: &str) -> Result<u64, String> {
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let scale = match value[end..].to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return Err("invalid memory unit; use 512m or 1g".into()),
    };
    value[..end]
        .parse::<u64>()
        .ok()
        .and_then(|v| v.checked_mul(scale))
        .filter(|v| *v >= 6 * 1024 * 1024 && *v <= i64::MAX as u64)
        .ok_or_else(|| "memory must fit Docker's limit and be at least 6m".into())
}

fn host_path(value: &str) -> Result<&Path, String> {
    let path = Path::new(value);
    if !path.is_absolute()
        || path.parent().is_none()
        || path.components().any(|c| c == Component::ParentDir)
        || value.chars().any(char::is_control)
    {
        return Err("use the resolved absolute path on the host".into());
    }
    Ok(path)
}

fn apply_answers(
    opts: &mut BTreeMap<&str, &str>,
    answers: &BTreeMap<String, String>,
) -> Result<(), String> {
    let selected = |step: &str| {
        answers
            .get(step)
            .map(String::as_str)
            .ok_or_else(|| format!("missing confirmed {step} answer"))
    };
    let conflict =
        |step: &str| format!("installation options conflict with the confirmed {step} answer");
    let require = |present: bool, option: &str, step: &str| {
        if present {
            Ok(())
        } else {
            Err(format!("provide {option} for the confirmed {step} answer"))
        }
    };
    match selected("start")? {
        "example"
            if !opts.contains_key("--wiki")
                && opts.get("--start").is_none_or(|v| *v == "example") =>
        {
            opts.insert("--start", "example");
        }
        "wiki" if !opts.contains_key("--start") => {
            require(opts.contains_key("--wiki"), "--wiki", "start")?
        }
        "other" => require(
            opts.contains_key("--start") || opts.contains_key("--wiki"),
            "--start or --wiki",
            "start",
        )?,
        _ => return Err(conflict("start")),
    }
    let db = match selected("database")? {
        "sqlite" => "sqlite",
        "postgres" => "postgres",
        "other" => opts
            .get("--database")
            .copied()
            .ok_or("resolve the database answer with --database")?,
        _ => return Err(conflict("database")),
    };
    if opts.get("--database").is_some_and(|v| *v != db) {
        return Err(conflict("database"));
    }
    if db == "postgres" {
        require(
            opts.contains_key("--database-secret-file"),
            "--database-secret-file",
            "database",
        )?;
    }
    opts.insert("--database", db);
    match selected("memory")? {
        "default"
            if memory_bytes(opts.get("--memory").copied().unwrap_or("512m"))?
                == memory_bytes("512m")? =>
        {
            ()
        }
        "other" => require(opts.contains_key("--memory"), "--memory", "memory")?,
        _ => return Err(conflict("memory")),
    }
    match selected("shelf")? {
        "default" if !opts.contains_key("--shelf") => (),
        "folder" | "other" => require(opts.contains_key("--shelf"), "--shelf", "shelf")?,
        _ => return Err(conflict("shelf")),
    }
    Ok(())
}

fn overlay(args: &[String]) -> Result<(String, Value), String> {
    let mut opts = options(args)?;
    let project = opts
        .get("--project")
        .copied()
        .ok_or("missing --project NAME")?;
    if !project
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        || !project
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_".contains(&c))
    {
        return Err("project must be a Docker Compose project name (lowercase, digits, hyphen or underscore)".into());
    }
    if let Some(path) = opts.get("--answers") {
        let choices = crate::install_journal::selections(Path::new(path), project)?;
        apply_answers(&mut opts, &choices)?;
    }
    let output = opts
        .get("--output")
        .ok_or("missing --output FILE")?
        .to_string();
    let memory = memory_bytes(opts.get("--memory").copied().unwrap_or("512m"))?;
    let mut env = json!({"LIBRARY_PROJECT":project, "LIBRARY_DB":"/data/library.sqlite3", "LIBRARY_CONFIG":"/data/library.json", "LIBRARY_ROOM_SOCKET":"/data/rooms.sock"});
    let secret_file = opts
        .get("--database-secret-file")
        .map(|s| host_path(s))
        .transpose()?;
    match (
        opts.get("--database").copied().unwrap_or("sqlite"),
        secret_file,
    ) {
        ("sqlite", None) | ("postgres", Some(_)) => (),
        ("sqlite", Some(_)) => return Err("--database-secret-file is for PostgreSQL".into()),
        ("postgres", None) => {
            env["LIBRARY_DB_URL"] =
                json!("${LIBRARY_DB_URL:?provide the approved PostgreSQL connection}")
        }
        _ => return Err("--database must be sqlite or postgres".into()),
    }
    let wiki = opts.get("--wiki").map(|s| host_path(s)).transpose()?;
    let shelf = opts.get("--shelf").map(|s| host_path(s)).transpose()?;
    if let (Some(w), Some(s)) = (wiki, shelf) {
        if w.starts_with(s) || s.starts_with(w) {
            return Err("the writable wiki and read-only shelf paths must not overlap".into());
        }
    }
    if let Some(secret) = secret_file {
        if [wiki, shelf]
            .into_iter()
            .flatten()
            .any(|dir| secret.starts_with(dir) || dir.starts_with(secret))
        {
            return Err("keep the database secret outside wiki and shelf storage".into());
        }
    }
    let example = match opts.get("--start").copied() {
        None if wiki.is_some() => false,
        None => return Err(crate::wizard::prompt("start", false)?.to_string()),
        Some("empty") if wiki.is_none() => false,
        Some("empty") => return Err("choose the empty library or an existing wiki".into()),
        Some("example") if wiki.is_none() => true,
        Some("example") => return Err("choose the example or an existing wiki".into()),
        _ => return Err("--start must be empty or example".into()),
    };
    let mut mounts = vec![];
    for (source, target, read_only) in [(wiki, "/data/wiki", false), (shelf, "/shelf", true)] {
        if let Some(host) = source {
            mounts.push(
                json!({"type":"bind", "source":host.to_string_lossy().replace('$',"$$"),
                "target":target,"read_only":read_only,"bind":{"create_host_path":false}}),
            );
        }
    }
    if wiki.is_some() {
        env["LIBRARY_WIKI_USER"] = json!(rustix::process::geteuid().as_raw().to_string());
    }
    let service = json!({"mem_limit":memory, "volumes":mounts, "environment":env});
    let mut profile = json!({"name":project,"services":{"library":service,"librarian":service,
        "room":{"mem_limit":memory,"environment":env}}});
    if example {
        profile["services"]["library"]["environment"]["LIBRARY_START"] = "example".into();
    }
    if let Some(file) = secret_file {
        profile["secrets"] =
            json!({"library_db_url":{"file":file.to_string_lossy().replace('$',"$$")}});
        for svc in ["library", "room"] {
            let service = &mut profile["services"][svc];
            service["secrets"] = json!(["library_db_url"]);
            service["environment"]["LIBRARY_DB_URL_FILE"] = "/run/secrets/library_db_url".into();
        }
    }
    Ok((output, profile))
}

const COMMON_SKILL: &[u8] = include_bytes!("../../docs/skills/benedict-library/SKILL.md");

fn install_common_skill(wiki: &Path) -> Result<(), String> {
    let directory = wiki.join("entities");
    crate::files::no_link(&directory)?;
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let page = directory.join("skill_benedict-library.md");
    crate::files::no_link(&page)?;
    match fs::symlink_metadata(&page) {
        Ok(meta) if meta.is_file() && fs::read(&page).map_err(|e| e.to_string())? == COMMON_SKILL => Ok(()),
        Ok(_) => Err("bundled skill conflicts with entities/skill_benedict-library.md; preserve and review the existing page".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound =>
            crate::files::create_wiki_page(&page, COMMON_SKILL, wiki),
        Err(e) => Err(e.to_string()),
    }
}

fn activate(data: &Path) -> Result<(), String> {
    let lock_path = data.join(".setup.lock");
    crate::files::no_link(&lock_path)?;
    let _lock = crate::files::lock(&lock_path)?;
    let state = data.join(".library-state");
    crate::files::no_link(&state)?;
    fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let registry_lock = state.join("registry.lock");
    crate::files::no_link(&registry_lock)?;
    let _registry_lock = crate::files::lock(&registry_lock)?;
    let registry = data.join("library.json");
    let wiki = data.join("wiki");
    let mut new_registry = true;
    let mut upgrade = None;
    match fs::symlink_metadata(&registry) {
        Ok(meta) if meta.is_file() => {
            let mut previous: Value =
                serde_json::from_slice(&fs::read(&registry).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("cannot read the existing registry: {e}"))?;
            let central: Vec<_> = previous["libraries"]
                .as_object()
                .ok_or("invalid existing registry")?
                .values()
                .filter(|v| v["central"] == true)
                .collect();
            if previous["version"] != 1 || central.len() != 1 || central[0]["wiki_root"] != "wiki" {
                return Err(
                    "existing registry has another configuration; use its original installation"
                        .into(),
                );
            }
            if central[0].get("shelf_root").is_none() {
                let row = previous["libraries"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .find(|v| v["central"] == true)
                    .unwrap();
                row["shelf_root"] = json!("/shelf");
                upgrade = Some(previous);
            }
            new_registry = false;
        }
        Ok(_) => return Err("existing registry is not a regular file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    if !wiki.exists() {
        if !new_registry {
            return Err("registered wiki is missing; restore its mount before continuing".into());
        }
        fs::create_dir(&wiki).map_err(|e| format!("create {}: {e}", wiki.display()))?;
    }
    if !fs::symlink_metadata(&wiki)
        .map_err(|e| e.to_string())?
        .is_dir()
    {
        return Err("wiki must be a directory".into());
    }
    // Probe using the runtime UID, then remove the temporary file without changing existing wiki content.
    fs::read_dir(&wiki).map_err(|e| format!("wiki is unreadable: {e}"))?;
    crate::files::probe_wiki(&wiki)?;
    fs::read_dir("/shelf").map_err(|e| format!("shelf is unreadable: {e}"))?;
    crate::records::initialize()?;
    if let Some(socket) = crate::rooms::socket_path() {
        crate::rooms::request(
            &socket,
            json!({"op":"ping","project":crate::records::project()?}),
        )?;
    }
    if new_registry {
        if std::env::var("LIBRARY_START").as_deref() == Ok("example") {
            crate::catalog::seed_example(&wiki)?;
        }
        let config = json!({"version":1,"libraries":{"main":{"central":true,"name":"Main library","wiki_root":"wiki","shelf_root":"/shelf"}}});
        crate::files::atomic(&registry, config.to_string().as_bytes(), true)?;
    }
    if let Some(reg) = upgrade {
        crate::files::atomic(&registry, reg.to_string().as_bytes(), false)?;
    }
    install_common_skill(&wiki)?;
    let indexed = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("index")
        .arg("--config")
        .arg(&registry)
        .output()
        .map_err(|e| format!("could not start indexer: {e}"))?;
    if !indexed.status.success() {
        return Err(format!(
            "initial wiki indexing failed: {}",
            String::from_utf8_lossy(&indexed.stderr).trim()
        ));
    }
    let index: Value = serde_json::from_slice(&indexed.stdout)
        .map_err(|e| format!("invalid index result: {e}"))?;
    println!(
        "{}",
        json!({"registry":registry,"wiki":wiki,"database_ready":true,"index":index})
    );
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args == ["--activate"] {
        return activate(Path::new("/data"));
    }
    if args == ["--help"] || args.is_empty() {
        println!(
            "setup --project NAME --output FILE [--memory 512m] [--database sqlite|postgres] [--database-secret-file /host/secret] [--answers INTERVIEW.json] [--start empty|example] [--wiki /host/wiki] [--shelf /host/shelf]\nsetup --activate"
        );
        return Ok(());
    }
    let (output, config) = overlay(args)?;
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())? + "\n";
    let path = Path::new(&output);
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.is_file() && fs::read(path).map_err(|e| e.to_string())? == text.as_bytes() =>
        {
            ()
        }
        Ok(_) => {
            return Err(format!(
                "refusing to replace existing {output}; choose a new output file"
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::files::atomic(path, text.as_bytes(), true)?
        }
        Err(e) => return Err(e.to_string()),
    }
    println!("{}", json!({"project":config["name"],"compose":output}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn isolates_mounts_and_credentials() {
        let (_, v) = overlay(&args(&[
            "--project",
            "one",
            "--output",
            "a.json",
            "--database",
            "postgres",
            "--wiki",
            "/tmp/a $b",
            "--shelf",
            "/tmp/b",
        ]))
        .unwrap();
        let svc = &v["services"]["library"];
        assert_eq!(svc["mem_limit"], 512 * 1024 * 1024);
        assert_eq!(svc["volumes"][0]["source"], "/tmp/a $$b");
        assert_eq!(svc["volumes"][1]["read_only"], true);
        assert_eq!(svc["volumes"][0]["bind"]["create_host_path"], false);
        assert_eq!(svc["environment"]["LIBRARY_PROJECT"], "one");
        assert_eq!(svc, &v["services"]["librarian"]);
        assert_eq!(svc["environment"], v["services"]["room"]["environment"]);
        assert_eq!(svc["mem_limit"], v["services"]["room"]["mem_limit"]);
    }
    #[test]
    fn profile_only_references_secret_and_seeds_the_chosen_library() {
        let (_, profile) = overlay(&args(&[
            "--project",
            "test",
            "--output",
            "profile.json",
            "--database",
            "postgres",
            "--database-secret-file",
            "/secrets/postgres",
            "--start",
            "example",
        ]))
        .unwrap();
        assert_eq!(
            profile["secrets"]["library_db_url"]["file"],
            "/secrets/postgres"
        );
        for name in ["library", "room"] {
            let service = &profile["services"][name];
            assert_eq!(service["secrets"], json!(["library_db_url"]));
            assert_eq!(
                service["environment"]["LIBRARY_DB_URL_FILE"],
                "/run/secrets/library_db_url"
            );
            assert!(service["environment"].get("LIBRARY_DB_URL").is_none());
        }
        assert_eq!(
            profile["services"]["library"]["environment"]["LIBRARY_START"],
            "example"
        );
        assert!(profile["services"]["librarian"].get("secrets").is_none());
    }

    #[test]
    fn example_can_resume_but_never_overwrites_existing_work() {
        let wiki = tempfile::tempdir().unwrap();
        crate::catalog::seed_example(wiki.path()).unwrap();
        let docs = wiki.path().join("entities");
        fs::remove_file(docs.join("shipping.md")).unwrap();
        crate::catalog::seed_example(wiki.path()).unwrap();
        assert!(docs.join("shipping.md").is_file());
        fs::write(docs.join("refund.md"), "my existing policy").unwrap();
        assert!(crate::catalog::seed_example(wiki.path()).is_err());
        assert_eq!(
            fs::read_to_string(docs.join("refund.md")).unwrap(),
            "my existing policy"
        );
    }
    #[test]
    fn recorded_answers_drive_settings_and_reject_conflicts() {
        let mut answers: BTreeMap<String, String> = [
            ("start", "example"),
            ("database", "sqlite"),
            ("memory", "default"),
            ("shelf", "default"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let mut defaults = BTreeMap::new();
        apply_answers(&mut defaults, &answers).unwrap();
        assert_eq!(defaults["--start"], "example");
        assert_eq!(defaults["--database"], "sqlite");
        for (k, v) in [
            ("--wiki", "/wiki"),
            ("--database", "postgres"),
            ("--memory", "2g"),
            ("--shelf", "/shelf"),
        ] {
            assert!(apply_answers(&mut BTreeMap::from([(k, v)]), &answers).is_err());
        }
        answers.insert("start".into(), "wiki".into());
        answers.insert("database".into(), "postgres".into());
        answers.insert("memory".into(), "other".into());
        answers.insert("shelf".into(), "folder".into());
        let paths = BTreeMap::from([
            ("--wiki", "/wiki"),
            ("--database-secret-file", "/secret"),
            ("--memory", "128m"),
            ("--shelf", "/shelf"),
        ]);
        let mut resolved = paths.clone();
        apply_answers(&mut resolved, &answers).unwrap();
        assert_eq!(resolved["--database"], "postgres");
        for key in paths.keys() {
            let mut unresolved = paths.clone();
            unresolved.remove(key);
            assert!(apply_answers(&mut unresolved, &answers).is_err());
        }
        answers.remove("memory");
        assert!(apply_answers(&mut resolved, &answers).is_err());
    }
    #[test]
    fn retains_profile_on_retry_and_rejects_conflicting_storage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile.json");
        let mut opts = args(&["--project", "a", "--output", path.to_str().unwrap()]);
        assert!(run(&opts).unwrap_err().contains("어떤 자료로 시작할까요?"));
        assert!(!path.exists());
        opts.extend(args(&["--start", "empty"]));
        run(&opts).unwrap();
        let first = fs::read(&path).unwrap();
        run(&opts).unwrap();
        opts.extend(args(&["--wiki", "/var/wikis/a"]));
        assert!(run(&opts).is_err());
        assert_eq!(fs::read(path).unwrap(), first);
    }
    #[test]
    fn rejects_overlap_duplicates_and_invalid_values() {
        for extra in [
            &["--shelf", "/wiki/x", "--wiki", "/wiki"][..],
            &["--wiki", "relative"],
            &["--database", "mysql"],
            &["--database-secret-file", "/secrets/db"],
            &[
                "--database",
                "postgres",
                "--database-secret-file",
                "/wiki/file",
                "--wiki",
                "/wiki",
            ],
            &["--start", "unknown"],
            &["--start", "example", "--wiki", "/wiki"],
            &["--memory", "2m"],
            &["--memory", "1g", "--memory", "2g"],
        ] {
            let mut all = args(&["--project", "a", "--output", "x"]);
            all.extend(args(extra));
            assert!(overlay(&all).is_err());
        }
    }
}
