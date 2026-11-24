//! The host agent renders these choices using its own interaction UI.
use serde_json::{Value, json};

fn choice(id: &str, label: &str, reason: &str) -> Value {
    json!({"id":id, "label":label, "reason":reason, "recommended":false})
}

pub fn prompt(step: &str, wiki_found: bool) -> Result<Value, String> {
    let (question, mut choices) = match step {
        "start" => {
            let wiki = choice(
                "wiki",
                "내 위키 연결하기",
                "쓰던 자료와 기록으로 시작합니다.",
            );
            let example = choice(
                "example",
                "예제 자료로 시작하기",
                "위키가 없어도 도서관을 바로 써볼 수 있습니다.",
            );
            let ordered = if wiki_found {
                vec![wiki, example]
            } else {
                vec![example, wiki]
            };
            ("어떤 자료로 시작할까요?", ordered)
        }
        "database" => (
            "각 실의 기록을 어디에 저장할까요?",
            vec![
                choice(
                    "sqlite",
                    "SQLite 사용하기",
                    "도서관 볼륨에 저장합니다. 별도 서버나 비밀번호가 필요 없습니다.",
                ),
                choice(
                    "postgres",
                    "기존 PostgreSQL 연결하기",
                    "이미 운영 중인 데이터베이스에 기록합니다.",
                ),
            ],
        ),
        "memory" => (
            "컨테이너 하나가 쓸 메모리 상한을 어떻게 정할까요?",
            vec![choice(
                "default",
                "512MiB로 시작하기",
                "현재 기본값입니다. 사용량을 확인한 뒤 조정할 수 있습니다.",
            )],
        ),
        // Asked only for your own material: the example start brings its own example shelf.
        "shelf" => (
            "서가 원본은 어디에서 읽을까요?",
            vec![
                choice(
                    "folder",
                    "내 원본 폴더 연결하기",
                    "위키가 근거로 삼는 원본 폴더를 읽기 전용으로 연결합니다.",
                ),
                choice(
                    "none",
                    "서가 없이 시작하기",
                    "빈 서가로 시작합니다. 예제 서가는 붙지 않습니다.",
                ),
            ],
        ),
        // Asked last: a trial library can stay down after a host restart.
        "autostart" => (
            "서버를 다시 켜면 도서관도 저절로 켤까요?",
            vec![
                choice(
                    "on",
                    "시동 때 다시 켜기",
                    "재부팅 뒤 방과 사서 컨테이너가 저절로 다시 뜹니다. 직접 멈춘 컨테이너는 그대로 둡니다.",
                ),
                choice(
                    "off",
                    "직접 켤 때만",
                    "시험처럼 잠깐 쓰는 도서관에 맞습니다. 재부팅 뒤에는 직접 다시 올립니다.",
                ),
            ],
        ),
        _ => {
            return Err(
                "expected setup question start, database, memory, shelf or autostart".into(),
            );
        }
    };
    choices[0]["recommended"] = true.into();
    choices.push(choice(
        "other",
        "기타 (직접 입력)",
        "원하는 방식을 직접 적습니다.",
    ));
    let mut out = json!({"id":step,"question":question,"choices":choices});
    if step == "start" {
        out["intro"] = "에이전트와 페르소나를 위한 나의 도서관을 만듭니다.".into();
    }
    Ok(out)
}

pub fn show(args: &[String]) -> Result<(), String> {
    let (step, rest) = args.split_first().ok_or("setup question requires a step")?;
    let found = match rest {
        [] => false,
        [flag] if flag == "--wiki-found" => true,
        _ => return Err("usage: setup question STEP [--wiki-found]".into()),
    };
    println!("{}", prompt(step, found)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_question_fits_the_host_picker() {
        for found in [false, true] {
            for step in ["start", "database", "memory", "shelf", "autostart"] {
                let p = prompt(step, found).unwrap();
                let choices = p["choices"].as_array().unwrap();
                assert!(choices.len() <= 3);
                assert_eq!(choices.last().unwrap()["id"], "other");
                assert_eq!(choices[0]["recommended"], true);
                assert_eq!(
                    choices.iter().filter(|c| c["recommended"] == true).count(),
                    1
                );
                if step == "start" {
                    assert_eq!(choices[0]["id"], if found { "wiki" } else { "example" });
                }
                if step == "shelf" {
                    assert_eq!(choices[0]["id"], "folder");
                    assert!(choices.iter().all(|c| c["id"] != "default"));
                }
                if step == "autostart" {
                    assert_eq!(choices[0]["id"], "on");
                    assert_eq!(choices[1]["id"], "off");
                }
            }
        }
    }
}
