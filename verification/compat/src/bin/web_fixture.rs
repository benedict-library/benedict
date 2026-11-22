//! Offline provider and cabinet writer for the web compatibility tests.
use serde_json::json;
use std::{
    env,
    fs::OpenOptions,
    io::{Read, Write},
};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open("provider.jsonl")
        .unwrap();
    writeln!(log, "{}", json!({"args":args,"stdin":input})).unwrap();
    match args[0].as_str() {
        "find" => {
            println!("{}", env::var("FIXTURE_RESPONSE").unwrap());
            std::process::exit(env::var("FIXTURE_EXIT").unwrap().parse().unwrap());
        }
        "file" => println!("{}", json!({"saved":true,"path":"queries/public-note.md"})),
        "legacy-file" => println!("저장: queries/public-note.md"),
        _ => std::process::exit(2),
    }
}
