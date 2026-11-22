//! Drive the shipped Python compatibility APIs and exchange JSON with Rust assertions.
#![allow(dead_code)]
use benedict_compat::{python, repo_root};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

pub fn call(cwd: &Path, code: &str, input: &Value, env: &[(&str, &str)]) -> Output {
    let root = repo_root();
    let mut cmd = Command::new(python());
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("LIBRARY_") || name.starts_with("BENEDICT_") {
            cmd.env_remove(key);
        }
    }
    cmd.current_dir(cwd)
        .env(
            "PYTHONPATH",
            std::env::join_paths([root.clone(), root.join("bin"), root.join("runtime")]).unwrap(),
        )
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("HOME", cwd)
        .envs(env.iter().copied())
        .arg("-c")
        .arg(format!(
            "import json, sys, os\nfrom pathlib import Path\ndata = json.load(sys.stdin)\n{code}"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), input).unwrap();
    child.wait_with_output().unwrap()
}

pub fn result(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: Vec<u8>,
}
impl Response {
    pub fn new(status: u16, mime: &str, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), mime.into())],
            bytes: bytes.into(),
        }
    }
    pub fn json(value: Value) -> Self {
        Self::new(200, "application/json", value.to_string())
    }
}

pub struct Server {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn new(handle: impl Fn(&str, &[u8]) -> Response + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (history, shutdown) = (Arc::clone(&requests), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                let mut size = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, v)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            size = v.trim().parse().unwrap();
                        }
                    }
                }
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes).unwrap();
                let r = handle(&path, &bytes);
                history.lock().unwrap().push((path, bytes));
                write!(
                    stream,
                    "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
                    r.status,
                    r.bytes.len()
                )
                .unwrap();
                for (k, v) in r.headers {
                    write!(stream, "{k}: {v}\r\n").unwrap();
                }
                stream.write_all(b"\r\n").unwrap();
                let _ = stream.write_all(&r.bytes);
            }
        });
        Self {
            addr,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn embeddings() -> Self {
        Self::new(|path, bytes| {
            if path == "/health" {
                return Response::json(json!({"status":"ok"}));
            }
            assert_eq!(path, "/v1/embeddings");
            let request: Value = serde_json::from_slice(bytes).unwrap();
            let items = &request["input"];
            if items.as_str().is_some_and(|s| s.len() > 2048) {
                return Response::new(413, "text/plain", "too long");
            }
            let n = items.as_array().map_or(1, Vec::len);
            Response::json(json!({"data":vec![json!({"embedding":[0.25,0.5]});n]}))
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            if !thread::panicking() {
                t.join().unwrap();
            }
        }
    }
}
