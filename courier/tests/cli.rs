//! End-to-end runs of the built binary in a fresh state folder.
use std::process::Command;

fn run(state: &std::path::Path, env: &[(&str, &str)], args: &[&str]) -> (i32, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_courier"));
    c.args(args).env("COURIER_HOME", state).env_remove("COURIER_DENY").env_remove("COURIER_SANDBOX");
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().expect("run courier");
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

fn fresh(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("courier-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn runs_keeps_exit_code_and_ledger() {
    let st = fresh("run");
    let (code, out, _) = run(&st, &[], &["--why", "check", "--", "echo out; echo err >&2; exit 3"]);
    assert_eq!(code, 3);
    assert!(out.contains("out\nerr\n"));
    assert!(out.contains("[courier] exit 3"));
    assert!(out.contains("sandbox not set"));
    let ledger = std::fs::read_to_string(st.join("ledger.jsonl")).unwrap();
    assert!(ledger.contains("\"why\":\"check\"") && ledger.contains("\"code\":3"));
}

#[test]
fn refuses_without_reason_or_command() {
    let st = fresh("refuse");
    assert_eq!(run(&st, &[], &["--", "true"]).0, 2);
    assert_eq!(run(&st, &[], &["--why", "x", "--"]).0, 2);
}

#[test]
fn deny_list_refuses_and_records() {
    let st = fresh("deny");
    std::fs::create_dir_all(&st).unwrap();
    let deny = st.join("deny");
    std::fs::write(&deny, "rm -rf: \\brm\\s+-\\S*r\\S*f\n").unwrap();
    let (code, _, err) = run(&st, &[("COURIER_DENY", deny.to_str().unwrap())], &["--why", "x", "--", "rm -rf /nonexistent-courier"]);
    assert_eq!(code, 2);
    assert!(err.contains("rm -rf (deny list)"));
    assert!(std::fs::read_to_string(st.join("ledger.jsonl")).unwrap().contains("\"refused\":\"rm -rf\""));
    let (code, _, err) = run(&st, &[("COURIER_DENY", "/nonexistent/deny")], &["--why", "x", "--", "true"]);
    assert_eq!(code, 2, "a deny file that cannot be read is a failure, not an open door: {err}");
}

#[test]
fn long_output_is_cut_and_kept_in_full() {
    let st = fresh("long");
    let (code, out, _) = run(&st, &[], &["--why", "x", "--", "seq 1 5000"]);
    assert_eq!(code, 0);
    assert!(out.contains("lines hidden (lines 21-4960 of "));
    let full = out.split(" of ").nth(1).unwrap().split(')').next().unwrap();
    assert_eq!(std::fs::read_to_string(full).unwrap().lines().count(), 5000);
}

#[test]
fn sandbox_prefix_is_used() {
    let st = fresh("sandbox");
    let (code, out, _) = run(&st, &[("COURIER_SANDBOX", "env COURIER_INSIDE=yes")], &["--why", "x", "--", "echo $COURIER_INSIDE"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("yes\n") && out.contains("sandbox on"));
}

fn run_in(state: &std::path::Path, env: &[(&str, &str)], args: &[&str], stdin: &str) -> (i32, String, String) {
    use std::io::Write;
    let mut c = Command::new(env!("CARGO_BIN_EXE_courier"));
    c.args(args).env("COURIER_HOME", state).env_remove("COURIER_DENY").env_remove("COURIER_SANDBOX");
    for (k, v) in env {
        c.env(k, v);
    }
    let mut ch = c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    ch.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let o = ch.wait_with_output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

#[test]
fn write_then_edit_with_reason_in_ledger() {
    let st = fresh("write");
    std::fs::create_dir_all(&st).unwrap();
    let f = st.join("note.txt");
    let p = f.to_str().unwrap();
    let (code, out, _) = run_in(&st, &[], &["--why", "make note", "--write", p], "one\ntwo\n");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("2 lines (new file)"));
    assert_eq!(run_in(&st, &[], &["--why", "again", "--write", p], "x\n").0, 2, "no silent overwrite");
    let (code, out, _) = run_in(&st, &[], &["--why", "fix", "--edit", p], "<<<<<<< old\ntwo\n=======\n2\n>>>>>>> new\n");
    assert_eq!(code, 0, "{out}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "one\n2\n");
    let (code, _, err) = run_in(&st, &[], &["--why", "fix", "--edit", p], "<<<<<<< old\nmissing\n=======\nz\n>>>>>>> new\n");
    assert_eq!(code, 2);
    assert!(err.contains("nothing changed"));
    assert_eq!(run_in(&st, &[], &["--why", "r", "--write", p, "--replace"], "z\n").0, 0);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "z\n");
    let ledger = std::fs::read_to_string(st.join("ledger.jsonl")).unwrap();
    assert!(ledger.contains("\"why\":\"make note\"") && ledger.contains("\"why\":\"fix\""));
}

#[test]
fn deny_list_covers_file_paths() {
    let st = fresh("denyfile");
    std::fs::create_dir_all(&st).unwrap();
    let deny = st.join("deny");
    std::fs::write(&deny, "secrets: secrets\\.env\n").unwrap();
    let target = st.join("secrets.env");
    let (code, _, err) = run_in(&st, &[("COURIER_DENY", deny.to_str().unwrap())], &["--why", "x", "--write", target.to_str().unwrap()], "K=V\n");
    assert_eq!(code, 2);
    assert!(err.contains("secrets (deny list)") && !target.exists());
}

#[test]
fn timeout_kills_the_group() {
    let st = fresh("timeout");
    let t = std::time::Instant::now();
    let (code, out, _) = run(&st, &[], &["--why", "x", "--timeout", "1", "--", "sleep 30 & sleep 30"]);
    assert_eq!(code, 124);
    assert!(out.contains("stopped at 1s limit"));
    assert!(t.elapsed().as_secs() < 5);
}

#[test]
fn file_modes_never_touch_the_host_when_a_sandbox_is_configured() {
    let st = fresh("sandbox-file");
    std::fs::create_dir_all(&st).unwrap();
    let file = st.join("protected");
    std::fs::write(&file, "before").unwrap();
    for option in ["--write", "--edit"] {
        let (code, _, _) = run(&st, &[("COURIER_SANDBOX","false")],
            &["--why","check", option,file.to_str().unwrap(),"--replace"]);
        assert_eq!(code, 2);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    }
}
