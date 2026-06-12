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
            // Array of team objects (some API versions)
            teams_arr
                .iter()
                .filter_map(|team| extract_team_roster(team, None))
                .collect()
        } else if let Some(teams_obj) = teams_value.and_then(|v| v.as_object()) {
            // Object keyed by team ID (Fantrax beta API format)
            teams_obj
                .iter()
                .filter_map(|(tid, team)| extract_team_roster(team, Some(tid)))
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

        let players = parse_player_id_entries(&resp);
        if players.is_empty() {
            tracing::warn!(
                sport,
                "getPlayerIds parsed to zero players — wire shape may have drifted, \
                 roster names will render as Unknown (see issue #27)"
            );
        }

        Ok(PlayerIds {
            sport: sport.to_string(),
            players,
            raw: Some(resp),
        })
    }

    /// Fetch team rosters enriched with player names from `getPlayerIds`.
    ///
    /// The roster endpoint does not include player names — only IDs. This
    /// method calls both `getTeamRosters` and `getPlayerIds`, then resolves
    /// names via a HashMap join.
    #[instrument(skip(self), fields(endpoint = "getTeamRosters+getPlayerIds"))]
    pub async fn get_team_rosters_enriched(
        &self,
        league_id: &LeagueId,
        period: &str,
    ) -> Result<Roster, FantraxError> {
        let (mut roster, player_ids) = tokio::try_join!(
            self.get_team_rosters(league_id, period),
            self.get_player_ids("MLB"),
        )?;

        enrich_roster_names(&mut roster, &player_ids);
        Ok(roster)
    }
}

/// Resolve "Unknown" player names in a roster using the player ID→name map.
fn enrich_roster_names(roster: &mut Roster, player_ids: &PlayerIds) {
    let name_map: std::collections::HashMap<&str, &str> = player_ids
        .players
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();

    for team in &mut roster.teams {
        for player in &mut team.players {
            if player.name == "Unknown"
                && let Some(&name) = name_map.get(player.player_id.as_str())
            {
                player.name = name.to_string();
            }
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Parse the `getPlayerIds` response into ID/name entries.
///
/// Live wire shape (captured 2026-06-11): the root is an object keyed by
/// Fantrax player ID, with object values like
/// `{"statsIncId": 11731, "rotowireId": 16070, "sportRadarId": "…",
///   "name": "Henderson, Gunnar", "fantraxId": "050w9", "team": "BAL",
///   "position": "SS"}` — there is no `playerIds`/`players` wrapper key.
/// Some entries lack some cross-ID fields; some have team `"(N/A)"`.
/// The wrapper-key paths are kept as fallbacks for forward compatibility.
fn parse_player_id_entries(resp: &Value) -> Vec<PlayerIdEntry> {
    let players_value = resp
        .get("playerIds")
        .or_else(|| resp.get("players"))
        .or_else(|| resp.get("data").and_then(|d| d.get("playerIds")));

    if let Some(arr) = players_value.and_then(|v| v.as_array()) {
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
    } else if let Some(root) = resp.as_object() {
        // Live shape: the root itself is the ID→entry map. Only accept
        // entries whose value is an object carrying the expected fields
        // (`fantraxId` + `name`), so an error body like {"error": "…"}
        // parses to zero entries instead of garbage.
        // Note: entry values also carry cross-IDs (statsIncId, rotowireId,
        // sportRadarId) on the wire — not parsed here, see issue #11.
        root.iter()
            .filter_map(|(id, entry)| {
                let entry = entry.as_object()?;
                if !entry.contains_key("fantraxId") {
                    return None;
                }
                let name = entry.get("name")?.as_str()?;
                Some(PlayerIdEntry {
                    id: PlayerId::new(id),
                    name: name.to_string(),
                })
            })
            .collect()
    } else {
        vec![]
    }
}

fn extract_team_roster(team: &Value, key_as_id: Option<&str>) -> Option<TeamRoster> {
    let team_id: TeamId = if let Some(tid) = key_as_id {
        TeamId::new(tid)
    } else {
        team.get("teamId")
            .or_else(|| team.get("team_id"))
            .and_then(|v| v.as_str())
            .map(TeamId::new)?
    };
    let team_name = team
        .get("teamName")
        .or_else(|| team.get("team_name"))
        .or_else(|| team.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "Unknown".to_string());

    let players = team
        .get("rosterItems")
        .or_else(|| team.get("players"))
        .or_else(|| team.get("roster"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(extract_roster_player).collect())
        .unwrap_or_default();

    Some(TeamRoster {
        team_id,
        team_name,
        players,
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LeagueId, PlayerId, TeamId};

    fn make_roster_player(id: &str, name: &str, pos: &str) -> RosterPlayer {
        RosterPlayer {
            player_id: PlayerId::new(id),
            name: name.to_string(),
            position: pos.to_string(),
            roster_status: "ACTIVE".to_string(),
        }
    }

    fn make_roster(players: Vec<RosterPlayer>) -> Roster {
        Roster {
            league_id: LeagueId::new("test-league"),
            period: "current".to_string(),
            teams: vec![TeamRoster {
                team_id: TeamId::new("01k3nunjmlznzs4q"),
                team_name: "Giladelphia Gillies".to_string(),
                players,
            }],
            raw: None,
        }
    }

    fn make_player_ids(entries: &[(&str, &str)]) -> PlayerIds {
        PlayerIds {
            sport: "MLB".to_string(),
            players: entries
                .iter()
                .map(|(id, name)| PlayerIdEntry {
                    id: PlayerId::new(*id),
                    name: name.to_string(),
                })
                .collect(),
            raw: None,
        }
    }

    #[test]
    fn enrich_resolves_unknown_names() {
        // Fixture: roster from live API — names absent, parsed as "Unknown"
        let mut roster = make_roster(vec![
            make_roster_player("03pit", "Unknown", "UT"),
            make_roster_player("04ru7", "Unknown", "3B"),
            make_roster_player("05y7s", "Unknown", "SP"),
        ]);
        let player_ids = make_player_ids(&[
            ("03pit", "Shohei Ohtani"),
            ("04ru7", "Manny Machado"),
            ("05y7s", "Zack Wheeler"),
        ]);

        enrich_roster_names(&mut roster, &player_ids);

        let names: Vec<&str> = roster.teams[0]
            .players
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Shohei Ohtani", "Manny Machado", "Zack Wheeler"]
        );
    }

    #[test]
    fn enrich_preserves_existing_names() {
        // If a player already has a name (not "Unknown"), enrichment should not overwrite it.
        let mut roster = make_roster(vec![
            make_roster_player("03pit", "Already Named", "UT"),
            make_roster_player("04ru7", "Unknown", "3B"),
        ]);
        let player_ids = make_player_ids(&[("03pit", "Shohei Ohtani"), ("04ru7", "Manny Machado")]);

        enrich_roster_names(&mut roster, &player_ids);

        assert_eq!(roster.teams[0].players[0].name, "Already Named");
        assert_eq!(roster.teams[0].players[1].name, "Manny Machado");
    }

    #[test]
    fn enrich_leaves_unknown_when_id_not_in_map() {
        // Player ID not in the player_ids map — name stays "Unknown".
        let mut roster = make_roster(vec![make_roster_player("zzzzz", "Unknown", "OF")]);
        let player_ids = make_player_ids(&[("03pit", "Shohei Ohtani")]);

        enrich_roster_names(&mut roster, &player_ids);

        assert_eq!(roster.teams[0].players[0].name, "Unknown");
    }

    #[test]
    fn enrich_handles_empty_roster() {
        let mut roster = make_roster(vec![]);
        let player_ids = make_player_ids(&[("03pit", "Shohei Ohtani")]);

        enrich_roster_names(&mut roster, &player_ids);

        assert!(roster.teams[0].players.is_empty());
    }

    #[test]
    fn enrich_handles_empty_player_ids() {
        let mut roster = make_roster(vec![make_roster_player("03pit", "Unknown", "UT")]);
        let player_ids = make_player_ids(&[]);

        enrich_roster_names(&mut roster, &player_ids);

        assert_eq!(roster.teams[0].players[0].name, "Unknown");
    }

    #[test]
    fn extract_roster_player_without_name_field() {
        // Simulates the live API response: player entry with ID but no name key.
        let json: Value = serde_json::json!({
            "playerId": "03pit",
            "position": "UT",
            "rosterStatus": "ACTIVE"
        });
        let player = extract_roster_player(&json).unwrap();
        assert_eq!(player.player_id.as_str(), "03pit");
        assert_eq!(player.name, "Unknown");
    }

    /// Captured real `getPlayerIds?sport=MLB` response (2026-06-11), trimmed
    /// to 12 entries verbatim. The root is the ID→entry map — no wrapper key.
    /// Includes an entry with no cross-IDs (05294) and team "(N/A)" entries.
    const PLAYER_IDS_FIXTURE: &str = r#"{
        "050w9": {
            "statsIncId": 11731,
            "rotowireId": 16070,
            "sportRadarId": "770894ef-45bc-4a10-a18e-d35b4b1fa867",
            "name": "Henderson, Gunnar",
            "fantraxId": "050w9",
            "team": "BAL",
            "position": "SS"
        },
        "050w8": {
            "statsIncId": 11792,
            "rotowireId": 16074,
            "name": "Cavaco, Keoni",
            "fantraxId": "050w8",
            "team": "(N/A)",
            "position": "SS"
        },
        "05294": {
            "name": "Sanchez, Junior",
            "fantraxId": "05294",
            "team": "(N/A)",
            "position": "3B"
        },
        "0527f": {
            "rotowireId": 16328,
            "sportRadarId": "163b3757-65f4-4475-86d5-2beb07c2fc57",
            "name": "Made, Kevin",
            "fantraxId": "0527f",
            "team": "WSH",
            "position": "2B"
        },
        "03nuc": {
            "statsIncId": 10514,
            "rotowireId": 14495,
            "sportRadarId": "71791d18-9c59-4e2f-863d-007d3cdd7efd",
            "name": "Castillo, Luis",
            "fantraxId": "03nuc",
            "team": "SEA",
            "position": "SP"
        },
        "0527e": {
            "rotowireId": 16332,
            "sportRadarId": "6b303eb0-7b59-49fa-807a-633e73c54ed6",
            "name": "Mena, Ismael",
            "fantraxId": "0527e",
            "team": "(N/A)",
            "position": "CF"
        },
        "05tzo": {
            "rotowireId": 17964,
            "sportRadarId": "1aa50e2a-e8f4-46ce-a5e9-556db30a8848",
            "name": "Ritchie, J.R.",
            "fantraxId": "05tzo",
            "team": "ATL",
            "position": "SP"
        },
        "05tzp": {
            "rotowireId": 18869,
            "sportRadarId": "202b4e9a-ee8b-4146-9935-8b4bbb0ed0ea",
            "name": "Gonzalez, Jacob",
            "fantraxId": "05tzp",
            "team": "CHW",
            "position": "2B"
        },
        "05tzm": {
            "rotowireId": 17429,
            "name": "Hickey, Nathan",
            "fantraxId": "05tzm",
            "team": "BOS",
            "position": "1B"
        },
        "05tzk": {
            "name": "Shawver, Evan",
            "fantraxId": "05tzk",
            "team": "COL",
            "position": "RP"
        },
        "05ua0": {
            "rotowireId": 17450,
            "name": "Debiec, Max",
            "fantraxId": "05ua0",
            "team": "(N/A)",
            "position": "SP"
        },
        "0529m": {
            "rotowireId": 16450,
            "sportRadarId": "05687b82-fe13-46b3-8bf8-0df7a8aaa2e4",
            "name": "Cerda, Allan",
            "fantraxId": "0529m",
            "team": "(N/A)",
            "position": "CF"
        }
    }"#;

    #[test]
    fn parse_player_ids_live_wire_shape() {
        // The live response root IS the ID→entry map (no playerIds/players key).
        let resp: Value = serde_json::from_str(PLAYER_IDS_FIXTURE).unwrap();
        let players = parse_player_id_entries(&resp);

        assert_eq!(players.len(), 12);

        let gunnar = players
            .iter()
            .find(|p| p.id.as_str() == "050w9")
            .expect("Gunnar Henderson (050w9) should be parsed");
        assert_eq!(gunnar.name, "Henderson, Gunnar");

        // Entry with no cross-IDs (statsIncId/rotowireId/sportRadarId) still parses.
        let junior = players
            .iter()
            .find(|p| p.id.as_str() == "05294")
            .expect("entry without cross-IDs should be parsed");
        assert_eq!(junior.name, "Sanchez, Junior");
    }

    #[test]
    fn parse_player_ids_error_body_yields_no_entries() {
        // A root object whose values are not player entries (e.g. an error
        // body) must not produce garbage PlayerIdEntry values.
        let resp: Value = serde_json::json!({ "error": "something went wrong" });
        let players = parse_player_id_entries(&resp);
        assert!(players.is_empty());
    }

    #[test]
    fn parse_player_ids_legacy_wrapper_keys_still_work() {
        // Forward-compat fallbacks: array under "playerIds" and {id: name} map
        // under "players".
        let resp: Value = serde_json::json!({
            "playerIds": [{ "playerId": "03pit", "name": "Shohei Ohtani" }]
        });
        let players = parse_player_id_entries(&resp);
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].id.as_str(), "03pit");
        assert_eq!(players[0].name, "Shohei Ohtani");

        let resp: Value = serde_json::json!({
            "players": { "04ru7": "Manny Machado" }
        });
        let players = parse_player_id_entries(&resp);
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].id.as_str(), "04ru7");
        assert_eq!(players[0].name, "Manny Machado");
    }

    // ── Live API smoke test (ignored by default — hits the real Fantrax API) ──

    #[tokio::test]
    #[ignore]
    async fn live_player_ids() {
        // getPlayerIds requires no auth; the secret is unused on this endpoint.
        let client = FantraxClient::new(secrecy::SecretString::from("unused"));

        let result = client
            .get_player_ids("MLB")
            .await
            .expect("live getPlayerIds should return HTTP 200 and parse");

        assert!(
            result.players.len() > 1000,
            "live getPlayerIds should return >1000 MLB players, got {}",
            result.players.len()
        );
    }
}
