use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::instrument;

use crate::db::Database;

// ─── Cache TTLs (seconds) ──────────────────────────────────────────────────

const PROJECTIONS_TTL: i64 = 24 * 3600; // 24 hours

/// MLB Stats API base URL for projections.
const API_BASE: &str = "https://statsapi.mlb.com/api/v1/stats";

// ─── Error type ────────────────────────────────────────────────────────────

/// Errors that can occur when fetching MLB Stats API projections.
#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    /// HTTP transport error from reqwest.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// The API returned data we couldn't parse.
    #[error("MLB Stats API parse error: {0}")]
    Parse(String),
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

/// HTTP client for MLB Stats API projections with SQLite-backed caching.
pub struct ProjectionClient {
    http: reqwest::Client,
    db: Arc<Database>,
    /// Projection stat type (e.g. "projected_ZipsRos").
    source: String,
    /// Cache TTL in seconds (from config.projections.refresh_hours).
    ttl: i64,
}

impl ProjectionClient {
    pub fn new(db: Arc<Database>, source: String, refresh_hours: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let ttl = if refresh_hours > 0 {
            (refresh_hours * 3600) as i64
        } else {
            PROJECTIONS_TTL
        };

        const VALID_SOURCES: &[&str] = &[
            "projected",
            "projectedRos",
            "projected_Zips",
            "projected_ZipsRos",
            "projected_Zips2YR",
            "projected_Zips3YR",
        ];

        let source = if source.is_empty() || !VALID_SOURCES.contains(&source.as_str()) {
            if !source.is_empty() {
                tracing::warn!(
                    old_source = %source,
                    "unrecognized projection source (FanGraphs values are no longer valid), defaulting to projected_ZipsRos"
                );
            }
            "projected_ZipsRos".to_string()
        } else {
            source
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
        let cache_key = format!("mlb:proj:bat:{}", self.source);

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!("batter projections cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| ProjectionError::Parse(format!("cache deserialize error: {e}")));
        }

        let mut projections = self.fetch_batter_projections().await?;

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
        let cache_key = format!("mlb:proj:pit:{}", self.source);

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!("pitcher projections cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| ProjectionError::Parse(format!("cache deserialize error: {e}")));
        }

        let mut projections = self.fetch_pitcher_projections().await?;

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

    // ─── MLB Stats API fetch ──────────────────────────────────────────────

    async fn fetch_batter_projections(&self) -> Result<Vec<BatterProjection>, ProjectionError> {
        let resp: Value = self
            .http
            .get(API_BASE)
            .query(&[
                ("stats", self.source.as_str()),
                ("group", "hitting"),
                ("sportIds", "1"),
                ("season", "2026"),
                ("sortStat", "war"),
                ("order", "desc"),
                ("limit", "500"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!("batter projections fetched from MLB Stats API");
        parse_batter_response(&resp)
    }

    async fn fetch_pitcher_projections(&self) -> Result<Vec<PitcherProjection>, ProjectionError> {
        let resp: Value = self
            .http
            .get(API_BASE)
            .query(&[
                ("stats", self.source.as_str()),
                ("group", "pitching"),
                ("sportIds", "1"),
                ("season", "2026"),
                ("sortStat", "war"),
                ("order", "desc"),
                ("limit", "500"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!("pitcher projections fetched from MLB Stats API");
        parse_pitcher_response(&resp)
    }
}

// ─── MLB Stats API response parsing ───────────────────────────────────────

/// Extract the `splits` array from the MLB Stats API response envelope.
///
/// The response shape is: `{ "stats": [ { "splits": [ ... ] } ] }`.
fn extract_splits(resp: &Value) -> Result<&Vec<Value>, ProjectionError> {
    resp.get("stats")
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.first())
        .and_then(|group| group.get("splits"))
        .and_then(|s| s.as_array())
        .ok_or_else(|| {
            ProjectionError::Parse(format!(
                "unexpected MLB API response structure: {}",
                resp.to_string().chars().take(300).collect::<String>()
            ))
        })
}

/// Parse a string-encoded stat value to f64. Returns 0.0 on failure.
fn parse_stat_str(val: &Value) -> f64 {
    if let Some(n) = val.as_f64() {
        return n;
    }
    if let Some(n) = val.as_i64() {
        return n as f64;
    }
    if let Some(s) = val.as_str() {
        return s.parse::<f64>().unwrap_or(0.0);
    }
    0.0
}

/// Parse batter projections from the MLB Stats API response.
fn parse_batter_response(resp: &Value) -> Result<Vec<BatterProjection>, ProjectionError> {
    let splits = extract_splits(resp)?;

    let projections = splits
        .iter()
        .filter_map(|split| {
            let player_name = split
                .get("player")
                .and_then(|p| p.get("fullName"))
                .and_then(|n| n.as_str())?
                .to_string();

            let team = split
                .get("team")
                .and_then(|t| {
                    t.get("abbreviation")
                        .or_else(|| t.get("name"))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("")
                .to_string();

            let stat = split.get("stat")?;

            Some(BatterProjection {
                player_name,
                team,
                pa: parse_stat_str(stat.get("plateAppearances").unwrap_or(&Value::Null)),
                hr: parse_stat_str(stat.get("homeRuns").unwrap_or(&Value::Null)),
                rbi: parse_stat_str(stat.get("rbi").unwrap_or(&Value::Null)),
                sb: parse_stat_str(stat.get("stolenBases").unwrap_or(&Value::Null)),
                avg: parse_stat_str(stat.get("avg").unwrap_or(&Value::Null)),
                obp: parse_stat_str(stat.get("obp").unwrap_or(&Value::Null)),
                slg: parse_stat_str(stat.get("slg").unwrap_or(&Value::Null)),
                ops: parse_stat_str(stat.get("ops").unwrap_or(&Value::Null)),
                war: parse_stat_str(stat.get("war").unwrap_or(&Value::Null)),
            })
        })
        .collect();

    Ok(projections)
}

/// Parse pitcher projections from the MLB Stats API response.
fn parse_pitcher_response(resp: &Value) -> Result<Vec<PitcherProjection>, ProjectionError> {
    let splits = extract_splits(resp)?;

    let projections = splits
        .iter()
        .filter_map(|split| {
            let player_name = split
                .get("player")
                .and_then(|p| p.get("fullName"))
                .and_then(|n| n.as_str())?
                .to_string();

            let team = split
                .get("team")
                .and_then(|t| {
                    t.get("abbreviation")
                        .or_else(|| t.get("name"))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("")
                .to_string();

            let stat = split.get("stat")?;

            Some(PitcherProjection {
                player_name,
                team,
                ip: parse_stat_str(stat.get("inningsPitched").unwrap_or(&Value::Null)),
                w: parse_stat_str(stat.get("wins").unwrap_or(&Value::Null)),
                era: parse_stat_str(stat.get("era").unwrap_or(&Value::Null)),
                whip: parse_stat_str(stat.get("whip").unwrap_or(&Value::Null)),
                k: parse_stat_str(stat.get("strikeOuts").unwrap_or(&Value::Null)),
                sv: parse_stat_str(stat.get("saves").unwrap_or(&Value::Null)),
                war: parse_stat_str(stat.get("war").unwrap_or(&Value::Null)),
            })
        })
        .collect();

    Ok(projections)
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured real MLB Stats API response for hitting projections (3 players).
    const BATTER_FIXTURE: &str = r#"{
        "copyright": "Copyright 2026 MLB Advanced Media, L.P.",
        "stats": [{
            "type": { "displayName": "projected_ZipsRos" },
            "group": { "displayName": "hitting" },
            "totalSplits": 544,
            "exemptions": [],
            "splits": [
                {
                    "season": "2026",
                    "stat": {
                        "gamesPlayed": 87,
                        "runs": 65,
                        "doubles": 16,
                        "triples": 0,
                        "homeRuns": 26,
                        "strikeOuts": 98,
                        "baseOnBalls": 67,
                        "intentionalWalks": 12,
                        "hits": 86,
                        "hitByPitch": 4,
                        "avg": ".278",
                        "atBats": 309,
                        "obp": ".411",
                        "slg": ".583",
                        "ops": ".994",
                        "caughtStealing": 2,
                        "stolenBases": 5,
                        "stolenBasePercentage": ".714",
                        "plateAppearances": 381,
                        "totalBases": 180,
                        "rbi": 69,
                        "sacBunts": 0,
                        "sacFlies": 2,
                        "babip": ".321",
                        "war": 4.49635372009521
                    },
                    "player": {
                        "id": 592450,
                        "fullName": "Aaron Judge",
                        "link": "/api/v1/people/592450",
                        "firstName": "Aaron",
                        "lastName": "Judge"
                    },
                    "sport": { "id": 1, "link": "/api/v1/sports/1" },
                    "rank": 1,
                    "position": { "code": "9", "name": "Outfielder", "abbreviation": "RF" }
                },
                {
                    "season": "2026",
                    "stat": {
                        "gamesPlayed": 91,
                        "runs": 56,
                        "doubles": 23,
                        "triples": 4,
                        "homeRuns": 15,
                        "strikeOuts": 70,
                        "baseOnBalls": 32,
                        "hits": 103,
                        "avg": ".288",
                        "atBats": 358,
                        "obp": ".349",
                        "slg": ".500",
                        "ops": ".849",
                        "stolenBases": 22,
                        "plateAppearances": 396,
                        "totalBases": 179,
                        "rbi": 52,
                        "war": 4.26370278208768
                    },
                    "player": {
                        "id": 677951,
                        "fullName": "Bobby Witt Jr.",
                        "link": "/api/v1/people/677951",
                        "firstName": "Bobby",
                        "lastName": "Witt"
                    },
                    "sport": { "id": 1, "link": "/api/v1/sports/1" },
                    "rank": 2,
                    "position": { "code": "6", "name": "Shortstop", "abbreviation": "SS" }
                },
                {
                    "season": "2026",
                    "stat": {
                        "gamesPlayed": 91,
                        "runs": 76,
                        "doubles": 18,
                        "triples": 3,
                        "homeRuns": 26,
                        "strikeOuts": 98,
                        "baseOnBalls": 57,
                        "hits": 100,
                        "avg": ".288",
                        "atBats": 347,
                        "obp": ".392",
                        "slg": ".582",
                        "ops": ".974",
                        "stolenBases": 15,
                        "plateAppearances": 411,
                        "totalBases": 202,
                        "rbi": 72,
                        "war": 3.75397483830303
                    },
                    "player": {
                        "id": 660271,
                        "fullName": "Shohei Ohtani",
                        "link": "/api/v1/people/660271",
                        "firstName": "Shohei",
                        "lastName": "Ohtani"
                    },
                    "sport": { "id": 1, "link": "/api/v1/sports/1" },
                    "rank": 3,
                    "position": { "code": "Y", "name": "Two-Way Player", "abbreviation": "TWP" }
                }
            ],
            "splitsTiedWithOffset": [],
            "splitsTiedWithLimit": [],
            "playerPool": "ALL"
        }]
    }"#;

    /// Captured real MLB Stats API response for pitching projections (3 players).
    const PITCHER_FIXTURE: &str = r#"{
        "copyright": "Copyright 2026 MLB Advanced Media, L.P.",
        "stats": [{
            "type": { "displayName": "projected_ZipsRos" },
            "group": { "displayName": "pitching" },
            "totalSplits": 641,
            "exemptions": [],
            "splits": [
                {
                    "season": "2026",
                    "stat": {
                        "gamesPlayed": 16,
                        "gamesStarted": 16,
                        "runs": 30,
                        "homeRuns": 8,
                        "strikeOuts": 119,
                        "baseOnBalls": 17,
                        "hits": 77,
                        "era": "2.56",
                        "inningsPitched": "98.1",
                        "wins": 7,
                        "losses": 2,
                        "saves": 0,
                        "earnedRuns": 28,
                        "whip": "0.96",
                        "battersFaced": 387,
                        "war": 3.41971693984317
                    },
                    "player": {
                        "id": 669373,
                        "fullName": "Tarik Skubal",
                        "link": "/api/v1/people/669373",
                        "firstName": "Tarik",
                        "lastName": "Skubal"
                    },
                    "sport": { "id": 1, "link": "/api/v1/sports/1" },
                    "rank": 1,
                    "position": { "code": "1", "name": "Pitcher", "abbreviation": "P" }
                },
                {
                    "season": "2026",
                    "stat": {
                        "gamesPlayed": 18,
                        "gamesStarted": 18,
                        "runs": 40,
                        "homeRuns": 8,
                        "strikeOuts": 116,
                        "baseOnBalls": 25,
                        "hits": 99,
                        "era": "2.96",
                        "inningsPitched": "112.1",
                        "wins": 7,
                        "losses": 3,
                        "saves": 0,
                        "earnedRuns": 37,
                        "whip": "1.10",
                        "battersFaced": 458,
                        "war": 3.22891314211437
                    },
                    "player": {
                        "id": 650911,
                        "fullName": "Cristopher Sánchez",
                        "link": "/api/v1/people/650911",
                        "firstName": "Cristopher",
                        "lastName": "Sánchez"
                    },
                    "sport": { "id": 1, "link": "/api/v1/sports/1" },
                    "rank": 2,
                    "position": { "code": "1", "name": "Pitcher", "abbreviation": "P" }
                },
                {
                    "season": "2026",
                    "stat": {
                        "gamesPlayed": 19,
                        "gamesStarted": 19,
                        "runs": 35,
                        "homeRuns": 9,
                        "strikeOuts": 121,
                        "baseOnBalls": 26,
                        "hits": 84,
                        "era": "2.83",
                        "inningsPitched": "105.0",
                        "wins": 8,
                        "losses": 4,
                        "saves": 0,
                        "earnedRuns": 33,
                        "whip": "1.05",
                        "battersFaced": 422,
                        "war": 3.00923241215303
                    },
                    "player": {
                        "id": 694973,
                        "fullName": "Paul Skenes",
                        "link": "/api/v1/people/694973",
                        "firstName": "Paul",
                        "lastName": "Skenes"
                    },
                    "sport": { "id": 1, "link": "/api/v1/sports/1" },
                    "rank": 3,
                    "position": { "code": "1", "name": "Pitcher", "abbreviation": "P" }
                }
            ],
            "splitsTiedWithOffset": [],
            "splitsTiedWithLimit": [],
            "playerPool": "ALL"
        }]
    }"#;

    #[test]
    fn parse_batter_projections_from_mlb_api() {
        let resp: Value = serde_json::from_str(BATTER_FIXTURE).unwrap();
        let batters = parse_batter_response(&resp).unwrap();

        assert_eq!(batters.len(), 3);

        // Aaron Judge
        let judge = &batters[0];
        assert_eq!(judge.player_name, "Aaron Judge");
        assert_eq!(judge.team, ""); // MLB API doesn't include team in projections
        assert!((judge.pa - 381.0).abs() < f64::EPSILON);
        assert!((judge.hr - 26.0).abs() < f64::EPSILON);
        assert!((judge.rbi - 69.0).abs() < f64::EPSILON);
        assert!((judge.sb - 5.0).abs() < f64::EPSILON);
        assert!((judge.avg - 0.278).abs() < 0.001);
        assert!((judge.obp - 0.411).abs() < 0.001);
        assert!((judge.slg - 0.583).abs() < 0.001);
        assert!((judge.ops - 0.994).abs() < 0.001);
        assert!((judge.war - 4.496).abs() < 0.01);

        // Bobby Witt Jr.
        let witt = &batters[1];
        assert_eq!(witt.player_name, "Bobby Witt Jr.");
        assert!((witt.pa - 396.0).abs() < f64::EPSILON);
        assert!((witt.sb - 22.0).abs() < f64::EPSILON);
        assert!((witt.avg - 0.288).abs() < 0.001);

        // Shohei Ohtani
        let ohtani = &batters[2];
        assert_eq!(ohtani.player_name, "Shohei Ohtani");
        assert!((ohtani.hr - 26.0).abs() < f64::EPSILON);
        assert!((ohtani.ops - 0.974).abs() < 0.001);
    }

    #[test]
    fn parse_pitcher_projections_from_mlb_api() {
        let resp: Value = serde_json::from_str(PITCHER_FIXTURE).unwrap();
        let pitchers = parse_pitcher_response(&resp).unwrap();

        assert_eq!(pitchers.len(), 3);

        // Tarik Skubal
        let skubal = &pitchers[0];
        assert_eq!(skubal.player_name, "Tarik Skubal");
        assert_eq!(skubal.team, "");
        assert!((skubal.ip - 98.1).abs() < 0.1);
        assert!((skubal.w - 7.0).abs() < f64::EPSILON);
        assert!((skubal.era - 2.56).abs() < 0.01);
        assert!((skubal.whip - 0.96).abs() < 0.01);
        assert!((skubal.k - 119.0).abs() < f64::EPSILON);
        assert!((skubal.sv - 0.0).abs() < f64::EPSILON);
        assert!((skubal.war - 3.42).abs() < 0.01);

        // Cristopher Sanchez
        let sanchez = &pitchers[1];
        assert_eq!(sanchez.player_name, "Cristopher S\u{00e1}nchez");
        assert!((sanchez.ip - 112.1).abs() < 0.1);
        assert!((sanchez.era - 2.96).abs() < 0.01);
        assert!((sanchez.whip - 1.10).abs() < 0.01);

        // Paul Skenes
        let skenes = &pitchers[2];
        assert_eq!(skenes.player_name, "Paul Skenes");
        assert!((skenes.w - 8.0).abs() < f64::EPSILON);
        assert!((skenes.k - 121.0).abs() < f64::EPSILON);
        assert!((skenes.war - 3.009).abs() < 0.01);
    }

    #[test]
    fn parse_empty_splits_returns_empty_vec() {
        let resp: Value =
            serde_json::from_str(r#"{"stats": [{"splits": [], "type": {}, "group": {}}]}"#)
                .unwrap();

        let batters = parse_batter_response(&resp).unwrap();
        assert!(batters.is_empty());

        let pitchers = parse_pitcher_response(&resp).unwrap();
        assert!(pitchers.is_empty());
    }

    #[test]
    fn parse_malformed_response_returns_error() {
        let resp: Value = serde_json::from_str(r#"{"unexpected": true}"#).unwrap();
        assert!(parse_batter_response(&resp).is_err());
        assert!(parse_pitcher_response(&resp).is_err());
    }

    #[test]
    fn parse_stat_str_handles_all_types() {
        // String-encoded float
        assert!((parse_stat_str(&Value::String(".278".into())) - 0.278).abs() < 0.001);
        // Integer
        assert!((parse_stat_str(&serde_json::json!(26)) - 26.0).abs() < f64::EPSILON);
        // Float
        assert!((parse_stat_str(&serde_json::json!(4.5)) - 4.5).abs() < f64::EPSILON);
        // Null
        assert!((parse_stat_str(&Value::Null)).abs() < f64::EPSILON);
        // Unparseable string
        assert!((parse_stat_str(&Value::String("N/A".into()))).abs() < f64::EPSILON);
    }
}
