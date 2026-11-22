use super::*;

const PROCEDURE: &str = "---\nname: sample\ndescription: >-\n  Identify amber markers\n  in the sample catalog.\n---\n# Sample\n\nCheck the marker table.\n";
struct Source {
    lib: Library,
    first: String,
}
impl Source {
    fn new() -> Self {
        let l = Library::demo();
        l.put("source/skills/sample/SKILL.md", PROCEDURE);
        let mut s = Self {
            lib: l,
            first: String::new(),
        };
        s.git(&["init", "-q"]);
        s.git(&["config", "user.email", "reader@example.invalid"]);
        s.git(&["config", "user.name", "Example Reader"]);
        s.git(&["config", "core.autocrlf", "false"]);
        s.commit("first");
        s.first = s.git(&["rev-parse", "HEAD"]);
        s
    }
    fn git(&self, args: &[&str]) -> String {
        text(
            &checked(
                self.lib
                    .cmd("git")
                    .arg("-C")
                    .arg(self.lib.path("source"))
                    .args(args)
                    .output()
                    .unwrap(),
                0,
            )
            .stdout,
        )
        .trim()
        .to_owned()
    }
    fn commit(&self, label: &str) {
        self.git(&["add", "."]);
        self.git(&["commit", "-qm", label]);
    }
    fn run(&self, args: &[&str]) -> Output {
        self.lib
            .py("publish_skills.py")
            .args([
                "--config",
                &self.lib.cfg(),
                "--library",
                "main",
                "--repo",
                "source",
            ])
            .args(args)
            .output()
            .unwrap()
    }
    fn publish(&self, args: &[&str]) -> Value {
        value(self.run(args), 0)
    }
    fn refuse(&self, args: &[&str]) {
        assert!(!self.run(args).status.success());
    }
    fn body(&self) -> String {
        self.lib.read("wiki/entities/skill_sample.md")
    }
}
#[test]
fn committed_skill_provenance_and_revision_history() {
    let s = Source::new();
    let mut r = s.lib.registry();
    r["libraries"]["other"] = json!({"wiki_root":"other"});
    s.lib.registry_set(&r);
    fs::create_dir(s.lib.path("other")).unwrap();
    assert_eq!(s.publish(&["--dry-run"])["published"], json!(["sample"]));
    assert!(!s.lib.path("wiki/entities/skill_sample.md").exists());
    let url = "https://example.invalid/skills";
    assert_eq!(
        s.publish(&["--source-url", url])["published"],
        json!(["sample"])
    );
    let page = s.body();
    for fragment in [
        PROCEDURE,
        s.first.as_str(),
        &s.git(&["rev-parse", "HEAD:skills/sample"]),
        &format!("{url}/src/commit/{}", s.first),
    ] {
        assert!(page.contains(fragment));
    }
    // SHA-256 of the committed synthetic procedure, including its final newline.
    assert!(page.contains("12896c96ed820f750a46564e61c92080118c65c92e28978338375fee1748f22f"));
    assert!(files(&s.lib.path("other")).is_empty());
    assert_eq!(
        s.publish(&["--source-url", url])["unchanged"],
        json!(["sample"])
    );
    assert_eq!(s.body(), page);
    assert_eq!(
        s.lib.ok(&["search", "amber"])["hits"][0]["path"],
        "entities/skill_sample.md"
    );
    s.lib.put(
        "source/skills/sample/SKILL.md",
        format!("{PROCEDURE}Second revision"),
    );
    s.commit("second");
    assert_eq!(s.publish(&[])["published"], json!(["sample"]));
    let latest = s.body();
    assert!(latest.contains("Second revision"));
    assert_eq!(
        s.publish(&["--revision", &s.first])["stale_release_skipped"],
        json!(["sample"])
    );
    assert_eq!(s.body(), latest);
    s.git(&["checkout", "-q", &s.first]);
    s.lib.put(
        "source/skills/sample/SKILL.md",
        format!("{PROCEDURE}Alternate revision"),
    );
    s.commit("alternate");
    s.refuse(&[]);
    assert_eq!(s.body(), latest);
}

#[test]
fn publication_preflight_preserves_existing_material() {
    let s = Source::new();
    let card = "wiki/entities/skill_sample.md";
    s.refuse(&["--skill", "absent"]);
    s.refuse(&["--library", "absent"]);
    s.refuse(&["--source-url", "https://reader@example.invalid/path"]);
    s.lib.put("source/skills/sample/SKILL.md", "Draft");
    s.refuse(&[]);
    s.git(&["checkout", "--", "."]);
    s.lib.put("source/skills/sample/draft.txt", "Draft");
    s.refuse(&[]);
    fs::remove_file(s.lib.path("source/skills/sample/draft.txt")).unwrap();
    assert!(!s.lib.path(card).exists());
    s.lib.put(card, "Handwritten");
    s.refuse(&[]);
    assert_eq!(s.body(), "Handwritten");
    fs::remove_file(s.lib.path(card)).unwrap();
    s.lib.choose(json!({"save_block":{"color":"amber"}}));
    s.refuse(&[]);
    assert!(!s.lib.path(card).exists());
    s.lib.choose(json!({"save_block":{}}));
    s.lib.put("source/skills/second/SKILL.md", "Missing header");
    s.commit("incomplete");
    s.refuse(&[]);
    assert!(!s.lib.path(card).exists());
}
#[test]
fn publication_refuses_linked_source_or_destination() {
    let s = Source::new();
    let dest = s.lib.path("wiki/entities");
    fs::rename(&dest, s.lib.path("catalog")).unwrap();
    link(&s.lib.path("catalog"), &dest, true);
    s.refuse(&[]);
    assert!(!s.lib.path("catalog/skill_sample.md").exists());
    let s = Source::new();
    let skill = s.lib.path("source/skills/sample/SKILL.md");
    fs::remove_file(&skill).unwrap();
    s.lib.put("source/README.md", PROCEDURE);
    link(Path::new("../../README.md"), &skill, false);
    s.commit("linked document");
    s.refuse(&[]);
    assert!(!s.lib.path("wiki/entities/skill_sample.md").exists());
}

#[test]
fn approved_sources_are_checked_again_before_import() {
    let l = Library::demo();
    l.put("note.md", "First note");
    let args = ["import", "--library", "main", "wiki", "note.md"];
    let token = l.ok(&args)["confirmation"].clone();
    assert!(!l.path("wiki/imports").exists());
    l.put("note.md", "Adjusted note");
    let run = |token: &str| {
        l.cli(&[&args[..], &["--approve", token]].concat())
            .output()
            .unwrap()
    };
    assert!(!run(field(&token)).status.success());
    let token = l.ok(&args)["confirmation"].clone();
    for _ in 0..2 {
        checked(run(field(&token)), 0);
    }
    l.put("note.md", "Conflicting note");
    let token = l.ok(&args)["confirmation"].clone();
    assert!(!run(field(&token)).status.success());
    assert_eq!(l.read("wiki/imports/note.md"), "Adjusted note");
    let bytes = b"---\r\nname: marker\r\ndescription: Amber labels\r\n---\r\nUse the table.";
    l.put("marker/SKILL.md", bytes);
    l.put("marker/table.txt", "Seven");
    let args = ["import", "--library", "main", "skill", "marker"];
    let preview = l.ok(&args);
    assert_eq!(preview["files"], json!(["SKILL.md", "table.txt"]));
    let approve = [&args[..], &["--approve", field(&preview["confirmation"])]].concat();
    l.ok(&approve);
    let result = l.ok(&approve);
    let shelf = Path::new(field(&result["shelf"]));
    assert_eq!(
        fs::read(shelf.join("skills/marker/table.txt")).unwrap(),
        b"Seven"
    );
    let blob = checked(
        l.cmd("git")
            .arg("-C")
            .arg(shelf)
            .args(["show", "HEAD:skills/marker/SKILL.md"])
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(blob.stdout, bytes);
    assert!(
        l.ok(&["search", "amber"])
            .to_string()
            .contains("entities/skill_marker.md")
    );
}
#[cfg(unix)]
#[test]
fn approval_preserves_operating_system_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let l = Library::demo();
    l.put("note.md", "Example");
    let path = l.path("note.md");
    fs::set_permissions(&path, fs::Permissions::from_mode(0)).unwrap();
    if fs::read(&path).is_ok() {
        return;
    }
    let args = ["import", "--library", "main", "wiki", "note.md"];
    let preview = l.ok(&args);
    assert!(
        l.reject(&[&args[..], &["--approve", field(&preview["confirmation"])]].concat())
            .contains("Permission denied")
    );
    assert!(!l.path("wiki/imports/note.md").exists());
}
