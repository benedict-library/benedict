use benedict_compat::{python, repo_root};
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn checked(cmd: &mut Command) -> String {
    let result = cmd.output().unwrap();
    assert!(
        result.status.success(),
        "{cmd:?}: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn copy_sources(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|ext| ext == "py") {
            fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}
#[test]
fn source_checkout_can_hand_over_a_filed_answer_in_an_isolated_home() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("source");
    copy_sources(&repo_root().join("bin"), &source.join("bin"));
    fs::copy(
        repo_root().join("library_local.py"),
        source.join("library_local.py"),
    )
    .unwrap();
    let settings = root.join(".config/library");
    fs::create_dir_all(&settings).unwrap();
    fs::write(
        settings.join("search.env"),
        "LIBRARY_ROOM_CMD=invalid-command\n",
    )
    .unwrap();
    let invoke = |verb: &str, args: &[&str]| -> Value {
        let mut cmd = Command::new(python());
        cmd.arg("-I")
            .arg(source.join("bin/library.py"))
            .arg(verb)
            .arg("--config")
            .arg(root.join("library.json"))
            .args(args)
            .current_dir(root)
            .env("HOME", root)
            .env("USERPROFILE", root)
            .env("DATABASE_URL", "must-not-connect")
            .env("LIBRARY_ROOM_CMD", "invalid-command")
            .env_remove("BENEDICT_CLI");
        serde_json::from_str(&checked(&mut cmd)).unwrap()
    };
    invoke("init", &["--demo"]);
    let found = invoke("search", &["refund"]);
    let path = found["hits"][0]["path"].as_str().unwrap();
    assert!(
        invoke("read", &[path])["text"]
            .as_str()
            .unwrap()
            .contains("14 days")
    );
    let saved = invoke(
        "save",
        &[
            "Portable warranty",
            "The synthetic warranty lasts ninety days.",
            "--refs",
            path,
        ],
    );
    let filed = saved["path"].as_str().unwrap();
    assert_eq!(invoke("search", &["warranty"])["hits"][0]["path"], filed);
    assert!(
        invoke("read", &[filed])["text"]
            .as_str()
            .unwrap()
            .contains("ninety")
    );
}
fn bash() -> PathBuf {
    if cfg!(windows) {
        for dir in env::split_paths(&env::var_os("PATH").unwrap()) {
            if dir.join("git.exe").is_file() {
                if let Some(parent) = dir.parent() {
                    let exe = parent.join("bin/bash.exe");
                    if exe.is_file() {
                        return exe;
                    }
                }
            }
        }
        panic!("Git Bash is required on Windows");
    }
    "bash".into()
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
#[test]
fn python_compatibility_quickstart_files_a_searchable_answer() {
    for name in ["docs/python-compat.md"] {
        let temp = tempfile::tempdir().unwrap();
        let doc = fs::read_to_string(repo_root().join(name)).unwrap();
        let source = doc
            .split("```bash")
            .skip(1)
            .map(|block| block.split_once("```").unwrap().0)
            .find(|block| block.contains("bin/library.py init"))
            .unwrap();
        let root = shell_quote(
            &temp
                .path()
                .join("demo")
                .to_string_lossy()
                .replace('\\', "/"),
        );
        let py = checked(Command::new(python()).args(["-c", "import sys;print(sys.executable)"]));
        let py = shell_quote(&py.trim().replace('\\', "/"));
        let script = format!(
            "python3() {{ {py} \"$@\"; }}\n{}",
            source.replace("/tmp/library-demo", &root)
        );
        let output = checked(
            Command::new(bash())
                .args(["-e", "-c", &script])
                .current_dir(repo_root())
                .env("HOME", temp.path())
                .env_remove("LIBRARY_ROOM_HOME")
                .env_remove("BENEDICT_CLI"),
        );
        let rows: Vec<Value> = output
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert!(rows.iter().any(|r| r["saved"] == true));
        assert!(!rows.last().unwrap()["hits"].as_array().unwrap().is_empty());
    }
}
#[test]
fn configured_commands_preserve_os_quoting() {
    let wins = [
        (
            r"C:\demo\python.exe D:\suite\room.py",
            vec![r"C:\demo\python.exe", r"D:\suite\room.py"],
        ),
        (
            r#""C:\Program Files\py.exe" -u "a b""#,
            vec![r"C:\Program Files\py.exe", "-u", "a b"],
        ),
        (
            r"'C:\demo\python.exe' 'D:\suite\room.py' exec",
            vec![r"C:\demo\python.exe", r"D:\suite\room.py", "exec"],
        ),
    ];
    for (os, line, wanted) in wins.into_iter().map(|(l, w)| ("1", l, w)).chain([(
        "0",
        "timeout 20 python3 '/x y/z.py' --agent {who}",
        vec!["timeout", "20", "python3", "/x y/z.py", "--agent", "{who}"],
    )]) {
        let out=checked(Command::new(python()).current_dir(repo_root().join("bin")).args(["-c","import json,sys,search; print(json.dumps(search.split_cmd(sys.argv[1], windows=sys.argv[2]=='1'))) ",line,os]));
        assert_eq!(serde_json::from_str::<Vec<String>>(&out).unwrap(), wanted);
    }
}
