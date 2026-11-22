mod search;
use benedict_compat::{python, repo_root};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
use tempfile::TempDir;

pub struct ResultText {
    pub status: Option<i32>,
    pub out: String,
    pub err: String,
}
impl ResultText {
    pub fn ok(self) -> String {
        assert_eq!(self.status, Some(0), "{}{}", self.out, self.err);
        self.out
    }
    pub fn failed(self) -> Self {
        assert_ne!(self.status, Some(0), "{}", self.out);
        self
    }
}
pub struct Library {
    _dir: TempDir,
    pub home: PathBuf,
    pub env: BTreeMap<String, String>,
}
impl Library {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home: PathBuf = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .into();
        let repo = repo_root();
        let quoted_python = python().to_string_lossy().replace('\\', "/");
        let runner = repo
            .join("bin/room.py")
            .to_string_lossy()
            .replace('\\', "/");
        let env = BTreeMap::from([
            (
                "LIBRARY_ROOM_HOME".into(),
                home.join("rooms").display().to_string(),
            ),
            (
                "LIBRARY_SEARCH_ENV".into(),
                home.join("search.env").display().to_string(),
            ),
            (
                "LIBRARY_ROOM_CMD".into(),
                format!("\"{quoted_python}\" \"{runner}\" exec"),
            ),
            ("LIBRARY_REGISTRY_ROOM".into(), "main-search".into()),
            (
                "LIBRARY_ROOM_LIB".into(),
                repo.join("runtime").display().to_string(),
            ),
            ("LIBRARY_MAIL_FROM".into(), "reader".into()),
            (
                "PYTHONPATH".into(),
                std::env::join_paths([repo.clone(), repo.join("bin"), repo.join("runtime")])
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]);
        Self {
            _dir: dir,
            home,
            env,
        }
    }
    pub fn path(&self, rel: &str) -> String {
        self.home
            .join(rel)
            .components()
            .collect::<PathBuf>()
            .display()
            .to_string()
    }
    pub fn write(&self, rel: &str, body: &str) -> String {
        let p: PathBuf = self.home.join(rel).components().collect();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p.display().to_string()
    }
    pub fn python(&self, args: &[&str], input: &str) -> ResultText {
        let mut cmd = Command::new(python());
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("LIBRARY_")
                || k == "DOSEOGWAN_CLI"
                || k == "DATABASE_URL"
            {
                cmd.env_remove(k);
            }
        }
        cmd.current_dir(&self.home)
            .envs(&self.env)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONDONTWRITEBYTECODE", "1");
        let mut proc = cmd
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if !input.is_empty() {
            proc.stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = proc.wait_with_output().unwrap();
        let decode = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n");
        ResultText {
            status: output.status.code(),
            out: decode(&output.stdout),
            err: decode(&output.stderr),
        }
    }
    pub fn cmd(&self, name: &str, args: &[&str], input: &str) -> ResultText {
        let prog = repo_root()
            .join("bin")
            .join(format!("{name}.py"))
            .display()
            .to_string();
        let mut argv = vec![prog.as_str()];
        argv.extend_from_slice(args);
        self.python(&argv, input)
    }
    pub fn room(&self, args: &[&str], input: &str) -> String {
        self.cmd("room", args, input).ok()
    }
    pub fn exec(&self, room: &str, source: &str) -> String {
        self.room(&["exec", room], source)
    }
    pub fn init(&self) {
        self.cmd(
            "library",
            &["init", "--config", &self.path("library.json"), "--demo"],
            "",
        )
        .ok();
    }
    pub fn sync(&self, args: &[&str]) -> Value {
        let cfg = self.path("library.json");
        let mut argv = vec!["--config", &cfg];
        argv.extend_from_slice(args);
        serde_json::from_str(&self.cmd("room_sync", &argv, "").ok()).unwrap()
    }
    pub fn registry(&self) -> Value {
        serde_json::from_slice(&fs::read(self.home.join("library.json")).unwrap()).unwrap()
    }
    pub fn set_registry(&self, v: &Value) {
        self.write("library.json", &v.to_string());
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        if let Some(root) = self.env.get("LIBRARY_ROOM_HOME")
            && let Ok(files) = fs::read_dir(root)
        {
            for entry in files.flatten() {
                if let Some(room) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.strip_suffix(".pid"))
                {
                    self.cmd("room", &["stop", room], "");
                }
            }
        }
    }
}
