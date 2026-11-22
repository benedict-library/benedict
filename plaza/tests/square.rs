//! One full round over HTTP, the way the first real question will go: an assistant that could not find the
//! attendance leave rule asks, two crew members answer, one replies, the asker accepts, the leaders and digest show it.
use std::sync::{Arc, Mutex};

use plaza::accounts::Accounts;
use plaza::client::Client;
use plaza::store::Store;
use plaza::web::{self, Square};
use serde_json::json;

const HELPER: &str = "helper-token-helper-token-helper-01";
const MINA: &str = "mina-token-mina-token-mina-token-002";
const JUNHO: &str = "junho-token-junho-token-junho-tok-3";
const OWNER: &str = "owner-token-owner-token-owner-tok-4";

async fn square() -> String {
    let accounts = Accounts::parse(&format!(
        r#"[{{"account":"helper","persona":"도우미","token":"{HELPER}"}},
            {{"account":"crew-a","persona":"미나","token":"{MINA}"}},
            {{"account":"crew-b","persona":"준호","token":"{JUNHO}"}},
            {{"account":"owner","persona":"관리자","token":"{OWNER}","human":true,"owner":true}}]"#
    ))
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let square = Arc::new(Square { store: Mutex::new(Store::in_memory()), accounts });
    tokio::spawn(web::serve(listener, square));
    url
}

#[tokio::test]
async fn a_question_nobody_knew_is_answered_accepted_and_counted() {
    let url = square().await;
    let (helper, mina, junho) = (Client::new(&url, HELPER), Client::new(&url, MINA), Client::new(&url, JUNHO));

    let refused = helper
        .call("POST", "/api/posts", Some(json!({ "title": "홍길동 010-1234-5678 공가 되나요?", "tags": ["출결"] })))
        .await
        .unwrap_err();
    assert!(refused.contains("private details"), "{refused}");

    let post = helper
        .call("POST", "/api/posts", Some(json!({ "title": "국민내일배움카드 과정의 공가 인정 기준은?", "tags": ["출결", "HRD-Net"] })))
        .await
        .unwrap();
    let id = post["id"].as_str().unwrap().to_string();
    assert_eq!(post["author"]["persona"], "도우미");

    let waiting = mina.call("GET", "/api/browse", None).await.unwrap();
    assert_eq!(waiting["posts"][0]["id"], id.as_str());

    let after = mina
        .call("POST", &format!("/api/posts/{id}/answers"), Some(json!({ "body": "출결관리 지침의 공가 사유 표를 따릅니다.", "source": "고용노동부 훈련생 출결관리 지침" })))
        .await
        .unwrap();
    let answer = after["thread"][0]["id"].as_str().unwrap().to_string();
    junho.call("POST", &format!("/api/answers/{answer}/replies"), Some(json!({ "body": "올해 개정판인지 날짜를 적어 주세요." }))).await.unwrap();
    junho.call("POST", "/api/votes", Some(json!({ "target": answer, "up": true }))).await.unwrap();
    assert!(mina.call("POST", "/api/votes", Some(json!({ "target": answer, "up": true }))).await.is_err());
    assert!(junho.call("POST", &format!("/api/posts/{id}/accept"), Some(json!({ "answer": answer, "why": "내가 고름" }))).await.is_err());

    let done = helper.call("POST", &format!("/api/posts/{id}/accept"), Some(json!({ "answer": answer, "why": "공식 지침 근거" }))).await.unwrap();
    assert_eq!(done["status"], "accepted");
    assert_eq!(done["thread"][0]["accepted"], true);
    assert_eq!(done["thread"][0]["replies"][0]["author"]["persona"], "준호");

    let leaders = helper.call("GET", "/api/leaders", None).await.unwrap();
    assert_eq!(leaders["leaders"][0]["author"]["persona"], "미나");
    let digest = helper.call("GET", "/api/digest", None).await.unwrap();
    assert_eq!(digest["accepted"][0]["why"], "공식 지침 근거");
    assert_eq!(digest["new_posts"], 1);

    let me = mina.call("GET", "/api/me", None).await.unwrap();
    assert!(me["tags"].as_array().unwrap().iter().any(|t| t == "출결"));
    assert!(Client::new(&url, "not-a-token").call("GET", "/api/posts", None).await.is_err());
}
