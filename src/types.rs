//! Newtype wrappers for domain identifiers.
//!
//! These prevent accidentally mixing semantically different string IDs
//! (league, team, player) at compile time.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ─── Macro ────────────────────────────────────────────────────────────────

/// Generate a newtype ID wrapper with standard trait impls.
macro_rules! newtype_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    };
}

// ─── ID types ─────────────────────────────────────────────────────────────

newtype_id!(
    /// A Fantrax league identifier.
    LeagueId
);

newtype_id!(
    /// A Fantrax team identifier.
    TeamId
);

newtype_id!(
    /// A Fantrax player identifier.
    ///
    /// This is the Fantrax-side ID, which may or may not be numeric.
    /// For MLB Stats API lookups, parse with `.as_str().parse::<u64>()`.
    PlayerId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_delegates_to_inner() {
        let id = LeagueId::new("abc123");
        assert_eq!(id.to_string(), "abc123");
    }

    #[test]
    fn as_str_returns_inner() {
        let id = TeamId::new("team-42");
        assert_eq!(id.as_str(), "team-42");
    }

    #[test]
    fn as_ref_returns_inner() {
        let id = PlayerId::new("p1");
        let s: &str = id.as_ref();
        assert_eq!(s, "p1");
    }

    #[test]
    fn from_string() {
        let id: LeagueId = String::from("league-1").into();
        assert_eq!(id.as_str(), "league-1");
    }

    #[test]
    fn from_str_ref() {
        let id: TeamId = "team-x".into();
        assert_eq!(id.as_str(), "team-x");
    }

    #[test]
    fn equality() {
        assert_eq!(PlayerId::new("a"), PlayerId::new("a"));
        assert_ne!(PlayerId::new("a"), PlayerId::new("b"));
    }

    #[test]
    fn serde_transparent_roundtrip() {
        let id = LeagueId::new("my-league");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"my-league\"");
        let back: LeagueId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn hash_works() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(TeamId::new("t1"));
        set.insert(TeamId::new("t2"));
        set.insert(TeamId::new("t1"));
        assert_eq!(set.len(), 2);
    }
}
