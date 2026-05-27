use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::instrument;

use crate::types::{LeagueId, PlayerId, TeamId};
use crate::util::{json_f64, json_u32};

/// Base URL for the Fantrax beta API.
const BASE_URL: &str = "https://www.fantrax.com/fxea/general";

// ─── Error type ─────────────────────────────────────────────────────────────

/// Errors that can occur when calling the Fantrax API.
///
/// The `Http` variant deliberately strips the URL from reqwest errors to avoid
/// leaking `userSecretId` query parameters in logs or error messages.
#[derive(Debug, thiserror::Error)]
pub enum FantraxError {
    /// HTTP transport error from reqwest (URL stripped to protect credentials).
    #[error("HTTP error: {0}")]
    Http(String),
    /// The API returned a response we couldn't parse or that indicates failure.
    #[error("Fantrax API error: {0}")]
    Api(String),
}

impl From<reqwest::Error> for FantraxError {
    fn from(e: reqwest::Error) -> Self {
        // Strip the URL from the error to avoid leaking userSecretId.
        let sanitized = if let Some(status) = e.status() {
            format!("HTTP {status}")
        } else if e.is_timeout() {
            "request timed out".to_string()
        } else if e.is_connect() {
            "connection failed".to_string()
        } else if e.is_decode() {
            "response decode error".to_string()
        } else {
            "request failed".to_string()
        };
        Self::Http(sanitized)
    }
}

// ─── Response types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct League {
    pub id: LeagueId,
    pub name: String,
    /// Raw JSON from the API for fields we haven't strongly typed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roster {
    pub league_id: LeagueId,
    pub period: String,
    pub teams: Vec<TeamRoster>,
    /// Raw JSON from the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRoster {
    pub team_id: TeamId,
    pub team_name: String,
    pub players: Vec<RosterPlayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterPlayer {
    pub player_id: PlayerId,
    pub name: String,
    pub position: String,
    pub roster_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Standings {
    pub league_id: LeagueId,
    pub teams: Vec<TeamStanding>,
    /// Raw JSON from the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamStanding {
    pub team_id: TeamId,
    pub team_name: String,
    pub rank: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub points: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeagueInfo {
    pub id: LeagueId,
    pub name: String,
    /// Raw JSON from the API — contains teams, matchup schedules, roster
    /// constraints, scoring config, player eligibility, etc.
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerIds {
    pub sport: String,
    pub players: Vec<PlayerIdEntry>,
    /// Raw JSON from the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// A player ID/name pair from the Fantrax `getPlayerIds` endpoint.
///
/// Not to be confused with `crate::types::PlayerId`, which is the newtype
/// wrapper around a single player ID string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerIdEntry {
    pub id: PlayerId,
    pub name: String,
}

// ─── Client implementation ──────────────────────────────────────────────────

/// HTTP client for the Fantrax beta API.
pub struct FantraxClient {
    http: reqwest::Client,
    user_secret_id: SecretString,
}

impl FantraxClient {
    pub fn new(user_secret_id: SecretString) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("fantrax-mcp/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            user_secret_id,
        }
    }

    /// Fetch all leagues for the configured user.
    ///
    /// Calls `GET /getLeagues?userSecretId={id}`.
    #[instrument(skip(self), fields(endpoint = "getLeagues"))]
    pub async fn get_leagues(&self) -> Result<Vec<League>, FantraxError> {
        let url = format!("{BASE_URL}/getLeagues");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[("userSecretId", self.user_secret_id.expose_secret())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(response = %resp, "getLeagues raw response");

        // The response typically has a "leagues" array. Each entry has
        // "leagueId" and "leagueName" at minimum.
        let leagues_array = resp
            .get("leagues")
            .or_else(|| resp.get("data").and_then(|d| d.get("leagues")))
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                FantraxError::Api(format!(
                    "unexpected getLeagues response shape: {}",
                    serde_json::to_string(&resp).unwrap_or_default()
                ))
            })?;

        let leagues = leagues_array
            .iter()
            .filter_map(|entry| {
                let id: LeagueId = entry
                    .get("leagueId")
                    .or_else(|| entry.get("league_id"))
                    .and_then(|v| v.as_str())
                    .map(LeagueId::new)?;
                let name = entry
                    .get("leagueName")
                    .or_else(|| entry.get("league_name"))
                    .or_else(|| entry.get("name"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| "Unknown".to_string());
                Some(League {
                    id,
                    name,
                    raw: Some(entry.clone()),
                })
            })
            .collect();

        Ok(leagues)
    }

    /// Fetch league metadata/info.
    ///
    /// Calls `GET /getLeagueInfo?leagueId={id}`.
    #[instrument(skip(self), fields(endpoint = "getLeagueInfo"))]
    pub async fn get_league_info(&self, league_id: &LeagueId) -> Result<LeagueInfo, FantraxError> {
        let url = format!("{BASE_URL}/getLeagueInfo");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[("leagueId", league_id.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(league_id = %league_id, response = %resp, "getLeagueInfo raw response");

        // Extract league name from response if available.
        let name = resp
            .get("leagueName")
            .or_else(|| resp.get("name"))
            .or_else(|| {
                resp.get("data")
                    .and_then(|d| d.get("leagueName").or_else(|| d.get("name")))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        Ok(LeagueInfo {
            id: league_id.clone(),
            name,
            raw: resp,
        })
    }

    /// Fetch team rosters for a given league and scoring period.
    ///
    /// Calls `GET /getTeamRosters?leagueId={id}&period={n}`.
    #[instrument(skip(self), fields(endpoint = "getTeamRosters"))]
    pub async fn get_team_rosters(
        &self,
        league_id: &LeagueId,
        period: &str,
    ) -> Result<Roster, FantraxError> {
        let url = format!("{BASE_URL}/getTeamRosters");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[("leagueId", league_id.as_str()), ("period", period)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(league_id = %league_id, period, response = %resp, "getTeamRosters raw response");

        // Parse team rosters from the response. Expected shape varies, so we
        // try multiple paths.
        let teams_value = resp
            .get("rosters")
            .or_else(|| resp.get("teams"))
            .or_else(|| resp.get("data").and_then(|d| d.get("rosters")));

        let teams = if let Some(teams_arr) = teams_value.and_then(|v| v.as_array()) {
            teams_arr
                .iter()
                .filter_map(|team| {
                    let team_id: TeamId = team
                        .get("teamId")
                        .or_else(|| team.get("team_id"))
                        .and_then(|v| v.as_str())
                        .map(TeamId::new)?;
                    let team_name = team
                        .get("teamName")
                        .or_else(|| team.get("team_name"))
                        .or_else(|| team.get("name"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| "Unknown".to_string());

                    let players = team
                        .get("players")
                        .or_else(|| team.get("roster"))
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(extract_roster_player).collect())
                        .unwrap_or_default();

                    Some(TeamRoster {
                        team_id,
                        team_name,
                        players,
                    })
                })
                .collect()
        } else {
            vec![]
        };

        Ok(Roster {
            league_id: league_id.clone(),
            period: period.to_string(),
            teams,
            raw: Some(resp),
        })
    }

    /// Fetch current standings for a league.
    ///
    /// Calls `GET /getStandings?leagueId={id}`.
    #[instrument(skip(self), fields(endpoint = "getStandings"))]
    pub async fn get_standings(&self, league_id: &LeagueId) -> Result<Standings, FantraxError> {
        let url = format!("{BASE_URL}/getStandings");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[("leagueId", league_id.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(league_id = %league_id, response = %resp, "getStandings raw response");

        // Parse standings from the response.
        let standings_value = resp
            .get("standings")
            .or_else(|| resp.get("teams"))
            .or_else(|| resp.get("data").and_then(|d| d.get("standings")));

        let teams = if let Some(arr) = standings_value.and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|entry| {
                    let team_id: TeamId = entry
                        .get("teamId")
                        .or_else(|| entry.get("team_id"))
                        .and_then(|v| v.as_str())
                        .map(TeamId::new)?;
                    let team_name = entry
                        .get("teamName")
                        .or_else(|| entry.get("team_name"))
                        .or_else(|| entry.get("name"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| "Unknown".to_string());
                    let rank = json_u32(entry, &["rank", "rk"]).unwrap_or(0);
                    let wins = json_u32(entry, &["wins", "w"]).unwrap_or(0);
                    let losses = json_u32(entry, &["losses", "l"]).unwrap_or(0);
                    let ties = json_u32(entry, &["ties", "t", "draws"]).unwrap_or(0);
                    let points = json_f64(entry, &["points", "pts", "fantasyPoints"]);

                    Some(TeamStanding {
                        team_id,
                        team_name,
                        rank,
                        wins,
                        losses,
                        ties,
                        points,
                    })
                })
                .collect()
        } else {
            vec![]
        };

        Ok(Standings {
            league_id: league_id.clone(),
            teams,
            raw: Some(resp),
        })
    }

    /// Fetch all player IDs for a given sport.
    ///
    /// Calls `GET /getPlayerIds?sport={sport}`.
    #[instrument(skip(self), fields(endpoint = "getPlayerIds"))]
    pub async fn get_player_ids(&self, sport: &str) -> Result<PlayerIds, FantraxError> {
        let url = format!("{BASE_URL}/getPlayerIds");
        let resp: Value = self
            .http
            .get(&url)
            .query(&[("sport", sport)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tracing::debug!(sport, response_keys = ?resp.as_object().map(|o| o.keys().collect::<Vec<_>>()), "getPlayerIds raw response");

        // Parse player IDs. Expected shape: array of {id, name} or similar.
        let players_value = resp
            .get("playerIds")
            .or_else(|| resp.get("players"))
            .or_else(|| resp.get("data").and_then(|d| d.get("playerIds")));

        let players = if let Some(arr) = players_value.and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|entry| {
                    let id: PlayerId = entry
                        .get("playerId")
                        .or_else(|| entry.get("id"))
                        .and_then(|v| v.as_str())
                        .map(PlayerId::new)?;
                    let name = entry
                        .get("name")
                        .or_else(|| entry.get("playerName"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| "Unknown".to_string());
                    Some(PlayerIdEntry { id, name })
                })
                .collect()
        } else if let Some(obj) = players_value.and_then(|v| v.as_object()) {
            // Sometimes the API returns {playerId: playerName, ...} as an object.
            obj.iter()
                .map(|(id, name)| PlayerIdEntry {
                    id: PlayerId::new(id),
                    name: name.as_str().unwrap_or("Unknown").to_string(),
                })
                .collect()
        } else {
            vec![]
        };

        Ok(PlayerIds {
            sport: sport.to_string(),
            players,
            raw: Some(resp),
        })
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn extract_roster_player(p: &Value) -> Option<RosterPlayer> {
    let player_id: PlayerId = p
        .get("playerId")
        .or_else(|| p.get("player_id"))
        .or_else(|| p.get("id"))
        .and_then(|v| v.as_str())
        .map(PlayerId::new)?;
    let name = p
        .get("name")
        .or_else(|| p.get("playerName"))
        .or_else(|| p.get("player_name"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "Unknown".to_string());
    let position = p
        .get("position")
        .or_else(|| p.get("pos"))
        .or_else(|| p.get("eligiblePos"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "Unknown".to_string());
    let roster_status = p
        .get("rosterStatus")
        .or_else(|| p.get("status"))
        .or_else(|| p.get("roster_status"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "Active".to_string());

    Some(RosterPlayer {
        player_id,
        name,
        position,
        roster_status,
    })
}
