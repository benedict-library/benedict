//! A project-local Unix socket serves a live memory window over durable room
//! events. All four rooms use the same write-through path. The database keeps
//! the complete history even after the window is evicted or the process exits.
use crate::records;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Bound the whole resident event window, across all libraries and rooms. Each
// event is stored as one JSON byte buffer rather than a second nested tree.
const CACHE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_LIMIT: u64 = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) fn socket_path() -> Option<PathBuf> {
    env::var_os("LIBRARY_ROOM_SOCKET").map(PathBuf::from)
}

fn packet(stream: &UnixStream, limit: u64) -> Result<Value> {
    let mut data = Vec::new();
    BufReader::new(stream)
        .take(limit + 1)
        .read_until(b'\n', &mut data)
        .map_err(err)?;
    if data.len() as u64 > limit {
        return Err("room packet exceeds size limit".into());
    }
    if !data.ends_with(b"\n") {
        return Err("incomplete room packet".into());
    }
    serde_json::from_slice(&data).map_err(err)
}

fn write_packet(stream: &mut UnixStream, v: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stream, v).map_err(err)?;
    stream.write_all(b"\n").map_err(err)
}

pub(crate) fn request(path: &Path, message: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(path)
        .map_err(|e| format!("connect shared room {}: {e}", path.display()))?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(err)?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(err)?;
    write_packet(&mut stream, &message)?;
    let mut response = packet(&stream, REQUEST_LIMIT * 2)?;
    if let Some(reason) = response["error"].as_str() {
        return Err(reason.to_string());
    }
    response
        .get_mut("result")
        .map(Value::take)
        .ok_or_else(|| "invalid room response".into())
}

#[derive(Default)]
struct LiveMemory {
    events: VecDeque<(String, String, Vec<u8>)>,
    bytes: usize,
}

impl LiveMemory {
    fn remember(&mut self, library: &str, room: &str, event: &Value) -> Result<bool> {
        let event = serde_json::to_vec(event).map_err(err)?;
        let size = event.len()
            + library.len()
            + room.len()
            + std::mem::size_of::<(String, String, Vec<u8>)>();
        if size > CACHE_BYTES {
            return Ok(false);
        }
        while self.bytes + size > CACHE_BYTES {
            if let Some((key, name, data)) = self.events.pop_front() {
                self.bytes -= data.len()
                    + key.len()
                    + name.len()
                    + std::mem::size_of::<(String, String, Vec<u8>)>();
            }
        }
        self.bytes += size;
        self.events.push_back((library.into(), room.into(), event));
        Ok(true)
    }
    fn list(&self, library: &str, room: &str) -> Result<Vec<Value>> {
        self.events
            .iter()
            .filter(|(l, r, _)| l == library && r == room)
            .map(|(_, _, data)| serde_json::from_slice(data).map_err(err))
            .collect()
    }

    fn handle(&mut self, project: &str, msg: Value) -> Result<Value> {
        let text = |key: &str| msg[key].as_str().ok_or_else(|| format!("missing {key}"));
        if text("project")? != project {
            return Err("room belongs to another project".into());
        }
        match text("op")? {
            "ping" => Ok(json!({"project":project,"ready":true,"version":env!("CARGO_PKG_VERSION")})),
            "store" => {
                let (library, room) = (text("library")?, text("room")?);
                let mut saved = records::persist(
                    library,
                    room,
                    text("question")?,
                    text("query")?,
                    text("answer")?,
                    text("source")?,
                )?;
                if !self.remember(library, room, &saved)? {
                    // Oversize evidence stays retrievable by database ID.
                    saved["memory"] = json!("exceeds room window");
                }
                Ok(saved)
            }
            "memory" => Ok(
                json!({"project":project,"library":text("library")?,"room":text("room")?,
                                "records":self.list(text("library")?,text("room")?)?}),
            ),
            _ => Err("unknown room operation".into()),
        }
    }
}

fn serve(path: &Path) -> Result<()> {
    let project = records::project()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(err)?;
    let mut lock_name = path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_name)
        .map_err(err)?;
    lock.try_lock()
        .map_err(|_| "room service already running".to_string())?;
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err("room socket path is occupied by another file".into());
        }
        if UnixStream::connect(path).is_ok() {
            return Err("room socket is in use".into());
        }
        fs::remove_file(path).map_err(err)?;
    }
    let listener = UnixListener::bind(path).map_err(err)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(err)?;
    let mut memory = LiveMemory::default();
    for client in listener.incoming() {
        let mut stream = client.map_err(err)?;
        stream.set_read_timeout(Some(TIMEOUT)).map_err(err)?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(err)?;
        let res = match packet(&stream, REQUEST_LIMIT)
            .and_then(|message| memory.handle(&project, message))
        {
            Ok(value) => json!({"result":value}),
            Err(error) => json!({"error":error}),
        };
        // A client disconnect after a committed write must not stop other rooms.
        let _ = write_packet(&mut stream, &res);
    }
    Ok(())
}

fn task(library: &str, id: &str) -> Result<Value> {
    let entry = records::recall(library, "id", id)?;
    if entry["room"] != "meeting" || entry["query"] != "meeting/task" {
        return Err("not a meeting task".into());
    }
    Ok(entry)
}

pub fn run(args: &[String]) -> Result<()> {
    let (cmd, args) = args
        .split_first()
        .ok_or("expected room serve, ping, memory, task, open, reply, dispatch or responses")?;
    let flags = match cmd.as_str() {
        "serve" | "ping" => &[][..],
        "memory" => &["room", "library"][..],
        "dispatch" => &["room", "question", "library"][..],
        "task" => &["question", "source", "library"][..],
        "open" | "responses" => &["id", "library"][..],
        "reply" => &["id", "answer", "source", "library"][..],
        _ => return Err("unknown room command".into()),
    };
    let fields = records::options(args, flags)?;
    let required = |key: &str| {
        fields
            .get(key)
            .copied()
            .ok_or_else(|| format!("missing --{key}"))
    };
    let library = fields.get("library").copied().unwrap_or("main");
    let socket = || socket_path().ok_or("set LIBRARY_ROOM_SOCKET for the shared room service");
    let result = match cmd.as_str() {
        "serve" => return serve(&socket()?),
        "ping" => request(
            &socket()?,
            json!({"op":"ping","project":records::project()?}),
        )?,
        "memory" => request(
            &socket()?,
            json!({"op":"memory","project":records::project()?,"library":library,"room":required("room")?}),
        )?,
        "dispatch" => {
            let room = required("room")?;
            let question = required("question")?;
            if !matches!(room, "reading" | "search" | "meeting" | "librarian")
                || question.trim().is_empty()
            {
                return Err(
                    "dispatch requires one of the four rooms and a nonempty question".into(),
                );
            }
            let thread = if room == "meeting" {
                "meeting/task"
            } else {
                ""
            };
            records::store(library, room, question, thread, "", "courier:request")?
        }
        "responses" => {
            let origin = records::recall(library, "id", required("id")?)?;
            if origin["source"] != "courier:request" {
                return Err("expected an external work request".into());
            }
            let id = &origin["id"];
            let thread = if origin["room"] == "meeting" && origin["query"] == "meeting/task" {
                format!("meeting/task/{id}")
            } else {
                format!("courier/request/{id}")
            };
            let source = format!("courier:result/{id}");
            let records = records::recall(library, "query", &thread)?;
            let replies: Vec<_> = records["records"]
                .as_array()
                .ok_or("invalid reply list")?
                .iter()
                .filter(|r| r["room"] == origin["room"] && r["source"] == source)
                .collect();
            json!({"request":origin,"replies":replies})
        }
        "task" => records::store(
            library,
            "meeting",
            required("question")?,
            "meeting/task",
            "",
            required("source")?,
        )?,
        "open" => {
            let id = required("id")?;
            let origin = task(library, id)?;
            let replies =
                records::recall(library, "query", &format!("meeting/task/{}", origin["id"]))?;
            let replies = replies["records"]
                .as_array()
                .ok_or("invalid meeting reply list")?
                .iter()
                .filter(|r| r["room"] == "meeting")
                .collect::<Vec<_>>();
            json!({"task":origin,"replies":replies})
        }
        "reply" => {
            let origin = task(library, required("id")?)?;
            records::store(
                library,
                "meeting",
                origin["question"].as_str().ok_or("invalid task question")?,
                &format!("meeting/task/{}", origin["id"]),
                required("answer")?,
                fields.get("source").copied().unwrap_or(""),
            )?
        }
        _ => unreachable!(),
    };
    println!("{result}");
    Ok(())
}
