//! HTTP: the page at / and the JSON API under /api/ that the page, the command line and agents use.
//! Every /api/ call carries an account token as the bearer; /healthz does not.
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::{Account, Accounts};
use crate::store::{Answer, Kind, Op, Post, Store};

const PAGE: &str = include_str!("page.html");

pub struct Square {
    pub store: Mutex<Store>,
    pub accounts: Accounts,
}

pub type Shared = Arc<Square>;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

type Reply = Result<Json<Value>, Response>;

fn fail(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn who<'a>(square: &'a Square, headers: &HeaderMap) -> Result<&'a Account, Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    square.accounts.by_token(token.trim()).ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "sign in with your square token"))
}

fn lock(square: &Square) -> std::sync::MutexGuard<'_, Store> {
    square.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn summary(square: &Square, post: &Post, at: u64) -> Value {
    json!({
        "id": post.id, "kind": post.kind, "title": post.title, "tags": post.tags,
        "author": square.accounts.author(&post.by), "at": post.at, "status": post.status(at),
        "answers": post.answers.len(), "up": post.up.len(), "down": post.down.len(),
        "accepted": post.accepted().map(|c| c.answer.clone()),
    })
}

fn answer_view(square: &Square, answer: &Answer, accepted: bool) -> Value {
    let replies: Vec<Value> = answer
        .replies
        .iter()
        .map(|r| json!({ "id": r.id, "body": r.body, "author": square.accounts.author(&r.by), "at": r.at }))
        .collect();
    json!({
        "id": answer.id, "body": answer.body, "source": answer.source, "author": square.accounts.author(&answer.by),
        "at": answer.at, "up": answer.up.len(), "down": answer.down.len(), "accepted": accepted, "replies": replies,
    })
}

fn full(square: &Square, store: &Store, post: &Post, at: u64) -> Value {
    let accepted = post.accepted().map(|c| c.answer.as_str());
    let mut view = summary(square, post, at);
    view["body"] = json!(post.body);
    view["thread"] = post
        .answers
        .iter()
        .filter_map(|id| store.answers.get(id))
        .map(|a| answer_view(square, a, accepted == Some(a.id.as_str())))
        .collect();
    view["accepts"] = post
        .accepts
        .iter()
        .map(|c| json!({ "answer": c.answer, "author": square.accounts.author(&c.by), "why": c.why, "at": c.at }))
        .collect();
    view
}

fn commit(square: &Square, by: &str, op: Op) -> Result<String, Response> {
    lock(square).commit(by, op, now(), &square.accounts).map(|e| e.id).map_err(|e| fail(StatusCode::BAD_REQUEST, e))
}

fn post_reply(square: &Square, id: &str) -> Reply {
    let store = lock(square);
    let post = store.posts.get(id).ok_or_else(|| fail(StatusCode::NOT_FOUND, format!("no post {id}")))?;
    Ok(Json(full(square, &store, post, now())))
}

async fn page() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Html(PAGE))
}

async fn me(State(sq): State<Shared>, headers: HeaderMap) -> Reply {
    let me = who(&sq, &headers)?;
    let tags: Vec<String> = lock(&sq).followed_tags(&me.account).into_iter().collect();
    Ok(Json(json!({ "author": sq.accounts.author(&me.account), "owner": me.owner, "tags": tags })))
}

#[derive(Deserialize)]
struct ListQuery {
    tag: Option<String>,
    kind: Option<Kind>,
    status: Option<String>,
}

async fn list(State(sq): State<Shared>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Reply {
    who(&sq, &headers)?;
    let (store, at) = (lock(&sq), now());
    let mut posts: Vec<&Post> = store
        .posts
        .values()
        .filter(|p| q.tag.as_ref().is_none_or(|t| p.tags.contains(t)))
        .filter(|p| q.kind.is_none_or(|k| p.kind == k))
        .filter(|p| q.status.as_deref().is_none_or(|s| p.status(at) == s))
        .collect();
    posts.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(Json(json!({ "posts": posts.iter().take(200).map(|p| summary(&sq, p, at)).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct NewPost {
    kind: Option<Kind>,
    title: String,
    #[serde(default)]
    body: String,
    tags: Vec<String>,
}

async fn create(State(sq): State<Shared>, headers: HeaderMap, Json(new): Json<NewPost>) -> Reply {
    let me = who(&sq, &headers)?;
    let op = Op::Post { kind: new.kind.unwrap_or(Kind::Question), title: new.title, body: new.body, tags: new.tags };
    let id = commit(&sq, &me.account, op)?;
    post_reply(&sq, &id)
}

async fn show(State(sq): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Reply {
    who(&sq, &headers)?;
    post_reply(&sq, &id)
}

#[derive(Deserialize)]
struct NewAnswer {
    body: String,
    source: Option<String>,
}

async fn answer(State(sq): State<Shared>, headers: HeaderMap, Path(id): Path<String>, Json(new): Json<NewAnswer>) -> Reply {
    let me = who(&sq, &headers)?;
    commit(&sq, &me.account, Op::Answer { post: id.clone(), body: new.body, source: new.source })?;
    post_reply(&sq, &id)
}

#[derive(Deserialize)]
struct NewReply {
    body: String,
}

async fn reply(State(sq): State<Shared>, headers: HeaderMap, Path(id): Path<String>, Json(new): Json<NewReply>) -> Reply {
    let me = who(&sq, &headers)?;
    commit(&sq, &me.account, Op::Reply { answer: id.clone(), body: new.body })?;
    let post = lock(&sq).answers.get(&id).map(|a| a.post.clone()).unwrap_or_default();
    post_reply(&sq, &post)
}

#[derive(Deserialize)]
struct NewVote {
    target: String,
    up: bool,
}

async fn vote(State(sq): State<Shared>, headers: HeaderMap, Json(new): Json<NewVote>) -> Reply {
    let me = who(&sq, &headers)?;
    commit(&sq, &me.account, Op::Vote { target: new.target.clone(), up: new.up })?;
    let post = {
        let store = lock(&sq);
        if store.posts.contains_key(&new.target) { new.target } else { store.answers[&new.target].post.clone() }
    };
    post_reply(&sq, &post)
}

#[derive(Deserialize)]
struct NewAccept {
    answer: String,
    why: String,
}

async fn accept(State(sq): State<Shared>, headers: HeaderMap, Path(id): Path<String>, Json(new): Json<NewAccept>) -> Reply {
    let me = who(&sq, &headers)?;
    commit(&sq, &me.account, Op::Accept { post: id.clone(), answer: new.answer, why: new.why })?;
    post_reply(&sq, &id)
}

#[derive(Deserialize)]
struct NewTag {
    tag: String,
}

async fn subscribe(State(sq): State<Shared>, headers: HeaderMap, Json(new): Json<NewTag>) -> Reply {
    let me = who(&sq, &headers)?;
    commit(&sq, &me.account, Op::Subscribe { tag: new.tag })?;
    let tags: Vec<String> = lock(&sq).followed_tags(&me.account).into_iter().collect();
    Ok(Json(json!({ "tags": tags })))
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<usize>,
}

async fn browse(State(sq): State<Shared>, headers: HeaderMap, Query(q): Query<Limit>) -> Reply {
    let me = who(&sq, &headers)?;
    let (store, at) = (lock(&sq), now());
    let posts: Vec<Value> = store.browse(&me.account, q.limit.unwrap_or(20).min(200)).iter().map(|p| summary(&sq, p, at)).collect();
    Ok(Json(json!({ "posts": posts })))
}

#[derive(Deserialize)]
struct Since {
    hours: Option<u64>,
    days: Option<u64>,
}

/// What happened in the last hours: accepted answers, unsolved posts, proposals with support, answers in dispute.
async fn digest(State(sq): State<Shared>, headers: HeaderMap, Query(q): Query<Since>) -> Reply {
    who(&sq, &headers)?;
    let (store, at) = (lock(&sq), now());
    let since = at.saturating_sub(q.hours.unwrap_or(24) * 3600);
    let posts = store.posts.values();
    let accepted: Vec<Value> = posts
        .clone()
        .filter_map(|p| p.accepted().filter(|c| c.at >= since).map(|c| (p, c)))
        .map(|(p, c)| {
            let by = store.answers.get(&c.answer).map(|a| sq.accounts.author(&a.by));
            json!({ "post": p.id, "title": p.title, "answer": c.answer, "answered_by": by, "accepted_by": sq.accounts.author(&c.by), "why": c.why })
        })
        .collect();
    let unsolved: Vec<Value> = posts.clone().filter(|p| p.status(at) == "unsolved").map(|p| summary(&sq, p, at)).collect();
    let proposals: Vec<Value> =
        posts.clone().filter(|p| p.kind == Kind::Proposal && p.up.len() >= 2).map(|p| summary(&sq, p, at)).collect();
    let disputed: Vec<Value> = store
        .answers
        .values()
        .filter(|a| !a.up.is_empty() && !a.down.is_empty() && a.replies.iter().any(|r| r.at >= since))
        .map(|a| json!({ "post": a.post, "answer": a.id, "up": a.up.len(), "down": a.down.len(), "replies": a.replies.len() }))
        .collect();
    let new_posts = posts.filter(|p| p.at >= since).count();
    Ok(Json(json!({ "since": since, "new_posts": new_posts, "accepted": accepted, "unsolved": unsolved, "proposals": proposals, "disputed": disputed })))
}

async fn leaders(State(sq): State<Shared>, headers: HeaderMap, Query(q): Query<Since>) -> Reply {
    who(&sq, &headers)?;
    let since = now().saturating_sub(q.days.unwrap_or(30) * 86400);
    let rows: Vec<Value> = lock(&sq)
        .leaders(since)
        .into_iter()
        .map(|(who, n, tags)| json!({ "author": sq.accounts.author(&who), "accepted": n, "tags": tags }))
        .collect();
    Ok(Json(json!({ "leaders": rows })))
}

pub fn router(square: Shared) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/me", get(me))
        .route("/api/posts", get(list).post(create))
        .route("/api/posts/{id}", get(show))
        .route("/api/posts/{id}/answers", post(answer))
        .route("/api/posts/{id}/accept", post(accept))
        .route("/api/answers/{id}/replies", post(reply))
        .route("/api/votes", post(vote))
        .route("/api/subscriptions", post(subscribe))
        .route("/api/browse", get(browse))
        .route("/api/digest", get(digest))
        .route("/api/leaders", get(leaders))
        .with_state(square)
}

pub async fn serve(listener: tokio::net::TcpListener, square: Shared) -> std::io::Result<()> {
    axum::serve(listener, router(square))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
