//! Produce an isolated Docker Compose installation and activate it inside the runtime container.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;

use std::path::{Component, Path};

/// The memory answer "default" in the setup questions (wizard: 1GiB로 시작하기).
const DEFAULT_MEMORY: &str = "1g";

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
                | "--autostart"
                | "--transcript-days"
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
            if memory_bytes(opts.get("--memory").copied().unwrap_or(DEFAULT_MEMORY))?
                == memory_bytes(DEFAULT_MEMORY)? =>
        {
            opts.insert("--memory", DEFAULT_MEMORY);
        }
        "other" => require(opts.contains_key("--memory"), "--memory", "memory")?,
        _ => return Err(conflict("memory")),
    }
    // The bundled example shelf belongs to the example start; own material connects a folder or starts with no shelf.
    let example = opts.get("--start") == Some(&"example");
    let shelf = opts.get("--shelf").copied();
    match (answers.get("shelf").map(String::as_str), shelf) {
        (None, _) if example => (),
        (None, _) => return Err(selected("shelf").unwrap_err()),
        (Some("default"), None) if example => (),
        (Some("default"), _) if !example => {
            return Err("the bundled shelf comes only with the example start; connect a folder or choose none".into())
        }
        (Some("none"), None | Some("none")) => {
            opts.insert("--shelf", "none");
        }
        (Some("folder" | "other"), Some(s)) if s != "none" => (),
        (Some("folder" | "other"), _) => require(false, "--shelf", "shelf")?,
        _ => return Err(conflict("shelf")),
    }
    // Answers recorded before this question have no autostart choice; overlay asks it then.
    match (answers.get("autostart").map(String::as_str), opts.get("--autostart").copied()) {
        (None, _) => (),
        (Some("on"), None | Some("on")) => {
            opts.insert("--autostart", "on");
        }
        (Some("off"), None | Some("off")) => {
            opts.insert("--autostart", "off");
        }
        (Some("other"), Some(_)) => (),
        (Some("other"), None) => require(false, "--autostart", "autostart")?,
        _ => return Err(conflict("autostart")),
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
    let own = opts.contains_key("--wiki");
    let example = match opts.get("--start").copied() {
        None if own => false,
        None => return Err(crate::wizard::prompt("start", false)?.to_string()),
        Some("empty") if !own => false,
        Some("empty") => return Err("choose the empty library or an existing wiki".into()),
        Some("example") if !own => true,
        Some("example") => return Err("choose the example or an existing wiki".into()),
        _ => return Err("--start must be empty or example".into()),
    };
    // Only the example keeps its one-line start; any other library asks every remaining question in order.
    if !example {
        for (option, step) in [
            ("--database", "database"),
            ("--memory", "memory"),
            ("--shelf", "shelf"),
            ("--autostart", "autostart"),
        ] {
            if !opts.contains_key(option) {
                return Err(crate::wizard::prompt(step, own)?.to_string());
            }
        }
    }
    let output = opts
        .get("--output")
        .ok_or("missing --output FILE")?
        .to_string();
    let memory = memory_bytes(opts.get("--memory").copied().unwrap_or(DEFAULT_MEMORY))?;
    // The example start keeps its one line and takes the recommended answer.
    let restart = match opts.get("--autostart").copied().unwrap_or("on") {
        "on" => "unless-stopped",
        "off" => "no",
        _ => return Err("--autostart must be on or off".into()),
    };
    let transcripts = opts.get("--transcript-days").copied().unwrap_or("keep");
    if transcripts != "keep" && !transcripts.parse::<u64>().is_ok_and(|d| d > 0) {
        return Err("--transcript-days must be keep or a number of days".into());
    }
    let mut env = json!({"LIBRARY_PROJECT":project, "LIBRARY_DB":"/data/library.sqlite3", "LIBRARY_CONFIG":"/data/library.json", "LIBRARY_ROOM_SOCKET":"/data/rooms.sock", "LIBRARY_TRANSCRIPT_DAYS":transcripts});
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
    let empty_shelf = opts.get("--shelf") == Some(&"none");
    let shelf = opts
        .get("--shelf")
        .filter(|s| **s != "none")
        .map(|s| host_path(s))
        .transpose()?;
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
    let mut mounts = vec![];
    for (source, target, read_only) in [(wiki, "/data/wiki", false), (shelf, "/shelf", true)] {
        if let Some(host) = source {
            mounts.push(
                json!({"type":"bind", "source":host.to_string_lossy().replace('$',"$$"),
                "target":target,"read_only":read_only,"bind":{"create_host_path":false}}),
            );
        }
    }
    if empty_shelf {
        // An empty named volume replaces the bundled example shelf; nocopy keeps image files out of it.
        mounts.push(json!({"type":"volume","source":"empty_shelf","target":"/shelf",
            "read_only":true,"volume":{"nocopy":true}}));
    }
    if wiki.is_some() {
        env["LIBRARY_WIKI_USER"] = json!(rustix::process::geteuid().as_raw().to_string());
    }
    let service = json!({"mem_limit":memory, "volumes":mounts, "environment":env});
    let mut profile = json!({"name":project,"services":{"library":service,"librarian":service,
        "room":service}});
    if example {
        profile["services"]["library"]["environment"]["LIBRARY_START"] = "example".into();
    } else {
        // The bundled golden questions fit only the example; other libraries grow their own.
        profile["services"]["librarian"]["environment"]["LIBRARY_GOLDEN"] = "".into();
    }
    // Only the long-running services restart; the library service runs one command and exits.
    for svc in ["room", "librarian"] {
        profile["services"][svc]["restart"] = restart.into();
    }
    if empty_shelf {
        profile["volumes"] = json!({"empty_shelf":{}});
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
// SHA-256 of the bundled skill page as earlier releases wrote it. An upgrade replaces
// an untouched earlier page; a page someone edited is left alone.
const PREVIOUS_SKILLS: &[&str] = &[
    "8bdb125594bb8ab8dd90381824ce5b6e96d02d4f3f84087bb84ecdc6dc3dacd9",
    "48d763b1b48362b3b706ee865dd3c73eb7bcfe1e8490e73fddf0d47590a619ce",
];
const EDITED_SKILL: &str = "entities/skill_benedict-library.md was edited after installation; \
    move your version to another name (for example entities/skill_benedict-library.local.md) \
    and run the installer again";

fn install_common_skill(wiki: &Path) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let directory = wiki.join("entities");
    crate::files::no_link(&directory)?;
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let page = directory.join("skill_benedict-library.md");
    crate::files::no_link(&page)?;
    match fs::symlink_metadata(&page) {
        Ok(meta) if meta.is_file() => {
            let current = fs::read(&page).map_err(|e| e.to_string())?;
            if current == COMMON_SKILL {
                Ok(())
            } else if PREVIOUS_SKILLS.contains(&format!("{:x}", Sha256::digest(&current)).as_str()) {
                crate::files::wiki_page(&page, COMMON_SKILL, wiki)
            } else {
                Err(EDITED_SKILL.into())
            }
        }
        Ok(_) => Err(EDITED_SKILL.into()),
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

/// Memory limit in MiB for a wiki of this size, and whether it fits in two thirds of Docker's memory.
/// Indexing reads one document at a time and the search room holds no document text. Measured: a
/// wiki of 42,000 files (540 MB, 31,162 indexed documents) indexed within 1 GiB at a peak of 0.82 GB
/// with the page cache, and its search room held 0.2 GB. Beyond 50,000 pages or 700 MB, each further
/// 50,000 pages adds 1 GiB. The installer retries with a larger limit if initial indexing still runs
/// out of memory.
fn recommend_memory(pages: u64, bytes: u64, docker: u64) -> (u64, bool) {
    const MIB: u64 = 1024 * 1024;
    let mb = bytes / MIB;
    let need = if pages <= 50_000 && mb <= 700 {
        1024
    } else {
        1024 + 1024 * pages.saturating_sub(50_000).div_ceil(50_000).max(1)
    };
    let cap = docker / MIB / 3 * 2;
    if docker == 0 || need <= cap {
        (need, true)
    } else {
        (cap.max(512), false)
    }
}

fn recommend(args: &[String]) -> Result<(), String> {
    let mut values = BTreeMap::new();
    for pair in args.chunks(2) {
        match (pair[0].as_str(), pair.get(1).and_then(|v| v.parse::<u64>().ok())) {
            (key @ ("--pages" | "--bytes" | "--docker-memory"), Some(v)) => {
                values.insert(key, v);
            }
            _ => return Err("setup --recommend-memory --pages N --bytes N --docker-memory N".into()),
        }
    }
    let get = |k| values.get(k).copied().unwrap_or(0);
    let (mib, fits) = recommend_memory(get("--pages"), get("--bytes"), get("--docker-memory"));
    let memory = if mib % 1024 == 0 {
        format!("{}g", mib / 1024)
    } else {
        format!("{mib}m")
    };
    println!("{}", json!({"memory": memory, "fits": fits}));
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args == ["--activate"] {
        return activate(Path::new("/data"));
    }
    if args.first().map(String::as_str) == Some("--recommend-memory") {
        return recommend(&args[1..]);
    }
    if args == ["--help"] || args.is_empty() {
        println!(
            "setup --project NAME --output FILE [--memory 1g] [--database sqlite|postgres] [--database-secret-file /host/secret] [--answers INTERVIEW.json] [--start empty|example] [--wiki /host/wiki] [--shelf /host/shelf] [--autostart on|off]\nsetup --activate\nsetup --recommend-memory --pages N --bytes N --docker-memory N"
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
    fn recommends_memory_from_measured_wiki_sizes() {
        const MIB: u64 = 1024 * 1024;
        let docker = 32 * 1024 * MIB;
        assert_eq!(recommend_memory(5, MIB, docker), (1024, true));
        assert_eq!(recommend_memory(2_285, 35 * MIB, docker), (1024, true));
        assert_eq!(recommend_memory(21_706, 135 * MIB, docker), (1024, true));
        assert_eq!(recommend_memory(42_064, 540 * MIB, docker), (1024, true));
        assert_eq!(recommend_memory(90_000, 900 * MIB, docker), (2048, true));
        assert_eq!(recommend_memory(160_000, 2_000 * MIB, docker), (4096, true));
        // A Docker VM of 1.5 GiB leaves 1 GiB for the library.
        assert_eq!(recommend_memory(90_000, 900 * MIB, 1536 * MIB), (1024, false));
        assert!(recommend(&args(&["--pages", "x"])).is_err());
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
            "--memory",
            "512m",
            "--autostart",
            "on",
        ]))
        .unwrap();
        let svc = &v["services"]["library"];
        assert_eq!(svc["mem_limit"], 512 * 1024 * 1024);
        assert_eq!(svc["volumes"][0]["source"], "/tmp/a $$b");
        assert_eq!(svc["volumes"][1]["read_only"], true);
        assert_eq!(svc["volumes"][0]["bind"]["create_host_path"], false);
        assert_eq!(svc["environment"]["LIBRARY_PROJECT"], "one");
        assert!(svc.get("restart").is_none());
        // Entry point commands run in the room container, so it opens the same wiki and shelf.
        for other in ["librarian", "room"] {
            let mut same = v["services"][other].clone();
            let restart = same.as_object_mut().unwrap().remove("restart");
            assert_eq!(restart, Some(json!("unless-stopped")));
            if other == "librarian" {
                // Only the example start uses the bundled golden questions.
                let golden = same["environment"].as_object_mut().unwrap().remove("LIBRARY_GOLDEN");
                assert_eq!(golden, Some(json!("")));
            }
            assert_eq!(svc, &same);
        }
    }
    #[test]
    fn an_untouched_earlier_skill_page_is_replaced_and_an_edited_one_is_kept() {
        let wiki = tempfile::tempdir().unwrap();
        let page = wiki.path().join("entities/skill_benedict-library.md");
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::write(&page, include_bytes!("../tests/fixtures/skill-0.2.0.md")).unwrap();
        install_common_skill(wiki.path()).unwrap();
        assert_eq!(fs::read(&page).unwrap(), COMMON_SKILL);
        fs::write(&page, b"my own notes").unwrap();
        let err = install_common_skill(wiki.path()).unwrap_err();
        assert!(err.contains("edited after installation"), "{err}");
        assert_eq!(fs::read(&page).unwrap(), b"my own notes");
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
        opts.extend(args(&["--start", "empty", "--database", "sqlite", "--memory", "512m", "--shelf", "none", "--autostart", "off"]));
        run(&opts).unwrap();
        let first = fs::read(&path).unwrap();
        run(&opts).unwrap();
        opts.extend(args(&["--wiki", "/var/wikis/a"]));
        assert!(run(&opts).is_err());
        assert_eq!(fs::read(path).unwrap(), first);
    }
    fn base(extra: &[&str]) -> Vec<String> {
        let mut all = args(&["--project", "a", "--output", "x"]);
        all.extend(args(extra));
        all
    }
    fn shelf_mount(profile: &Value, service: &str) -> Option<Value> {
        profile["services"][service]["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["target"] == "/shelf")
            .cloned()
    }
    #[test]
    fn own_material_install_asks_every_remaining_question_in_order() {
        let wiki = ["--wiki", "/srv/wiki"];
        let cases: Vec<(&[&str], &str)> = vec![
            (&[], "각 실의 기록을 어디에 저장할까요?"),
            (&["--database", "sqlite"], "컨테이너 하나가 쓸 메모리 상한을 어떻게 정할까요?"),
            (&["--database", "sqlite", "--memory", "512m"], "서가 원본은 어디에서 읽을까요?"),
            (&["--memory", "512m", "--shelf", "none"], "각 실의 기록을 어디에 저장할까요?"),
            (&["--database", "sqlite", "--memory", "512m", "--shelf", "none"], "서버를 다시 켜면 도서관도 저절로 켤까요?"),
        ];
        for (given, question) in cases {
            let mut all = wiki.to_vec();
            all.extend_from_slice(given);
            let error = overlay(&base(&all)).unwrap_err();
            assert!(error.contains(question), "{given:?}: {error}");
        }
        let empty = overlay(&base(&["--start", "empty", "--database", "sqlite", "--memory", "512m"])).unwrap_err();
        assert!(empty.contains("서가 원본은 어디에서 읽을까요?"), "{empty}");
    }
    #[test]
    fn own_material_never_receives_the_example_shelf() {
        let answered = ["--wiki", "/srv/wiki", "--database", "sqlite", "--memory", "512m", "--autostart", "on"];
        let mut folder = answered.to_vec();
        folder.extend(["--shelf", "/srv/originals"]);
        let (_, v) = overlay(&base(&folder)).unwrap();
        for service in ["library", "librarian", "room"] {
            let mount = shelf_mount(&v, service).expect("shelf mount");
            assert_eq!(mount["source"], "/srv/originals");
            assert_eq!(mount["read_only"], true);
        }
        let mut none = answered.to_vec();
        none.extend(["--shelf", "none"]);
        let (_, v) = overlay(&base(&none)).unwrap();
        for service in ["library", "librarian", "room"] {
            let mount = shelf_mount(&v, service).expect("empty shelf mount");
            assert_eq!(mount["type"], "volume");
            assert_eq!(mount["source"], "empty_shelf");
            assert_eq!(mount["read_only"], true);
            assert_eq!(mount["volume"]["nocopy"], true);
        }
        assert!(v["volumes"].get("empty_shelf").is_some());
        let mut both = none.clone();
        both.extend(["--start", "example"]);
        assert!(overlay(&base(&both)).is_err());
    }
    #[test]
    fn the_example_keeps_its_one_line_start_and_its_own_shelf() {
        let (_, v) = overlay(&base(&["--start", "example"])).unwrap();
        assert_eq!(v["services"]["library"]["environment"]["LIBRARY_START"], "example");
        assert!(shelf_mount(&v, "room").is_none(), "the bundled shelf comes from compose");
        let (_, v) = overlay(&base(&["--start", "example", "--shelf", "/srv/originals"])).unwrap();
        assert_eq!(shelf_mount(&v, "room").unwrap()["source"], "/srv/originals");
    }
    #[test]
    fn recorded_shelf_answers_follow_the_start_choice() {
        let answers = |start: &str, shelf: Option<&str>| -> BTreeMap<String, String> {
            let mut a: BTreeMap<String, String> = [("start", start), ("database", "sqlite"), ("memory", "default")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            if let Some(s) = shelf {
                a.insert("shelf".into(), s.into());
            }
            a
        };
        let wiki = || BTreeMap::from([("--wiki", "/srv/wiki")]);
        let error = apply_answers(&mut wiki(), &answers("wiki", Some("default"))).unwrap_err();
        assert!(error.contains("example"), "{error}");
        assert!(apply_answers(&mut wiki(), &answers("wiki", None)).is_err());
        let mut none = wiki();
        apply_answers(&mut none, &answers("wiki", Some("none"))).unwrap();
        assert_eq!(none["--shelf"], "none");
        assert_eq!(none["--memory"], "1g");
        let mut conflict = BTreeMap::from([("--wiki", "/srv/wiki"), ("--shelf", "/srv/x")]);
        assert!(apply_answers(&mut conflict, &answers("wiki", Some("none"))).is_err());
        let mut example = BTreeMap::new();
        apply_answers(&mut example, &answers("example", None)).unwrap();
        assert!(!example.contains_key("--shelf"));
        apply_answers(&mut BTreeMap::new(), &answers("example", Some("default"))).unwrap();
    }
    #[test]
    fn autostart_answer_sets_restart_for_long_running_services() {
        let own = ["--wiki", "/srv/wiki", "--database", "sqlite", "--memory", "512m", "--shelf", "none"];
        for (answer, policy) in [("on", "unless-stopped"), ("off", "no")] {
            let mut all = own.to_vec();
            all.extend(["--autostart", answer]);
            let (_, v) = overlay(&base(&all)).unwrap();
            assert_eq!(v["services"]["room"]["restart"], policy);
            assert_eq!(v["services"]["librarian"]["restart"], policy);
            assert!(v["services"]["library"].get("restart").is_none());
        }
        let (_, v) = overlay(&base(&["--start", "example"])).unwrap();
        assert_eq!(v["services"]["room"]["restart"], "unless-stopped");
        let answers = |choice: &str| -> BTreeMap<String, String> {
            [("start", "wiki"), ("database", "sqlite"), ("memory", "default"), ("shelf", "none"), ("autostart", choice)]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect()
        };
        let mut opts = BTreeMap::from([("--wiki", "/srv/wiki")]);
        apply_answers(&mut opts, &answers("off")).unwrap();
        assert_eq!(opts["--autostart"], "off");
        let mut conflict = BTreeMap::from([("--wiki", "/srv/wiki"), ("--autostart", "on")]);
        assert!(apply_answers(&mut conflict, &answers("off")).is_err());
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
            &["--start", "example", "--autostart", "maybe"],
        ] {
            let mut all = args(&["--project", "a", "--output", "x"]);
            all.extend(args(extra));
            assert!(overlay(&all).is_err());
        }
    }
}
