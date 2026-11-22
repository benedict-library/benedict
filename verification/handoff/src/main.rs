//! Read-only grader for the synthetic Oak handoff. Agent runs happen before grading.
use serde_json::{json, Value};
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

type Check<T> = Result<T, String>;

fn output(cmd: &mut Command) -> Check<String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

fn text_at<'a>(value: &'a Value, key: &str) -> Check<&'a str> {
    value[key].as_str().ok_or_else(|| format!("missing {key}"))
}

fn includes(body: &str, words: &[&str]) -> Check<()> {
    let lower = body.to_lowercase();
    for word in words {
        if !lower.contains(word) {
            return Err(format!("missing evidence: {word}"));
        }
    }
    Ok(())
}

struct Trial {
    cli: PathBuf,
    registry: PathBuf,
}
impl Trial {
    fn run(&self, args: &[&str]) -> Check<String> {
        output(Command::new("python3").arg(&self.cli).args(args))
    }
    fn library(&self, op: &str, args: &[&str]) -> Check<Value> {
        let out = output(
            Command::new("python3")
                .arg(&self.cli)
                .arg(op)
                .arg("--config")
                .arg(&self.registry)
                .args(args),
        )?;
        serde_json::from_str(&out).map_err(|e| e.to_string())
    }
    fn read(&self, path: &str) -> Check<String> {
        Ok(text_at(
            &self.library("read", &["--library", "depot", path])?,
            "text",
        )?
        .to_owned())
    }
    fn find(&self, query: &str, prefix: &str) -> Check<String> {
        let found = self.library("search", &[query])?;
        let projects = found["near"]
            .as_array()
            .ok_or("central search missing linked projects")?;
        let depot = projects
            .iter()
            .find(|p| p["library"] == "depot")
            .ok_or("depot not linked")?;
        depot["hits"]
            .as_array()
            .ok_or("depot index unavailable")?
            .iter()
            .filter_map(|h| h["path"].as_str())
            .find(|p| p.starts_with(prefix))
            .map(str::to_owned)
            .ok_or_else(|| format!("no matching central result for {query}"))
    }
    fn grade(&self, stages: &mut Vec<Value>) -> Check<()> {
        self.run(&["method"])?;
        self.run(&["add", "--help"])?;
        stages.push(json!({"stage":"P","result":"pass"}));
        let card = self.find("depot dispatch skill", "entities/skill_")?;
        let body = self.read(&card)?;
        let raw = body
            .lines()
            .find_map(|l| {
                l.strip_prefix("<!-- library-skill-source ")
                    .and_then(|v| v.strip_suffix(" -->"))
            })
            .ok_or("missing skill provenance")?;
        let prov: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        let commit = text_at(&prov, "commit")?;
        let path = text_at(&prov, "path")?;
        let shelf = self
            .registry
            .parent()
            .ok_or("no registry directory")?
            .join("shelf");
        let original = output(
            Command::new("git")
                .arg("-C")
                .arg(shelf)
                .arg("show")
                .arg(format!("{commit}:{path}")),
        )?;
        includes(&original, &["lots of 6", "field"])?;
        if !body.contains(original.trim_end()) {
            return Err("skill differs from committed original".into());
        }
        stages.push(json!({"stage":"I/S","result":"pass","commit":commit}));
        let note = self.find("Oak camera inspection", "oak-inspection.md")?;
        includes(&self.read(&note)?, &["teal", "copper", "camera"])?;
        let decision = self.find("Which marker color", "queries/")?;
        let previous = self.read(&decision)?;
        includes(&previous, &["teal", "copper", "oak-inspection.md"])?;
        stages.push(json!({"stage":"E","result":"pass","decision":decision}));
        let filed = self.find("13 dispatch order markers", "queries/")?;
        if filed == decision {
            return Err("no distinct handoff found".into());
        }
        let page = self.read(&filed)?;
        let (answer, refs) = page
            .split_once("## 답변\n\n")
            .and_then(|(_, tail)| tail.split_once("\n\n## 검색 결과\n\n"))
            .ok_or("missing current answer and references")?;
        includes(
            answer,
            &[
                "field",
                "3 lots",
                "18 markers",
                "teal",
                "copper",
                "camera",
                "hold",
            ],
        )?;
        if !answer.contains("5 spare") && !answer.contains("five spare") {
            return Err("missing 5 spare markers".into());
        }
        for reference in [&card, &note, &decision] {
            if !refs.contains(reference) {
                return Err(format!("missing reference {reference}"));
            }
        }
        includes(refs, &[commit, path.to_lowercase().as_str()])?;
        stages.push(json!({"stage":"A/F","result":"pass","record":filed,"scope":"fixture assertions; inspect reasoning separately"}));
        Ok(())
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: verify-library-handoff CHECKOUT REGISTRY");
        return ExitCode::from(2);
    }
    let trial = Trial {
        cli: Path::new(&args[0]).join("bin/library.py"),
        registry: PathBuf::from(&args[1]),
    };
    let mut stages = Vec::new();
    match trial.grade(&mut stages) {
        Ok(()) => {
            println!("{}", json!({"result":"pass","stages":stages}));
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("{}", json!({"result":"fail","passed":stages,"reason":e}));
            ExitCode::FAILURE
        }
    }
}
