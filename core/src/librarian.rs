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

fn cycle(config: &Path, key: &str, golden: Option<&str>, backup_dir: Option<&str>) -> (Value, i32) {
    let mut row = json!({"library":key,"started":Utc::now().to_rfc3339_opts(SecondsFormat::Micros,false),"steps":{}});
    let work = (|| -> Result<i32> {
        row["steps"]["index"] = local::index(config, key)?;
        let code = if let Some(golden) = golden {
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
        } else {
            row["steps"]["golden"] = json!({"status":"not_configured"});
            1
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
