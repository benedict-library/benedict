//! The square's memory: an append-only log of events (one JSON line each) and the state replayed from it.
//! Nothing is ever deleted or overwritten — a later accept or vote is a new line, so the history stays readable.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::guard;

/// A post with no answer this long after it went up shows as unsolved; it stays open for answers.
pub const UNSOLVED_AFTER: u64 = 24 * 60 * 60;
const MAX_TITLE: usize = 200;
const MAX_TEXT: usize = 20_000;
const MAX_TAG: usize = 30;
const MAX_TAGS: usize = 5;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Question,
    Opinion,
    Proposal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Post { kind: Kind, title: String, body: String, tags: Vec<String> },
    Answer { post: String, body: String, source: Option<String> },
    Reply { answer: String, body: String },
    Vote { target: String, up: bool },
    Accept { post: String, answer: String, why: String },
    Subscribe { tag: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub at: u64,
    pub by: String,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Clone, Debug, Serialize)]
pub struct Accepted {
    pub answer: String,
    pub by: String,
    pub why: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Post {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub by: String,
    pub at: u64,
    pub answers: Vec<String>,
    pub up: BTreeSet<String>,
    pub down: BTreeSet<String>,
    /// Every accept in order; the last one stands (a later one, such as the owner's, overrides).
    pub accepts: Vec<Accepted>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reply {
    pub id: String,
    pub body: String,
    pub by: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Answer {
    pub id: String,
    pub post: String,
    pub body: String,
    pub source: Option<String>,
    pub by: String,
    pub at: u64,
    pub up: BTreeSet<String>,
    pub down: BTreeSet<String>,
    pub replies: Vec<Reply>,
}

impl Post {
    pub fn accepted(&self) -> Option<&Accepted> {
        self.accepts.last()
    }

    /// "accepted", "unsolved" (no answer a day after it went up) or "open".
    pub fn status(&self, now: u64) -> &'static str {
        if self.accepted().is_some() {
            "accepted"
        } else if self.answers.is_empty() && now.saturating_sub(self.at) >= UNSOLVED_AFTER {
            "unsolved"
        } else {
            "open"
        }
    }
}

/// The square's rules from its accounts: who may accept besides the asker (owners), and the names of member
/// institutions (academy, company) that must never appear in a post, answer or reply.
pub trait Rules {
    fn is_owner(&self, account: &str) -> bool;
    fn private_words(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Default)]
pub struct Store {
    log: Option<PathBuf>,
    next: u64,
    pub posts: BTreeMap<String, Post>,
    pub answers: BTreeMap<String, Answer>,
    pub subscriptions: BTreeMap<String, BTreeSet<String>>,
    pub events: Vec<Event>,
}

fn clean(text: &str) -> String {
    text.trim().to_string()
}

/// `words`: the member institutions' names; None on replay, where only the shape of a line is checked.
fn check_text(what: &str, text: &str, max: usize, words: Option<&[String]>) -> Result<(), String> {
    if text.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if text.chars().count() > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    let Some(words) = words else { return Ok(()) };
    let lower = text.to_lowercase();
    if words.iter().any(|w| lower.contains(&w.to_lowercase())) || !guard::findings(text).is_empty() {
        return Err(format!("{what} contains private details: rewrite it without names or identifying numbers"));
    }
    Ok(())
}

/// Tags trimmed, without a leading '#', deduplicated in order.
pub fn tidy_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim().trim_start_matches('#').trim().to_string();
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

impl Store {
    /// Replays the log at `path` (created when missing). A line that does not read stops the start with its number:
    /// skipping it would quietly change the square's history.
    pub fn open(path: &Path, rules: &dyn Rules) -> Result<Self, String> {
        let mut store = Store { log: Some(path.to_path_buf()), next: 1, ..Store::default() };
        if path.exists() {
            let file = File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            for (n, line) in BufReader::new(file).lines().enumerate() {
                let line = line.map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?;
                if line.trim().is_empty() {
                    continue;
                }
                let event: Event = serde_json::from_str(&line).map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?;
                store.check(&event.by, &event.op, rules, None).map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?;
                store.apply(event);
            }
        }
        Ok(store)
    }

    /// A store that keeps nothing on disk (tests).
    pub fn in_memory() -> Self {
        Store { next: 1, ..Store::default() }
    }

    /// Checks the change, writes it to the log, then applies it. On any error nothing changes.
    pub fn commit(&mut self, by: &str, op: Op, at: u64, rules: &dyn Rules) -> Result<Event, String> {
        let op = self.normalize(op);
        let words = rules.private_words();
        self.check(by, &op, rules, Some(&words))?;
        let prefix = match op {
            Op::Post { .. } => "p",
            Op::Answer { .. } => "a",
            Op::Reply { .. } => "r",
            Op::Vote { .. } => "v",
            Op::Accept { .. } => "c",
            Op::Subscribe { .. } => "s",
        };
        let event = Event { id: format!("{prefix}{}", self.next), at, by: by.to_string(), op };
        if let Some(path) = &self.log {
            let line = serde_json::to_string(&event).map_err(|e| e.to_string())?;
            let mut file = OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            writeln!(file, "{line}").and_then(|_| file.sync_data()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
        self.apply(event.clone());
        Ok(event)
    }

    fn normalize(&self, op: Op) -> Op {
        match op {
            Op::Post { kind, title, body, tags } => Op::Post { kind, title: clean(&title), body: clean(&body), tags: tidy_tags(&tags) },
            Op::Answer { post, body, source } => {
                Op::Answer { post, body: clean(&body), source: source.map(|s| clean(&s)).filter(|s| !s.is_empty()) }
            }
            Op::Reply { answer, body } => Op::Reply { answer, body: clean(&body) },
            Op::Accept { post, answer, why } => Op::Accept { post, answer, why: clean(&why) },
            Op::Subscribe { tag } => Op::Subscribe { tag: tidy_tags(&[tag]).pop().unwrap_or_default() },
            vote => vote,
        }
    }

    fn check(&self, by: &str, op: &Op, rules: &dyn Rules, words: Option<&[String]>) -> Result<(), String> {
        match op {
            Op::Post { title, body, tags, .. } => {
                check_text("the title", title, MAX_TITLE, words)?;
                if !body.is_empty() {
                    check_text("the body", body, MAX_TEXT, words)?;
                }
                if tags.is_empty() || tags.len() > MAX_TAGS {
                    return Err(format!("give 1 to {MAX_TAGS} tags"));
                }
                for tag in tags {
                    check_text("the tag", tag, MAX_TAG, words)?;
                }
                Ok(())
            }
            Op::Answer { post, body, source } => {
                self.posts.get(post).ok_or_else(|| format!("no post {post}"))?;
                check_text("the answer", body, MAX_TEXT, words)?;
                source.as_deref().map_or(Ok(()), |s| check_text("the source", s, MAX_TITLE * 5, words))
            }
            Op::Reply { answer, body } => {
                self.answers.get(answer).ok_or_else(|| format!("no answer {answer}"))?;
                check_text("the reply", body, MAX_TEXT, words)
            }
            Op::Vote { target, .. } => {
                let author = match (self.posts.get(target), self.answers.get(target)) {
                    (Some(p), _) => &p.by,
                    (_, Some(a)) => &a.by,
                    _ => return Err(format!("no post or answer {target}")),
                };
                if author == by {
                    return Err("you cannot vote on what you wrote".into());
                }
                Ok(())
            }
            Op::Accept { post, answer, why } => {
                let p = self.posts.get(post).ok_or_else(|| format!("no post {post}"))?;
                if !p.answers.contains(answer) {
                    return Err(format!("{answer} is not an answer to {post}"));
                }
                if p.by != by && !rules.is_owner(by) {
                    return Err("only the one who asked, or an owner, accepts an answer".into());
                }
                check_text("the reason", why, MAX_TITLE, words)
            }
            Op::Subscribe { tag } => {
                check_text("the tag", tag, MAX_TAG, words)
            }
        }
    }

    /// Applies a checked event.
    fn apply(&mut self, event: Event) {
        let n: u64 = event.id[1..].parse().unwrap_or(0);
        self.next = self.next.max(n + 1);
        let (id, at, by) = (event.id.clone(), event.at, event.by.clone());
        match &event.op {
            Op::Post { kind, title, body, tags } => {
                let post = Post {
                    id: id.clone(), kind: *kind, title: title.clone(), body: body.clone(), tags: tags.clone(), by, at,
                    answers: Vec::new(), up: BTreeSet::new(), down: BTreeSet::new(), accepts: Vec::new(),
                };
                self.posts.insert(id, post);
            }
            Op::Answer { post, body, source } => {
                let answer = Answer {
                    id: id.clone(), post: post.clone(), body: body.clone(), source: source.clone(), by, at,
                    up: BTreeSet::new(), down: BTreeSet::new(), replies: Vec::new(),
                };
                if let Some(p) = self.posts.get_mut(post) {
                    p.answers.push(id.clone());
                }
                self.answers.insert(id, answer);
            }
            Op::Reply { answer, body } => {
                if let Some(a) = self.answers.get_mut(answer) {
                    a.replies.push(Reply { id, body: body.clone(), by, at });
                }
            }
            Op::Vote { target, up } => {
                let sets = match self.posts.get_mut(target) {
                    Some(p) => Some((&mut p.up, &mut p.down)),
                    None => self.answers.get_mut(target).map(|a| (&mut a.up, &mut a.down)),
                };
                if let Some((ups, downs)) = sets {
                    let (add, remove) = if *up { (ups, downs) } else { (downs, ups) };
                    remove.remove(&by);
                    add.insert(by);
                }
            }
            Op::Accept { post, answer, why } => {
                if let Some(p) = self.posts.get_mut(post) {
                    p.accepts.push(Accepted { answer: answer.clone(), by, why: why.clone(), at });
                }
            }
            Op::Subscribe { tag } => {
                self.subscriptions.entry(by).or_default().insert(tag.clone());
            }
        }
        self.events.push(event);
    }

    /// The tags an account follows: the ones it subscribed to and the ones where its answer was accepted.
    pub fn followed_tags(&self, account: &str) -> BTreeSet<String> {
        let mut tags = self.subscriptions.get(account).cloned().unwrap_or_default();
        for post in self.posts.values() {
            if post.accepted().is_some_and(|c| self.answers.get(&c.answer).is_some_and(|a| a.by == account)) {
                tags.extend(post.tags.iter().cloned());
            }
        }
        tags
    }

    /// What an account should look at when it has time: posts without an accepted answer that it neither wrote nor
    /// answered — those in its tags first (newest first), then the ones nobody answered (oldest first).
    pub fn browse(&self, account: &str, limit: usize) -> Vec<&Post> {
        let tags = self.followed_tags(account);
        let mine = |p: &Post| p.by == account || p.answers.iter().any(|a| self.answers.get(a).is_some_and(|a| a.by == account));
        let waiting: Vec<&Post> = self.posts.values().filter(|p| p.accepted().is_none() && !mine(p)).collect();
        let mut followed: Vec<&Post> = waiting.iter().copied().filter(|p| p.tags.iter().any(|t| tags.contains(t))).collect();
        followed.sort_by(|a, b| b.at.cmp(&a.at));
        let mut unanswered: Vec<&Post> =
            waiting.iter().copied().filter(|p| p.answers.is_empty() && !followed.iter().any(|f| f.id == p.id)).collect();
        unanswered.sort_by_key(|p| p.at);
        followed.into_iter().chain(unanswered).take(limit).collect()
    }

    /// Accepted answers counted by who wrote them, since `since`, with the tags they were accepted under.
    pub fn leaders(&self, since: u64) -> Vec<(String, usize, Vec<String>)> {
        let mut count: BTreeMap<String, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
        for post in self.posts.values() {
            let Some(c) = post.accepted().filter(|c| c.at >= since) else { continue };
            let Some(answer) = self.answers.get(&c.answer) else { continue };
            let entry = count.entry(answer.by.clone()).or_default();
            entry.0 += 1;
            for tag in &post.tags {
                *entry.1.entry(tag.clone()).or_default() += 1;
            }
        }
        let mut out: Vec<(String, usize, Vec<String>)> = count
            .into_iter()
            .map(|(who, (n, tags))| {
                let mut tags: Vec<(String, usize)> = tags.into_iter().collect();
                tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                (who, n, tags.into_iter().take(3).map(|t| t.0).collect())
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Owner(&'static str);
    impl Rules for Owner {
        fn is_owner(&self, account: &str) -> bool {
            account == self.0
        }
        fn private_words(&self) -> Vec<String> {
            vec!["새싹학원".into(), "Sprout Academy".into()]
        }
    }
    const OWNER: Owner = Owner("owner");

    fn ask(store: &mut Store, by: &str, title: &str, tags: &[&str], at: u64) -> String {
        let op = Op::Post { kind: Kind::Question, title: title.into(), body: String::new(), tags: tags.iter().map(|t| t.to_string()).collect() };
        store.commit(by, op, at, &OWNER).unwrap().id
    }

    fn answer(store: &mut Store, by: &str, post: &str, at: u64) -> String {
        let op = Op::Answer { post: post.into(), body: "지침 표를 따릅니다".into(), source: Some("고용노동부 지침".into()) };
        store.commit(by, op, at, &OWNER).unwrap().id
    }

    #[test]
    fn the_log_replays_to_the_same_square() {
        let dir = std::env::temp_dir().join(format!("plaza-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::open(&path, &OWNER).unwrap();
        let p = ask(&mut store, "helper", "공가 인정 기준은?", &["출결", "#HRD-Net", "출결"], 100);
        let a = answer(&mut store, "crew-a", &p, 200);
        store.commit("crew-b", Op::Vote { target: a.clone(), up: true }, 210, &OWNER).unwrap();
        store.commit("helper", Op::Accept { post: p.clone(), answer: a.clone(), why: "공식 근거".into() }, 300, &OWNER).unwrap();
        let again = Store::open(&path, &OWNER).unwrap();
        let post = &again.posts[&p];
        assert_eq!(post.tags, ["출결", "HRD-Net"]);
        assert_eq!(post.accepted().unwrap().answer, a);
        assert_eq!(again.answers[&a].up.len(), 1);
        assert_eq!(again.events.len(), 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_asker_or_an_owner_accepts_and_nobody_votes_on_their_own() {
        let mut store = Store::in_memory();
        let p = ask(&mut store, "helper", "공가 인정 기준은?", &["출결"], 100);
        let a = answer(&mut store, "crew-a", &p, 200);
        let accept = |why: &str| Op::Accept { post: p.clone(), answer: a.clone(), why: why.into() };
        assert!(store.commit("crew-b", accept("내 생각"), 300, &OWNER).is_err());
        assert!(store.commit("crew-a", Op::Vote { target: a.clone(), up: true }, 300, &OWNER).is_err());
        store.commit("helper", accept("공식 근거"), 300, &OWNER).unwrap();
        store.commit("owner", accept("관리자가 확인"), 400, &OWNER).unwrap();
        assert_eq!(store.posts[&p].accepts.len(), 2);
        assert_eq!(store.posts[&p].accepted().unwrap().by, "owner");
    }

    #[test]
    fn private_details_never_reach_the_log() {
        let mut store = Store::in_memory();
        let op = Op::Post { kind: Kind::Question, title: "홍길동 010-1234-5678 공가?".into(), body: String::new(), tags: vec!["출결".into()] };
        let err = store.commit("helper", op, 100, &OWNER).unwrap_err();
        assert!(err.contains("private details"), "{err}");
        assert!(store.events.is_empty());
    }

    #[test]
    fn member_institution_names_never_reach_the_log_but_old_lines_still_replay() {
        let mut store = Store::in_memory();
        for title in ["새싹학원 공가 기준은?", "sprout academy leave rule?"] {
            let op = Op::Post { kind: Kind::Question, title: title.into(), body: String::new(), tags: vec!["출결".into()] };
            let err = store.commit("helper", op, 100, &OWNER).unwrap_err();
            assert_eq!(err, "the title contains private details: rewrite it without names or identifying numbers");
        }
        let private_tag = "Sprout Academy";
        let post = Op::Post { kind: Kind::Question, title: "question".into(), body: String::new(), tags: vec![private_tag.into()] };
        let subscription = Op::Subscribe { tag: private_tag.into() };
        for op in [post, subscription] {
            let err = store.commit("helper", op, 100, &OWNER).unwrap_err();
            assert_eq!(err, "the tag contains private details: rewrite it without names or identifying numbers");
        }
        let p = ask(&mut store, "helper", "공가 인정 기준은?", &["출결"], 100);
        let op = Op::Answer { post: p.clone(), body: "새싹학원에서는 이렇게 합니다".into(), source: None };
        assert!(store.commit("crew-a", op, 200, &OWNER).is_err());
        assert_eq!(store.events.len(), 1);

        let dir = std::env::temp_dir().join(format!("plaza-words-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        std::fs::write(&path, r#"{"id":"p1","at":1,"by":"helper","op":"post","kind":"question","title":"새싹학원 옛 글","body":"","tags":["출결"]}"#).unwrap();
        assert_eq!(Store::open(&path, &OWNER).unwrap().posts.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_post_nobody_answers_for_a_day_is_unsolved_and_browse_puts_followed_tags_first() {
        let mut store = Store::in_memory();
        let old = ask(&mut store, "helper", "훈련일지 서명 누락은?", &["훈련일지"], 0);
        let mine = ask(&mut store, "crew-b", "내 질문", &["출결"], 10);
        let followed = ask(&mut store, "helper", "공가 인정 기준은?", &["출결"], 20);
        store.commit("crew-b", Op::Subscribe { tag: "#출결".into() }, 30, &OWNER).unwrap();
        assert_eq!(store.posts[&old].status(UNSOLVED_AFTER), "unsolved");
        assert_eq!(store.posts[&followed].status(UNSOLVED_AFTER), "open");
        let seen: Vec<&str> = store.browse("crew-b", 10).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(seen, [followed.as_str(), old.as_str()]);
        assert!(!seen.contains(&mine.as_str()));
    }

    #[test]
    fn accepted_answers_make_the_leaders_and_grow_the_answerers_tags() {
        let mut store = Store::in_memory();
        let p = ask(&mut store, "helper", "공가 인정 기준은?", &["출결", "HRD-Net"], 100);
        let a = answer(&mut store, "crew-a", &p, 200);
        store.commit("helper", Op::Accept { post: p, answer: a, why: "공식 근거".into() }, 300, &OWNER).unwrap();
        assert_eq!(store.leaders(0), [("crew-a".to_string(), 1, vec!["HRD-Net".to_string(), "출결".to_string()])]);
        assert!(store.followed_tags("crew-a").contains("출결"));
    }
}
