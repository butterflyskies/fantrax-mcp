use std::fmt;
use std::path::{Path, PathBuf};

use secrecy::SecretString;
use serde::Deserialize;

// ─── Error type ────────────────────────────────────────────────────────────

/// Errors that can occur when loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Failed to read the config file.
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse the config TOML.
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
}

pub struct Config {
    pub fantrax: FantraxConfig,
    pub leagues: Vec<LeagueConfig>,
    pub projections: ProjectionsConfig,
    pub server: ServerConfig,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("fantrax", &self.fantrax)
            .field("leagues", &self.leagues)
            .field("projections", &self.projections)
            .field("server", &self.server)
            .finish()
    }
}

pub struct FantraxConfig {
    /// Wrapped in `Option` so the secret can be moved out exactly once (into
    /// `FantraxClient`) without cloning the plaintext.
    pub user_secret_id: Option<SecretString>,
}

impl fmt::Debug for FantraxConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FantraxConfig")
            .field("user_secret_id", &"[REDACTED]")
            .finish()
    }
}

/// Intermediate struct for deserializing the config file before wrapping secrets.
#[derive(Deserialize)]
struct RawConfig {
    fantrax: RawFantraxConfig,
    leagues: Vec<LeagueConfig>,
    projections: ProjectionsConfig,
    server: ServerConfig,
}

#[derive(Deserialize)]
struct RawFantraxConfig {
    user_secret_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LeagueConfig {
    pub id: String,
    pub name: String,
    pub league_type: LeagueType,
    pub lineup: LineupType,
    #[serde(default)]
    pub keeper: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LeagueType {
    H2h,
    Roto,
    Hybrid,
    Custom,
}

impl fmt::Display for LeagueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::H2h => write!(f, "h2h"),
            Self::Roto => write!(f, "roto"),
            Self::Hybrid => write!(f, "hybrid"),
            Self::Custom => write!(f, "custom"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LineupType {
    Daily,
    Weekly,
    Bestball,
    Worstball,
}

impl fmt::Display for LineupType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Daily => write!(f, "daily"),
            Self::Weekly => write!(f, "weekly"),
            Self::Bestball => write!(f, "bestball"),
            Self::Worstball => write!(f, "worstball"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ProjectionsConfig {
    pub source: String,
    pub refresh_hours: u64,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub db_path: String,
}

impl ServerConfig {
    /// Resolve the db_path, expanding `~` to the user's home directory.
    pub fn resolved_db_path(&self) -> PathBuf {
        if self.db_path.starts_with("~/")
            && let Some(home) = dirs_fallback()
        {
            return home.join(&self.db_path[2..]);
        }
        PathBuf::from(&self.db_path)
    }
}

fn dirs_fallback() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let raw: RawConfig = toml::from_str(&contents)?;
        Ok(Config {
            fantrax: FantraxConfig {
                user_secret_id: Some(SecretString::from(raw.fantrax.user_secret_id)),
            },
            leagues: raw.leagues,
            projections: raw.projections,
            server: raw.server,
        })
    }
}
