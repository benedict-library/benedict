//! courier — run one shell command for an agent, with a stated reason.
//!
//!   courier --why "<reason>" [--timeout <seconds>] -- '<shell command>'
//!
//! - The full output (stdout and stderr together) goes to a private file; the agent sees all of it when it is
//!   short, otherwise the first and last lines and a note that names the file and the hidden line range.
//! - Every run (and every refusal) is one JSON line in the ledger, with the reason.
//! - An optional deny list (`COURIER_DENY`) refuses accidents such as `rm -rf`. It is not a security boundary:
//!   encodings, indirection and other interpreters get past any pattern list. Isolation is the boundary; set
//!   `COURIER_SANDBOX` to a command prefix (a container or sandbox tool) and every command runs inside it.
//!
//! Settings (environment): COURIER_HOME (state folder, default ~/.local/state/library/courier),
//! COURIER_DENY (deny file), COURIER_SANDBOX (command prefix), COURIER_SHOW (characters shown in full, default 3000).

use regex::RegexBuilder;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{exit, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEAD: usize = 20;
const TAIL: usize = 40;
const LINE_MAX: usize = 400;

mod file;
mod handoff;

#[derive(Debug, PartialEq, Default)]
pub struct Args {
    pub why: String,
    pub timeout: u64,
    pub cmd: String,
    pub write: Option<String>,
    pub edit: Option<String>,
    pub replace: bool,
}

pub fn parse(args: &[String]) -> Args {
    let (mut why, mut timeout, mut i) = (String::new(), 300u64, 0);
    let (mut write, mut edit, mut replace) = (None, None, false);
    while i < args.len() {
        match args[i].as_str() {
            "--why" => {
                why = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--timeout" => {
                timeout = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(300);
                i += 2;
            }
            "--write" => {
                write = args.get(i + 1).cloned();
                i += 2;
            }
            "--edit" => {
                edit = args.get(i + 1).cloned();
                i += 2;
            }
            "--replace" => {
                replace = true;
                i += 1;
            }
            "--" => {
                i += 1;
                break;
            }
            _ => break,
        }
    }
    Args { why: why.trim().to_string(), timeout, cmd: args[i.min(args.len())..].join(" "), write, edit, replace }
}

/// `--write` / `--edit`: content or blocks from stdin; one short line back.
fn file_work(a: &Args, dir: &PathBuf, iso: &str, deny: &Option<Vec<(String, String)>>) -> ! {
    let (mode, path) = match (&a.write, &a.edit) {
        (Some(p), None) => ("write", p.clone()),
        (None, Some(p)) => ("edit", p.clone()),
        _ => refuse("use one of --write <path> or --edit <path>"),
    };
    if let Some(rules) = deny {
        match denied(rules, &format!("{mode} {path}")) {
            Ok(Some(name)) => {
                ledger(dir, serde_json::json!({"at": iso, "why": a.why, mode: path, "refused": name}));
                refuse(&format!("{name} (deny list)"));
            }
            Ok(None) => {}
            Err(e) => refuse(&e),
        }
    }
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        refuse(&format!("cannot read stdin: {e}"));
    }
    let before = std::fs::read_to_string(&path).ok();
    let after = if mode == "write" {
        if before.is_some() && !a.replace {
            refuse(&format!("{path} exists — add --replace to overwrite, or use --edit"));
        }
        input
    } else {
        let Some(orig) = &before else { refuse(&format!("{path} does not exist")) };
        match file::blocks(&input).and_then(|b| file::apply(orig, &b)) {
            Ok(s) => s,
            Err(e) => refuse(&format!("{path}: {e} (nothing changed)")),
        }
    };
    // write in place so the file keeps its owner and mode
    if let Err(e) = std::fs::write(&path, &after) {
        refuse(&format!("cannot write {path}: {e}"));
    }
    let (was, now) = (before.as_deref().map(file::line_count), file::line_count(&after));
    ledger(dir, serde_json::json!({"at": iso, "why": a.why, mode: path, "lines_before": was, "lines_after": now}));
    match was {
        Some(w) => println!("[courier] {mode} {path} · {now} lines (was {w})"),
        None => println!("[courier] {mode} {path} · {now} lines (new file)"),
    }
    exit(0)
}

/// Deny file: one rule per line, `name: regex` (or just `regex`); blank lines and `#` lines are skipped.
pub fn deny_rules(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.split_once(": ") {
            Some((n, p)) if n.chars().count() <= 40 => (n.trim().to_string(), p.trim().to_string()),
            _ => (l.to_string(), l.to_string()),
        })
        .collect()
}

/// The name of the first rule the command matches (case-insensitive). A rule that does not compile is an error.
pub fn denied(rules: &[(String, String)], cmd: &str) -> Result<Option<String>, String> {
    for (name, pat) in rules {
        let re = RegexBuilder::new(pat).case_insensitive(true).build().map_err(|e| format!("deny rule '{name}': {e}"))?;
        if re.is_match(cmd) {
            return Ok(Some(name.clone()));
        }
    }
    Ok(None)
}

fn clip_line(l: &str) -> String {
    if l.chars().count() <= LINE_MAX { l.to_string() } else { l.chars().take(LINE_MAX).collect::<String>() + "…" }
}

/// What the agent sees: everything when short, otherwise head and tail with the hidden range named.
/// The head takes up to HEAD lines within a third of the limit, the tail up to TAIL lines within the rest
/// (at least one line each), so a long output never shows more than about the limit.
pub fn shown(out: &str, full: &str, show_all: usize) -> String {
    if out.chars().count() <= show_all {
        return out.to_string();
    }
    let lines: Vec<String> = out.lines().map(clip_line).collect();
    let size = |l: &String| l.chars().count() + 1;
    let (mut head, mut used) = (0, 0);
    while head < lines.len() && head < HEAD && (head == 0 || used + size(&lines[head]) <= show_all / 3) {
        used += size(&lines[head]);
        head += 1;
    }
    let (mut tail, mut used_tail) = (0, 0);
    while head + tail < lines.len() && tail < TAIL
        && (tail == 0 || used_tail + size(&lines[lines.len() - 1 - tail]) <= show_all - show_all / 3)
    {
        used_tail += size(&lines[lines.len() - 1 - tail]);
        tail += 1;
    }
    let cut = lines.len() - head - tail;
    if cut == 0 {
        return lines.join("\n");
    }
    let mut s: Vec<String> = lines[..head].to_vec();
    s.push(format!("… {cut} lines hidden (lines {}-{} of {full})", head + 1, head + cut));
    s.extend(lines[lines.len() - tail..].iter().cloned());
    s.join("\n")
}

/// Single-quote a string for the shell.
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The script bash runs: the command with stderr joined, inside the sandbox prefix when one is set.
pub fn script(cmd: &str, sandbox: Option<&str>) -> String {
    let inner = format!("{{ {cmd}\n}} 2>&1");
    match sandbox {
        Some(p) => format!("{{ {p} bash -c {}\n}} 2>&1", sq(&inner)),
        None => inner,
    }
}

/// UTC time as (YYYYMMDD-HHMMSS, ISO 8601).
pub fn stamp(secs: u64) -> (String, String) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    (format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}"), format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z"))
}

fn home_state() -> PathBuf {
    if let Ok(h) = std::env::var("COURIER_HOME") {
        return PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/state/library/courier")
}

fn ledger(dir: &PathBuf, row: serde_json::Value) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join("ledger.jsonl")) {
        let _ = writeln!(f, "{row}");
    }
}

fn refuse(msg: &str) -> ! {
    eprintln!("[courier] refused: {msg}");
    exit(2)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if matches!(argv.first().map(String::as_str), Some("handoff" | "deliver")) {
        match handoff::run(&argv) {
            Ok(code) => exit(code),
            Err(error) => refuse(&error),
        }
    }
    if argv.is_empty() || matches!(argv.first().map(String::as_str), Some("-h" | "--help")) {
        println!("courier --why \"<reason>\" [--timeout <seconds>] -- '<shell command>'\ncourier handoff ENTRY LIBRARY REQUEST_ID NEW_RECEIPT -- '<host command>'\ncourier deliver ENTRY RECEIPT");
        return;
    }
    let a = parse(&argv);
    if a.why.is_empty() {
        refuse("no reason → --why \"<reason>\"");
    }
    if a.write.is_none() && a.edit.is_none() && a.cmd.trim().is_empty() {
        refuse("no command → courier --why \"<reason>\" -- '<command>'");
    }
    let dir = home_state();
    let _ = std::fs::create_dir_all(dir.join("runs"));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (day, iso) = stamp(now);
    let cmd_short: String = a.cmd.chars().take(300).collect();

    // deny list (optional): not set is shown as "not set", a broken file is a failure
    let deny = match std::env::var("COURIER_DENY") {
        Ok(p) => match std::fs::read_to_string(&p) {
            Ok(t) => Some(deny_rules(&t)),
            Err(e) => refuse(&format!("COURIER_DENY {p}: {e}")),
        },
        Err(_) => None,
    };
    if a.write.is_some() || a.edit.is_some() {
        if std::env::var_os("COURIER_SANDBOX").is_some_and(|v| !v.to_string_lossy().trim().is_empty()) {
            ledger(&dir, serde_json::json!({"at": iso, "why": a.why, "refused":"host file mode disabled under COURIER_SANDBOX"}));
            refuse("direct file mode is unavailable under COURIER_SANDBOX; run the file operation inside the configured sandbox");
        }
        file_work(&a, &dir, &iso, &deny);
    }
    if let Some(rules) = &deny {
        match denied(rules, &a.cmd) {
            Ok(Some(name)) => {
                ledger(&dir, serde_json::json!({"at": iso, "why": a.why, "cmd": cmd_short, "refused": name}));
                refuse(&format!("{name} (deny list)"));
            }
            Ok(None) => {}
            Err(e) => refuse(&e),
        }
    }
    let sandbox = std::env::var("COURIER_SANDBOX").ok().filter(|s| !s.trim().is_empty());
    let show_all = std::env::var("COURIER_SHOW").ok().and_then(|s| s.parse().ok()).unwrap_or(3000usize);

    let t0 = Instant::now();
    let mut child = match Command::new("bash")
        .arg("-c")
        .arg(script(&a.cmd, sandbox.as_deref()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => refuse(&format!("cannot start bash: {e}")),
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    // wake as soon as the child ends; past the limit, kill the whole process group
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait().map(|st| st.code().unwrap_or(1)).unwrap_or(1));
    });
    let code = match rx.recv_timeout(Duration::from_secs(a.timeout)) {
        Ok(c) => c,
        Err(_) => {
            let _ = Command::new("kill").args(["-9", "--", &format!("-{pid}")]).stderr(Stdio::null()).status();
            124
        }
    };
    let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    let ms = t0.elapsed().as_millis() as u64;
    let full = dir.join("runs").join(format!("{day}-{}.txt", std::process::id()));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&full) {
        let _ = f.write_all(out.as_bytes());
    }
    let fp = full.to_string_lossy().into_owned();
    let preview_source = if std::env::var_os("COURIER_ROOM_PREVIEW").is_some() { "private host log" } else { &fp };
    let show = shown(&out, preview_source, show_all);
    let (n_out, n_show) = (out.chars().count(), show.chars().count());
    ledger(&dir, serde_json::json!({"at": iso, "why": a.why, "cmd": cmd_short, "code": code, "ms": ms,
                                    "out_chars": n_out, "shown_chars": n_show, "full": fp,
                                    "sandbox": sandbox.is_some(), "deny": deny.is_some()}));
    print!("{show}");
    if !show.is_empty() && !show.ends_with('\n') {
        println!();
    }
    let timeout_note = if code == 124 { format!(" · stopped at {}s limit", a.timeout) } else { String::new() };
    let sandbox_note = if sandbox.is_some() { "sandbox on" } else { "sandbox not set" };
    println!("[courier] exit {code} · {:.1}s · shown {n_show} of {n_out} chars · {sandbox_note}{timeout_note}", ms as f64 / 1000.0);
    exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_reason_timeout_command() {
        let a = parse(&v(&["--why", " build ", "--timeout", "9", "--", "cargo", "build"]));
        assert_eq!(a, Args { why: "build".into(), timeout: 9, cmd: "cargo build".into(), ..Default::default() });
        let w = parse(&v(&["--why", "w", "--write", "/tmp/f", "--replace"]));
        assert_eq!((w.write.as_deref(), w.replace, w.cmd.as_str()), (Some("/tmp/f"), true, ""));
    }

    #[test]
    fn deny_file_and_match() {
        let rules = deny_rules("# accidents\nrm -rf: \\brm\\s+-\\S*r\\S*f\n\nforce push: git\\s+push\\b.*--force\n");
        assert_eq!(rules.len(), 2);
        assert_eq!(denied(&rules, "RM -rf /tmp/x").unwrap(), Some("rm -rf".into()));
        assert_eq!(denied(&rules, "git push --force origin main").unwrap(), Some("force push".into()));
        assert_eq!(denied(&rules, "git push origin main").unwrap(), None);
        assert!(denied(&deny_rules("bad: ("), "x").is_err());
    }

    #[test]
    fn short_output_is_whole_long_output_names_hidden_range() {
        assert_eq!(shown("a\nb\n", "/f", 3000), "a\nb\n");
        let long: String = (1..=200).map(|i| format!("line {i} {}\n", "x".repeat(30))).collect();
        let s = shown(&long, "/f", 3000);
        assert!(s.starts_with("line 1 "));
        assert!(s.contains("… 140 lines hidden (lines 21-160 of /f)"));
        assert!(s.trim_end().ends_with(&format!("line 200 {}", "x".repeat(30))));
    }

    #[test]
    fn few_long_lines_stay_within_the_character_limit() {
        // 55 lines over the limit used to come back whole: the limit counts characters, not lines
        let out: String = (1..=55).map(|i| format!("/container-name-{i:02} 0\n")).collect();
        let s = shown(&out, "/f", 600);
        assert!(s.starts_with("/container-name-01 0"));
        assert!(s.trim_end().ends_with("/container-name-55 0"));
        assert!(s.contains("lines hidden (lines "), "{s}");
        let note = s.lines().find(|l| l.contains("lines hidden")).unwrap().chars().count();
        assert!(s.chars().count() <= 600 + note + 2, "{} chars", s.chars().count());
    }

    #[test]
    fn sandbox_prefix_wraps_and_quotes() {
        assert_eq!(script("echo hi", None), "{ echo hi\n} 2>&1");
        assert_eq!(script("echo 'a'", Some("env -i")), "{ env -i bash -c '{ echo '\\''a'\\''\n} 2>&1'\n} 2>&1");
    }

    #[test]
    fn utc_stamp() {
        assert_eq!(stamp(0), ("19700101-000000".into(), "1970-01-01T00:00:00Z".into()));
        // expected values from `date -u -d @<secs>`
        assert_eq!(stamp(1_794_810_871).1, "2026-11-16T06:34:31Z");
        assert_eq!(stamp(951_782_400).1, "2000-02-29T00:00:00Z");
    }
}
