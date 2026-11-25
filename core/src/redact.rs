//! Mask personal data in command results before they reach an agent or a
//! language model. Korean identifiers: resident and foreign registration numbers
//! keep the birth date and one digit (900101-1******), mobile numbers keep the
//! first and last groups (010-****-5678), sole-proprietor business numbers keep
//! three leading and two trailing digits; card, account, passport and driver
//! licence numbers and personal e-mail become labels; a person's name keeps the
//! first and last syllable (홍*동). Public organisation data stays: corporate
//! business numbers, landlines and role mailboxes such as info@.

const ACCOUNT_WORDS: [&str; 5] = ["계좌", "은행", "입금", "예금", "통장"];
const PASSPORT_WORDS: [&str; 1] = ["여권"];
const ROLE_MAIL: [&str; 14] = [
    "info",
    "contact",
    "admin",
    "help",
    "support",
    "sales",
    "press",
    "ir",
    "hr",
    "office",
    "master",
    "webmaster",
    "noreply",
    "no-reply",
];
const SURNAMES: &str = "김이박최정강조윤장임한오서신권황안송류유전홍고문양손배백허남심노하곽성차주우구민진나지엄원천방공현함변염여추도소석선설마길연위표명기반왕금옥육인맹제모탁국어은편용예경봉사부";
const COMPOUND: [&str; 7] = ["남궁", "황보", "제갈", "선우", "독고", "사공", "서문"];
const TITLES: [&str; 26] = [
    "대표",
    "이사",
    "팀장",
    "매니저",
    "과장",
    "부장",
    "차장",
    "대리",
    "사원",
    "실장",
    "본부장",
    "상무",
    "전무",
    "사장",
    "회장",
    "교수",
    "박사",
    "심사역",
    "변호사",
    "회계사",
    "책임",
    "선임",
    "님",
    "씨",
    "PM",
    "CEO",
];
const NAME_LABELS: [&str; 4] = ["성명", "이름", "담당자", "작성자"];
const ORG_ENDS: [&str; 16] = [
    "전자",
    "전력",
    "은행",
    "증권",
    "산업",
    "그룹",
    "기술",
    "바이오",
    "회사",
    "상사",
    "물산",
    "투자",
    "벤처스",
    "파트너스",
    "랩",
    "재단",
];

struct Hit {
    start: usize,
    end: usize,
    text: String,
    kind: &'static str,
}

fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

fn before(chars: &[char], i: usize, n: usize) -> String {
    chars[i.saturating_sub(n)..i].iter().collect()
}

fn has_any(hay: &str, words: &[&str]) -> bool {
    words.iter().any(|w| hay.contains(w))
}

/// The letter of an escape such as \\n written out in text (code, logs, JSON inside a
/// string) is a boundary, not part of the number or address after it.
fn escape_letter(chars: &[char], i: usize) -> bool {
    i > 0 && chars[i - 1] == '\\' && "ntr".contains(chars[i])
}

/// Stars before a number belong to it only when they continue digits (010****5678);
/// markdown bold before a number (**010-…) does not.
fn stars_continue_digits(chars: &[char], i: usize) -> bool {
    let mut k = i;
    while k > 0 && chars[k - 1] == '*' {
        k -= 1;
    }
    k > 0 && chars[k - 1].is_ascii_digit()
}

fn emails(chars: &[char], hits: &mut Vec<Hit>) {
    let local_ok = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    let dom_ok = |c: char| c.is_ascii_alphanumeric() || ".-".contains(c);
    for (at, &c) in chars.iter().enumerate() {
        if c != '@' {
            continue;
        }
        let mut s = at;
        while s > 0 && local_ok(chars[s - 1]) && !escape_letter(chars, s - 1) {
            s -= 1;
        }
        let mut e = at + 1;
        while e < chars.len() && dom_ok(chars[e]) {
            e += 1;
        }
        while e > at + 1 && chars[e - 1] == '.' {
            e -= 1;
        }
        let local: String = chars[s..at].iter().collect();
        let domain: String = chars[at + 1..e].iter().collect();
        let tld_ok = domain
            .rsplit('.')
            .next()
            .is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()));
        if local.is_empty() || !domain.contains('.') || !tld_ok {
            continue;
        }
        if ROLE_MAIL.contains(&local.to_ascii_lowercase().as_str()) {
            continue;
        }
        hits.push(Hit {
            start: s,
            end: e,
            text: "[이메일]".into(),
            kind: "email",
        });
    }
}

fn numbers(chars: &[char], hits: &mut Vec<Hit>) {
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let plus = c == '+' && i + 1 < n && chars[i + 1].is_ascii_digit();
        if !(c.is_ascii_digit() || plus) {
            i += 1;
            continue;
        }
        let prev_bad = i > 0 && {
            let p = chars[i - 1];
            (p.is_ascii_alphanumeric() && !escape_letter(chars, i - 1))
                || (p == '*' && stars_continue_digits(chars, i))
                || ((p == '.' || p == ',' || p == ':') && i > 1 && chars[i - 2].is_ascii_digit())
        };
        let start = i;
        let mut j = if plus { i + 1 } else { i };
        let mut groups: Vec<String> = Vec::new();
        loop {
            let gs = j;
            while j < n && (chars[j].is_ascii_digit() || (chars[j] == '*' && j > gs)) {
                j += 1;
            }
            groups.push(chars[gs..j].iter().collect());
            let sep = if j + 2 < n && chars[j] == ' ' && chars[j + 1] == '-' && chars[j + 2] == ' '
            {
                3
            } else if j < n && (chars[j] == '-' || chars[j] == ' ') {
                1
            } else {
                0
            };
            if sep > 0 && j + sep < n && chars[j + sep].is_ascii_digit() {
                j += sep;
            } else {
                break;
            }
        }
        let mut end = j;
        // Markdown bold right after a number (**010-1234-5678**) is not part of it: stars are
        // dropped only when the number matches without them, so a masked number keeps its stars.
        let trail = groups
            .last()
            .map_or(0, |g| g.chars().rev().take_while(|&c| c == '*').count());
        if trail > 0 && groups.last().is_some_and(|g| g.chars().count() > trail) {
            let mut bare = groups.clone();
            let last = bare.pop().unwrap_or_default();
            bare.push(last.chars().take(last.chars().count() - trail).collect());
            let lens: Vec<usize> = groups.iter().map(|g| g.chars().count()).collect();
            let bare_lens: Vec<usize> = bare.iter().map(|g| g.chars().count()).collect();
            let ctx = before(chars, start, 20);
            if classify(plus, &lens, &groups.concat(), lens.iter().sum(), &ctx).is_none()
                && classify(
                    plus,
                    &bare_lens,
                    &bare.concat(),
                    bare_lens.iter().sum(),
                    &ctx,
                )
                .is_some()
            {
                groups = bare;
                end = j - trail;
            }
        }
        let next_bad = end < n && {
            let q = chars[end];
            q.is_ascii_alphanumeric()
                || q == '%'
                || ((q == '.' || q == ',' || q == ':')
                    && end + 1 < n
                    && chars[end + 1].is_ascii_digit())
        };
        i = end.max(i + 1);
        if prev_bad || next_bad {
            continue;
        }
        let lens: Vec<usize> = groups.iter().map(|g| g.chars().count()).collect();
        let digits: String = groups.concat();
        let total = digits.chars().count();
        let ctx = before(chars, start, 20);
        let span = &chars[start..end];
        let label = classify(plus, &lens, &digits, total, &ctx);
        let kind = match label {
            Some("[주민·법인번호]") => "rrn",
            Some("[휴대폰]") => "phone",
            Some("[개인사업자]") => "biz",
            Some("[카드번호]") => "card",
            Some("[계좌번호]") => "account",
            Some("[운전면허번호]") => "driver",
            _ => "other",
        };
        let text = match label {
            Some("[주민·법인번호]") => {
                // 900101-1234568 -> 900101-1******, 8501012345675 -> 850101-2******
                let d: Vec<char> = digits.chars().collect();
                let head: String = d[..6].iter().collect();
                format!("{head}-{}******", d[6])
            }
            Some("[휴대폰]") => star_middle(span, if plus { 4 } else { 3 }, 4),
            Some("[개인사업자]") => star_middle(span, 3, 2),
            Some(l) => l.to_string(),
            None => continue,
        };
        hits.push(Hit {
            start,
            end,
            text,
            kind,
        });
    }
}

/// Keep the first `head` and last `tail` digits, star the rest, keep separators.
fn star_middle(span: &[char], head: usize, tail: usize) -> String {
    let total = span
        .iter()
        .filter(|c| c.is_ascii_digit() || **c == '*')
        .count();
    let mut seen = 0;
    span.iter()
        .map(|&c| {
            if c.is_ascii_digit() || c == '*' {
                seen += 1;
                if seen > head && seen <= total - tail {
                    '*'
                } else {
                    c
                }
            } else {
                c
            }
        })
        .collect()
}

fn classify(plus: bool, lens: &[usize], d: &str, total: usize, ctx: &str) -> Option<&'static str> {
    let mobile = |s: &str| s.len() >= 3 && s.starts_with("01") && "016789".contains(&s[2..3]);
    if plus {
        let rest = format!("0{}", d.strip_prefix("82")?);
        return if mobile(&rest) {
            Some("[휴대폰]")
        } else {
            None
        };
    }
    if lens == [6, 7] || lens == [13] {
        return Some("[주민·법인번호]");
    }
    if mobile(d) && (total == 10 || total == 11) && (lens.len() == 1 || lens.len() == 3) {
        return Some("[휴대폰]");
    }
    if d.starts_with('0') && (9..=11).contains(&total) && lens.len() <= 3 {
        return None; // landline: company public number
    }
    if lens == [4, 4, 4, 4] || lens == [16] {
        return Some("[카드번호]");
    }
    if lens == [2, 2, 6, 2] {
        return Some("[운전면허번호]");
    }
    if lens == [3, 2, 5] || (lens == [10] && ctx.contains("사업자")) {
        // middle code 80~88: corporation or non-profit (public), others: sole proprietor
        let code: u32 = d[3..5].parse().unwrap_or(0);
        return if (80..=88).contains(&code) {
            None
        } else {
            Some("[개인사업자]")
        };
    }
    if (2..=4).contains(&lens.len()) && (10..=14).contains(&total) && has_any(ctx, &ACCOUNT_WORDS) {
        return Some("[계좌번호]");
    }
    None
}

fn passports(chars: &[char], hits: &mut Vec<Hit>) {
    let n = chars.len();
    for i in 0..n {
        if !chars[i].is_ascii_uppercase() || i + 9 > n {
            continue;
        }
        if i > 0 && chars[i - 1].is_alphanumeric() {
            continue;
        }
        if !chars[i + 1..i + 9].iter().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if i + 9 < n && chars[i + 9].is_alphanumeric() {
            continue;
        }
        if has_any(&before(chars, i, 15), &PASSPORT_WORDS) {
            hits.push(Hit {
                start: i,
                end: i + 9,
                text: "[여권번호]".into(),
                kind: "passport",
            });
        }
    }
}

/// 홍길동 → 홍*동, 김철 → 김*, 남궁민수 → 남궁*수, 김수한무 → 김**무
pub fn mask_name(name: &str) -> String {
    let cs: Vec<char> = name.chars().collect();
    let head = if cs.len() >= 4 && COMPOUND.iter().any(|c| name.starts_with(c)) {
        2
    } else {
        1
    };
    match cs.len() {
        0 | 1 => name.to_string(),
        2 => format!("{}*", cs[0]),
        _ => {
            let mut out: String = cs[..head].iter().collect();
            out.push_str(&"*".repeat(cs.len() - head - 1));
            out.push(cs[cs.len() - 1]);
            out
        }
    }
}

fn looks_like_surname(tok: &str) -> bool {
    COMPOUND.iter().any(|c| tok.starts_with(c))
        || tok.chars().next().is_some_and(|c| SURNAMES.contains(c))
}

fn names(chars: &[char], hits: &mut Vec<Hit>) {
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if !is_hangul(chars[i]) || (i > 0 && is_hangul(chars[i - 1])) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && is_hangul(chars[j]) {
            j += 1;
        }
        let run: String = chars[i..j].iter().collect();
        let (tok, attached) = match run.strip_suffix('님').or_else(|| run.strip_suffix('씨')) {
            Some(t) if t.chars().count() >= 2 => (t.to_string(), true),
            _ => (run.clone(), false),
        };
        let len = tok.chars().count();
        let tok_end = i + len;
        let mut k = j;
        while k < n && chars[k] == ' ' {
            k += 1;
        }
        let after: String = chars[k..n.min(k + 4)].iter().collect();
        let titled = attached || (k > j && TITLES.iter().any(|t| after.starts_with(t)));
        let label_before = NAME_LABELS.iter().any(|l| before(chars, i, 6).contains(l));
        let org = ORG_ENDS.iter().any(|e| tok.ends_with(e));
        // A title on its own (부장님, 차장님) is not a name even though it starts with a surname.
        let title = TITLES.contains(&tok.as_str());
        if (2..=4).contains(&len)
            && looks_like_surname(&tok)
            && !org
            && !title
            && (titled || label_before)
        {
            hits.push(Hit {
                start: i,
                end: tok_end,
                text: mask_name(&tok),
                kind: "name",
            });
        }
        i = j;
    }
}

pub fn mask(text: &str) -> String {
    mask_counted(text).0
}

/// Masked text and how many items of each kind were masked.
pub fn mask_counted(text: &str) -> (String, std::collections::BTreeMap<&'static str, usize>) {
    let mut counts = std::collections::BTreeMap::new();
    let chars: Vec<char> = text.chars().collect();
    let mut hits = Vec::new();
    emails(&chars, &mut hits);
    numbers(&chars, &mut hits);
    passports(&chars, &mut hits);
    names(&chars, &mut hits);
    hits.sort_by_key(|h| (h.start, std::cmp::Reverse(h.end)));
    let mut out = String::new();
    let mut pos = 0;
    for h in hits {
        if h.start < pos {
            continue;
        }
        out.extend(&chars[pos..h.start]);
        out.push_str(&h.text);
        *counts.entry(h.kind).or_insert(0) += 1;
        pos = h.end;
    }
    out.extend(&chars[pos..]);
    (out, counts)
}

/// Keys that name where a result came from. They stay as written so the
/// original can still be opened; every other string in the result is masked.
const SOURCE_KEYS: [&str; 7] = [
    "path",
    "library",
    "sha256",
    "record_id",
    "indexed_at",
    "root",
    "name",
];

/// Mask every string in a command result except source keys, and record how many
/// items were masked under `"redacted"` when there were any.
pub fn value(output: &mut serde_json::Value) {
    fn walk(v: &mut serde_json::Value, total: &mut usize) {
        match v {
            serde_json::Value::String(s) => {
                let (masked, counts) = mask_counted(s);
                *total += counts.values().sum::<usize>();
                *s = masked;
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|i| walk(i, total)),
            serde_json::Value::Object(map) => {
                for (k, item) in map.iter_mut() {
                    if !SOURCE_KEYS.contains(&k.as_str()) {
                        walk(item, total);
                    }
                }
            }
            _ => {}
        }
    }
    let mut total = 0;
    walk(output, &mut total);
    if total > 0
        && let Some(map) = output.as_object_mut()
    {
        map.insert("redacted".into(), serde_json::json!(total));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MASKED: [(&str, &str); 37] = [
        (
            "연락처는 **010-1234-5678**입니다",
            "연락처는 **010-****-5678**입니다",
        ),
        ("이미 가린 010****5678", "이미 가린 010****5678"),
        ("줄바꿈\\n010-1234-5678", "줄바꿈\\n010-****-5678"),
        ("메일\\nhong@example.com", "메일\\n[이메일]"),
        ("휴대폰010-1234-5678/메일", "휴대폰010-****-5678/메일"),
        ("연락처01012345678로 연락", "연락처010****5678로 연락"),
        ("주민등록번호:900101-1234568", "주민등록번호:900101-1******"),
        ("사업자등록번호 123-45-67890", "사업자등록번호 123-**-***90"),
        (
            "사업자번호 1234567890 로 등록",
            "사업자번호 123*****90 로 등록",
        ),
        (
            "대표 주민번호 900101-1234568 입니다",
            "대표 주민번호 900101-1****** 입니다",
        ),
        (
            "주민등록번호: 8501012345675",
            "주민등록번호: 850101-2******",
        ),
        ("주민번호 850101 - 2345675", "주민번호 850101-2******"),
        (
            "2021년생 아이 210315-3456789",
            "2021년생 아이 210315-3******",
        ),
        (
            "외국인등록번호 950505-5123456",
            "외국인등록번호 950505-5******",
        ),
        ("외국인 010203-6123457", "외국인 010203-6******"),
        ("가린 번호 900101-1******", "가린 번호 900101-1******"),
        ("법인등록번호 110111-0123456", "법인등록번호 110111-0******"),
        ("법인번호 1101110123456", "법인번호 110111-0******"),
        ("휴대폰 010-1234-5678", "휴대폰 010-****-5678"),
        ("연락처 01012345678", "연락처 010****5678"),
        ("+82 10-1234-5678 로 연락", "+82 10-****-5678 로 연락"),
        ("카드 1234-5678-9012-3456", "카드 [카드번호]"),
        ("카드번호 1234 5678 9012 3456", "카드번호 [카드번호]"),
        ("계좌 110-123-456789 (신한)", "계좌 [계좌번호] (신한)"),
        ("국민은행 123456-01-234567", "국민은행 [계좌번호]"),
        ("입금 계좌 1002-123-456789", "입금 계좌 [계좌번호]"),
        ("메일 hong.gildong@example.co.kr", "메일 [이메일]"),
        ("e-mail: a_b+c@test.com", "e-mail: [이메일]"),
        ("여권번호 M12345678", "여권번호 [여권번호]"),
        ("운전면허 11-12-345678-90", "운전면허 [운전면허번호]"),
        ("홍길동 대표(010-9876-5432)", "홍*동 대표(010-****-5432)"),
        ("박서윤 대표님께 보고", "박*윤 대표님께 보고"),
        ("담당자 홍길동 매니저", "담당자 홍*동 매니저"),
        ("성명: 김철수", "성명: 김*수"),
        ("남궁민수 이사", "남궁*수 이사"),
        ("이영 팀장", "이* 팀장"),
        ("홍길동님 안녕하세요", "홍*동님 안녕하세요"),
    ];

    const KEPT: [&str; 27] = [
        "차장님께 보고드립니다",
        "부장님 확인 부탁",
        "사장님 말씀 이사 선임",
        "법인 사업자번호605-86-23194",
        "총 2026년11월24일",
        "법인 사업자번호 605-86-23194",
        "비영리 사업자 123-82-45678",
        "작성일 2026-11-24",
        "기간 240101-240131 동안",
        "투자금 1,234,567,890원",
        "지분율 12.34%",
        "공고번호 2026-0123",
        "시리즈A 3,000,000,000원 유치",
        "버전 1.2.3-456",
        "문서번호 CX-2026-11-0042",
        "우편번호 06236",
        "주식수 120,000주",
        "ISBN 979-11-1234-567-8",
        "회의 14:30-16:00",
        "사업비 2억 5천만원",
        "특허 10-2345678",
        "IP 203.0.113.7",
        "대표전화 02-345-6789",
        "문의 info@example.com",
        "삼성전자 대표 발표",
        "대표이사 선임 안건",
        "투자 심사역 회의",
    ];

    #[test]
    fn masks_personal_data() {
        let mut bad = Vec::new();
        for (input, want) in MASKED {
            let got = mask(input);
            if got != want {
                bad.push(format!("{input} => {got} (want {want})"));
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {} wrong:\n{}",
            bad.len(),
            MASKED.len(),
            bad.join("\n")
        );
    }

    #[test]
    fn keeps_public_and_ordinary_text() {
        let mut bad = Vec::new();
        for input in KEPT {
            let got = mask(input);
            if got != input {
                bad.push(format!("{input} => {got}"));
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {} changed:\n{}",
            bad.len(),
            KEPT.len(),
            bad.join("\n")
        );
    }

    #[test]
    fn name_middle_is_starred() {
        assert_eq!(mask_name("홍길동"), "홍*동");
        assert_eq!(mask_name("김철"), "김*");
        assert_eq!(mask_name("남궁민수"), "남궁*수");
        assert_eq!(mask_name("김수한무"), "김**무");
    }

    #[test]
    fn a_result_is_masked_except_where_it_came_from() {
        let mut v = serde_json::json!({"library":"main","path":"people/홍길동 대표.md",
            "text":"홍길동 대표 010-1234-5678","hits":[{"path":"a.md","snippet":"900101-1234568"}]});
        value(&mut v);
        assert_eq!(v["path"], "people/홍길동 대표.md");
        assert_eq!(v["text"], "홍*동 대표 010-****-5678");
        assert_eq!(v["hits"][0]["snippet"], "900101-1******");
        assert_eq!(v["redacted"], 3);
    }
}
