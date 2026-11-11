//! Wanted executor: a set of `key=value` filters on actor fields.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Actor fields a filter can test.
pub const EXECUTOR_FIELDS: &[&str] = &["kind", "harness", "model", "effort"];

/// The fields of an actor that a wanted executor tests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActorFields {
    pub kind: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ActorFields {
    fn get(&self, field: &str) -> Option<&str> {
        match field {
            "kind" => Some(&self.kind),
            "harness" => self.harness.as_deref(),
            "model" => self.model.as_deref(),
            "effort" => self.effort.as_deref(),
            _ => None,
        }
    }
}

/// An empty set means anyone. An actor matches when every filter matches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct WantedExecutor(pub BTreeMap<String, String>);

impl WantedExecutor {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matches(&self, actor: &ActorFields) -> bool {
        self.0
            .iter()
            .all(|(k, v)| actor.get(k).is_some_and(|a| a.eq_ignore_ascii_case(v)))
    }

    /// True when every filter in `self` also appears in `query` with the same
    /// value. The frontier `--for` filter uses this: an issue that wants
    /// `kind=agent` suits a caller who says `kind=agent,model=opus`.
    pub fn satisfied_by(&self, query: &WantedExecutor) -> bool {
        self.0
            .iter()
            .all(|(k, v)| query.0.get(k).is_some_and(|q| q.eq_ignore_ascii_case(v)))
    }
}

impl fmt::Display for WantedExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(|(k, v)| format!("{k}={v}")).collect();
        f.write_str(&parts.join(","))
    }
}

impl FromStr for WantedExecutor {
    type Err = String;

    /// Parses `kind=agent,model=opus-5.5`. An empty string means anyone.
    fn from_str(s: &str) -> Result<Self, String> {
        let mut map = BTreeMap::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got {part:?}"))?;
            let k = k.trim().to_ascii_lowercase();
            if !EXECUTOR_FIELDS.contains(&k.as_str()) {
                return Err(format!(
                    "unknown executor field {k:?}; expected one of: {}",
                    EXECUTOR_FIELDS.join(", ")
                ));
            }
            map.insert(k, v.trim().to_string());
        }
        Ok(Self(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(model: &str) -> ActorFields {
        ActorFields {
            kind: "agent".into(),
            harness: Some("claude-code".into()),
            model: Some(model.into()),
            effort: Some("high".into()),
        }
    }

    #[test]
    fn matching() {
        let any: WantedExecutor = "".parse().unwrap();
        assert!(any.matches(&agent("opus-5.5")));
        let opus: WantedExecutor = "kind=agent, model=opus-5.5".parse().unwrap();
        assert!(opus.matches(&agent("opus-5.5")));
        assert!(!opus.matches(&agent("sonnet-5.5")));
        let human: WantedExecutor = "kind=human".parse().unwrap();
        assert!(!human.matches(&agent("opus-5.5")));
        assert_eq!(opus.to_string(), "kind=agent,model=opus-5.5");
    }

    #[test]
    fn satisfied_by_query() {
        let wants: WantedExecutor = "kind=agent".parse().unwrap();
        assert!(wants.satisfied_by(&"kind=agent,model=opus".parse().unwrap()));
        assert!(!wants.satisfied_by(&"model=opus".parse().unwrap()));
        assert!(WantedExecutor::default().satisfied_by(&"model=x".parse().unwrap()));
    }

    #[test]
    fn rejects_unknown_field() {
        assert!("color=red".parse::<WantedExecutor>().is_err());
    }
}
