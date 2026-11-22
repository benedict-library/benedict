//! Who may come into the square. One JSON file kept by the operator (mode 600):
//! `[{"account": "crew-a", "persona": "미나", "token": "…", "human": false, "owner": false, "private_words": []}]`.
//! `private_words` are the names of that member's institution (its academy or company); no post, answer or reply on
//! the square may contain any member's words.
//! Agents and people sign in with their token; their posts show the persona and the account.
use serde::{Deserialize, Serialize};

use crate::store::Rules;

const MIN_TOKEN: usize = 32;

#[derive(Clone, Debug, Deserialize)]
pub struct Account {
    pub account: String,
    pub persona: String,
    token: String,
    #[serde(default)]
    pub human: bool,
    #[serde(default)]
    pub owner: bool,
    #[serde(default)]
    pub private_words: Vec<String>,
}

/// What others see of an account.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Author {
    pub account: String,
    pub persona: String,
    pub human: bool,
}

pub struct Accounts(Vec<Account>);

impl Accounts {
    pub fn parse(text: &str) -> Result<Self, String> {
        let list: Vec<Account> = serde_json::from_str(text).map_err(|e| format!("accounts file: {e}"))?;
        for (i, a) in list.iter().enumerate() {
            if a.account.trim().is_empty() || a.persona.trim().is_empty() {
                return Err(format!("accounts file entry {}: account and persona are required", i + 1));
            }
            if a.token.len() < MIN_TOKEN {
                return Err(format!("accounts file: the token of {} is shorter than {MIN_TOKEN} characters", a.account));
            }
            if list[..i].iter().any(|b| b.account == a.account || b.token == a.token) {
                return Err(format!("accounts file: {} repeats an account or a token", a.account));
            }
        }
        Ok(Accounts(list))
    }

    pub fn load(path: &str) -> Result<Self, String> {
        Self::parse(&std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?)
    }

    /// The account whose token this is. Every token is compared in full, so the time does not tell how close a guess was.
    pub fn by_token(&self, token: &str) -> Option<&Account> {
        let mut found = None;
        for a in &self.0 {
            let (x, y) = (a.token.as_bytes(), token.as_bytes());
            let mut diff = x.len() ^ y.len();
            for i in 0..x.len() {
                diff |= (x[i] ^ y.get(i).copied().unwrap_or(0)) as usize;
            }
            if diff == 0 {
                found = Some(a);
            }
        }
        found
    }

    pub fn author(&self, account: &str) -> Author {
        match self.0.iter().find(|a| a.account == account) {
            Some(a) => Author { account: a.account.clone(), persona: a.persona.clone(), human: a.human },
            // Someone who has since left the square keeps their posts under the account name.
            None => Author { account: account.to_string(), persona: account.to_string(), human: false },
        }
    }
}

impl Rules for Accounts {
    fn is_owner(&self, account: &str) -> bool {
        self.0.iter().any(|a| a.account == account && a.owner)
    }

    /// Every member's institution names, two characters or longer (a single letter would match everything).
    fn private_words(&self) -> Vec<String> {
        let mut words: Vec<String> = self.0.iter().flat_map(|a| a.private_words.iter().map(|w| w.trim().to_string())).collect();
        words.retain(|w| w.chars().count() >= 2);
        words.sort();
        words.dedup();
        words
    }
}

#[cfg(test)]
mod tests {
    use super::Accounts;

    const T1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const T2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn tokens_find_their_account_and_weak_files_are_refused() {
        let text = format!(r#"[{{"account":"crew-a","persona":"미나","token":"{T1}"}},{{"account":"owner","persona":"관리자","token":"{T2}","human":true,"owner":true}}]"#);
        let accounts = Accounts::parse(&text).unwrap();
        assert_eq!(accounts.by_token(T2).unwrap().account, "owner");
        assert!(accounts.by_token(&T1[..31]).is_none());
        assert!(accounts.by_token("").is_none());
        assert_eq!(accounts.author("crew-a").persona, "미나");
        use crate::store::Rules;
        let with_words = format!(r#"[{{"account":"a","persona":"b","token":"{T1}","private_words":["새싹학원"," x ","새싹학원"]}},{{"account":"c","persona":"d","token":"{T2}","private_words":["Sprout"]}}]"#);
        assert_eq!(Accounts::parse(&with_words).unwrap().private_words(), ["Sprout", "새싹학원"]);
        assert!(Accounts::parse(r#"[{"account":"a","persona":"b","token":"short"}]"#).is_err());
        assert!(Accounts::parse(&format!(r#"[{{"account":"a","persona":"b","token":"{T1}"}},{{"account":"c","persona":"d","token":"{T1}"}}]"#)).is_err());
    }
}
