//! Test double for psql. Only the presence and transport of the generated test
//! value are reported; the value itself is never retained by the recorder.
use serde_json::{Map, Value, json};
use std::{
    env, fs,
    io::{self, Read, Write},
};
fn main() {
    if env::var_os("EXAMPLE_DB_UNAVAILABLE").is_some() {
        eprintln!("database unavailable");
        std::process::exit(2);
    }
    let mut sql = String::new();
    io::stdin().read_to_string(&mut sql).unwrap();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut bindings = Map::new();
    for pair in args.windows(2).filter(|pair| pair[0] == "-v") {
        let (k, v) = pair[1].split_once('=').unwrap();
        bindings.insert(k.to_owned(), Value::from(v));
    }
    let expected = env::var("EXAMPLE_JOURNAL_NONCE").ok();
    let output = json!({"sql":sql,"bindings":bindings,"host":env::var("PGHOST").ok(),"database":env::var("PGDATABASE").ok(),"user":env::var("PGUSER").ok(),
        "passed_in_environment":expected.as_ref().is_some_and(|s|env::var("PGPASSWORD").as_ref()==Ok(s)),
        "passed_in_arguments":expected.as_ref().is_some_and(|s|args.iter().any(|a|a.contains(s)))});
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(env::var_os("EXAMPLE_JOURNAL_LOG").unwrap())
        .unwrap();
    writeln!(file, "{output}").unwrap();
}
