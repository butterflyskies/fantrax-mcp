use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::instrument;

use crate::db::Database;
use crate::util;

// ─── Cache TTLs (seconds) ──────────────────────────────────────────────────

const PROJECTIONS_TTL: i64 = 24 * 3600; // 24 hours

/// FanGraphs projections API base URL.
const API_BASE: &str = "https://www.fangraphs.com/api/projections";

/// FanGraphs CSV export URL (fallback).
const CSV_BASE: &str = "https://www.fangraphs.com/projections";

// ─── Error type ────────────────────────────────────────────────────────────

/// Errors that can occur when fetching FanGraphs projections.
#[derive(Debug)]
pub enum ProjectionError {
    /// HTTP transport error from reqwest.
    Http(reqwest::Error),
    /// The API/CSV returned data we couldn't parse.
    Parse(String),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Parse(msg) => write!(f, "FanGraphs parse error: {msg}"),
        }
    }
}

impl std::error::Error for ProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Parse(_) => None,
        }
    }
}

impl From<reqwest::Error> for ProjectionError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

// ─── Projection types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatterProjection {
    pub player_name: String,
    pub team: String,
    pub pa: f64,
    pub hr: f64,
    pub rbi: f64,
    pub sb: f64,
    pub avg: f64,
    pub obp: f64,
    pub slg: f64,
    pub ops: f64,
    pub war: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PitcherProjection {
    pub player_name: String,
    pub team: String,
    pub ip: f64,
    pub w: f64,
    pub era: f64,
    pub whip: f64,
    pub k: f64,
    pub sv: f64,
    pub war: f64,
}

// ─── Client implementation ─────────────────────────────────────────────────

/// HTTP client for FanGraphs projections with SQLite-backed caching.
pub struct ProjectionClient {
    http: reqwest::Client,
    db: Arc<Database>,
    /// Projection system name (e.g. "steamer", "zips", "atc").
    source: String,
    /// Cache TTL in seconds (from config.projections.refresh_hours).
    ttl: i64,
}

impl ProjectionClient {
    pub fn new(db: Arc<Database>, source: String, refresh_hours: u64) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:138.0) Gecko/20100101 Firefox/138.0")
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let ttl = if refresh_hours > 0 {
            (refresh_hours * 3600) as i64
        } else {
            PROJECTIONS_TTL
        };

        Self {
            http,
            db,
            source,
            ttl,
        }
    }

    /// Fetch batter projections, returning results sorted by WAR descending.
    #[instrument(skip(self), fields(source = %self.source))]
    pub async fn get_batter_projections(&self) -> Result<Vec<BatterProjection>, ProjectionError> {
        let cache_key = format!("fg:proj:bat:{}", self.source);

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!("batter projections cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| ProjectionError::Parse(format!("cache deserialize error: {e}")));
        }

        let mut projections = match self.fetch_batter_api().await {
            Ok(p) => p,
            Err(api_err) => {
                tracing::warn!(%api_err, "API fetch failed, falling back to CSV");
                self.fetch_batter_csv().await?
            }
        };

        // Sort by WAR descending.
        projections.sort_by(|a, b| {
            b.war
                .partial_cmp(&a.war)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let value = serde_json::to_value(&projections)
            .map_err(|e| ProjectionError::Parse(format!("serialize error: {e}")))?;
        self.db.set_cached(&cache_key, &value, self.ttl);

        Ok(projections)
    }

    /// Fetch pitcher projections, returning results sorted by WAR descending.
    #[instrument(skip(self), fields(source = %self.source))]
    pub async fn get_pitcher_projections(&self) -> Result<Vec<PitcherProjection>, ProjectionError> {
        let cache_key = format!("fg:proj:pit:{}", self.source);

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!("pitcher projections cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| ProjectionError::Parse(format!("cache deserialize error: {e}")));
        }

        let mut projections = match self.fetch_pitcher_api().await {
            Ok(p) => p,
            Err(api_err) => {
                tracing::warn!(%api_err, "API fetch failed, falling back to CSV");
                self.fetch_pitcher_csv().await?
            }
        };

        // Sort by WAR descending.
        projections.sort_by(|a, b| {
            b.war
                .partial_cmp(&a.war)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let value = serde_json::to_value(&projections)
            .map_err(|e| ProjectionError::Parse(format!("serialize error: {e}")))?;
        self.db.set_cached(&cache_key, &value, self.ttl);

        Ok(projections)
    }

    // ─── API fetch (JSON) ──────────────────────────────────────────────────

    async fn fetch_batter_api(&self) -> Result<Vec<BatterProjection>, ProjectionError> {
        let resp: Value = self
            .http
            .get(API_BASE)
            .query(&[
                ("type", self.source.as_str()),
                ("stats", "bat"),
                ("pos", "all"),
                ("team", "0"),
                ("players", "0"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!("batter projections fetched from API");
        parse_batter_json(&resp)
    }

    async fn fetch_pitcher_api(&self) -> Result<Vec<PitcherProjection>, ProjectionError> {
        let resp: Value = self
            .http
            .get(API_BASE)
            .query(&[
                ("type", self.source.as_str()),
                ("stats", "pit"),
                ("pos", "all"),
                ("team", "0"),
                ("players", "0"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!("pitcher projections fetched from API");
        parse_pitcher_json(&resp)
    }

    // ─── CSV fetch (fallback) ──────────────────────────────────────────────

    async fn fetch_batter_csv(&self) -> Result<Vec<BatterProjection>, ProjectionError> {
        let url = format!(
            "{CSV_BASE}?pos=all&stats=bat&type={}&team=0&lg=all&players=0",
            self.source
        );
        let text = self
            .http
            .get(&url)
            .header("Accept", "text/csv, text/plain, */*")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        tracing::debug!(bytes = text.len(), "batter CSV fetched");
        parse_batter_csv(&text)
    }

    async fn fetch_pitcher_csv(&self) -> Result<Vec<PitcherProjection>, ProjectionError> {
        let url = format!(
            "{CSV_BASE}?pos=all&stats=pit&type={}&team=0&lg=all&players=0",
            self.source
        );
        let text = self
            .http
            .get(&url)
            .header("Accept", "text/csv, text/plain, */*")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        tracing::debug!(bytes = text.len(), "pitcher CSV fetched");
        parse_pitcher_csv(&text)
    }
}

// ─── JSON parsing ──────────────────────────────────────────────────────────

/// Parse batter projections from FanGraphs JSON API response.
///
/// The API returns an array of player objects with varying field names.
fn parse_batter_json(resp: &Value) -> Result<Vec<BatterProjection>, ProjectionError> {
    let arr = resp.as_array().ok_or_else(|| {
        ProjectionError::Parse(format!(
            "expected JSON array, got: {}",
            resp.to_string().chars().take(200).collect::<String>()
        ))
    })?;

    let projections = arr
        .iter()
        .filter_map(|entry| {
            let player_name = util::json_str(entry, &["PlayerName", "Name", "playerName", "name"])?;
            let team = util::json_str(entry, &["Team", "team", "TeamName"]).unwrap_or_default();

            Some(BatterProjection {
                player_name,
                team,
                pa: util::json_f64(entry, &["PA", "pa", "PlateAppearances"]),
                hr: util::json_f64(entry, &["HR", "hr", "HomeRuns"]),
                rbi: util::json_f64(entry, &["RBI", "rbi"]),
                sb: util::json_f64(entry, &["SB", "sb", "StolenBases"]),
                avg: util::json_f64(entry, &["AVG", "avg", "BA"]),
                obp: util::json_f64(entry, &["OBP", "obp"]),
                slg: util::json_f64(entry, &["SLG", "slg"]),
                ops: util::json_f64(entry, &["OPS", "ops"]),
                war: util::json_f64(entry, &["WAR", "war"]),
            })
        })
        .collect();

    Ok(projections)
}

/// Parse pitcher projections from FanGraphs JSON API response.
fn parse_pitcher_json(resp: &Value) -> Result<Vec<PitcherProjection>, ProjectionError> {
    let arr = resp.as_array().ok_or_else(|| {
        ProjectionError::Parse(format!(
            "expected JSON array, got: {}",
            resp.to_string().chars().take(200).collect::<String>()
        ))
    })?;

    let projections = arr
        .iter()
        .filter_map(|entry| {
            let player_name = util::json_str(entry, &["PlayerName", "Name", "playerName", "name"])?;
            let team = util::json_str(entry, &["Team", "team", "TeamName"]).unwrap_or_default();

            Some(PitcherProjection {
                player_name,
                team,
                ip: util::json_f64(entry, &["IP", "ip", "InningsPitched"]),
                w: util::json_f64(entry, &["W", "w", "Wins"]),
                era: util::json_f64(entry, &["ERA", "era"]),
                whip: util::json_f64(entry, &["WHIP", "whip"]),
                k: util::json_f64(entry, &["SO", "K", "k", "so", "Strikeouts"]),
                sv: util::json_f64(entry, &["SV", "sv", "Saves"]),
                war: util::json_f64(entry, &["WAR", "war"]),
            })
        })
        .collect();

    Ok(projections)
}

// ─── CSV parsing ───────────────────────────────────────────────────────────

/// Parse batter projections from FanGraphs CSV export.
fn parse_batter_csv(text: &str) -> Result<Vec<BatterProjection>, ProjectionError> {
    let mut lines = text.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| ProjectionError::Parse("empty CSV".into()))?;
    let headers: Vec<&str> = header_line.split(',').map(str::trim).collect();

    let col = |name: &str| -> Option<usize> {
        headers.iter().position(|h| {
            h.eq_ignore_ascii_case(name) || h.trim_matches('"').eq_ignore_ascii_case(name)
        })
    };

    let name_col = col("Name")
        .or_else(|| col("PlayerName"))
        .ok_or_else(|| ProjectionError::Parse("no Name column in CSV".into()))?;
    let team_col = col("Team");
    let pa_col = col("PA");
    let hr_col = col("HR");
    let rbi_col = col("RBI");
    let sb_col = col("SB");
    let avg_col = col("AVG");
    let obp_col = col("OBP");
    let slg_col = col("SLG");
    let ops_col = col("OPS");
    let war_col = col("WAR");

    let mut projections = Vec::new();
    for line in lines {
        let fields = parse_csv_line(line);
        if fields.len() <= name_col {
            continue;
        }

        let player_name = fields[name_col].trim_matches('"').to_string();
        if player_name.is_empty() {
            continue;
        }
        let team = team_col
            .and_then(|c| fields.get(c))
            .map(|s| s.trim_matches('"').to_string())
            .unwrap_or_default();

        projections.push(BatterProjection {
            player_name,
            team,
            pa: util::csv_f64(&fields, pa_col),
            hr: util::csv_f64(&fields, hr_col),
            rbi: util::csv_f64(&fields, rbi_col),
            sb: util::csv_f64(&fields, sb_col),
            avg: util::csv_f64(&fields, avg_col),
            obp: util::csv_f64(&fields, obp_col),
            slg: util::csv_f64(&fields, slg_col),
            ops: util::csv_f64(&fields, ops_col),
            war: util::csv_f64(&fields, war_col),
        });
    }

    if projections.is_empty() {
        return Err(ProjectionError::Parse(
            "no batter projections parsed from CSV".into(),
        ));
    }

    Ok(projections)
}

/// Parse pitcher projections from FanGraphs CSV export.
fn parse_pitcher_csv(text: &str) -> Result<Vec<PitcherProjection>, ProjectionError> {
    let mut lines = text.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| ProjectionError::Parse("empty CSV".into()))?;
    let headers: Vec<&str> = header_line.split(',').map(str::trim).collect();

    let col = |name: &str| -> Option<usize> {
        headers.iter().position(|h| {
            h.eq_ignore_ascii_case(name) || h.trim_matches('"').eq_ignore_ascii_case(name)
        })
    };

    let name_col = col("Name")
        .or_else(|| col("PlayerName"))
        .ok_or_else(|| ProjectionError::Parse("no Name column in CSV".into()))?;
    let team_col = col("Team");
    let ip_col = col("IP");
    let w_col = col("W");
    let era_col = col("ERA");
    let whip_col = col("WHIP");
    let k_col = col("SO").or_else(|| col("K"));
    let sv_col = col("SV");
    let war_col = col("WAR");

    let mut projections = Vec::new();
    for line in lines {
        let fields = parse_csv_line(line);
        if fields.len() <= name_col {
            continue;
        }

        let player_name = fields[name_col].trim_matches('"').to_string();
        if player_name.is_empty() {
            continue;
        }
        let team = team_col
            .and_then(|c| fields.get(c))
            .map(|s| s.trim_matches('"').to_string())
            .unwrap_or_default();

        projections.push(PitcherProjection {
            player_name,
            team,
            ip: util::csv_f64(&fields, ip_col),
            w: util::csv_f64(&fields, w_col),
            era: util::csv_f64(&fields, era_col),
            whip: util::csv_f64(&fields, whip_col),
            k: util::csv_f64(&fields, k_col),
            sv: util::csv_f64(&fields, sv_col),
            war: util::csv_f64(&fields, war_col),
        });
    }

    if projections.is_empty() {
        return Err(ProjectionError::Parse(
            "no pitcher projections parsed from CSV".into(),
        ));
    }

    Ok(projections)
}

/// Simple CSV line parser that handles quoted fields with commas.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}
