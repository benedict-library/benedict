//! A project-local Unix socket serves the rooms. Every event is appended to the
//! day's transcript on the shelf first, then kept in that room's memory window.
//! Only records an agent stores reach the database; the transcript keeps the rest.
use crate::records;
use chrono::{Days, Local, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
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

// Room windows share half of the container's memory limit; without a readable
// limit they share half of 512 MiB. Each event is one JSON byte buffer.
const FALLBACK_BUDGET: usize = 256 * 1024 * 1024;
// A single event larger than this keeps an excerpt and its transcript line.
const EXCERPT_BYTES: usize = 4096;
const REQUEST_LIMIT: u64 = 32 * 1024 * 1024;
const NOT_RUNNING: &str = "room service not running";
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
        .map_err(|e| format!("{NOT_RUNNING}: connect {}: {e}", path.display()))?;
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

/// The folder that holds library.json also holds the shelf transcripts and golden files.
/// Without LIBRARY_CONFIG the room socket's folder is used, then the working folder.
pub(crate) fn data_dir() -> PathBuf {
    let config = env::var_os("LIBRARY_CONFIG")
        .map(PathBuf::from)
        .or_else(|| socket_path().map(|s| s.with_file_name("library.json")))
        .unwrap_or_else(|| PathBuf::from("library.json"));
    let config = std::path::absolute(&config).unwrap_or(config);
    config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn transcripts_dir() -> PathBuf {
    data_dir().join("shelf").join("transcripts")
}

pub(crate) fn day_folder(days_ago: u64) -> String {
    (Local::now().date_naive() - Days::new(days_ago))
        .format("%y%m%d")
        .to_string()
}

fn budget() -> usize {
    fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .map(|limit| limit / 2)
        .unwrap_or(FALLBACK_BUDGET)
}

fn name_part(value: &str, what: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid {what} name for a transcript"));
    }
    Ok(())
}

#[derive(Default)]
struct Window {
    events: VecDeque<Vec<u8>>,
    bytes: usize,
    evicted: u64,
}

/// Lines already in a day file and the request IDs they carry.
struct DayFile {
    lines: u64,
    requests: HashMap<String, u64>,
}

struct LiveMemory {
    rooms: HashMap<(String, String), Window>,
    bytes: usize,
    budget: usize,
    files: HashMap<String, DayFile>,
}

impl LiveMemory {
    fn new() -> Self {
        LiveMemory {
            rooms: HashMap::new(),
            bytes: 0,
            budget: budget(),
            files: HashMap::new(),
        }
    }

    fn remember(&mut self, library: &str, room: &str, event: &Value) -> Result<()> {
        let mut data = serde_json::to_vec(event).map_err(err)?;
        if data.len() > EXCERPT_BYTES.max(self.budget / 4) {
            let text = String::from_utf8_lossy(&data);
            let cut: String = text.chars().take(EXCERPT_BYTES).collect();
            data = serde_json::to_vec(
                &json!({"excerpt":cut,"bytes":data.len(),"turn":event["turn"]}),
            )
            .map_err(err)?;
        }
        let key = (library.to_owned(), room.to_owned());
        self.bytes += data.len();
        let window = self.rooms.entry(key.clone()).or_default();
        window.bytes += data.len();
        window.events.push_back(data);
        // Evict this room's oldest events first; a room never loses its newest one.
        while self.bytes > self.budget {
            let room = if self.rooms[&key].events.len() > 1 {
                key.clone()
            } else {
                match self
                    .rooms
                    .iter()
                    .filter(|(k, w)| **k != key && !w.events.is_empty())
                    .max_by_key(|(_, w)| w.bytes)
                {
                    Some((k, _)) => k.clone(),
                    None => break,
                }
            };
            let window = self.rooms.get_mut(&room).unwrap();
            if let Some(old) = window.events.pop_front() {
                window.bytes -= old.len();
                window.evicted += 1;
                self.bytes -= old.len();
            }
        }
        Ok(())
    }

    fn list(&self, library: &str, room: &str) -> Result<Vec<Value>> {
        self.rooms
            .get(&(library.to_owned(), room.to_owned()))
            .map(|w| {
                w.events
                    .iter()
                    .map(|data| serde_json::from_slice(data).map_err(err))
                    .collect()
            })
            .unwrap_or_else(|| Ok(vec![]))
    }

    fn day_file(&mut self, relative: &str) -> Result<&mut DayFile> {
        if !self.files.contains_key(relative) {
            let mut file = DayFile {
                lines: 0,
                requests: HashMap::new(),
            };
            match fs::read_to_string(transcripts_dir().join(relative)) {
                Ok(content) => {
                    for line in content.lines() {
                        file.lines += 1;
                        if let Some(id) = serde_json::from_str::<Value>(line)
                            .ok()
                            .and_then(|v| v["request"].as_str().map(str::to_owned))
                            .filter(|id| !id.is_empty())
                        {
                            file.requests.insert(id, file.lines);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => {}
                Err(e) => return Err(format!("transcript {relative}: {e}")),
            }
            self.files.insert(relative.to_owned(), file);
        }
        Ok(self.files.get_mut(relative).unwrap())
    }

    /// Append one event to today's transcript before anything else sees it.
    /// A request ID already written today or yesterday returns its earlier line.
    fn transcribe(&mut self, library: &str, room: &str, event: Value) -> Result<(Value, bool)> {
        let (event, duplicate) = self.write_turn(library, room, event)?;
        if !duplicate {
            self.remember(library, room, &event)?;
        }
        Ok((event["turn"].clone(), duplicate))
    }

    fn write_turn(&mut self, library: &str, room: &str, mut event: Value) -> Result<(Value, bool)> {
        name_part(library, "library")?;
        name_part(room, "room")?;
        let request = event["request"].as_str().unwrap_or("").to_owned();
        if !request.is_empty() {
            for days_ago in [0, 1] {
                let relative = format!("{}/{library}-{room}.jsonl", day_folder(days_ago));
                if let Some(line) = self.day_file(&relative)?.requests.get(&request) {
                    return Ok((json!({"turn":{"file":relative,"line":line}}), true));
                }
            }
        }
        let day = day_folder(0);
        let relative = format!("{day}/{library}-{room}.jsonl");
        let folder = transcripts_dir().join(&day);
        fs::create_dir_all(&folder).map_err(|e| format!("transcript {}: {e}", folder.display()))?;
        let state = self.day_file(&relative)?;
        let line = state.lines + 1;
        event["turn"] = json!({"file":relative,"line":line});
        let mut bytes = serde_json::to_vec(&event).map_err(err)?;
        bytes.push(b'\n');
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(transcripts_dir().join(&relative))
            .map_err(|e| format!("transcript {relative}: {e}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_data())
            .map_err(|e| format!("transcript {relative}: {e}"))?;
        let state = self.day_file(&relative)?;
        state.lines = line;
        if !request.is_empty() {
            state.requests.insert(request, line);
        }
        Ok((event, false))
    }

    fn usage(&self) -> Value {
        let mut rooms: Vec<Value> = self
            .rooms
            .iter()
            .map(|((library, room), w)| {
                json!({"library":library,"room":room,"events":w.events.len(),
                       "bytes":w.bytes,"evicted":w.evicted})
            })
            .collect();
        rooms.sort_by_key(|r| (r["library"].to_string(), r["room"].to_string()));
        json!({"budget":self.budget,"bytes":self.bytes,"rooms":rooms})
    }

    fn handle(&mut self, project: &str, msg: Value) -> Result<Value> {
        let text = |key: &str| msg[key].as_str().ok_or_else(|| format!("missing {key}"));
        let optional = |key: &str| msg[key].as_str().unwrap_or("");
        if text("project")? != project {
            return Err("room belongs to another project".into());
        }
        let at = || Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        match text("op")? {
            "ping" => Ok(json!({"project":project,"ready":true,"version":env!("CARGO_PKG_VERSION"),
                                "memory":self.usage()})),
            "append" => {
                let (library, room) = (text("library")?, text("room")?);
                let event = json!({"at":at(),"session":optional("session"),
                    "request":optional("request"),"role":text("role")?,"text":text("text")?});
                let (turn, duplicate) = self.transcribe(library, room, event)?;
                Ok(json!({"turn":turn,"duplicate":duplicate}))
            }
            "store" => {
                let (library, room) = (text("library")?, text("room")?);
                let extra = records::Extra {
                    session: optional("session"),
                    turns: optional("turns"),
                    kind: if optional("kind").is_empty() {
                        "event"
                    } else {
                        optional("kind")
                    },
                };
                let event = json!({"at":at(),"session":extra.session,"role":extra.kind,
                    "question":text("question")?,"query":text("query")?,"answer":text("answer")?,
                    "source":text("source")?,"turns":extra.turns});
                // The transcript line comes first; memory keeps it with the database ID.
                let (mut event, _) = self.write_turn(library, room, event)?;
                let saved = records::persist(
                    library,
                    room,
                    text("question")?,
                    text("query")?,
                    text("answer")?,
                    text("source")?,
                    &extra,
                )?;
                event["id"] = saved["id"].clone();
                self.remember(library, room, &event)?;
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
    let mut memory = LiveMemory::new();
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
        .ok_or("expected room serve, ping, memory, append, task, open, reply, dispatch or responses")?;
    let flags = match cmd.as_str() {
        "serve" | "ping" => &[][..],
        "memory" => &["room", "library"][..],
        "append" => &["room", "session", "request", "role", "text", "library"][..],
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
    let socket = || {
        socket_path().ok_or_else(|| {
            if cmd == "serve" {
                "set LIBRARY_ROOM_SOCKET for the shared room service".to_string()
            } else {
                format!("{NOT_RUNNING}: set LIBRARY_ROOM_SOCKET")
            }
        })
    };
    // Room conversation must reach the shelf transcript, so it needs the service.
    if cmd != "serve" {
        socket()?;
    }
    let project = records::project()?;
    let note = |room: &str, role: &str, text: &str| {
        request(
            &socket()?,
            json!({"op":"append","project":project,"library":library,"room":room,
                   "role":role,"text":text}),
        )
    };
    let result = match cmd.as_str() {
        "serve" => return serve(&socket()?),
        "append" => request(
            &socket()?,
            json!({"op":"append","project":project,"library":library,"room":required("room")?,
                   "session":fields.get("session").copied().unwrap_or(""),
                   "request":fields.get("request").copied().unwrap_or(""),
                   "role":required("role")?,"text":required("text")?}),
        )?,
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
            note(
                origin["room"].as_str().ok_or("invalid request room")?,
                "responses",
                &origin["id"].to_string(),
            )?;
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
            note("meeting", "open", &origin["id"].to_string())?;
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
