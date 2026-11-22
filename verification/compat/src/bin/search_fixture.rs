//! Local stand-in used to test how the compatibility search client routes a request.
use serde_json::json;
use std::{
    env,
    fs::OpenOptions,
    io::{self, Read, Write},
    process,
};

fn room_reply(room: &str, code: &str) {
    if code.contains("library_registry") {
        println!("{}", env::var("SAMPLE_REGISTRY").unwrap_or("null".into()));
        return;
    }
    if code.contains("library_terms") {
        println!(
            "{}",
            json!([{"w":"garden","e":{"표준어":"garden","별칭":[],"절":"plants","실체":"Planting notes."}}])
        );
        return;
    }
    if room == env::var("SAMPLE_STALE_ROOM").unwrap_or_default() {
        println!("STALE at=2001-01-01 n=1 docs=2");
        return;
    }
    let distance = if room == "far" { "0.400" } else { "0.100" };
    println!("# 하이브리드 벡터 3건 · 최근접 {distance}");
    println!("/sample/{room}/entities/flowers.md:1:title: Planting notes");
    println!("/sample/{room}/queries/garden.md:1:title: Garden question");
    let entry = code.contains("_ENTRY = True");
    println!("# 위키방 hit 2 · {}", if entry { "entry" } else { "plain" });
    if entry {
        println!("## 오늘 들어온 글\ngarden.md\n## 결정 모음");
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    match args.first().map(String::as_str) {
        Some("room") => room_reply(&args[1], &input),
        Some("box") => println!(
            "# 자료 · 최근접 0.150\n/sample/box/notes.md:1:title: Box answer ({})",
            args[1]
        ),
        Some("web") => {
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(env::var_os("SAMPLE_WEB_LOG").unwrap())
                .unwrap();
            writeln!(log, "{}", json!({"args": &args[1..], "stdin":input})).unwrap();
            println!("Sample external finding");
            if env::var_os("SAMPLE_FAIL_WEB").is_some() {
                process::exit(3)
            }
        }
        // Transport fixtures parse a known delivery request without executing arbitrary commands.
        Some("wiki-host") => process::exit(3),
        Some("room-host") => {
            if args[1].starts_with("timeout ") {
                room_reply(args[1].split_whitespace().last().unwrap(), &input)
            } else if args[1].starts_with("bash ") {
                println!("branch-wiki/entities/flowers.md:2: garden on room host")
            } else {
                process::exit(2)
            }
        }
        _ => process::exit(2),
    }
}
