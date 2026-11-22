use benedict_compat::{python, repo_root};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

#[path = "data/evaluate.rs"]
mod evaluate;
#[path = "data/filing.rs"]
mod filing;
#[path = "data/librarian.rs"]
mod librarian;
#[path = "data/local.rs"]
mod local;
#[path = "data/skills.rs"]
mod skills;

struct Library(TempDir);
impl Library {
    fn empty() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn demo() -> Self {
        let l = Self::empty();
        l.ok(&["init", "--demo"]);
        l
    }
    fn path(&self, p: &str) -> PathBuf {
        self.0.path().join(p)
    }
    fn cfg(&self) -> String {
        self.path("library.json").display().to_string()
    }
    fn cmd(&self, exe: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(exe);
        c.env_clear().current_dir(self.0.path());
        for k in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "TMPDIR",
            "LD_LIBRARY_PATH",
            "DYLD_LIBRARY_PATH",
        ] {
            if let Some(v) = env::var_os(k) {
                c.env(k, v);
            }
        }
        c.env("HOME", self.0.path())
            .env("USERPROFILE", self.0.path())
            .env("PYTHONUTF8", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("LIBRARY_SEARCH_ENV", self.path("absent.env"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.path("empty.gitconfig"));
        c
    }
    fn py(&self, name: &str) -> Command {
        let mut c = self.cmd(python());
        c.arg(repo_root().join("bin").join(name));
        c
    }
    fn bridge(&self, body: &str, args: &[&str]) -> Output {
        let mut c = self.cmd(python());
        c.current_dir(repo_root())
            .arg("-c")
            .arg(format!(
                "import sys, json, os\nfrom pathlib import Path\nsys.path.insert(0,'bin')\n{body}"
            ))
            .args(args)
            .output()
            .unwrap()
    }
    fn cli(&self, args: &[&str]) -> Command {
        let mut c = self.py("library.py");
        c.arg(args[0])
            .args(["--config", &self.cfg()])
            .args(&args[1..]);
        c
    }
    fn ok(&self, args: &[&str]) -> Value {
        value(self.cli(args).output().unwrap(), 0)
    }
    fn reject(&self, args: &[&str]) -> String {
        let o = self.cli(args).output().unwrap();
        assert!(!o.status.success(), "{args:?}");
        text(&o.stderr)
    }
    fn put(&self, p: &str, body: impl AsRef<[u8]>) {
        let dest = self.path(p);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(dest, body).unwrap();
    }
    fn read(&self, p: &str) -> String {
        fs::read_to_string(self.path(p))
            .unwrap()
            .replace("\r\n", "\n")
    }
    fn registry(&self) -> Value {
        serde_json::from_str(&self.read("library.json")).unwrap()
    }
    fn registry_set(&self, v: &Value) {
        self.put("library.json", v.to_string());
    }
    fn choose(&self, opts: Value) {
        let mut reg = self.registry();
        for (k, v) in opts.as_object().unwrap() {
            reg["libraries"]["main"][k] = v.clone();
        }
        self.registry_set(&reg);
    }
}
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}
fn checked(o: Output, code: i32) -> Output {
    assert_eq!(
        o.status.code(),
        Some(code),
        "{} {}",
        text(&o.stderr),
        text(&o.stdout)
    );
    o
}
fn value(o: Output, code: i32) -> Value {
    serde_json::from_slice(&checked(o, code).stdout).unwrap()
}
fn field(v: &Value) -> &str {
    v.as_str().unwrap()
}
fn files(p: &Path) -> Vec<PathBuf> {
    if !p.is_dir() {
        return Vec::new();
    }
    fs::read_dir(p)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}
fn tree(p: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for x in files(p) {
        if x.is_dir() && !x.is_symlink() {
            out.extend(tree(&x));
        } else {
            out.push(x);
        }
    }
    out
}
fn link(src: &Path, dst: &Path, dir: bool) {
    #[cfg(unix)]
    {
        let _ = dir;
        std::os::unix::fs::symlink(src, dst).unwrap();
    }
    #[cfg(windows)]
    {
        if dir {
            std::os::windows::fs::symlink_dir(src, dst)
        } else {
            std::os::windows::fs::symlink_file(src, dst)
        }
        .unwrap();
    }
}
