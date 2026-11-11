//! Issue keys such as `ALX-123`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Longest team key we accept, such as `ALX` or `WEB2`.
pub const MAX_TEAM_KEY_LEN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueKey {
    pub team: String,
    pub number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("issue key must look like ABC-123, got {0:?}")]
    Malformed(String),
    #[error(
        "team key must be 1 to {MAX_TEAM_KEY_LEN} characters, start with a letter, and hold only A-Z and 0-9; got {0:?}"
    )]
    BadTeamKey(String),
}

/// Checks a team key and returns it in upper case.
pub fn normalize_team_key(raw: &str) -> Result<String, KeyError> {
    let up = raw.trim().to_ascii_uppercase();
    let ok = !up.is_empty()
        && up.len() <= MAX_TEAM_KEY_LEN
        && up.as_bytes()[0].is_ascii_uppercase()
        && up
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if ok {
        Ok(up)
    } else {
        Err(KeyError::BadTeamKey(raw.to_string()))
    }
}

impl IssueKey {
    pub fn new(team: impl Into<String>, number: u32) -> Self {
        Self {
            team: team.into(),
            number,
        }
    }
}

impl fmt::Display for IssueKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.team, self.number)
    }
}

impl FromStr for IssueKey {
    type Err = KeyError;

    /// Parses `ALX-123`. Lower case input is accepted: `alx-123`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (team, num) = s
            .rsplit_once('-')
            .ok_or_else(|| KeyError::Malformed(s.into()))?;
        let team = normalize_team_key(team).map_err(|_| KeyError::Malformed(s.into()))?;
        if num.is_empty()
            || num.len() > 9
            || !num.bytes().all(|b| b.is_ascii_digit())
            || num.starts_with('0')
        {
            return Err(KeyError::Malformed(s.into()));
        }
        let number = num.parse().map_err(|_| KeyError::Malformed(s.into()))?;
        Ok(Self { team, number })
    }
}

impl Serialize for IssueKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IssueKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for IssueKey {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "IssueKey".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^[A-Z][A-Z0-9]{0,9}-[1-9][0-9]{0,8}$", "examples": ["ALX-123"] })
    }
}

/// A key found inside free text, with its byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMatch {
    pub key: IssueKey,
    pub start: usize,
    pub end: usize,
}

/// Finds issue keys in text such as a PR title, a branch name or Markdown.
///
/// Only upper-case keys count in prose (`ALX-12`), so words like `utf-8` do
/// not match. Branch names are usually lower case, so use
/// [`find_keys_any_case`] for them.
pub fn find_keys(text: &str) -> Vec<KeyMatch> {
    scan(text, false)
}

/// Like [`find_keys`], but also matches `alx-12`, as in `alx-12-fix-login`.
pub fn find_keys_any_case(text: &str) -> Vec<KeyMatch> {
    scan(text, true)
}

fn scan(text: &str, any_case: bool) -> Vec<KeyMatch> {
    let b = text.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let letter = if any_case {
            b[i].is_ascii_alphabetic()
        } else {
            b[i].is_ascii_uppercase()
        };
        if !letter || (i > 0 && is_word(b[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < b.len()
            && (if any_case {
                b[j].is_ascii_alphanumeric()
            } else {
                b[j].is_ascii_uppercase() || b[j].is_ascii_digit()
            })
        {
            j += 1;
        }
        if j - start > MAX_TEAM_KEY_LEN || j >= b.len() || b[j] != b'-' {
            i = j.max(i + 1);
            continue;
        }
        let mut k = j + 1;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        let ends_clean = k == b.len() || !is_word(b[k]);
        if k > j + 1
            && ends_clean
            && let Ok(key) = text[start..k].parse::<IssueKey>()
        {
            out.push(KeyMatch { key, start, end: k });
        }
        i = k.max(j + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let k: IssueKey = "alx-42".parse().unwrap();
        assert_eq!(k, IssueKey::new("ALX", 42));
        assert_eq!(k.to_string(), "ALX-42");
    }

    #[test]
    fn rejects_bad_keys() {
        for bad in [
            "ALX",
            "ALX-",
            "-1",
            "ALX-01",
            "1AB-3",
            "ALX-1a",
            "TOOLONGTEAMKEY-1",
            "ALX-1234567890",
        ] {
            assert!(bad.parse::<IssueKey>().is_err(), "{bad}");
        }
    }

    #[test]
    fn finds_keys_in_text() {
        let found: Vec<_> = find_keys("Fixes ALX-12, see (WEB2-3). Not utf-8 or xALX-9 or ALX-5x.")
            .into_iter()
            .map(|m| m.key.to_string())
            .collect();
        assert_eq!(found, ["ALX-12", "WEB2-3"]);
    }

    #[test]
    fn finds_keys_in_branches() {
        let found: Vec<_> = find_keys_any_case("alex/alx-435-fix-login")
            .into_iter()
            .map(|m| m.key.to_string())
            .collect();
        assert_eq!(found, ["ALX-435"]);
    }

    #[test]
    fn team_keys() {
        assert_eq!(normalize_team_key("alx").unwrap(), "ALX");
        assert!(normalize_team_key("a-b").is_err());
        assert!(normalize_team_key("").is_err());
    }
}
