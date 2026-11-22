// One Benedict version: every crate, the newest CHANGELOG release and `benedict version` say the same number,
// so an adopter can tell which release is installed and read what changed since theirs.
use std::{fs, path::Path, process::Command};

fn crate_version(manifest: &Path) -> String {
    let text = fs::read_to_string(manifest).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("version = \""))
        .unwrap_or_else(|| panic!("{} has no version", manifest.display()));
    line.trim_start_matches("version = \"").trim_end_matches('"').to_string()
}

#[test]
fn every_crate_the_changelog_and_the_cli_name_one_version() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let version = env!("CARGO_PKG_VERSION");
    for dir in ["core", "courier", "native", "plaza", "verification/compat", "verification/handoff"] {
        assert_eq!(crate_version(&repo.join(dir).join("Cargo.toml")), version, "{dir}/Cargo.toml");
        let lock = fs::read_to_string(repo.join(dir).join("Cargo.lock")).unwrap();
        let name = fs::read_to_string(repo.join(dir).join("Cargo.toml")).unwrap();
        let name = name.lines().find(|l| l.starts_with("name = \"")).unwrap();
        assert!(lock.contains(&format!("{name}\nversion = \"{version}\"")), "{dir}/Cargo.lock");
    }
    let changelog = fs::read_to_string(repo.join("CHANGELOG.md")).unwrap();
    let newest = changelog
        .lines()
        .find(|l| l.starts_with("## [") && !l.starts_with("## [Unreleased]"))
        .unwrap();
    assert!(newest.starts_with(&format!("## [{version}] - ")), "newest release in CHANGELOG.md: {newest}");
    let out = Command::new(env!("CARGO_BIN_EXE_benedict")).arg("version").output().unwrap();
    assert!(out.status.success());
    let printed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["version"], version);
}
