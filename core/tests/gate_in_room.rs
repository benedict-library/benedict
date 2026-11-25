//! The room service opens the gate itself, so a library stays one container.
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn benedict(dir: &Path, gate: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_benedict"));
    cmd.env("LIBRARY_ROOM_SOCKET", dir.join("room.sock"))
        .env("LIBRARY_DB", dir.join("rooms.sqlite"))
        .env_remove("LIBRARY_DB_URL")
        .env("LIBRARY_PROJECT", "test-project")
        .env("LIBRARY_GATE", gate);
    cmd
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn room(dir: &Path, gate: &str) -> Running {
    let child = benedict(dir, gate)
        .args(["room", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let running = Running(child);
    let started = Instant::now();
    while !benedict(dir, gate).args(["room", "ping"]).output().unwrap().status.success() {
        assert!(started.elapsed() < Duration::from_secs(10), "room did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    running
}

#[test]
fn the_room_service_opens_the_gate_named_in_library_gate() {
    let dir = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let address = format!("127.0.0.1:{port}");
    let _room = room(dir.path(), &address);
    let started = Instant::now();
    let mut stream = loop {
        if let Ok(stream) = TcpStream::connect(&address) {
            break stream;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "gate never opened");
        std::thread::sleep(Duration::from_millis(20));
    };
    stream.write_all(b"{\"cmd\":\"save\",\"args\":[\"x\"]}\n").unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert!(reply["error"].as_str().unwrap().contains("search or read"), "{line}");
}

#[test]
fn a_gate_that_cannot_open_leaves_the_room_serving() {
    let dir = tempfile::tempdir().unwrap();
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    let _room = room(dir.path(), &address);
    std::thread::sleep(Duration::from_millis(300));
    let ping = benedict(dir.path(), &address).args(["room", "ping"]).output().unwrap();
    assert!(ping.status.success(), "{}", String::from_utf8_lossy(&ping.stderr));
}
