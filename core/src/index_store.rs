//! The search index in the database. The librarian writes the catalog (each document's title,
//! summary and hash, not its text), the word counts and the lengths in one transaction; the
//! search room loads them into memory. SQLite and PostgreSQL hold the same four tables.
use crate::records::{Database, Fields, backend, pg_failure, pg_variables, project, psql, scope, sqlite};
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    io::{BufRead, Read, Seek, Write},
    process::Stdio,
    time::Duration,
};

type Result<T> = std::result::Result<T, String>;

/// One version of a library's index. `shelf_root` is empty without a shelf.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IndexState {
    pub root: String,
    pub shelf_root: String,
    pub indexed_at: String,
    pub documents: u64,
}

/// A document indexed again: its catalog row, length and word counts.
pub(crate) struct IndexedDoc {
    pub path: String,
    pub title: String,
    pub summary: String,
    pub hash: String,
    pub length: u64,
    pub terms: Vec<(String, u64)>,
}

pub(crate) enum IndexEntry {
    Document(IndexedDoc),
    Removed(String),
}

pub(crate) enum IndexRow {
    Catalog {
        path: String,
        title: String,
        summary: String,
        hash: String,
    },
    Length {
        path: String,
        length: u64,
    },
    Term {
        path: String,
        word: String,
        freq: u64,
    },
}

/// Index writers wait this long for the room's reads to finish.
const WRITE_WAIT: Duration = Duration::from_secs(120);
const TABLES: [&str; 3] = ["benedict_terms", "benedict_lengths", "benedict_catalog"];
const SCOPE: &str = "project=:'project' AND library=:'library'";

fn schema(sqlite: bool) -> String {
    let rowid = if sqlite { " WITHOUT ROWID" } else { "" };
    format!(
        "CREATE TABLE IF NOT EXISTS benedict_catalog (project TEXT NOT NULL, library TEXT NOT NULL,
            path TEXT NOT NULL, title TEXT NOT NULL, summary TEXT NOT NULL, hash TEXT NOT NULL,
            PRIMARY KEY (project, library, path)){rowid};
        CREATE TABLE IF NOT EXISTS benedict_terms (project TEXT NOT NULL, library TEXT NOT NULL,
            word TEXT NOT NULL, path TEXT NOT NULL, freq BIGINT NOT NULL,
            PRIMARY KEY (project, library, path, word)){rowid};
        CREATE TABLE IF NOT EXISTS benedict_lengths (project TEXT NOT NULL, library TEXT NOT NULL,
            path TEXT NOT NULL, length BIGINT NOT NULL, PRIMARY KEY (project, library, path)){rowid};
        CREATE TABLE IF NOT EXISTS benedict_index_state (project TEXT NOT NULL, library TEXT NOT NULL,
            root TEXT NOT NULL, shelf_root TEXT NOT NULL, indexed_at TEXT NOT NULL,
            documents BIGINT NOT NULL, PRIMARY KEY (project, library)){rowid};"
    )
}

fn postgres_schema() -> String {
    format!(
        "BEGIN;
        DO $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('benedict_index')); END $$;
        {}
        COMMIT;\n",
        schema(false)
    )
}

fn sql_error(e: rusqlite::Error) -> String {
    format!("index database: {e}")
}

fn io_error(e: std::io::Error) -> String {
    e.to_string()
}

/// A read connection to an SQLite database that holds an index, if any.
fn sqlite_index(path: &std::path::Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let db = sqlite(path, false)?;
    let present: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='benedict_index_state'",
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    Ok((present > 0).then_some(db))
}

fn sqlite_state(db: &Connection, project: &str, library: &str) -> Result<Option<IndexState>> {
    use rusqlite::OptionalExtension as _;
    db.query_row(
        "SELECT root, shelf_root, indexed_at, documents FROM benedict_index_state
        WHERE project=?1 AND library=?2",
        params![project, library],
        |r| {
            Ok(IndexState {
                root: r.get(0)?,
                shelf_root: r.get(1)?,
                indexed_at: r.get(2)?,
                documents: r.get::<_, i64>(3)? as u64,
            })
        },
    )
    .optional()
    .map_err(sql_error)
}

/// The fields of one line of COPY text output.
fn copy_fields(line: &str) -> Vec<String> {
    line.split('\t')
        .map(|field| {
            let mut out = String::with_capacity(field.len());
            let mut chars = field.chars();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some('v') => out.push('\u{b}'),
                    Some(other) => out.push(other),
                    None => out.push('\\'),
                }
            }
            out
        })
        .collect()
}

/// One line of COPY text input.
fn copy_line(out: &mut dyn Write, fields: &[&str]) -> std::io::Result<()> {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.write_all(b"\t")?;
        }
        let bytes = field.as_bytes();
        let mut start = 0;
        for (j, byte) in bytes.iter().enumerate() {
            let escaped: &[u8] = match byte {
                b'\\' => b"\\\\",
                b'\t' => b"\\t",
                b'\n' => b"\\n",
                b'\r' => b"\\r",
                _ => continue,
            };
            out.write_all(&bytes[start..j])?;
            out.write_all(escaped)?;
            start = j + 1;
        }
        out.write_all(&bytes[start..])?;
    }
    out.write_all(b"\n")
}

fn psql_errors(status: std::process::ExitStatus, mut errors: std::fs::File) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    let mut stderr = Vec::new();
    errors.rewind().map_err(io_error)?;
    errors.read_to_end(&mut stderr).map_err(io_error)?;
    Err(pg_failure(status, &stderr))
}

/// Run `sql` (COPY ... TO STDOUT statements) and hand each output row to `row` as it arrives.
fn postgres_rows(
    url: &str,
    fields: &Fields<'_>,
    sql: &str,
    row: &mut dyn FnMut(Vec<String>) -> Result<()>,
) -> Result<()> {
    let errors = tempfile::tempfile().map_err(io_error)?;
    let mut child = psql(url)?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(errors.try_clone().map_err(io_error)?)
        .spawn()
        .map_err(|e| format!("start PostgreSQL client: {e}"))?;
    let mut script = String::new();
    pg_variables(&mut script, fields);
    script.push_str("\\set VERBOSITY verbose\n");
    script.push_str(&postgres_schema());
    script.push_str(sql);
    let written = child
        .stdin
        .take()
        .ok_or("open psql stdin")?
        .write_all(script.as_bytes());
    let stdout = child.stdout.take().ok_or("open psql stdout")?;
    let mut handled = Ok(());
    for line in std::io::BufReader::new(stdout).split(b'\n') {
        let line = line.map_err(io_error).and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| "invalid UTF-8 from PostgreSQL".to_string())
        });
        if let Err(e) = line.and_then(|line| row(copy_fields(&line))) {
            let _ = child.kill();
            handled = Err(e);
            break;
        }
    }
    let status = child.wait().map_err(io_error)?;
    handled?;
    psql_errors(status, errors)?;
    written.map_err(io_error)
}

fn state_row(fields: &[String]) -> Result<IndexState> {
    match fields {
        [root, shelf_root, indexed_at, documents] => Ok(IndexState {
            root: root.clone(),
            shelf_root: shelf_root.clone(),
            indexed_at: indexed_at.clone(),
            documents: documents.parse().map_err(|_| "invalid index state")?,
        }),
        _ => Err("invalid index state".into()),
    }
}

/// The library's current index version; None before its first index.
pub(crate) fn state(library: &str) -> Result<Option<IndexState>> {
    let project = project()?;
    let fields = scope(library, &project)?;
    match backend()? {
        Database::Sqlite(path) => match sqlite_index(&path)? {
            Some(db) => sqlite_state(&db, &project, library),
            None => Ok(None),
        },
        Database::Postgres(url) => {
            let mut state = None;
            postgres_rows(
                &url,
                &fields,
                &format!(
                    "COPY (SELECT root, shelf_root, indexed_at, documents::text
                    FROM benedict_index_state WHERE {SCOPE}) TO STDOUT;\n"
                ),
                &mut |row| {
                    state = Some(state_row(&row)?);
                    Ok(())
                },
            )?;
            Ok(state)
        }
    }
}

/// The hash of every indexed document, by path.
pub(crate) fn hashes(library: &str) -> Result<HashMap<String, String>> {
    let project = project()?;
    let fields = scope(library, &project)?;
    let mut hashes = HashMap::new();
    match backend()? {
        Database::Sqlite(path) => {
            if let Some(db) = sqlite_index(&path)? {
                let mut stmt = db
                    .prepare("SELECT path, hash FROM benedict_catalog WHERE project=?1 AND library=?2")
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![project, library], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(sql_error)?;
                for row in rows {
                    let (path, hash) = row.map_err(sql_error)?;
                    hashes.insert(path, hash);
                }
            }
        }
        Database::Postgres(url) => postgres_rows(
            &url,
            &fields,
            &format!("COPY (SELECT path, hash FROM benedict_catalog WHERE {SCOPE}) TO STDOUT;\n"),
            &mut |row| match <[String; 2]>::try_from(row) {
                Ok([path, hash]) => {
                    hashes.insert(path, hash);
                    Ok(())
                }
                Err(_) => Err("invalid catalog row".into()),
            },
        )?,
    }
    Ok(hashes)
}

/// The number of catalog rows and of distinct indexed words.
pub(crate) fn counts(library: &str) -> Result<(u64, u64)> {
    let project = project()?;
    let fields = scope(library, &project)?;
    let sql = |scope: &str| {
        format!(
            "SELECT (SELECT count(*) FROM benedict_catalog WHERE {scope}),
            (SELECT count(DISTINCT word) FROM benedict_terms WHERE {scope})"
        )
    };
    match backend()? {
        Database::Sqlite(path) => match sqlite_index(&path)? {
            Some(db) => db
                .query_row(
                    &sql("project=?1 AND library=?2"),
                    params![project, library],
                    |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
                )
                .map_err(sql_error),
            None => Ok((0, 0)),
        },
        Database::Postgres(url) => {
            let mut counts = (0, 0);
            postgres_rows(
                &url,
                &fields,
                &format!("COPY ({}) TO STDOUT;\n", sql(SCOPE)),
                &mut |row| match row.as_slice() {
                    [catalog, words] => {
                        counts = (
                            catalog.parse().map_err(|_| "invalid index count")?,
                            words.parse().map_err(|_| "invalid index count")?,
                        );
                        Ok(())
                    }
                    _ => Err("invalid index count".into()),
                },
            )?;
            Ok(counts)
        }
    }
}

pub(crate) type Sink<'a> = dyn FnMut(IndexEntry) -> Result<()> + 'a;

/// Write a new index version in one transaction. `produce` sends each changed or removed document
/// to the sink and returns the version, or None when nothing changed; then nothing is written.
/// `reset` first removes the library's rows, for a first index or a moved wiki.
pub(crate) fn write(
    library: &str,
    reset: bool,
    produce: &mut dyn FnMut(&mut Sink<'_>) -> Result<Option<IndexState>>,
) -> Result<Option<IndexState>> {
    let project = project()?;
    let mut fields = scope(library, &project)?;
    match backend()? {
        Database::Sqlite(path) => {
            let mut db = sqlite(&path, true)?;
            db.busy_timeout(WRITE_WAIT).map_err(sql_error)?;
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            tx.execute_batch(&schema(true)).map_err(sql_error)?;
            if reset {
                for table in TABLES {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE project=?1 AND library=?2"),
                        params![project, library],
                    )
                    .map_err(sql_error)?;
                }
            }
            let state = {
                let mut remove = TABLES
                    .iter()
                    .map(|table| {
                        tx.prepare(&format!(
                            "DELETE FROM {table} WHERE project=?1 AND library=?2 AND path=?3"
                        ))
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(sql_error)?;
                let mut catalog = tx
                    .prepare(
                        "INSERT INTO benedict_catalog (project,library,path,title,summary,hash)
                        VALUES (?1,?2,?3,?4,?5,?6)",
                    )
                    .map_err(sql_error)?;
                let mut length = tx
                    .prepare(
                        "INSERT INTO benedict_lengths (project,library,path,length) VALUES (?1,?2,?3,?4)",
                    )
                    .map_err(sql_error)?;
                let mut term = tx
                    .prepare(
                        "INSERT INTO benedict_terms (project,library,word,path,freq) VALUES (?1,?2,?3,?4,?5)",
                    )
                    .map_err(sql_error)?;
                produce(&mut |entry| {
                    let path = match &entry {
                        IndexEntry::Document(doc) => &doc.path,
                        IndexEntry::Removed(path) => path,
                    };
                    for statement in &mut remove {
                        statement
                            .execute(params![project, library, path])
                            .map_err(sql_error)?;
                    }
                    if let IndexEntry::Document(doc) = &entry {
                        catalog
                            .execute(params![project, library, doc.path, doc.title, doc.summary, doc.hash])
                            .map_err(sql_error)?;
                        length
                            .execute(params![project, library, doc.path, doc.length as i64])
                            .map_err(sql_error)?;
                        for (word, freq) in &doc.terms {
                            term.execute(params![project, library, word, doc.path, *freq as i64])
                                .map_err(sql_error)?;
                        }
                    }
                    Ok(())
                })?
            };
            if let Some(state) = &state {
                tx.execute(
                    "INSERT INTO benedict_index_state (project,library,root,shelf_root,indexed_at,documents)
                    VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT (project, library) DO UPDATE SET
                    root=excluded.root, shelf_root=excluded.shelf_root,
                    indexed_at=excluded.indexed_at, documents=excluded.documents",
                    params![
                        project,
                        library,
                        state.root,
                        state.shelf_root,
                        state.indexed_at,
                        state.documents as i64
                    ],
                )
                .map_err(sql_error)?;
                tx.commit().map_err(sql_error)?;
            }
            Ok(state)
        }
        Database::Postgres(url) => {
            // Rows reach a temporary table through COPY while the documents are read.
            let errors = tempfile::tempfile().map_err(io_error)?;
            let mut child = psql(&url)?
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(errors.try_clone().map_err(io_error)?)
                .spawn()
                .map_err(|e| format!("start PostgreSQL client: {e}"))?;
            fields.insert("reset", if reset { "1" } else { "0" });
            let mut head = String::new();
            pg_variables(&mut head, &fields);
            head.push_str("\\set VERBOSITY verbose\n");
            head.push_str(&postgres_schema());
            head.push_str(
                "BEGIN;
                DO $$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('benedict_index')); END $$;
                CREATE TEMP TABLE benedict_staged (kind TEXT NOT NULL, path TEXT NOT NULL,
                    word TEXT NOT NULL, num BIGINT NOT NULL, title TEXT NOT NULL,
                    summary TEXT NOT NULL, hash TEXT NOT NULL) ON COMMIT DROP;
                COPY benedict_staged FROM STDIN;\n",
            );
            let mut input = std::io::BufWriter::new(child.stdin.take().ok_or("open psql stdin")?);
            let sent = (|| -> Result<Option<IndexState>> {
                input.write_all(head.as_bytes()).map_err(io_error)?;
                let state = produce(&mut |entry| match &entry {
                    IndexEntry::Document(doc) => {
                        let length = doc.length.to_string();
                        copy_line(
                            &mut input,
                            &["c", &doc.path, "", &length, &doc.title, &doc.summary, &doc.hash],
                        )
                        .map_err(io_error)?;
                        for (word, freq) in &doc.terms {
                            copy_line(&mut input, &["t", &doc.path, word, &freq.to_string(), "", "", ""])
                                .map_err(io_error)?;
                        }
                        Ok(())
                    }
                    IndexEntry::Removed(path) => {
                        copy_line(&mut input, &["d", path, "", "0", "", "", ""]).map_err(io_error)
                    }
                })?;
                input.write_all(b"\\.\n").map_err(io_error)?;
                let Some(state) = state else {
                    input.write_all(b"ROLLBACK;\n").map_err(io_error)?;
                    input.flush().map_err(io_error)?;
                    return Ok(None);
                };
                let documents = state.documents.to_string();
                let mut tail = String::new();
                pg_variables(
                    &mut tail,
                    &Fields::from([
                        ("root", state.root.as_str()),
                        ("shelf_root", state.shelf_root.as_str()),
                        ("indexed_at", state.indexed_at.as_str()),
                        ("documents", documents.as_str()),
                    ]),
                );
                for table in TABLES {
                    tail.push_str(&format!(
                        "DELETE FROM {table} WHERE {SCOPE} AND (:'reset'='1' OR path IN
                        (SELECT path FROM benedict_staged WHERE kind IN ('c','d')));\n"
                    ));
                }
                tail.push_str(
                    "INSERT INTO benedict_catalog (project,library,path,title,summary,hash)
                    SELECT :'project',:'library',path,title,summary,hash FROM benedict_staged WHERE kind='c';
                    INSERT INTO benedict_lengths (project,library,path,length)
                    SELECT :'project',:'library',path,num FROM benedict_staged WHERE kind='c';
                    INSERT INTO benedict_terms (project,library,word,path,freq)
                    SELECT :'project',:'library',word,path,num FROM benedict_staged WHERE kind='t';
                    INSERT INTO benedict_index_state (project,library,root,shelf_root,indexed_at,documents)
                    VALUES (:'project',:'library',:'root',:'shelf_root',:'indexed_at',:'documents'::bigint)
                    ON CONFLICT (project, library) DO UPDATE SET root=EXCLUDED.root,
                    shelf_root=EXCLUDED.shelf_root, indexed_at=EXCLUDED.indexed_at,
                    documents=EXCLUDED.documents;
                    COMMIT;\n",
                );
                input.write_all(tail.as_bytes()).map_err(io_error)?;
                input.flush().map_err(io_error)?;
                Ok(Some(state))
            })();
            // Without COMMIT, closing the input ends the session and PostgreSQL rolls the transaction back.
            drop(input);
            let status = child.wait().map_err(io_error)?;
            psql_errors(status, errors)?;
            sent
        }
    }
}

/// Hand every row of the library's current index to `row`: catalog rows first, then lengths,
/// then word counts, all from one version. None before the first index.
pub(crate) fn load(library: &str, row: &mut dyn FnMut(IndexRow) -> Result<()>) -> Result<Option<IndexState>> {
    let project = project()?;
    let fields = scope(library, &project)?;
    match backend()? {
        Database::Sqlite(path) => {
            let Some(mut db) = sqlite_index(&path)? else {
                return Ok(None);
            };
            let tx = db.transaction().map_err(sql_error)?;
            let Some(state) = sqlite_state(&tx, &project, library)? else {
                return Ok(None);
            };
            let mut each = |sql: &str,
                            make: &dyn Fn(&rusqlite::Row<'_>) -> rusqlite::Result<IndexRow>|
             -> Result<()> {
                let mut stmt = tx.prepare(sql).map_err(sql_error)?;
                let mut rows = stmt.query(params![project, library]).map_err(sql_error)?;
                while let Some(r) = rows.next().map_err(sql_error)? {
                    row(make(r).map_err(sql_error)?)?;
                }
                Ok(())
            };
            each(
                "SELECT path, title, summary, hash FROM benedict_catalog WHERE project=?1 AND library=?2",
                &|r| {
                    Ok(IndexRow::Catalog {
                        path: r.get(0)?,
                        title: r.get(1)?,
                        summary: r.get(2)?,
                        hash: r.get(3)?,
                    })
                },
            )?;
            each(
                "SELECT path, length FROM benedict_lengths WHERE project=?1 AND library=?2",
                &|r| {
                    Ok(IndexRow::Length {
                        path: r.get(0)?,
                        length: r.get::<_, i64>(1)? as u64,
                    })
                },
            )?;
            each(
                "SELECT path, word, freq FROM benedict_terms WHERE project=?1 AND library=?2",
                &|r| {
                    Ok(IndexRow::Term {
                        path: r.get(0)?,
                        word: r.get(1)?,
                        freq: r.get::<_, i64>(2)? as u64,
                    })
                },
            )?;
            Ok(Some(state))
        }
        Database::Postgres(url) => {
            let mut state = None;
            let invalid = "invalid index row";
            postgres_rows(
                &url,
                &fields,
                &format!(
                    "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
                    COPY (SELECT 's', root, shelf_root, indexed_at, documents::text
                        FROM benedict_index_state WHERE {SCOPE}) TO STDOUT;
                    COPY (SELECT 'c', path, title, summary, hash FROM benedict_catalog WHERE {SCOPE}) TO STDOUT;
                    COPY (SELECT 'l', path, length::text FROM benedict_lengths WHERE {SCOPE}) TO STDOUT;
                    COPY (SELECT 't', path, word, freq::text FROM benedict_terms WHERE {SCOPE}) TO STDOUT;
                    COMMIT;\n"
                ),
                &mut |mut fields| {
                    let kind = if fields.is_empty() { String::new() } else { fields.remove(0) };
                    if kind == "s" {
                        state = Some(state_row(&fields)?);
                        return Ok(());
                    }
                    if state.is_none() {
                        return Err(invalid.into());
                    }
                    let mut values = fields.into_iter();
                    let mut next = || values.next().ok_or(invalid);
                    row(match kind.as_str() {
                        "c" => IndexRow::Catalog {
                            path: next()?,
                            title: next()?,
                            summary: next()?,
                            hash: next()?,
                        },
                        "l" => IndexRow::Length {
                            path: next()?,
                            length: next()?.parse().map_err(|_| invalid)?,
                        },
                        "t" => IndexRow::Term {
                            path: next()?,
                            word: next()?,
                            freq: next()?.parse().map_err(|_| invalid)?,
                        },
                        _ => return Err(invalid.into()),
                    })
                },
            )?;
            Ok(state)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_text_round_trips_tabs_newlines_and_backslashes() {
        let fields = ["a\tb", "line\none\r", "back\\slash", "", "plain 한글"];
        let mut out = Vec::new();
        copy_line(&mut out, &fields).unwrap();
        let line = String::from_utf8(out).unwrap();
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        assert_eq!(copy_fields(line.trim_end_matches('\n')), fields);
    }
}
