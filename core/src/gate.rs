//! A narrow entrance for agents in other containers: search and read only.
//!
//! `benedict gate --listen ADDRESS` accepts one JSON line per connection,
//! `{"cmd":"search","args":["refund window"]}`, runs that command with this same
//! program and returns its output. Results are masked and recorded exactly as on
//! the command line. Anything else (other commands, `--config`, `--web`, unknown
//! options) is refused, so a caller reaches the library and nothing beside it.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;

const MAX_REQUEST: u64 = 16 * 1024;
const MAX_ARGS: usize = 8;
const MAX_RUNNING: usize = 8;
const TIME_LIMIT: Duration = Duration::from_secs(120);

/// Options each command may carry; every one takes a value.
fn options(cmd: &str) -> Option<&'static [&'static str]> {
    match cmd {
        "search" => Some(&["--top", "--question", "--library"]),
        "read" => Some(&["--pages", "--question", "--library"]),
        _ => None,
    }
}

/// The check every request passes before anything runs.
pub fn allowed(request: &Value) -> Result<Vec<String>> {
    let cmd = request["cmd"]
        .as_str()
        .ok_or("cmd must be search or read")?;
    let known = options(cmd).ok_or("cmd must be search or read")?;
    let args = request["args"].as_array().ok_or("args must be a list")?;
    if args.is_empty() || args.len() > MAX_ARGS {
        return Err(format!("args must hold 1 to {MAX_ARGS} values"));
    }
    let mut out = vec![cmd.to_owned()];
    let mut values = args.iter();
    let mut positional = false;
    while let Some(v) = values.next() {
        let s = v.as_str().ok_or("args must be text")?;
        if s.contains(['\n', '\r', '\0']) {
            return Err("args must be one line each".into());
        }
        if s.starts_with('-') {
            if !known.contains(&s) {
                return Err(format!("option {s} is not available here"));
            }
            let value = values
                .next()
                .and_then(Value::as_str)
                .filter(|x| !x.starts_with('-') && !x.contains(['\n', '\r', '\0']))
                .ok_or(format!("{s} needs a value"))?;
            out.extend([s.to_owned(), value.to_owned()]);
        } else {
            positional = true;
            out.push(s.to_owned());
        }
    }
    if !positional {
        return Err("give a query or a page to read".into());
    }
    Ok(out)
}

fn answer(stream: &mut TcpStream, value: &Value) {
    let _ = writeln!(stream, "{value}");
}

fn run_one(args: &[String]) -> Value {
    let Ok(program) = std::env::current_exe() else {
        return json!({"error": "gate cannot find its program"});
    };
    run_program(&program, args)
}

fn run_program(program: &std::path::Path, args: &[String]) -> Value {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return json!({"error": "gate cannot start the command"});
    };
    // Read while waiting: a result larger than the pipe buffer would otherwise block the command.
    fn collect(
        pipe: Option<impl Read + Send + 'static>,
    ) -> Option<std::thread::JoinHandle<String>> {
        pipe.map(|mut p| {
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = p.read_to_string(&mut text);
                text
            })
        })
    }
    let reader = collect(child.stdout.take());
    let errors = collect(child.stderr.take());
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIME_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return json!({"error": "the command took too long"});
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return json!({"error": "gate lost the command"}),
        }
    }
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    let errors = errors.and_then(|r| r.join().ok()).unwrap_or_default();
    // Benedict reports a refused command as one JSON object on standard error.
    let error = serde_json::from_str::<Value>(errors.trim())
        .ok()
        .filter(|v| v["error"].is_string());
    serde_json::from_str(text.trim())
        .ok()
        .or(error)
        .unwrap_or_else(|| json!({"error": "the command gave no result"}))
}

fn handle(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut line = String::new();
    let read = BufReader::new((&stream).take(MAX_REQUEST)).read_line(&mut line);
    if read.is_err() || !line.ends_with('\n') {
        return answer(&mut stream, &json!({"error": "send one JSON line"}));
    }
    let request = match serde_json::from_str::<Value>(&line) {
        Ok(v) => v,
        Err(_) => return answer(&mut stream, &json!({"error": "send one JSON line"})),
    };
    let reply = match allowed(&request) {
        Ok(args) => run_one(&args),
        Err(e) => json!({ "error": e }),
    };
    answer(&mut stream, &reply);
}

/// `benedict gate --listen ADDRESS`
pub fn run(args: &[String]) -> Result<()> {
    match args {
        [flag, address] if flag == "--listen" => listen(address),
        _ => Err("usage: benedict gate --listen HOST:PORT".into()),
    }
}

/// The room service opens the gate beside its socket when LIBRARY_GATE names an address, so a
/// library stays one container. A gate that cannot open tries again; the room keeps serving.
pub fn open_beside_room() {
    let Some(address) = std::env::var("LIBRARY_GATE").ok().filter(|a| !a.trim().is_empty()) else {
        return;
    };
    std::thread::spawn(move || loop {
        if let Err(e) = listen(&address) {
            eprintln!("{e}; trying again in 5 s");
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}

fn listen(address: &str) -> Result<()> {
    let listener = TcpListener::bind(address).map_err(|e| format!("gate {address}: {e}"))?;
    eprintln!("gate ready on {address}: search and read only");
    let running = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming().flatten() {
        if running.fetch_add(1, Ordering::SeqCst) >= MAX_RUNNING {
            running.fetch_sub(1, Ordering::SeqCst);
            let mut stream = stream;
            answer(
                &mut stream,
                &json!({"error": "the library is busy; try again"}),
            );
            continue;
        }
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            handle(stream);
            running.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(v: Value) -> Vec<String> {
        allowed(&v).unwrap()
    }

    #[test]
    fn search_and_read_with_their_options_are_allowed() {
        assert_eq!(
            ok(json!({"cmd":"search","args":["--top","5","--question","환불 기한","refund"]})),
            ["search", "--top", "5", "--question", "환불 기한", "refund"]
        );
        assert_eq!(
            ok(json!({"cmd":"read","args":["--pages","1-3","shelf:보고서.pdf"]})),
            ["read", "--pages", "1-3", "shelf:보고서.pdf"]
        );
        assert_eq!(
            ok(json!({"cmd":"read","args":["entities/refund.md"]})),
            ["read", "entities/refund.md"]
        );
    }

    #[test]
    fn a_result_larger_than_the_pipe_buffer_comes_back_whole() {
        let script =
            r#"printf '{"text":"'; head -c 200000 /dev/zero | tr '\0' x; printf '"}'"#.to_owned();
        let reply = run_program(std::path::Path::new("sh"), &["-c".into(), script]);
        assert_eq!(reply["text"].as_str().map(str::len), Some(200_000));
    }

    #[test]
    fn a_refused_command_returns_its_own_error() {
        let script = r#"printf '{"error":"empty shelf path"}' >&2; exit 2"#.to_owned();
        let reply = run_program(std::path::Path::new("sh"), &["-c".into(), script]);
        assert_eq!(reply["error"], "empty shelf path");
    }

    #[test]
    fn anything_beside_search_and_read_is_refused() {
        for bad in [
            json!({"cmd":"save","args":["q","a"]}),
            json!({"cmd":"room","args":["ping"]}),
            json!({"cmd":"search","args":["--config","/etc/passwd","x"]}),
            json!({"cmd":"search","args":["--web","x"]}),
            json!({"cmd":"search","args":["--top"]}),
            json!({"cmd":"search","args":["--","x"]}),
            json!({"cmd":"search","args":["-h"]}),
            json!({"cmd":"search","args":[]}),
            json!({"cmd":"search","args":["a\nb"]}),
            json!({"cmd":"search","args":[1]}),
            json!({"cmd":"search"}),
            json!(["search", "x"]),
        ] {
            assert!(allowed(&bad).is_err(), "allowed {bad}");
        }
    }
}
