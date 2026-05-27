use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::instrument;

use crate::db::Database;

/// Base URL for the MLB Stats API (no authentication required).
const BASE_URL: &str = "https://statsapi.mlb.com/api/v1";

// ─── Cache TTLs (seconds) ──────────────────────────────────────────────────

const SCHEDULE_TTL: i64 = 6 * 3600; // 6 hours
const PLAYER_TTL: i64 = 7 * 24 * 3600; // 7 days
const BOXSCORE_FINAL_TTL: i64 = 365 * 24 * 3600; // ~permanent (1 year)
const BOXSCORE_LIVE_TTL: i64 = 300; // 5 min for in-progress games

// ─── Error type ────────────────────────────────────────────────────────────

/// Errors that can occur when calling the MLB Stats API.
#[derive(Debug)]
pub enum MlbError {
    /// HTTP transport error from reqwest.
    Http(reqwest::Error),
    /// The API returned a response we couldn't parse or that indicates failure.
    Api(String),
}

impl fmt::Display for MlbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::Api(msg) => write!(f, "MLB Stats API error: {msg}"),
        }
    }
}

impl std::error::Error for MlbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Api(_) => None,
        }
    }
}

impl From<reqwest::Error> for MlbError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

// ─── Response types ────────────────────────────────────────────────────────

/// A single MLB game from the schedule endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    /// Unique game identifier used by the MLB Stats API.
    pub game_pk: u64,
    /// Human-readable game date (YYYY-MM-DD).
    pub game_date: String,
    /// Abstract game state: "Preview", "Live", "Final".
    pub status: String,
    /// Detailed status description (e.g. "Scheduled", "In Progress", "Final").
    pub detailed_state: String,
    pub away_team: TeamInfo,
    pub home_team: TeamInfo,
    /// Probable starting pitcher for the away team, if announced.
    pub away_probable_pitcher: Option<PitcherInfo>,
    /// Probable starting pitcher for the home team, if announced.
    pub home_probable_pitcher: Option<PitcherInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamInfo {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PitcherInfo {
    pub id: u64,
    pub full_name: String,
    pub pitch_hand: Option<String>,
}

/// Player information from the /people endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub id: u64,
    pub full_name: String,
    /// Primary position abbreviation (e.g. "SS", "SP", "RF").
    pub primary_position: String,
    /// Batting side: "R", "L", or "S" (switch).
    pub bat_side: String,
    /// Pitching hand: "R" or "L".
    pub pitch_hand: String,
    /// Current team name, if active.
    pub current_team: Option<String>,
    /// Current team ID, if active.
    pub current_team_id: Option<u64>,
}

/// Box score for a completed (or in-progress) game.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Boxscore {
    pub game_pk: u64,
    pub away_team_name: String,
    pub home_team_name: String,
    pub away_batters: Vec<BatterLine>,
    pub home_batters: Vec<BatterLine>,
    pub away_pitchers: Vec<PitcherLine>,
    pub home_pitchers: Vec<PitcherLine>,
}

/// A single batter's line from the box score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatterLine {
    pub player_id: u64,
    pub name: String,
    pub position: String,
    pub at_bats: u32,
    pub runs: u32,
    pub hits: u32,
    pub doubles: u32,
    pub triples: u32,
    pub home_runs: u32,
    pub rbi: u32,
    pub walks: u32,
    pub strikeouts: u32,
    pub stolen_bases: u32,
    pub avg: String,
}

/// A single pitcher's line from the box score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PitcherLine {
    pub player_id: u64,
    pub name: String,
    pub innings_pitched: String,
    pub hits: u32,
    pub runs: u32,
    pub earned_runs: u32,
    pub walks: u32,
    pub strikeouts: u32,
    pub home_runs: u32,
    pub pitches_thrown: u32,
    pub strikes: u32,
    pub era: String,
    pub decision: Option<String>,
}

// ─── Client implementation ─────────────────────────────────────────────────

/// HTTP client for the MLB Stats API with SQLite-backed caching.
pub struct MlbClient {
    http: reqwest::Client,
    db: Arc<Database>,
}

impl MlbClient {
    pub fn new(db: Arc<Database>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("fantrax-mcp/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self { http, db }
    }

    /// Fetch today's schedule with probable pitchers.
    ///
    /// `GET /schedule?sportId=1&date={YYYY-MM-DD}&hydrate=probablePitcher`
    #[instrument(skip(self), fields(endpoint = "schedule"))]
    pub async fn get_schedule(&self, date: NaiveDate) -> Result<Vec<Game>, MlbError> {
        let date_str = date.format("%Y-%m-%d").to_string();
        let cache_key = format!("mlb:schedule:{date_str}");

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!(%date_str, "schedule cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| MlbError::Api(format!("cache deserialize error: {e}")));
        }

        let url = format!("{BASE_URL}/schedule");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[
                ("sportId", "1"),
                ("date", &date_str),
                ("hydrate", "probablePitcher"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(%date_str, "schedule fetched from API");

        let games = parse_schedule(&resp)?;

        let value = serde_json::to_value(&games)
            .map_err(|e| MlbError::Api(format!("serialize error: {e}")))?;
        self.db.set_cached(&cache_key, &value, SCHEDULE_TTL);

        Ok(games)
    }

    /// Fetch player info (handedness, position, team).
    ///
    /// `GET /people/{playerId}?hydrate=stats`
    #[instrument(skip(self), fields(endpoint = "people"))]
    pub async fn get_player(&self, player_id: u64) -> Result<Player, MlbError> {
        let cache_key = format!("mlb:player:{player_id}");

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!(player_id, "player cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| MlbError::Api(format!("cache deserialize error: {e}")));
        }

        let url = format!("{BASE_URL}/people/{player_id}");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[("hydrate", "stats")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(player_id, "player fetched from API");

        let player = parse_player(&resp)?;

        let value = serde_json::to_value(&player)
            .map_err(|e| MlbError::Api(format!("serialize error: {e}")))?;
        self.db.set_cached(&cache_key, &value, PLAYER_TTL);

        Ok(player)
    }

    /// Fetch the box score for a game.
    ///
    /// `GET /game/{gamePk}/boxscore`
    ///
    /// The caller provides `is_final` (derived from the schedule's
    /// `abstractGameState == "Final"`) so the cache TTL is set correctly
    /// without guessing from the boxscore response body.
    #[instrument(skip(self), fields(endpoint = "boxscore"))]
    pub async fn get_boxscore(&self, game_pk: u64, is_final: bool) -> Result<Boxscore, MlbError> {
        let cache_key = format!("mlb:boxscore:{game_pk}");

        if let Some(cached) = self.db.get_cached(&cache_key) {
            tracing::debug!(game_pk, "boxscore cache hit");
            return serde_json::from_value(cached)
                .map_err(|e| MlbError::Api(format!("cache deserialize error: {e}")));
        }

        let url = format!("{BASE_URL}/game/{game_pk}/boxscore");
        let resp: Value = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(game_pk, "boxscore fetched from API");

        let boxscore = parse_boxscore(game_pk, &resp)?;

        // Use permanent TTL for final games, short TTL for live/preview.
        let ttl = if is_final {
            BOXSCORE_FINAL_TTL
        } else {
            BOXSCORE_LIVE_TTL
        };

        let value = serde_json::to_value(&boxscore)
            .map_err(|e| MlbError::Api(format!("serialize error: {e}")))?;
        self.db.set_cached(&cache_key, &value, ttl);

        Ok(boxscore)
    }
}

// ─── Parsing helpers ───────────────────────────────────────────────────────

fn parse_schedule(resp: &Value) -> Result<Vec<Game>, MlbError> {
    let dates = resp
        .get("dates")
        .and_then(|v| v.as_array())
        .ok_or_else(|| MlbError::Api("missing 'dates' array in schedule response".into()))?;

    let mut games = Vec::new();
    for date_entry in dates {
        let date_str = date_entry
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let game_array = match date_entry.get("games").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => continue,
        };

        for g in game_array {
            let game_pk = g.get("gamePk").and_then(|v| v.as_u64()).unwrap_or_default();

            let status_obj = g.get("status");
            let abstract_state = status_obj
                .and_then(|s| s.get("abstractGameState"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();
            let detailed_state = status_obj
                .and_then(|s| s.get("detailedState"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();

            let teams = g.get("teams");
            let away = teams.and_then(|t| t.get("away"));
            let home = teams.and_then(|t| t.get("home"));

            let away_team = parse_team_info(away);
            let home_team = parse_team_info(home);

            let away_probable_pitcher = away
                .and_then(|a| a.get("probablePitcher"))
                .and_then(parse_pitcher_info);
            let home_probable_pitcher = home
                .and_then(|h| h.get("probablePitcher"))
                .and_then(parse_pitcher_info);

            games.push(Game {
                game_pk,
                game_date: date_str.to_string(),
                status: abstract_state,
                detailed_state,
                away_team,
                home_team,
                away_probable_pitcher,
                home_probable_pitcher,
            });
        }
    }

    Ok(games)
}

fn parse_team_info(team: Option<&Value>) -> TeamInfo {
    let team_obj = team.and_then(|t| t.get("team"));
    TeamInfo {
        id: team_obj
            .and_then(|t| t.get("id"))
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
        name: team_obj
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string(),
    }
}

fn parse_pitcher_info(pitcher: &Value) -> Option<PitcherInfo> {
    let id = pitcher.get("id").and_then(|v| v.as_u64())?;
    let full_name = pitcher
        .get("fullName")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();
    let pitch_hand = pitcher
        .get("pitchHand")
        .and_then(|h| h.get("code"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(PitcherInfo {
        id,
        full_name,
        pitch_hand,
    })
}

fn parse_player(resp: &Value) -> Result<Player, MlbError> {
    let people = resp
        .get("people")
        .and_then(|v| v.as_array())
        .ok_or_else(|| MlbError::Api("missing 'people' array in player response".into()))?;

    let p = people
        .first()
        .ok_or_else(|| MlbError::Api("empty 'people' array".into()))?;

    let id = p.get("id").and_then(|v| v.as_u64()).unwrap_or_default();

    let full_name = p
        .get("fullName")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let primary_position = p
        .get("primaryPosition")
        .and_then(|pos| pos.get("abbreviation"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let bat_side = p
        .get("batSide")
        .and_then(|b| b.get("code"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let pitch_hand = p
        .get("pitchHand")
        .and_then(|h| h.get("code"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let current_team = p
        .get("currentTeam")
        .and_then(|t| t.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let current_team_id = p
        .get("currentTeam")
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_u64());

    Ok(Player {
        id,
        full_name,
        primary_position,
        bat_side,
        pitch_hand,
        current_team,
        current_team_id,
    })
}

fn parse_boxscore(game_pk: u64, resp: &Value) -> Result<Boxscore, MlbError> {
    let teams = resp
        .get("teams")
        .ok_or_else(|| MlbError::Api("missing 'teams' in boxscore response".into()))?;

    let away = teams
        .get("away")
        .ok_or_else(|| MlbError::Api("missing 'away' in boxscore teams".into()))?;
    let home = teams
        .get("home")
        .ok_or_else(|| MlbError::Api("missing 'home' in boxscore teams".into()))?;

    let away_team_name = away
        .get("team")
        .and_then(|t| t.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Away")
        .to_string();
    let home_team_name = home
        .get("team")
        .and_then(|t| t.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Home")
        .to_string();

    let away_batters = parse_batter_lines(away);
    let home_batters = parse_batter_lines(home);
    let away_pitchers = parse_pitcher_lines(away);
    let home_pitchers = parse_pitcher_lines(home);

    Ok(Boxscore {
        game_pk,
        away_team_name,
        home_team_name,
        away_batters,
        home_batters,
        away_pitchers,
        home_pitchers,
    })
}

fn parse_batter_lines(team: &Value) -> Vec<BatterLine> {
    let players = match team.get("players").and_then(|v| v.as_object()) {
        Some(obj) => obj,
        None => return vec![],
    };

    let batting_order: Vec<String> = team
        .get("battingOrder")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_u64()
                        .map(|n| format!("ID{n}"))
                        .or_else(|| v.as_str().map(|s| s.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut batters = Vec::new();
    for (key, player_val) in players {
        let stats = match player_val.get("stats").and_then(|s| s.get("batting")) {
            Some(b) => b,
            None => continue,
        };

        // Skip players who didn't actually bat (no at-bats and no walks etc.)
        let at_bats = val_u32(stats, "atBats");
        let walks = val_u32(stats, "baseOnBalls");
        if at_bats == 0 && walks == 0 && val_u32(stats, "hits") == 0 {
            continue;
        }

        let person = player_val.get("person");
        let player_id = person
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_u64())
            .unwrap_or_default();
        let name = person
            .and_then(|p| p.get("fullName"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let position = player_val
            .get("position")
            .and_then(|p| p.get("abbreviation"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let is_in_order = batting_order.contains(key);

        batters.push((
            is_in_order,
            BatterLine {
                player_id,
                name,
                position,
                at_bats,
                runs: val_u32(stats, "runs"),
                hits: val_u32(stats, "hits"),
                doubles: val_u32(stats, "doubles"),
                triples: val_u32(stats, "triples"),
                home_runs: val_u32(stats, "homeRuns"),
                rbi: val_u32(stats, "rbi"),
                walks,
                strikeouts: val_u32(stats, "strikeOuts"),
                stolen_bases: val_u32(stats, "stolenBases"),
                avg: val_str(stats, "avg"),
            },
        ));
    }

    // Sort: starters (in batting order) first, then bench/pinch hitters.
    batters.sort_by_key(|b| std::cmp::Reverse(b.0));
    batters.into_iter().map(|(_, line)| line).collect()
}

fn parse_pitcher_lines(team: &Value) -> Vec<PitcherLine> {
    let players = match team.get("players").and_then(|v| v.as_object()) {
        Some(obj) => obj,
        None => return vec![],
    };

    let pitchers_order: Vec<String> = team
        .get("pitchers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_u64()
                        .map(|n| format!("ID{n}"))
                        .or_else(|| v.as_str().map(|s| s.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut pitchers = Vec::new();
    for (key, player_val) in players {
        let stats = match player_val.get("stats").and_then(|s| s.get("pitching")) {
            Some(p) => p,
            None => continue,
        };

        // Only include players who actually pitched.
        let ip = val_str(stats, "inningsPitched");
        if ip.is_empty() || ip == "0" {
            continue;
        }

        let person = player_val.get("person");
        let player_id = person
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_u64())
            .unwrap_or_default();
        let name = person
            .and_then(|p| p.get("fullName"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let decision = stats.get("note").and_then(|v| v.as_str()).map(String::from);

        let order_pos = pitchers_order
            .iter()
            .position(|k| k == key)
            .unwrap_or(usize::MAX);

        pitchers.push((
            order_pos,
            PitcherLine {
                player_id,
                name,
                innings_pitched: ip,
                hits: val_u32(stats, "hits"),
                runs: val_u32(stats, "runs"),
                earned_runs: val_u32(stats, "earnedRuns"),
                walks: val_u32(stats, "baseOnBalls"),
                strikeouts: val_u32(stats, "strikeOuts"),
                home_runs: val_u32(stats, "homeRuns"),
                pitches_thrown: val_u32(stats, "numberOfPitches"),
                strikes: val_u32(stats, "strikes"),
                era: val_str(stats, "era"),
                decision,
            },
        ));
    }

    pitchers.sort_by_key(|(order, _)| *order);
    pitchers.into_iter().map(|(_, line)| line).collect()
}

fn val_u32(obj: &Value, key: &str) -> u32 {
    obj.get(key)
        .and_then(|v| {
            v.as_u64()
                .map(|n| n as u32)
                .or_else(|| v.as_f64().map(|n| n as u32))
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

fn val_str(obj: &Value, key: &str) -> String {
    obj.get(key)
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_f64().map(|n| format!("{n:.3}")))
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default()
}
