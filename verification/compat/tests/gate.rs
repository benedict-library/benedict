use benedict_compat::{python, repo_root};
use std::process::Command;

#[test]
fn rejects_a_route_without_an_absolute_socket_path() {
    let out = Command::new(python())
        .arg(repo_root().join("bin/gate.py"))
        .args(["--route", "relative.sock=127.0.0.1:1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("route"));
}

#[cfg(unix)]
#[test]
fn a_selected_socket_forwards_a_request_and_response() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        os::unix::{fs::PermissionsExt, net::UnixStream},
        time::{Duration, Instant},
    };
    let server = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = server.local_addr().unwrap();
    let home = tempfile::tempdir_in("/tmp").unwrap();
    let socket = home.path().join("gate.sock");
    let child = Command::new(python())
        .arg(repo_root().join("bin/gate.py"))
        .args(["--route", &format!("{}={addr}", socket.display())])
        .spawn()
        .unwrap();
    struct Gate(std::process::Child);
    impl Drop for Gate {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _gate = Gate(child);
    let until = Instant::now() + Duration::from_secs(10);
    let mut local = loop {
        if let Ok(link) = UnixStream::connect(&socket) {
            break link;
        }
        assert!(Instant::now() < until, "gate never opened");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        socket.metadata().unwrap().permissions().mode() & 0o777,
        0o660
    );
    local
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    local.write_all(b"page").unwrap();
    server.set_nonblocking(true).unwrap();
    let mut remote = loop {
        if let Ok((s, _)) = server.accept() {
            break s;
        }
        assert!(Instant::now() < until, "gate never reached service");
        std::thread::sleep(Duration::from_millis(10));
    };
    remote
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = [0; 4];
    remote.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"page");
    remote.write_all(b"read").unwrap();
    local.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"read");
}
