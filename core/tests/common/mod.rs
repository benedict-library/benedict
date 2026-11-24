//! Search goes through the room service, so a test that searches starts one.
#![allow(dead_code)]
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// A room service for one test; it stops when dropped.
pub struct Room {
    child: Child,
}

/// The room's socket in `dir`. Commands that search set LIBRARY_ROOM_SOCKET to it.
pub fn socket(dir: &Path) -> PathBuf {
    dir.join("room.sock")
}

impl Room {
    /// Start `room serve` with the environment `make` gives every command of the test,
    /// so the room and the commands use the same database and project.
    pub fn start(make: impl Fn() -> Command) -> Room {
        let child = make()
            .args(["room", "serve"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let room = Room { child };
        let started = Instant::now();
        while !make().args(["room", "ping"]).output().unwrap().status.success() {
            assert!(started.elapsed() < Duration::from_secs(10), "room did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        room
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
