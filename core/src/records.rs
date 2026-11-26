//! Durable evidence produced in a room. A question is the user's original text;
//! a query is the agent's search text. Preserve both byte-for-byte.
//! A `record` row is an agent's summary of finished work, waiting for the librarian
//! until `filed_at` is set; other rows are `event`s; rows from before 0.3 have no kind.
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

const TABLE_COLUMNS: &str = "
    project TEXT NOT NULL,
    library TEXT NOT NULL,
    room TEXT NOT NULL,
    question TEXT NOT NULL,
    query TEXT NOT NULL,
    answer TEXT NOT NULL,
    source TEXT NOT NULL,
    session TEXT NOT NULL DEFAULT '',
    turns TEXT NOT NULL DEFAULT '',
    at TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL DEFAULT '',
    filed_at TEXT NOT NULL DEFAULT ''";
/// Columns added in 0.3; older tables gain them with empty values.
const ADDED_COLUMNS: [&str; 5] = ["session", "turns", "at", "kind", "filed_at"];
const COLUMNS: &str = "id,project,library,room,question,query,answer,source,session,turns,at,kind,filed_at";

/// Session, transcript range and kind of a stored row.
pub struct Extra<'a> {
    pub session: &'a str,
    pub turns: &'a str,
    pub kind: &'a str,
}
pub const EVENT: Extra<'static> = Extra {
    session: "",
    turns: "",
    kind: "event",
};

pub(crate) type Fields<'a> = BTreeMap<&'a str, &'a str>;

pub(crate) enum Database {
    Sqlite(PathBuf),
    Postgres(String),
}

pub fn enabled() -> bool {
    env::var_os("LIBRARY_DB_URL_FILE").is_some()
        || env::var_os("LIBRARY_DB").is_some()
        || env::var("LIBRARY_DB_URL").is_ok_and(|s| !s.is_empty())
}

pub(crate) fn backend() -> Result<Database, String> {
    let url = if let Some(file) = env::var_os("LIBRARY_DB_URL_FILE") {
        let contents = std::fs::read_to_string(file)
            .map_err(|_| "cannot read PostgreSQL secret file as the runtime user")?;
        Some(contents.trim_end_matches(['\r', '\n']).to_owned())
    } else {
        env::var_os("LIBRARY_DB_URL")
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| "invalid PostgreSQL connection URI")
            })
            .transpose()?
    };
    if let Some(url) = url {
        if (url.starts_with("postgresql://") || url.starts_with("postgres://"))
            && !url.chars().any(char::is_control)
        {
            return Ok(Database::Postgres(url));
        }
        return Err("database secret must contain one PostgreSQL connection URI".into());
    }
    // Without LIBRARY_DB the database sits beside library.json (/data in the container).
    let path = env::var_os("LIBRARY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::rooms::data_dir().join("library.sqlite3"));
    if path.as_os_str().is_empty() || path.as_os_str() == ":memory:" {
        return Err("LIBRARY_DB must be a persistent file".into());
    }
    Ok(Database::Sqlite(path))
}

pub(crate) fn sqlite(path: &std::path::Path, create: bool) -> Result<Connection, String> {
    let flags = if create {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = Connection::open_with_flags(path, flags)
        .map_err(|e| format!("open evidence database: {e}"))?;
    db.busy_timeout(Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    if create {
        // In WAL mode readers do not block writers. The librarian and the room read for seconds
        // on a large library (counting words, loading an index); a write meanwhile must not wait.
        // Switching an earlier database needs a moment without other connections. While the room
        // is reading, this connection works in the current mode and a later one switches.
        let mode: String = db
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(|e| format!("open evidence database: {e}"))?;
        if !mode.eq_ignore_ascii_case("wal") {
            db.busy_timeout(Duration::ZERO).map_err(|e| e.to_string())?;
            let switched = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0));
            db.busy_timeout(Duration::from_secs(10))
                .map_err(|e| e.to_string())?;
            match switched {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::DatabaseBusy => {}
                Err(e) => return Err(format!("open evidence database: {e}")),
            }
        }
        let named = |table: &str| {
            db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())
        };
        if named(OLD_TABLE)? && !named("benedict_room_records")? {
            db.execute_batch(&format!("ALTER TABLE {OLD_TABLE} RENAME TO benedict_room_records"))
                .map_err(|e| e.to_string())?;
        }
        db.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS benedict_room_records (
            id INTEGER PRIMARY KEY, {TABLE_COLUMNS})"
        ))
        .map_err(|e| e.to_string())?;
        let missing = |db: &Connection| -> Result<Vec<&'static str>, String> {
            let mut stmt = db
                .prepare("SELECT name FROM pragma_table_info('benedict_room_records')")
                .map_err(|e| e.to_string())?;
            let have = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(ADDED_COLUMNS
                .into_iter()
                .filter(|c| !have.iter().any(|h| h == c))
                .collect())
        };
        if !missing(&db)?.is_empty() {
            db.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            let added = missing(&db).and_then(|columns| {
                for column in columns {
                    db.execute_batch(&format!(
                        "ALTER TABLE benedict_room_records ADD COLUMN {column} TEXT NOT NULL DEFAULT ''"
                    ))
                    .map_err(|e| e.to_string())?;
                }
                Ok(())
            });
            db.execute_batch(if added.is_ok() { "COMMIT" } else { "ROLLBACK" })
                .map_err(|e| e.to_string())?;
            added?;
        }
    }
    Ok(db)
}

fn pg_connection(command: &mut Command, uri: &str) -> Result<(), String> {
    let url = url::Url::parse(uri).map_err(|_| "invalid PostgreSQL connection URI")?;
    if url.fragment().is_some() {
        return Err("unexpected fragment in PostgreSQL connection URI".into());
    }
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8()
            .map(|v| v.into_owned())
            .map_err(|_| "invalid UTF-8 in connection URI")
    };
    if !url.username().is_empty() {
        command.env("PGUSER", decode(url.username())?);
    }
    if let Some(password) = url.password() {
        command.env("PGPASSWORD", decode(password)?);
    }
    if let Some(host) = url.host_str() {
        command.env(
            "PGHOST",
            decode(host.trim_start_matches('[').trim_end_matches(']'))?,
        );
    }
    if let Some(port) = url.port() {
        command.env("PGPORT", port.to_string());
    }
    if let Some(name) = url.path().strip_prefix('/').filter(|s| !s.is_empty()) {
        command.env("PGDATABASE", decode(name)?);
    }
    if env::var_os("PGCONNECT_TIMEOUT").is_none() {
        command.env("PGCONNECT_TIMEOUT", "10");
    }
    for (key, value) in url.query_pairs() {
        let var = match key.as_ref() {
            "host" => "PGHOST",
            "hostaddr" => "PGHOSTADDR",
            "port" => "PGPORT",
            "dbname" => "PGDATABASE",
            "user" => "PGUSER",
            "password" => "PGPASSWORD",
            "sslmode" => "PGSSLMODE",
            "sslcert" => "PGSSLCERT",
            "sslkey" => "PGSSLKEY",
            "sslrootcert" => "PGSSLROOTCERT",
            "application_name" => "PGAPPNAME",
            "options" => "PGOPTIONS",
            "connect_timeout" => "PGCONNECT_TIMEOUT",
            "channel_binding" => "PGCHANNELBINDING",
            "target_session_attrs" => "PGTARGETSESSIONATTRS",
            "service" => "PGSERVICE",
            _ => return Err("unsupported PostgreSQL URI parameter".into()),
        };
        command.env(var, value.as_ref());
    }
    Ok(())
}

// psql quotes :'name' as one SQL literal on the active connection. Both the
// credentials use libpq settings; evidence travels through stdin, including large documents.
pub(crate) fn psql(url: &str) -> Result<Command, String> {
    let mut command = Command::new("psql");
    command.args(["-X", "-w", "-qAt", "-v", "ON_ERROR_STOP=1"]);
    pg_connection(&mut command, url)?;
    Ok(command)
}

/// Set psql variables. Keys are internal field names. Hex avoids interpolation of user text or psql
/// commands. Setting a variable from a query also avoids the OS size limit on environment values.
pub(crate) fn pg_variables(script: &mut String, fields: &Fields<'_>) {
    use std::fmt::Write as _;
    for (key, value) in fields {
        script.push_str("SELECT convert_from(decode('");
        for byte in value.bytes() {
            write!(script, "{byte:02x}").unwrap();
        }
        writeln!(script, "', 'hex'), 'UTF8') AS {key} \\gset").unwrap();
    }
}

// A database error can quote a password from the URI or user content, so only its SQLSTATE is shown.
pub(crate) fn pg_failure(status: std::process::ExitStatus, stderr: &[u8]) -> String {
    let state = stderr.split(|byte| *byte == b'\n').find_map(|line| {
        let line = std::str::from_utf8(line).ok()?;
        let (_, rest) = line.split_once("ERROR:  ")?;
        let code = rest.get(..5)?;
        (code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            && rest.as_bytes().get(5) == Some(&b':'))
        .then_some(code)
    });
    format!(
        "PostgreSQL operation failed (psql {status}, SQLSTATE {}); check the connection, schema and permissions",
        state.unwrap_or("unavailable")
    )
}

fn postgres(url: &str, sql: &str, fields: &Fields<'_>) -> Result<String, String> {
    let mut command = psql(url)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut script = String::new();
    pg_variables(&mut script, fields);
    script.push_str("\\set VERBOSITY verbose\n");
    script.push_str(sql);
    let mut child = command
        .spawn()
        .map_err(|e| format!("start PostgreSQL client: {e}"))?;
    let write = child
        .stdin
        .take()
        .ok_or("open psql stdin")?
        .write_all(script.as_bytes());
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(pg_failure(result.status, &result.stderr));
    }
    write.map_err(|e| e.to_string())?;
    String::from_utf8(result.stdout).map_err(|e| e.to_string())
}

/// The record table's name before 0.2.0. Opening a database for writing renames it, so earlier records stay.
const OLD_TABLE: &str = "doseogwan_room_records";

fn postgres_schema() -> String {
    format!(
        "BEGIN;
        DO $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('benedict_room_records')); END $$;
        DO $$ BEGIN
            IF to_regclass('{OLD_TABLE}') IS NOT NULL AND to_regclass('benedict_room_records') IS NULL THEN
                ALTER TABLE {OLD_TABLE} RENAME TO benedict_room_records;
            END IF;
        END $$;
        CREATE TABLE IF NOT EXISTS benedict_room_records (
            id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, {TABLE_COLUMNS});
        ALTER TABLE benedict_room_records {};
        SELECT {COLUMNS} FROM benedict_room_records LIMIT 0;
        COMMIT;",
        ADDED_COLUMNS
            .map(|c| format!("ADD COLUMN IF NOT EXISTS {c} TEXT NOT NULL DEFAULT ''"))
            .join(", ")
    )
}

pub fn initialize() -> Result<(), String> {
    match backend()? {
        Database::Sqlite(path) => {
            sqlite(&path, true)?
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM benedict_room_records LIMIT 0"
                ))
                .map_err(|e| e.to_string())?;
        }
        Database::Postgres(url) => {
            postgres(&url, &postgres_schema(), &Fields::new())?;
        }
    }
    Ok(())
}

pub(crate) fn options<'a>(args: &'a [String], allowed: &[&str]) -> Result<Fields<'a>, String> {
    let mut fields = Fields::new();
    for pair in args.chunks(2) {
        let flag = pair[0]
            .strip_prefix("--")
            .ok_or("expected a --field option")?;
        if !allowed.contains(&flag) {
            return Err(format!("unknown option --{flag}"));
        }
        let value = pair
            .get(1)
            .ok_or_else(|| format!("missing value for --{flag}"))?;
        if value.trim().is_empty() || fields.insert(flag, value.as_str()).is_some() {
            return Err(format!("empty or repeated --{flag}"));
        }
    }
    Ok(fields)
}

pub(crate) fn scope<'a>(library: &'a str, project: &'a str) -> Result<Fields<'a>, String> {
    if library.trim().is_empty() || project.trim().is_empty() {
        return Err("project and library keys must be nonempty".into());
    }
    Ok(Fields::from([("project", project), ("library", library)]))
}

pub(crate) fn project() -> Result<String, String> {
    match env::var("LIBRARY_PROJECT") {
        Ok(name) => Ok(name),
        Err(env::VarError::NotPresent) => Ok("default".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Empty fields represent information the room was not given. Never reconstruct
/// the human utterance from a search query, or fabricate a source or answer.
pub fn store(
    library: &str,
    room: &str,
    question: &str,
    query: &str,
    answer: &str,
    source: &str,
) -> Result<Value, String> {
    store_with(library, room, question, query, answer, source, &EVENT)
}

/// With the room service, the service writes the transcript line first, then the row.
pub fn store_with(
    library: &str,
    room: &str,
    question: &str,
    query: &str,
    answer: &str,
    source: &str,
    extra: &Extra<'_>,
) -> Result<Value, String> {
    if let Some(socket) = crate::rooms::socket_path() {
        return crate::rooms::request(
            &socket,
            json!({"op":"store","project":project()?, "library":library,"room":room,
            "question":question,"query":query,"answer":answer,"source":source,
            "session":extra.session,"turns":extra.turns,"kind":extra.kind}),
        );
    }
    persist(library, room, question, query, answer, source, extra)
}

pub(crate) fn persist(
    library: &str,
    room: &str,
    question: &str,
    query: &str,
    answer: &str,
    source: &str,
    extra: &Extra<'_>,
) -> Result<Value, String> {
    if room.trim().is_empty()
        || [question, query, answer, source]
            .iter()
            .all(|v| v.trim().is_empty())
    {
        return Err("a room record needs a room and at least one evidence field".into());
    }
    if !matches!(extra.kind, "event" | "record") {
        return Err("a room record kind is event or record".into());
    }
    let project = project()?;
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let mut fields = scope(library, &project)?;
    fields.extend([
        ("room", room),
        ("question", question),
        ("query", query),
        ("answer", answer),
        ("source", source),
        ("session", extra.session),
        ("turns", extra.turns),
        ("at", &at),
        ("kind", extra.kind),
        ("filed_at", ""),
    ]);
    let id = match backend()? {
        Database::Sqlite(path) => {
            let db = sqlite(&path, true)?;
            db.execute(
                "INSERT INTO benedict_room_records
                (project,library,room,question,query,answer,source,session,turns,at,kind)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    project,
                    library,
                    room,
                    question,
                    query,
                    answer,
                    source,
                    extra.session,
                    extra.turns,
                    at,
                    extra.kind
                ],
            )
            .map_err(|e| e.to_string())?;
            db.last_insert_rowid()
        }
        Database::Postgres(url) => {
            let sql = format!("{}\nINSERT INTO benedict_room_records
                (project,library,room,question,query,answer,source,session,turns,at,kind) VALUES
                (:'project',:'library',:'room',:'question',:'query',:'answer',:'source',
                :'session',:'turns',:'at',:'kind') RETURNING id;", postgres_schema());
            postgres(&url, &sql, &fields)?
                .trim()
                .parse()
                .map_err(|_| "invalid record ID from PostgreSQL")?
        }
    };
    let mut result = json!(fields);
    result["id"] = json!(id);
    Ok(result)
}

pub(crate) fn recall(library: &str, field: &str, lookup: &str) -> Result<Value, String> {
    let project = project()?;
    let mut params = scope(library, &project)?;
    match field {
        "id" if lookup.parse::<i64>().unwrap_or(0) <= 0 => {
            return Err("--id must be a positive integer".into());
        }
        "id" | "question" | "query" | "recent_work" => (),
        _ => return Err("unknown record lookup".into()),
    }
    params.insert("lookup", lookup);
    // Offer the last ten task titles as navigation; older work remains addressable by ID.
    let recent = field == "recent_work";
    let order = if recent { "DESC LIMIT 10" } else { "ASC" };
    let filter = |parameter: &str| {
        if recent {
            format!("room='reading' AND query='work/open' AND {parameter}=''")
        } else {
            format!("{field}={parameter}")
        }
    };
    let records: Vec<Value> = match backend()? {
        Database::Sqlite(path) => {
            // An existing database gains the 0.3 columns before it is read.
            let db = sqlite(&path, path.exists())?;
            let mut stmt = db
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM benedict_room_records
                WHERE project=?1 AND library=?2 AND {} ORDER BY id {order}",
                    filter("?3")
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![project, library, lookup], row_json)
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
        Database::Postgres(url) => {
            let sql = format!("{}\nSELECT row_to_json(r) FROM (SELECT {COLUMNS}
                FROM benedict_room_records WHERE project=:'project' AND library=:'library'
                AND {} ORDER BY id {order}) r;", postgres_schema(), filter(":'lookup'") );
            postgres(&url, &sql, &params)?
                .lines()
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    if recent {
        return Ok(
            json!({"records":records.iter().map(|r| json!({"id":r["id"],"question":r["question"]})).collect::<Vec<_>>()}),
        );
    }
    if field == "id" {
        return records
            .into_iter()
            .next()
            .ok_or_else(|| "record not found".into());
    }
    Ok(json!({"records":records}))
}

/// Same question regardless of spacing and letter case.
fn same_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Saved answers (`save` records with an answer) beside a search: the same question first, then
/// answers whose question or search words share at least two words with this one (all of them when it
/// has fewer). They are listed apart from the ranked hits, so earlier answers never take a place from
/// the documents; the librarian files the ones worth keeping.
pub(crate) fn asked(library: &str, question: &str, query: &str, limit: usize) -> Result<Vec<Value>, String> {
    const ANSWER_CHARS: usize = 300;
    let text = if question.trim().is_empty() { query } else { question };
    let key = same_text(text);
    let mine: BTreeSet<String> = crate::local::words(text).chain(crate::local::words(query)).collect();
    if key.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let project = project()?;
    let params = scope(library, &project)?;
    // First only the questions, so a long record history stays cheap; then the chosen rows in full.
    let filter = "kind='record' AND answer<>''";
    let heads: Vec<(i64, String, String)> = match backend()? {
        Database::Sqlite(ref path) => {
            if !path.exists() {
                return Ok(Vec::new());
            }
            let db = sqlite(path, true)?;
            let mut stmt = db
                .prepare(&format!(
                    "SELECT id, question, query FROM benedict_room_records
                WHERE project=?1 AND library=?2 AND {filter} ORDER BY id DESC"
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![project, library], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
        Database::Postgres(ref url) => {
            let sql = format!("{}\nSELECT json_build_array(id, question, query) FROM benedict_room_records
                WHERE project=:'project' AND library=:'library' AND {filter} ORDER BY id DESC;", postgres_schema());
            postgres(url, &sql, &params)?
                .lines()
                .map(|line| serde_json::from_str::<(i64, String, String)>(line).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?
        }
    };
    let need = mine.len().min(2);
    let mut found: Vec<(bool, usize, i64)> = Vec::new();
    for (id, their_question, their_query) in heads {
        let same = !their_question.trim().is_empty() && same_text(&their_question) == key;
        let theirs: BTreeSet<String> =
            crate::local::words(&their_question).chain(crate::local::words(&their_query)).collect();
        let shared = mine.intersection(&theirs).count();
        if same || (need > 0 && shared >= need) {
            found.push((same, shared, id));
        }
    }
    // Newest first within the same standing: rows arrive newest first and the sort is stable.
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = Vec::new();
    for (same, _, id) in found.into_iter().take(limit) {
        let row = recall(library, "id", &id.to_string())?;
        let answer: String = row["answer"].as_str().unwrap_or("").chars().take(ANSWER_CHARS).collect();
        out.push(json!({"id":row["id"],"question":row["question"],"answer":answer,"source":row["source"],
            "at":row["at"],"filed":row["filed_at"].as_str().is_some_and(|s| !s.is_empty()),"same":same}));
    }
    Ok(out)
}

fn row_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let mut value = json!({"id": row.get::<_, i64>(0)?});
    for (i, name) in COLUMNS.split(',').enumerate().skip(1) {
        value[name] = json!(row.get::<_, String>(i)?);
    }
    Ok(value)
}

/// Mark a row as filed once. A repeated filing keeps the first time.
pub(crate) fn mark_filed(library: &str, id: &str) -> Result<String, String> {
    let project = project()?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let mut params = scope(library, &project)?;
    params.extend([("lookup", id), ("now", now.as_str())]);
    let filed = match backend()? {
        Database::Sqlite(path) => {
            let db = sqlite(&path, true)?;
            db.execute(
                "UPDATE benedict_room_records SET filed_at=?4
                WHERE project=?1 AND library=?2 AND id=?3 AND filed_at=''",
                params![project, library, id, now],
            )
            .map_err(|e| e.to_string())?;
            db.query_row(
                "SELECT filed_at FROM benedict_room_records WHERE project=?1 AND library=?2 AND id=?3",
                params![project, library, id],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| e.to_string())?
        }
        Database::Postgres(url) => {
            let sql = format!(
                "{}\nUPDATE benedict_room_records SET filed_at=:'now'
                WHERE project=:'project' AND library=:'library' AND id=:'lookup' AND filed_at='';
                SELECT filed_at FROM benedict_room_records
                WHERE project=:'project' AND library=:'library' AND id=:'lookup';",
                postgres_schema()
            );
            postgres(&url, &sql, &params)?.trim().to_owned()
        }
    };
    if filed.is_empty() {
        return Err("record not found".into());
    }
    Ok(filed)
}

/// Saved records the librarian has not filed yet. Rows from before 0.3 are not counted.
pub(crate) fn unfiled(library: &str) -> Result<u64, String> {
    let project = project()?;
    let params = scope(library, &project)?;
    let count = match backend()? {
        Database::Sqlite(path) => sqlite(&path, true)?
            .query_row(
                "SELECT count(*) FROM benedict_room_records
                WHERE project=?1 AND library=?2 AND kind='record' AND filed_at=''",
                params![project, library],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())?
            .to_string(),
        Database::Postgres(url) => {
            let sql = format!(
                "{}\nSELECT count(*) FROM benedict_room_records
                WHERE project=:'project' AND library=:'library' AND kind='record' AND filed_at='';",
                postgres_schema()
            );
            postgres(&url, &sql, &params)?.trim().to_owned()
        }
    };
    count.parse().map_err(|_| "invalid record count".into())
}

fn execute(cmd: &str, args: &[String]) -> Result<Value, String> {
    match cmd {
        "record" => {
            let fields = options(
                args,
                &["room", "question", "query", "answer", "source", "library"],
            )?;
            let value = |k| fields.get(k).copied().unwrap_or("");
            store(
                fields.get("library").copied().unwrap_or("main"),
                value("room"),
                value("question"),
                value("query"),
                value("answer"),
                value("source"),
            )
        }
        "recall" => {
            let fields = options(args, &["id", "question", "query", "library"])?;
            let mut filters = fields.iter().filter(|(k, _)| **k != "library");
            let (&field, &lookup) = filters.next().ok_or("supply --id, --question or --query")?;
            if filters.next().is_some() {
                return Err("supply exactly one record lookup".into());
            }
            recall(
                fields.get("library").copied().unwrap_or("main"),
                field,
                lookup,
            )
        }
        _ => Err("expected record or recall".into()),
    }
}

pub fn run(cmd: &str, args: &[String]) -> Result<(), String> {
    println!("{}", execute(cmd, args)?);
    Ok(())
}
