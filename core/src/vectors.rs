//! Document vectors for semantic search. After indexing, the librarian embeds the documents whose
//! catalog hash changed, part by part, and keeps the vectors in the database next to the index.
//! The search room loads them with the index. Without `LIBRARY_EMBED_URL` nothing happens.
use crate::embed;
use crate::index_store::{VECTOR_SCHEMA, WRITE_WAIT, has_table, sql_error};
use crate::local::{Registry, read};
use crate::records::{Database, backend, project, sqlite};
use chrono::{SecondsFormat, Utc};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;

/// Characters per part, as the earlier Python embedding sync split documents.
const PART: usize = 1_000;
/// Parts per request: 16 parts took about 8 seconds on two cores, well inside the request wait.
const BATCH: usize = 16;
/// Parts per librarian cycle: about 0.5 s each on two cores is under 20 minutes a day; the rest
/// waits for the next cycle and search uses words for those documents meanwhile.
const PER_CYCLE: usize = 2_000;
/// Seconds per part on two cores without a GPU (16 parts in about 8 s): the prediction on a machine
/// that has no record of its own yet.
const SEED_PACE: f64 = 0.5;
/// Records kept per library. The slowest of them sets the wait; older ones age out, so a machine or
/// server that becomes faster is followed.
const PACE_RECORDS: usize = 50;
/// With fewer records than this the wait is widened further: the first records on a shared four-core
/// virtual machine were more than seven times the seed (3.7 s per part and more).
const FEW_RECORDS: usize = 5;
const FEW_WIDEN: f64 = 8.0;

/// How long one request of `parts` parts may take. There is no fixed limit: the wait is the
/// predicted time (median seconds per part in the records, or the seed) times a multiplier, which is
/// twice the largest actual÷predicted ratio in the records, so no recorded request would have been
/// cut, and wider while there are few records. A cut request is recorded at the wait it had, a lower
/// bound of its time, so the next wait is longer.
fn wait_for(parts: usize, records: &[f64]) -> Duration {
    let mut sorted = records.to_vec();
    sorted.sort_by(f64::total_cmp);
    let predicted = sorted.get(sorted.len() / 2).copied().unwrap_or(SEED_PACE).max(f64::MIN_POSITIVE);
    let ratio = sorted.last().map_or(1.0, |slowest| (slowest / predicted).max(1.0));
    let widen = if records.len() < FEW_RECORDS { FEW_WIDEN } else { 1.0 };
    Duration::from_secs_f64(parts.max(1) as f64 * predicted * 2.0 * ratio * widen)
}

fn paces(db: &rusqlite::Connection, project: &str, key: &str) -> Result<Vec<f64>> {
    let mut stmt = db
        .prepare("SELECT seconds_per_part FROM benedict_embed_pace WHERE project=?1 AND library=?2 ORDER BY seq")
        .map_err(sql_error)?;
    stmt.query_map(params![project, key], |r| r.get(0))
        .map_err(sql_error)?
        .collect::<rusqlite::Result<_>>()
        .map_err(sql_error)
}

fn record_pace(db: &rusqlite::Connection, project: &str, key: &str, records: &mut Vec<f64>, pace: f64, cut: bool) -> Result<()> {
    let at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false);
    db.execute(
        "INSERT INTO benedict_embed_pace (project, library, seq, at, seconds_per_part, cut) VALUES (?1,?2,
        (SELECT COALESCE(MAX(seq), 0) + 1 FROM benedict_embed_pace WHERE project=?1 AND library=?2),?3,?4,?5)",
        params![project, key, at, pace, cut],
    )
    .map_err(sql_error)?;
    db.execute(
        "DELETE FROM benedict_embed_pace WHERE project=?1 AND library=?2 AND seq NOT IN
        (SELECT seq FROM benedict_embed_pace WHERE project=?1 AND library=?2 ORDER BY seq DESC LIMIT ?3)",
        params![project, key, PACE_RECORDS as i64],
    )
    .map_err(sql_error)?;
    records.push(pace);
    if records.len() > PACE_RECORDS {
        records.remove(0);
    }
    Ok(())
}

/// One request, and one more with the longer wait when the first was cut. A server that does not
/// answer the second either is left to the next cycle, which starts from the longer records, so a
/// slow server is waited on long enough and one that never answers is not waited on without end.
fn embed_batch(
    db: &rusqlite::Connection,
    (url, project, key): (&str, &str, &str),
    batch: &[String],
    records: &mut Vec<f64>,
    cuts: &mut usize,
) -> Result<std::result::Result<Vec<Vec<f32>>, String>> {
    let parts = batch.len().max(1) as f64;
    let mut last = String::new();
    for _ in 0..2 {
        let wait = wait_for(batch.len(), records);
        let started = Instant::now();
        match embed::embed(url, batch, wait) {
            Ok(v) => {
                record_pace(db, project, key, records, started.elapsed().as_secs_f64() / parts, false)?;
                return Ok(Ok(v));
            }
            Err(e) if embed::was_cut(&e) => {
                record_pace(db, project, key, records, wait.as_secs_f64() / parts, true)?;
                *cuts += 1;
                last = e;
            }
            Err(e) => return Ok(Err(e)),
        }
    }
    Ok(Err(last))
}

/// The librarian's vector step. Its status is `not_configured`, `ok`, `partial` (more documents
/// wait for the next cycle) or `failed` (the server could not be reached; the cycle goes on).
pub fn refresh(config: &Path, key: &str) -> Value {
    match run(config, key) {
        Ok(step) => step,
        Err(e) => json!({"status":"failed","error":e}),
    }
}

fn parts(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(PART).map(|c| c.iter().collect()).collect()
}

fn run(config: &Path, key: &str) -> Result<Value> {
    let Some(url) = embed::url() else {
        return Ok(json!({"status":"not_configured"}));
    };
    let Database::Sqlite(path) = backend()? else {
        return Ok(json!({"status":"not_configured","reason":"semantic search uses the SQLite database"}));
    };
    let started = Instant::now();
    let reg = Registry::open(config)?;
    let shelf = reg.shelf_root(key)?;
    let project = project()?;
    let model = embed::model();
    let mut db = sqlite(&path, true)?;
    db.busy_timeout(WRITE_WAIT).map_err(sql_error)?;
    if !has_table(&db, "benedict_catalog")? {
        return Ok(json!({"status":"ok","documents":0,"parts":0}));
    }
    db.execute_batch(VECTOR_SCHEMA).map_err(sql_error)?;
    let identity: Option<String> = {
        use rusqlite::OptionalExtension as _;
        db.query_row(
            "SELECT model FROM benedict_vector_identity WHERE project=?1 AND library=?2",
            params![project, key],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?
    };
    // Vectors from another model are not comparable with this one's.
    let reset = identity.as_ref().is_some_and(|m| *m != model);
    if reset {
        let tx = db.transaction().map_err(sql_error)?;
        tx.execute("DELETE FROM benedict_vectors WHERE project=?1 AND library=?2", params![project, key])
            .map_err(sql_error)?;
        tx.execute(
            "DELETE FROM benedict_vector_identity WHERE project=?1 AND library=?2",
            params![project, key],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
    }
    let catalog: Vec<(String, String, String)> = {
        let mut stmt = db
            .prepare("SELECT path, title, hash FROM benedict_catalog WHERE project=?1 AND library=?2 ORDER BY path")
            .map_err(sql_error)?;
        stmt.query_map(params![project, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql_error)?
    };
    let have: HashMap<String, String> = {
        let mut stmt = db
            .prepare("SELECT path, hash FROM benedict_vectors WHERE project=?1 AND library=?2 AND part=0")
            .map_err(sql_error)?;
        stmt.query_map(params![project, key], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql_error)?
    };
    let indexed: HashSet<&str> = catalog.iter().map(|(p, _, _)| p.as_str()).collect();
    let removed: Vec<&String> = have.keys().filter(|p| !indexed.contains(p.as_str())).collect();
    let todo: Vec<&(String, String, String)> = catalog
        .iter()
        .filter(|(p, _, h)| have.get(p) != Some(h))
        .collect();
    let mut records = paces(&db, &project, key)?;
    let mut cuts = 0usize;
    let mut dim = 0usize;
    let (mut documents, mut done, mut skipped) = (0usize, 0usize, 0usize);
    let mut failure = None;
    for (doc, title, hash) in &todo {
        if done >= PER_CYCLE {
            break;
        }
        let text = match doc.strip_prefix("shelf:") {
            Some(name) => match &shelf {
                Some(root) => crate::shelf::read(root, name).map(|t| t.0),
                None => Err("no shelf".into()),
            },
            None => read(&reg, key, doc, None).and_then(|v| {
                v["text"].as_str().map(str::to_owned).ok_or_else(|| "no text".to_string())
            }),
        };
        let Ok(text) = text else {
            // A document that cannot be read now is tried again next cycle.
            skipped += 1;
            continue;
        };
        let texts: Vec<String> = parts(&text).iter().map(|p| embed::document_text(title, p)).collect();
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            match embed_batch(&db, (&url, &project, key), batch, &mut records, &mut cuts)? {
                Ok(v) => vectors.extend(v),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if failure.is_some() {
            break;
        }
        dim = vectors[0].len();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        tx.execute(
            "DELETE FROM benedict_vectors WHERE project=?1 AND library=?2 AND path=?3",
            params![project, key, doc],
        )
        .map_err(sql_error)?;
        for (part, vector) in vectors.iter().enumerate() {
            tx.execute(
                "INSERT INTO benedict_vectors (project, library, path, part, hash, vec) VALUES (?1,?2,?3,?4,?5,?6)",
                params![project, key, doc, part as i64, hash, embed::to_bytes(vector)],
            )
            .map_err(sql_error)?;
        }
        tx.commit().map_err(sql_error)?;
        documents += 1;
        done += vectors.len();
    }
    let changed = reset || documents > 0 || !removed.is_empty();
    {
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        for doc in &removed {
            tx.execute(
                "DELETE FROM benedict_vectors WHERE project=?1 AND library=?2 AND path=?3",
                params![project, key, doc],
            )
            .map_err(sql_error)?;
        }
        // The room reloads when this changes, once per cycle rather than once per document.
        if changed {
            tx.execute(
                "INSERT INTO benedict_vector_identity (project, library, model, dim, updated_at)
                VALUES (?1,?2,?3,?4,?5) ON CONFLICT (project, library) DO UPDATE SET
                model=excluded.model, dim=CASE WHEN excluded.dim > 0 THEN excluded.dim ELSE dim END,
                updated_at=excluded.updated_at",
                params![project, key, model, dim as i64, Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)],
            )
            .map_err(sql_error)?;
        }
        tx.commit().map_err(sql_error)?;
    }
    let remaining = todo.len() - documents;
    let mut step = json!({"status": if failure.is_some() { "failed" } else if remaining > 0 { "partial" } else { "ok" },
        "model":model,"documents":documents,"parts":done,"removed":removed.len(),"remaining":remaining,
        "seconds":(started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0});
    if skipped > 0 {
        step["unreadable"] = json!(skipped);
    }
    if cuts > 0 {
        step["cut"] = json!(cuts);
    }
    if let Some(e) = failure {
        step["error"] = json!(e);
    }
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_follows_the_records_and_has_no_fixed_limit() {
        // No records: the seed, widened for a machine not yet measured.
        assert_eq!(wait_for(16, &[]), Duration::from_secs_f64(16.0 * 0.5 * 2.0 * 8.0));
        // Enough records: twice the slowest recorded pace, whatever its size.
        let records = [1.0, 1.0, 1.0, 1.0, 4.0];
        assert_eq!(wait_for(16, &records), Duration::from_secs_f64(16.0 * 4.0 * 2.0));
        let slow = [30.0; 5];
        assert!(wait_for(16, &slow) > Duration::from_secs(900));
    }

    #[test]
    fn a_cut_is_kept_as_a_lower_bound_and_old_records_age_out() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(VECTOR_SCHEMA).unwrap();
        let mut records = Vec::new();
        record_pace(&db, "p", "main", &mut records, 2.0, true).unwrap();
        assert!(wait_for(16, &records) >= Duration::from_secs_f64(16.0 * 2.0 * 2.0));
        for _ in 0..PACE_RECORDS + 10 {
            record_pace(&db, "p", "main", &mut records, 0.5, false).unwrap();
        }
        assert_eq!(paces(&db, "p", "main").unwrap().len(), PACE_RECORDS);
        assert_eq!(records.len(), PACE_RECORDS);
    }

    #[test]
    fn documents_split_into_parts_of_a_thousand_characters() {
        assert_eq!(super::parts(""), vec![String::new()]);
        let text: String = "가".repeat(2_500);
        let parts = super::parts(&text);
        assert_eq!(parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(), [1_000, 1_000, 500]);
    }
}
