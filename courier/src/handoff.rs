//! Host-initiated delivery. Room records supply context, never executable commands.
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;

type Result<T> = std::result::Result<T, String>;

fn library(entry: &str, args: &[&str]) -> Result<Value> {
    let output = Command::new(entry)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "library {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid library response: {e}"))
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn request(entry: &str, key: &str, id: &str) -> Result<Value> {
    let origin = library(entry, &["recall", "--library", key, "--id", id])?;
    field(&origin, "question")?;
    field(&origin, "project")?;
    if origin["source"] != "courier:request"
        || origin["library"] != key
        || origin["id"].as_i64().map(|v| v.to_string()).as_deref() != Some(id)
        || !matches!(
            field(&origin, "room")?,
            "search" | "reading" | "meeting" | "librarian"
        )
    {
        return Err("expected a scoped room request with source courier:request".into());
    }
    Ok(origin)
}

fn deliver(entry: &str, file: &File) -> Result<i32> {
    let receipt: Value =
        serde_json::from_reader(file).map_err(|e| format!("incomplete host receipt: {e}"))?;
    if field(&receipt, "entry")? != entry {
        return Err("use the same library entry point that received the request".into());
    }
    let origin = &receipt["request"];
    let key = field(origin, "library")?;
    let id = origin["id"]
        .as_i64()
        .ok_or("missing request ID")?
        .to_string();
    if request(entry, key, &id)? != *origin {
        return Err("destination request differs from the recorded request".into());
    }
    let answer = field(&receipt, "answer")?;
    let room = field(origin, "room")?;
    let question = field(origin, "question")?;
    let meeting = room == "meeting" && origin["query"] == "meeting/task";
    let thread = if meeting {
        format!("meeting/task/{id}")
    } else {
        format!("courier/request/{id}")
    };
    let source = format!("courier:result/{id}");
    let records = library(entry, &["recall", "--library", key, "--query", &thread])?;
    let prior = records["records"]
        .as_array()
        .ok_or("invalid result list")?
        .iter()
        .find(|r| r["room"] == room && r["source"] == source && r["answer"] == answer);
    let saved = if let Some(result) = prior {
        result.clone()
    } else if meeting {
        library(
            entry,
            &[
                "room",
                "reply",
                "--library",
                key,
                "--id",
                &id,
                "--answer",
                answer,
                "--source",
                &source,
            ],
        )?
    } else {
        library(
            entry,
            &[
                "record",
                "--library",
                key,
                "--room",
                room,
                "--question",
                question,
                "--query",
                &thread,
                "--answer",
                answer,
                "--source",
                &source,
            ],
        )?
    };
    println!(
        "{}",
        json!({"request_id":origin["id"],"result_id":saved["id"],"room":room,"exit_code":receipt["exit_code"]})
    );
    Ok(receipt["exit_code"]
        .as_i64()
        .and_then(|code| i32::try_from(code).ok())
        .ok_or("invalid exit code")?)
}

/// Reserve the operator-chosen receipt before starting external work. It survives a
/// failed delivery and an existing receipt never starts a second command.
fn handoff(entry: &str, key: &str, id: &str, path: &str, script: &str) -> Result<i32> {
    let origin = request(entry, key, id)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("reserve new receipt {path}: {e}"))?;
    file.try_lock()
        .map_err(|e| format!("receipt is in use: {e}"))?;
    let output = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--why", field(&origin, "question")?, "--", script])
        .env("COURIER_ROOM_PREVIEW", "1")
        .output()
        .map_err(|e| e.to_string())?;
    let code = output.status.code().unwrap_or(1);
    let preview = String::from_utf8_lossy(&output.stdout);
    let answer = format!("External work exit {code}\n{}", preview.trim_end());
    serde_json::to_writer(
        &mut file,
        &json!({"entry":entry,"request":origin,"answer":answer,"exit_code":code}),
    )
    .map_err(|e| e.to_string())?;
    file.sync_all()
        .and_then(|_| file.rewind())
        .map_err(|e| e.to_string())?;
    let _ = std::io::stderr().write_all(&output.stderr);
    deliver(entry, &file)
        .map_err(|e| format!("{e}; retry delivery with courier deliver ENTRY {path}"))
}

pub fn run(args: &[String]) -> Result<i32> {
    match args {
        [op, entry, path] if op == "deliver" => {
            let file = File::open(path).map_err(|e| e.to_string())?;
            file.try_lock().map_err(|e| format!("receipt is in use: {e}"))?;
            deliver(entry, &file)
        }
        [op, entry, key, id, path, sep, script] if op == "handoff" && sep == "--" && !script.trim().is_empty() => {
            handoff(entry, key, id, path, script)
        }
        _ => Err("use courier handoff ENTRY LIBRARY REQUEST_ID NEW_RECEIPT -- 'host command' or courier deliver ENTRY RECEIPT".into())
    }
}
