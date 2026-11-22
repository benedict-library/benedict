// An installed Rust library must accept the historical tar format and retain every original byte.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn library(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_doseogwan"))
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .output()
        .unwrap()
}

fn success(out: Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn archives_are_compatible_and_invalid_restore_never_creates_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let bridge = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin/wiki_backup.py");
    let legacy = |args: &[&str]| {
        Command::new("python3")
            .arg(&bridge)
            .args(args)
            .current_dir(root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root)
            .output()
            .unwrap()
    };
    success(library(root, &["init", "--demo"]));
    let original = b"# title\r\nbody\r\n";
    fs::write(root.join("wiki").join("a b.md"), original).unwrap();
    success(library(root, &["backup", "snapshot", "--out", "rust.tar"]));
    success(legacy(&[
        "restore",
        "--archive",
        "rust.tar",
        "--target",
        "old-client",
    ]));
    assert_eq!(fs::read(root.join("old-client/a b.md")).unwrap(), original);
    success(legacy(&[
        "snapshot",
        "--config",
        "library.json",
        "--out",
        "python.tar",
    ]));
    success(library(
        root,
        &[
            "backup",
            "restore",
            "--archive",
            "python.tar",
            "--target",
            "new-client",
        ],
    ));
    assert_eq!(fs::read(root.join("new-client/a b.md")).unwrap(), original);
    let old = fs::read(root.join("rust.tar")).unwrap();
    assert!(
        !library(root, &["backup", "snapshot", "--out", "rust.tar"])
            .status
            .success()
    );
    assert_eq!(fs::read(root.join("rust.tar")).unwrap(), old);
    let bad = root.join("bad");
    fs::create_dir(&bad).unwrap();
    success(
        Command::new("tar")
            .current_dir(&bad)
            .args(["xf", "../rust.tar"])
            .output()
            .unwrap(),
    );
    for field in ["files", "bundle_sha256"] {
        let manifest = fs::read(bad.join("manifest.json")).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        if field == "files" {
            value["files"]["a b.md"] = "mismatch".into();
        } else {
            value[field] = "mismatch".into();
        }
        fs::write(bad.join("manifest.json"), value.to_string()).unwrap();
        success(
            Command::new("tar")
                .current_dir(&bad)
                .args(["cf", "../bad.tar", "manifest.json", "wiki.bundle"])
                .output()
                .unwrap(),
        );
        assert!(
            !library(
                root,
                &[
                    "backup",
                    "restore",
                    "--archive",
                    "bad.tar",
                    "--target",
                    "rejected"
                ]
            )
            .status
            .success()
        );
        assert!(!root.join("rejected").exists());
        fs::write(bad.join("manifest.json"), manifest).unwrap();
    }
}
