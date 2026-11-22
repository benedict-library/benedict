//! What must not reach the square: contact details and identity numbers. The asker rewrites a question without
//! names and numbers first; this is the second look, on every post, answer and reply.

/// The kinds of private detail found in the text, in the order first seen; empty when the text may go up.
pub fn findings(text: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    let mut add = |kind| {
        if !found.contains(&kind) {
            found.push(kind);
        }
    };
    for groups in number_runs(text) {
        if let Some(kind) = number_kind(&groups) {
            add(kind);
        }
    }
    if text.split(|c: char| c.is_whitespace() || "<>()[]{},;:'\"".contains(c)).any(is_email) {
        add("email address");
    }
    found
}

/// Digit groups joined by one separator each ("010-1234-5678" → ["010", "1234", "5678"]).
fn number_runs(text: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = text.chars().collect();
    let (mut runs, mut groups, mut digits) = (Vec::new(), Vec::<String>::new(), String::new());
    let mut i = 0;
    while i <= chars.len() {
        let c = chars.get(i).copied();
        if c.is_some_and(|c| c.is_ascii_digit()) {
            digits.push(c.unwrap());
        } else {
            if !digits.is_empty() {
                groups.push(std::mem::take(&mut digits));
            }
            let joins = c.is_some_and(|c| "- .".contains(c)) && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
            if !joins && !groups.is_empty() {
                runs.push(std::mem::take(&mut groups));
            }
        }
        i += 1;
    }
    runs
}

fn number_kind(groups: &[String]) -> Option<&'static str> {
    let all: String = groups.concat();
    let lens: Vec<usize> = groups.iter().map(String::len).collect();
    if lens == [6, 7] || (lens == [13] && matches!(all.as_bytes()[6], b'1'..=b'4')) {
        return Some("resident registration number");
    }
    if all.starts_with("01") && matches!(all.len(), 10 | 11) && matches!(all.as_bytes()[2], b'0' | b'1' | b'6'..=b'9') {
        return Some("phone number");
    }
    if all.starts_with('0') && groups.len() == 3 && matches!(all.len(), 9..=11) && lens[0] <= 3 {
        return Some("phone number");
    }
    if matches!(all.len(), 15 | 16) && (groups.len() == 1 || lens.iter().all(|&l| l == 4) || lens == [4, 6, 5]) {
        return Some("card number");
    }
    None
}

fn is_email(word: &str) -> bool {
    let Some((user, host)) = word.split_once('@') else { return false };
    let host = host.trim_end_matches('.');
    !user.is_empty()
        && user.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
        && host.contains('.')
        && host.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

#[cfg(test)]
mod tests {
    use super::findings;

    #[test]
    fn contact_details_and_identity_numbers_are_found() {
        assert_eq!(findings("연락은 010-1234-5678 로"), ["phone number"]);
        assert_eq!(findings("01012345678"), ["phone number"]);
        assert_eq!(findings("학원 02-123-4567"), ["phone number"]);
        assert_eq!(findings("메일 hong@example.co.kr 로 보내 주세요."), ["email address"]);
        assert_eq!(findings("900101-1234567"), ["resident registration number"]);
        assert_eq!(findings("1234 5678 9012 3456"), ["card number"]);
        assert_eq!(findings("010 1234 5678, a@b.io"), ["phone number", "email address"]);
    }

    #[test]
    fn ordinary_numbers_go_up() {
        for text in ["2026-11-08 10:12", "공가 3일, 출석률 80%", "HRD-Net 1.5판", "과정 20250101", "1,500원", "@멘션"] {
            assert!(findings(text).is_empty(), "{text}");
        }
    }
}
