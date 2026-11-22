//! A reading-room work record is an ordinary durable room observation. Notes refer
//! to its numeric ID, so two sessions can independently open and extend it.
use crate::records;
use serde_json::{Value, json};

pub fn run(args: &[String]) -> Result<(), String> {
    let (action, rest) = args
        .split_first()
        .ok_or("expected work open, note or show")?;
    let mut args = rest.to_vec();
    let library = crate::option(&mut args, "--library")?.unwrap_or_else(|| "main".into());
    if args.first().is_some_and(|arg| arg == "--") {
        args.remove(0);
    }
    let result = match (action.as_str(), args.as_slice()) {
        ("open", [question, initial])
            if !question.trim().is_empty() && !initial.trim().is_empty() =>
        {
            let recent = records::recall(&library, "recent_work", "")?;
            let entry = records::store(&library, "reading", question, "work/open", initial, "")?;
            json!({"work_id":entry["id"], "initial":entry, "recent_work":recent["records"]})
        }
        ("note", [id, judgment, ..])
            if (args.len() == 2 || args.len() == 3) && !judgment.trim().is_empty() =>
        {
            let origin = open_record(&library, id)?;
            records::store(
                &library,
                "reading",
                origin["question"].as_str().ok_or("missing work question")?,
                &format!("work/{}", origin["id"]),
                judgment,
                args.get(2).map_or("", String::as_str),
            )?
        }
        ("show", [id]) => {
            let origin = open_record(&library, id)?;
            let notes = records::recall(&library, "query", &format!("work/{}", origin["id"]))?;
            json!({"initial":origin, "notes":notes["records"].as_array().ok_or("invalid work notes")?.iter()
                   .filter(|note| note["room"]=="reading").collect::<Vec<_>>()})
        }
        _ => {
            return Err(
                "use work open QUESTION INITIAL, work note ID JUDGMENT [SOURCE], or work show ID"
                    .into(),
            );
        }
    };
    println!("{result}");
    Ok(())
}

fn open_record(library: &str, id: &str) -> Result<Value, String> {
    let row = records::recall(library, "id", id)?;
    if row["room"] != "reading" || row["query"] != "work/open" {
        return Err("expected a reading-room work ID".into());
    }
    Ok(row)
}
