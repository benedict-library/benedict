use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn a_new_agent_can_find_the_approved_skill_without_python() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    std::os::unix::fs::symlink(
        String::from_utf8(git.stdout).unwrap().trim(),
        bin.join("git"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_benedict"))
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", root)
            .env("LIBRARY_CONFIG", root.join("library.json"))
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
    };
    let ok = |args: &[&str]| -> Value {
        let output = run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    ok(&["init"]);
    let src = root.join("sample");
    fs::create_dir(&src).unwrap();
    let doc =
        "---\nname: sample\ndescription: >\n  Compare sources\n---\nCheck the original witness.";
    fs::write(src.join("SKILL.md"), doc).unwrap();
    let import = |name: &str| {
        let args = ["import", "--library", "main", "skill", name];
        let plan = ok(&args);
        let mut cmd = args.to_vec();
        cmd.extend(["--approve", plan["confirmation"].as_str().unwrap()]);
        run(&cmd)
    };
    for status in ["published", "unchanged"] {
        let output = import("sample");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result[status][0], "sample");
    }
    let hits = ok(&["search", "witness"]);
    let card = hits["hits"][0]["path"].as_str().unwrap();
    assert!(ok(&["read", card])["text"].as_str().unwrap().contains(doc));
    fs::write(
        src.join("SKILL.md"),
        doc.replace("sample", "private")
            .replace("witness.", "witness. token=madeup0123456789"),
    )
    .unwrap();
    assert!(!import("sample").status.success());
    assert!(!root.join("wiki/entities/skill_private.md").exists());
}
