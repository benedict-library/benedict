use serde_json::Value;
use std::{
    process::{Child, Command, Output},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Rooms {
    dir: TempDir,
    server: Option<Child>,
}
impl Rooms {
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_doseogwan"));
        cmd.env("LIBRARY_ROOM_SOCKET", self.dir.path().join("room.sock"))
            .env("LIBRARY_DB", self.dir.path().join("rooms.sqlite"))
            .env_remove("LIBRARY_DB_URL")
            .env("LIBRARY_PROJECT", "test-project");
        cmd
    }
    fn call(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }
    fn get(&self, args: &[&str]) -> Value {
        let out = self.call(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn start(&mut self) {
        self.server = Some(self.cmd().args(["room", "serve"]).spawn().unwrap());
        let start = Instant::now();
        while !self.call(&["room", "ping"]).status.success() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "server did not become ready"
            );
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    fn stop(&mut self) {
        if let Some(mut server) = self.server.take() {
            let _ = server.kill();
            let _ = server.wait();
        }
    }
}
impl Drop for Rooms {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn separate_agents_share_four_live_rooms_and_reopen_a_task_after_server_replacement() {
    let mut rooms = Rooms {
        dir: tempfile::tempdir().unwrap(),
        server: None,
    };
    rooms.start();
    let q = "몇 개를 출고할 수 있습니까?";
    let entry = rooms.get(&[
        "room",
        "task",
        "--question",
        q,
        "--source",
        "검사표 A7: 59개 승인",
    ]);
    let id = entry["id"].to_string();
    let task = rooms.get(&["room", "open", "--id", &id]);
    assert_eq!(task["task"]["source"], "검사표 A7: 59개 승인");
    let reply = rooms.get(&[
        "room",
        "reply",
        "--id",
        &id,
        "--answer",
        "59개",
        "--source",
        "검사표 A7",
    ]);
    let reopened = rooms.get(&["room", "open", "--id", &id]);
    assert_eq!(reopened["replies"][0], reply);
    for name in ["search", "reading", "librarian"] {
        let saved = rooms.get(&[
            "record",
            "--room",
            name,
            "--question",
            q,
            "--source",
            "검사표 A7",
        ]);
        assert_eq!(
            rooms.get(&["room", "memory", "--room", name])["records"][0],
            saved
        );
    }
    let meeting = rooms.get(&["room", "memory", "--room", "meeting"]);
    assert_eq!(meeting["records"], serde_json::json!([entry, reply]));
    assert!(
        !rooms
            .call(&["room", "open", "--id", &id, "--library", "unrelated"])
            .status
            .success()
    );
    let denied = rooms
        .cmd()
        .env("LIBRARY_PROJECT", "other")
        .args(["room", "memory", "--room", "meeting"])
        .output()
        .unwrap();
    assert!(!denied.status.success());
    rooms.stop();
    rooms.start();
    assert_eq!(rooms.get(&["room", "open", "--id", &id]), reopened);
}
