use benedict_compat::{python, repo_root};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Meeting {
    temp: TempDir,
    trace: bool,
}
impl Meeting {
    fn new(trace: bool) -> Self {
        let room = Self {
            temp: tempfile::tempdir().unwrap(),
            trace,
        };
        fs::create_dir(room.temp.path().join("traces")).unwrap();
        room.success("room.py", &["start", "demo-meeting"], "");
        room
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(python());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("LIBRARY_") {
                cmd.env_remove(key);
            }
        }
        cmd.env("LIBRARY_ROOM_HOME", self.temp.path().join("rooms"))
            .env("LIBRARY_SEARCH_ENV", self.temp.path().join("absent.env"))
            .env("LIBRARY_MAIL_FROM", "sample reader")
            .env("HOME", self.temp.path())
            .env("USERPROFILE", self.temp.path())
            .env("PYTHONDONTWRITEBYTECODE", "1");
        if cfg!(unix) && !self.trace {
            cmd.env("LC_ALL", "C")
                .env("LANG", "C")
                .env("PYTHONUTF8", "0")
                .env("PYTHONCOERCECLOCALE", "0")
                .env("PYTHONIOENCODING", "");
        }
        if self.trace {
            let recorder = Path::new(env!("CARGO_BIN_EXE_trace_recorder"))
                .to_string_lossy()
                .replace('\\', "/");
            let dir = self
                .temp
                .path()
                .join("traces")
                .to_string_lossy()
                .replace('\\', "/");
            cmd.env("LIBRARY_TRACE_CMD",format!("\"{recorder}\" \"{dir}\" --agent {{who}} --caller library_{{room}} --session library-{{yymmdd}}"));
        }
        cmd
    }
    fn run(&self, tool: &str, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command()
            .arg(repo_root().join("bin").join(tool))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
    fn success(&self, tool: &str, args: &[&str], input: &str) -> String {
        let out = self.run(tool, args, input);
        assert!(
            out.status.success(),
            "{tool} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().replace("\r\n", "\n")
    }
    fn rows(&self) -> Vec<Value> {
        fs::read_dir(self.temp.path().join("traces"))
            .unwrap()
            .map(Result::unwrap)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
            .collect()
    }
    fn wait_for_rows(&self, n: usize) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let rows = self.rows();
            if rows.len() >= n {
                return rows;
            }
            assert!(
                Instant::now() < deadline,
                "expected {n} traces, got {}",
                rows.len()
            );
            thread::sleep(Duration::from_millis(60));
        }
    }
}
impl Drop for Meeting {
    fn drop(&mut self) {
        let _ = self.run("room.py", &["stop", "demo-meeting"], "");
    }
}
#[test]
fn unicode_mail_and_room_output_survive_the_host_locale_without_tracing() {
    let room = Meeting::new(false);
    assert!(
        room.success(
            "mail.py",
            &["--room", "demo-meeting", "put", "reply"],
            "첫 답 — 한글\n"
        )
        .contains("넣음 reply 1 건")
    );
    let mail = room.success("mail.py", &["--room", "demo-meeting", "get", "reply"], "");
    assert!(
        mail.contains("sample reader\n첫 답 — 한글\n-- 1 건"),
        "{mail}"
    );
    assert_eq!(
        room.success(
            "room.py",
            &["exec", "demo-meeting"],
            "print('방 안 한글')\n"
        ),
        "방 안 한글\n"
    );
    assert!(room.rows().is_empty());
}
#[test]
fn room_tools_trace_their_role_prompt_result_and_exit_status() {
    let room = Meeting::new(true);
    room.success(
        "mail.py",
        &["--room", "demo-meeting", "put", "review"],
        "첫 답\n",
    );
    room.success("mail.py", &["--room", "demo-meeting", "get", "review"], "");
    assert!(
        !room
            .run("task.py", &["--room", "demo-meeting", "unknown"], "")
            .status
            .success()
    );
    let tools = [
        ("read.py", "reading"),
        ("task.py", "meeting"),
        ("mail.py", "meeting"),
        ("web.py", "search"),
        ("librarian.py", "librarian"),
        ("room_sync.py", "librarian"),
        ("embed_sync.py", "librarian"),
        ("save_query.py", "librarian"),
        ("wiki_backup.py", "librarian"),
        ("evaluate.py", "librarian"),
        ("registry_export.py", "librarian"),
        ("publish_skills.py", "librarian"),
    ];
    for (tool, _) in tools {
        room.run(tool, &["--help"], "");
    }
    let probe = "import common;common.trace(common.settings(),'search','public question','queries/demo.md',1.5)";
    assert!(
        room.command()
            .current_dir(repo_root().join("bin"))
            .args(["-c", probe])
            .status()
            .unwrap()
            .success()
    );
    let rows = room.wait_for_rows(4 + tools.len());
    for (tool, role) in tools {
        assert!(
            rows.iter()
                .any(|r| r["--prompt"] == format!("{tool} --help")
                    && r["--caller"] == format!("library_{role}")),
            "missing {tool}: {rows:?}"
        );
    }
    let get = rows
        .iter()
        .find(|r| r["--prompt"] == "mail.py --room demo-meeting get review")
        .unwrap();
    assert_eq!(get["--agent"], "sample reader");
    let day = Command::new(python())
        .args([
            "-c",
            "import datetime;print(datetime.date.today().strftime('%y%m%d'))",
        ])
        .output()
        .unwrap();
    assert_eq!(
        get["--session"],
        format!("library-{}", String::from_utf8_lossy(&day.stdout).trim())
    );
    assert_eq!(get["--rc"], "0");
    assert!(get["--output"].as_str().unwrap().contains("첫 답"));
    assert!(get["--elapsed"].as_str().unwrap().parse::<f64>().unwrap() >= 0.);
    assert!(
        rows.iter()
            .any(|r| r["--prompt"] == "task.py --room demo-meeting unknown" && r["--rc"] != "0")
    );
    assert!(rows.iter().any(|r| r["--prompt"] == "public question"
        && r["--caller"] == "library_search"
        && r["--output"] == "queries/demo.md"));
}
