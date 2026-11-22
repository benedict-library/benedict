mod backup;
mod catalog;
mod evaluation;
mod files;
mod import;
mod install_journal;
mod librarian;
mod local;
mod promote;
mod records;
mod rooms;
mod setup;
mod shelf;
mod web_source;
mod wizard;
mod work;

use serde_json::json;
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const HELP: &str = "Benedict — a library for agents and personas

  setup        configure Docker storage, database and memory limits
  init | add   create a central or project library
  index        rebuild the wiki search index
  search       search a library; --question TEXT preserves the user's exact words
  read         read a wiki page or shelf:PATH (original UTF-8 text and SHA-256)
  file         promote an answered room record into the wiki with --id ID
  save         file a question, answer and --refs; --query TEXT keeps the search terms
  work         open, extend or resume a reading-room record
  record       retain a room observation
  recall       retrieve retained evidence
  librarian    refresh the index and journal a maintenance cycle
  evaluate     check search results against known sources
  backup       snapshot or restore the wiki
  room         share tasks, replies and live room observations
  method       read the library operating guide
  import | web-source   import a skill, wiki page or public source
  version      print the installed Benedict version (see CHANGELOG.md)

See docs/docker-setup.md for installation.";

fn usage(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "init" => "init [--config PATH] [--key KEY] [--name NAME] [--demo]",
        "add" => "add [--config PATH] --key KEY [--name NAME]",
        "index" => "index [--library KEY] [--config PATH]",
        "method" => "method [--lang en|ko]",
        "search" => "search [--config PATH] [--library KEY] [--top N] [--question ORIGINAL] QUERY",
        "read" => {
            "read [--config PATH] [--library KEY] [--question ORIGINAL] WIKI_PAGE.md|shelf:RELATIVE_PATH"
        }
        "save" => {
            "save [--config PATH] [--library KEY] [--query SEARCH_TERMS] [--refs SOURCES] QUESTION ANSWER"
        }
        "file" => "file [--config PATH] [--library KEY] --id RECORD [--as filing|concept]",
        "recall" => "recall [--library KEY] (--id ID | --question ORIGINAL | --query SEARCH_TERMS)",
        "record" => {
            "record --room ROOM [--library KEY] [--question ORIGINAL] [--query SEARCH_TERMS] [--answer TEXT] [--source SOURCE]"
        }
        "work" => {
            "work open QUESTION INITIAL | work note ID JUDGMENT [SOURCE] | work show ID (optional --library KEY)"
        }
        "room" => {
            "room (serve | ping | memory --room ROOM | task --question ORIGINAL --source SOURCE | open --id ID | reply --id ID --answer TEXT [--source SOURCE] | dispatch --room ROOM --question ORIGINAL | responses --id ID)"
        }
        "backup" => {
            "backup snapshot --out ARCHIVE [--config PATH] [--library KEY] | backup restore --archive ARCHIVE --target NEW_DIRECTORY"
        }
        "librarian" => {
            "librarian [--config PATH] [--library KEY] [--golden FILE] [--iterations N] [--interval SECONDS] [--backup-dir DIR]"
        }
        "evaluate" => {
            "evaluate --golden FILE [--config PATH] [--library KEY] [--k N] [--answers FILE] [--out FILE]"
        }
        "import" => {
            "import --library KEY [--config PATH] [--approve CONFIRMATION] (wiki | skill) SOURCE"
        }
        "web-source" => {
            "web-source [--config PATH] [--library KEY] [--question ORIGINAL] PUBLIC_QUERY SOURCE_URL"
        }
        _ => return None,
    })
}

fn option(args: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let end = args.iter().position(|v| v == "--").unwrap_or(args.len());
    let prefix = format!("{name}=");
    let Some(i) = args[..end]
        .iter()
        .position(|v| v == name || v.starts_with(&prefix))
    else {
        return Ok(None);
    };
    let flag = args.remove(i);
    if let Some(value) = flag.strip_prefix(&prefix) {
        return Ok(Some(value.into()));
    }
    if i == args.len() {
        return Err(format!("missing value for {name}"));
    }
    Ok(Some(args.remove(i)))
}

fn app() -> PathBuf {
    env::var_os("BENEDICT_APP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/app"))
}

fn legacy(cmd: &str, args: &[String]) -> Result<i32, String> {
    let dir = app();
    let mut adapter = Command::new("python3");
    adapter.env_remove("BENEDICT_CLI");
    if matches!(cmd, "evaluate" | "librarian") {
        adapter.arg(dir.join(format!("bin/{cmd}.py")));
    } else {
        adapter.arg(dir.join("bin/library.py")).arg(cmd);
    }
    let status = adapter
        .args(args)
        .status()
        .map_err(|e| format!("compatibility adapter {cmd}: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

fn run() -> Result<i32, String> {
    let mut it = env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| "--help".into());
    let mut args: Vec<_> = it.collect();
    if args
        .iter()
        .take_while(|s| s.as_str() != "--")
        .any(|s| s == "--help" || s == "-h")
        && let Some(description) = usage(&cmd)
    {
        println!("benedict {description}");
        return Ok(0);
    }
    match cmd.as_str() {
        "--help" | "-h" => {
            println!("{HELP}");
            return Ok(0);
        }
        "version" | "--version" | "-V" => {
            println!("{}", json!({"version": env!("CARGO_PKG_VERSION")}));
            return Ok(0);
        }
        "backup" => {
            backup::run(&args)?;
            return Ok(0);
        }
        "setup" => {
            match args.split_first() {
                Some((sub, rest)) if sub == "question" => wizard::show(rest)?,
                Some((sub, rest)) if sub == "journal" => install_journal::run(rest)?,
                _ => setup::run(&args)?,
            }
            return Ok(0);
        }
        "work" => {
            work::run(&args)?;
            return Ok(0);
        }
        "room" => {
            rooms::run(&args)?;
            return Ok(0);
        }
        "record" | "recall" => {
            records::run(&cmd, &args)?;
            return Ok(0);
        }
        _ => {}
    }
    if usage(&cmd).is_none() {
        return Err(format!("unknown command: {cmd}; run benedict --help"));
    }
    let default_config = env::var("LIBRARY_CONFIG").unwrap_or_else(|_| "library.json".into());
    let configured = args
        .iter()
        .take_while(|v| v.as_str() != "--")
        .any(|v| v == "--config" || v.starts_with("--config="));
    if !configured && cmd != "method" {
        args.splice(0..0, ["--config".into(), default_config.clone()]);
    }
    if let Some(code) = local::run(&cmd, &args)? {
        return Ok(code);
    }
    if matches!(
        cmd.as_str(),
        "file" | "save" | "add" | "init" | "import" | "evaluate" | "web-source" | "librarian"
    ) {
        let mut positional = args.clone();
        let config = option(&mut positional, "--config")?.unwrap_or(default_config);
        let library = option(&mut positional, "--library")?;
        let (config, key) = (Path::new(&config), library.as_deref());
        let result = match cmd.as_str() {
            "librarian" => {
                if let Some(code) = librarian::run(config, key, &positional)? {
                    return Ok(code);
                }
                None
            }
            "web-source" => Some((web_source::run(config, key, &positional)?, 0)),
            "evaluate" => evaluation::run(config, key, &positional)?,
            "file" => Some((promote::run(config, key, &positional)?, 0)),
            "import" => import::run(config, key.ok_or("import requires --library")?, &positional)?
                .map(|v| (v, 0)),
            _ => catalog::run(&cmd, config, key, &positional)?.map(|v| (v, 0)),
        };
        if let Some((value, code)) = result {
            println!("{value}");
            return Ok(code);
        }
    }
    // Preserve explicit user evidence when advanced policy requires a legacy handler.
    let evidence_flag = match cmd.as_str() {
        "read" | "search" => Some("--question"),
        "save" => Some("--query"),
        _ => None,
    };
    if let Some(flag) = evidence_flag
        && option(&mut args, flag)?.is_some()
    {
        return Err(format!(
            "{flag} capture requires a native registry; use record with this compatibility adapter"
        ));
    }
    legacy(&cmd, &args)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("{}", json!({"error": error}));
            ExitCode::from(2)
        }
    }
}
