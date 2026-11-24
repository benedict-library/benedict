//! Native maintenance for a file-backed wiki and shared Rust room service.
use crate::{backup, evaluation, local, option, records};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::Path,
    time::Duration,
};

type Result<T> = std::result::Result<T, String>;

/// Remove transcript day folders older than `LIBRARY_TRANSCRIPT_DAYS` (default keep).
fn transcripts() -> Result<Value> {
    let retention = std::env::var("LIBRARY_TRANSCRIPT_DAYS").unwrap_or_default();
    let days = match retention.trim() {
        "" | "keep" => None,
        n => Some(
            n.parse::<u64>()
                .map_err(|_| "LIBRARY_TRANSCRIPT_DAYS must be a number of days or keep")?,
        ),
    };
    let dir = crate::rooms::transcripts_dir();
    let mut removed = vec![];
    let mut folders = 0;
    if dir.is_dir() {
        let oldest = days.map(crate::rooms::day_folder);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| format!("transcripts: {e}"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.len() == 6 && n.chars().all(|c| c.is_ascii_digit()))
            .collect();
        names.sort();
        for name in names {
            // Day folders are YYMMDD, so string order is date order.
            if oldest.as_ref().is_some_and(|o| name < *o) {
                std::fs::remove_dir_all(dir.join(&name)).map_err(|e| format!("transcripts: {e}"))?;
                removed.push(name);
            } else {
                folders += 1;
            }
        }
    }
    Ok(json!({"retention":days.map_or(json!("keep"), |d| json!(d)),"folders":folders,"removed":removed}))
}

/// The golden file is optional. Missing means skip; a bad line is reported, not fatal.
fn golden_check(path: &Path) -> Option<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return Some(json!({"status":"not_configured","golden":path})),
    };
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let ok = serde_json::from_str::<Value>(line).ok().is_some_and(|v| {
            v["q"].as_str().is_some_and(|q| !q.trim().is_empty())
                && (v["answer_doc"].is_string() || v["answer_doc"].is_array())
        });
        if !ok {
            return Some(json!({"status":"invalid","golden":path,"line":i + 1}));
        }
    }
    None
}

fn cycle(config: &Path, key: &str, golden: Option<&str>, backup_dir: Option<&str>) -> (Value, i32) {
    let mut row = json!({"library":key,"started":Utc::now().to_rfc3339_opts(SecondsFormat::Micros,false),"steps":{}});
    // A golden file the operator names must exist; the default one is optional.
    let named = golden.map(str::to_owned).or_else(|| {
        std::env::var("LIBRARY_GOLDEN")
            .ok()
            .filter(|g| !g.trim().is_empty())
    });
    let default_golden = crate::rooms::data_dir()
        .join("golden")
        .join(format!("{key}.jsonl"))
        .to_string_lossy()
        .into_owned();
    let golden = named.as_deref().unwrap_or(&default_golden);
    let work = (|| -> Result<i32> {
        row["steps"]["index"] = local::index(config, key)?;
        row["steps"]["transcripts"] = transcripts()?;
        if records::enabled() {
            row["steps"]["records"] = json!({"unfiled":records::unfiled(key)?});
        }
        let skip = if named.is_some() {
            None
        } else {
            golden_check(Path::new(golden))
        };
        let code = if let Some(status) = skip {
            row["steps"]["golden"] = status;
            1
        } else {
            let (report, code) =
                evaluation::run(config, Some(key), &["--golden".into(), golden.into()])?
                    .ok_or("configured wiki requires its policy-aware evaluator")?;
            // Preserve the scores and the questions that missed so an agent can
            // repair them. Don't duplicate a successful golden corpus on each tick.
            let misses = report["rows"]
                .as_array()
                .ok_or("incomplete evaluation report")?
                .iter()
                .filter(|r| {
                    r["rank"]
                        .as_u64()
                        .is_none_or(|v| v > report["summary"]["k"].as_u64().unwrap_or(0))
                })
                .collect::<Vec<_>>();
            row["steps"]["golden"] =
                json!({"exit":code,"golden":golden,"summary":report["summary"],"misses":misses});
            code
        };
        if let Some(dir) = backup_dir {
            let name = Utc::now().format("%Y%m%dT%H%M%S%6fZ").to_string();
            row["steps"]["backup"] = backup::snapshot(
                config,
                Some(key),
                &Path::new(dir).join(format!("{key}-{name}.tar")),
            )?;
        }
        Ok(code)
    })();
    let code = match work {
        Ok(0) => {
            row["status"] = json!("ok");
            0
        }
        Ok(_) => {
            row["status"] = json!("warning");
            1
        }
        Err(error) => {
            row["status"] = json!("failed");
            row["error"] = json!(error);
            2
        }
    };
    row["ended"] = json!(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false));
    let evidence = json!({"registry":config,"golden":golden,"backup_dir":backup_dir}).to_string();
    match records::store(
        key,
        "librarian",
        "",
        "librarian/cycle",
        &row.to_string(),
        &evidence,
    ) {
        Ok(saved) => {
            row["record_id"] = saved["id"].clone();
            (row, code)
        }
        Err(error) => {
            row["status"] = json!("failed");
            row["journal_error"] = json!(error);
            (row, 2)
        }
    }
}

pub fn run(config: &Path, key: Option<&str>, args: &[String]) -> Result<Option<i32>> {
    if !local::simple(config)?
        || args.iter().any(|v| {
            matches!(
                v.as_str(),
                "--clean" | "--reap" | "--restart" | "--dry-run" | "--dump-dir" | "--prefix"
            )
        })
    {
        return Ok(None);
    }
    let mut flags = args.to_owned();
    let golden = option(&mut flags, "--golden")?;
    let backup_dir = option(&mut flags, "--backup-dir")?;
    let iterations: usize = option(&mut flags, "--iterations")?
        .unwrap_or_else(|| "1".into())
        .parse()
        .map_err(|_| "invalid iteration count")?;
    let interval: f64 = option(&mut flags, "--interval")?
        .unwrap_or_else(|| "86400".into())
        .parse()
        .map_err(|_| "invalid interval")?;
    let wait = Duration::try_from_secs_f64(interval).map_err(|_| "invalid interval")?;
    if iterations == 0 || wait.is_zero() || !flags.is_empty() {
        return Err(
            "expected positive iterations and interval, and recognized librarian options".into(),
        );
    }
    let key = local::selection(config, key)?.0;
    let mut outcome = 0;
    for i in 0..iterations {
        let (row, code) = cycle(config, &key, golden.as_deref(), backup_dir.as_deref());
        let mut out = io::stdout().lock();
        writeln!(out, "{row}")
            .and_then(|_| out.flush())
            .map_err(|e| e.to_string())?;
        outcome = outcome.max(code);
        if code == 2 || i == iterations - 1 {
            break;
        }
        std::thread::sleep(wait)
    }
    Ok(Some(outcome))
}
