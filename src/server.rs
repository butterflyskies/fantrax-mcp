use std::sync::Arc;

use chrono::NaiveDate;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ErrorData, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    analysis, briefing,
    config::Config,
    db::Database,
    fantrax::FantraxClient,
    mlb::MlbClient,
    projections::ProjectionClient,
    types::{LeagueId, PlayerId, TeamId},
};

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Parse a YYYY-MM-DD date string, or default to today (UTC).
fn parse_date_or_today(date: Option<&str>) -> Result<NaiveDate, ErrorData> {
    match date {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|e| {
            ErrorData::invalid_params(format!("invalid date format (use YYYY-MM-DD): {e}"), None)
        }),
        None => Ok(chrono::Utc::now().date_naive()),
    }
}

// ─── Tool argument structs ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NoArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LeagueIdArgs {
    /// The Fantrax league ID.
    pub league_id: LeagueId,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RosterArgs {
    /// The Fantrax league ID.
    pub league_id: LeagueId,
    /// The scoring period (e.g. "1", "2", or "current").
    pub period: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlayerIdsArgs {
    /// The sport code (e.g. "MLB", "NFL", "NBA").
    #[serde(default = "default_sport")]
    pub sport: String,
}

fn default_sport() -> String {
    "MLB".to_string()
}

/// Valid recommendation types for the ledger.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecommendationType {
    Pickup,
    Drop,
    Start,
    Sit,
    Waiver,
}

impl std::fmt::Display for RecommendationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pickup => write!(f, "pickup"),
            Self::Drop => write!(f, "drop"),
            Self::Start => write!(f, "start"),
            Self::Sit => write!(f, "sit"),
            Self::Waiver => write!(f, "waiver"),
        }
    }
}

/// Valid outcome values for a recommendation.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Win,
    Loss,
    Neutral,
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Win => write!(f, "win"),
            Self::Loss => write!(f, "loss"),
            Self::Neutral => write!(f, "neutral"),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LogRecommendationArgs {
    /// The agent making the recommendation (e.g. "ariadne", "vesper").
    pub agent_id: String,
    /// The Fantrax league ID.
    pub league_id: LeagueId,
    /// Type of recommendation: pickup, drop, start, sit, waiver.
    pub recommendation_type: RecommendationType,
    /// Player IDs involved in this recommendation.
    pub players: Vec<PlayerId>,
    /// The agent's rationale for this recommendation.
    pub reasoning: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryLedgerArgs {
    /// The Fantrax league ID.
    pub league_id: LeagueId,
    /// Optional: filter by agent ID.
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordOutcomeArgs {
    /// The recommendation ID to update.
    pub id: i64,
    /// The outcome: win, loss, or neutral.
    pub outcome: Outcome,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProbableStartersArgs {
    /// Date in YYYY-MM-DD format. Defaults to today if omitted.
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlayerSnippetArgs {
    /// The MLB Stats API player ID.
    pub player_id: u64,
    /// Date of the game to pull stats from (YYYY-MM-DD). Defaults to yesterday.
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OptimizeLineupArgs {
    /// The Fantrax league ID.
    pub league_id: LeagueId,
    /// The scoring period (e.g. "1", "2", or "current").
    pub period: String,
    /// Your team ID within the league.
    pub team_id: TeamId,
    /// Date for the schedule (YYYY-MM-DD). Defaults to today.
    pub date: Option<String>,
    /// Number of lineup slots to fill. If omitted, returns all recommendations.
    pub slots: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BriefingArgs {
    /// The Fantrax league ID. If omitted, generates briefings for all configured leagues.
    pub league_id: Option<LeagueId>,
    /// Your team ID within the league. When omitted with multiple leagues, only
    /// league-level information is included (roster-specific layers are skipped).
    pub team_id: Option<TeamId>,
    /// The scoring period (e.g. "1", "2", or "current").
    #[serde(default = "default_period")]
    pub period: String,
    /// Date for the briefing (YYYY-MM-DD). Defaults to today.
    pub date: Option<String>,
}

/// Player type for projection queries.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlayerType {
    #[serde(alias = "bat", alias = "hitter")]
    Batter,
    #[serde(alias = "pit", alias = "arm")]
    Pitcher,
}

impl PlayerType {
    fn is_batter(self) -> bool {
        matches!(self, Self::Batter)
    }
}

impl std::fmt::Display for PlayerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Batter => write!(f, "batter"),
            Self::Pitcher => write!(f, "pitcher"),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetProjectionsArgs {
    /// Player type: "batter" or "pitcher" (also accepts "bat", "hitter", "pit", "arm").
    pub player_type: PlayerType,
    /// Optional player name filter (case-insensitive substring match).
    pub player_name: Option<String>,
}

fn default_period() -> String {
    "current".to_string()
}

// ─── Shared state ───────────────────────────────────────────────────────────

/// Shared application state available to all MCP tool handlers.
pub struct AppState {
    pub config: Config,
    pub client: FantraxClient,
    pub mlb: MlbClient,
    pub projections: ProjectionClient,
    pub db: Arc<Database>,
}

impl AppState {
    /// Check the cache for `key`; on miss, run `fetch` and cache the result.
    ///
    /// Returns the pretty-printed JSON string of the (possibly cached) value.
    async fn cached_fetch<F, Fut, T>(
        &self,
        key: String,
        ttl_seconds: i64,
        fetch: F,
    ) -> Result<String, ErrorData>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, ErrorData>>,
        T: serde::Serialize,
    {
        if let Some(cached) = self.db.get_cached_async(key.clone()).await {
            return serde_json::to_string_pretty(&cached)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None));
        }

        let data = fetch().await?;
        let value = serde_json::to_value(&data)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        self.db.set_cached_async(key, value, ttl_seconds).await;

        serde_json::to_string_pretty(&data)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }
}

// ─── MCP server ─────────────────────────────────────────────────────────────

/// MCP server implementation for Fantrax.
#[derive(Clone)]
pub struct FantraxServer {
    state: Arc<AppState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl FantraxServer {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// List all leagues for the configured Fantrax user.
    #[tool(
        name = "list_leagues",
        description = "List all fantasy baseball leagues for the configured Fantrax user."
    )]
    async fn list_leagues(&self, _params: Parameters<NoArgs>) -> Result<String, ErrorData> {
        self.state
            .cached_fetch("leagues".into(), 900, || async {
                self.state
                    .client
                    .get_leagues()
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))
            })
            .await
    }

    /// Get current standings for a league.
    #[tool(
        name = "get_standings",
        description = "Get current standings for a fantasy baseball league."
    )]
    async fn get_standings(
        &self,
        Parameters(args): Parameters<LeagueIdArgs>,
    ) -> Result<String, ErrorData> {
        self.state
            .cached_fetch(format!("standings:{}", args.league_id), 900, || async {
                self.state
                    .client
                    .get_standings(&args.league_id)
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))
            })
            .await
    }

    /// Get team rosters for a league and scoring period.
    #[tool(
        name = "get_rosters",
        description = "Get team rosters for a fantasy baseball league and scoring period."
    )]
    async fn get_rosters(
        &self,
        Parameters(args): Parameters<RosterArgs>,
    ) -> Result<String, ErrorData> {
        self.state
            .cached_fetch(
                format!("roster:{}:{}", args.league_id, args.period),
                900,
                || async {
                    self.state
                        .client
                        .get_team_rosters_enriched(&args.league_id, &args.period)
                        .await
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))
                },
            )
            .await
    }

    /// Get league info/metadata.
    #[tool(
        name = "get_league_info",
        description = "Get metadata and configuration for a fantasy baseball league including teams, matchup schedules, roster constraints, scoring config, and player eligibility."
    )]
    async fn get_league_info(
        &self,
        Parameters(args): Parameters<LeagueIdArgs>,
    ) -> Result<String, ErrorData> {
        self.state
            .cached_fetch(format!("league_info:{}", args.league_id), 900, || async {
                self.state
                    .client
                    .get_league_info(&args.league_id)
                    .await
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))
            })
            .await
    }

    /// Get all player IDs for a sport.
    #[tool(
        name = "get_player_ids",
        description = "Get all player IDs for a sport (defaults to MLB). Useful for mapping player names to Fantrax IDs."
    )]
    async fn get_player_ids(
        &self,
        Parameters(args): Parameters<PlayerIdsArgs>,
    ) -> Result<String, ErrorData> {
        self.state
            .cached_fetch(
                format!("player_ids:{}", args.sport),
                604_800, // 7-day TTL
                || async {
                    let player_ids = self
                        .state
                        .client
                        .get_player_ids(&args.sport)
                        .await
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

                    // Return summary (count) plus the data — full list could be large.
                    Ok(json!({
                        "sport": player_ids.sport,
                        "total_players": player_ids.players.len(),
                        "players": player_ids.players,
                    }))
                },
            )
            .await
    }

    /// Log a recommendation to the ledger.
    #[tool(
        name = "log_recommendation",
        description = "Log a fantasy baseball recommendation (pickup, drop, start, sit, waiver) to the persistent ledger. Returns the recommendation ID."
    )]
    async fn log_recommendation(
        &self,
        Parameters(args): Parameters<LogRecommendationArgs>,
    ) -> Result<String, ErrorData> {
        let rec_type = args.recommendation_type.to_string();
        let players_json = serde_json::to_string(&args.players)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let id = self
            .state
            .db
            .log_recommendation_async(
                args.agent_id,
                args.league_id,
                rec_type,
                players_json,
                args.reasoning,
            )
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let response = json!({
            "status": "logged",
            "recommendation_id": id,
        });
        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Query the recommendation ledger.
    #[tool(
        name = "query_ledger",
        description = "Query the recommendation ledger for a league. Optionally filter by agent_id to see only one agent's recommendations."
    )]
    async fn query_ledger(
        &self,
        Parameters(args): Parameters<QueryLedgerArgs>,
    ) -> Result<String, ErrorData> {
        let recs = self
            .state
            .db
            .get_recommendations_async(args.league_id, args.agent_id)
            .await;
        serde_json::to_string_pretty(&recs)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Record the outcome of a previous recommendation.
    #[tool(
        name = "record_outcome",
        description = "Record the outcome (win, loss, neutral) of a previously logged recommendation."
    )]
    async fn record_outcome(
        &self,
        Parameters(args): Parameters<RecordOutcomeArgs>,
    ) -> Result<String, ErrorData> {
        let outcome_str = args.outcome.to_string();
        self.state
            .db
            .record_outcome_async(args.id, outcome_str.clone())
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let response = json!({
            "status": "updated",
            "id": args.id,
            "outcome": outcome_str,
        });
        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Ping --- health check tool.
    #[tool(
        name = "ping",
        description = "Health check. Returns server version and configured league count."
    )]
    async fn ping(&self, _params: Parameters<NoArgs>) -> Result<String, ErrorData> {
        let response = json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "leagues": self.state.config.leagues.len(),
        });
        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Get today's probable starting pitchers with handedness info.
    #[tool(
        name = "get_probable_starters",
        description = "Get today's MLB probable starting pitchers with handedness. Useful for setting fantasy lineups based on pitcher matchups."
    )]
    async fn get_probable_starters(
        &self,
        Parameters(args): Parameters<ProbableStartersArgs>,
    ) -> Result<String, ErrorData> {
        let date = parse_date_or_today(args.date.as_deref())?;

        let games = self
            .state
            .mlb
            .get_schedule(date)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // Enrich with handedness by fetching player info for each pitcher.
        let mut starters = Vec::new();
        for game in &games {
            let mut entry = json!({
                "game_pk": game.game_pk,
                "game_date": game.game_date,
                "status": game.status,
                "away_team": game.away_team.name,
                "home_team": game.home_team.name,
            });

            if let Some(ref p) = game.away_probable_pitcher {
                let hand = if let Some(ref h) = p.pitch_hand {
                    h.clone()
                } else {
                    self.state
                        .mlb
                        .get_player(p.id)
                        .await
                        .map(|pl| pl.pitch_hand)
                        .unwrap_or_else(|_| "Unknown".to_string())
                };
                entry["away_starter"] = json!({
                    "id": p.id,
                    "name": p.full_name,
                    "throws": hand,
                });
            }

            if let Some(ref p) = game.home_probable_pitcher {
                let hand = if let Some(ref h) = p.pitch_hand {
                    h.clone()
                } else {
                    self.state
                        .mlb
                        .get_player(p.id)
                        .await
                        .map(|pl| pl.pitch_hand)
                        .unwrap_or_else(|_| "Unknown".to_string())
                };
                entry["home_starter"] = json!({
                    "id": p.id,
                    "name": p.full_name,
                    "throws": hand,
                });
            }

            starters.push(entry);
        }

        let response = json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "games": starters,
            "total_games": starters.len(),
        });

        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Get yesterday's stat line for a player from the box score.
    #[tool(
        name = "get_player_snippet",
        description = "Get a player's stat line from a specific date's game (defaults to yesterday). Returns batting or pitching line from the box score."
    )]
    async fn get_player_snippet(
        &self,
        Parameters(args): Parameters<PlayerSnippetArgs>,
    ) -> Result<String, ErrorData> {
        let date = match &args.date {
            Some(d) => parse_date_or_today(Some(d))?,
            None => chrono::Utc::now().date_naive() - chrono::Duration::days(1),
        };

        // First get the schedule for that date to find which game the player was in.
        let games = self
            .state
            .mlb
            .get_schedule(date)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // Also fetch player info for context.
        let player = self
            .state
            .mlb
            .get_player(args.player_id)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // Search each game's boxscore for this player.
        for game in &games {
            let is_final = game.status == "Final";
            let boxscore = match self.state.mlb.get_boxscore(game.game_pk, is_final).await {
                Ok(bs) => bs,
                Err(_) => continue,
            };

            // Check batting lines.
            for line in boxscore
                .away_batters
                .iter()
                .chain(boxscore.home_batters.iter())
            {
                if line.player_id == args.player_id {
                    let response = json!({
                        "player": {
                            "id": player.id,
                            "name": player.full_name,
                            "position": player.primary_position,
                            "bats": player.bat_side,
                            "throws": player.pitch_hand,
                            "team": player.current_team,
                        },
                        "date": date.format("%Y-%m-%d").to_string(),
                        "game": format!("{} @ {}", boxscore.away_team_name, boxscore.home_team_name),
                        "type": "batting",
                        "line": {
                            "AB": line.at_bats,
                            "R": line.runs,
                            "H": line.hits,
                            "2B": line.doubles,
                            "3B": line.triples,
                            "HR": line.home_runs,
                            "RBI": line.rbi,
                            "BB": line.walks,
                            "SO": line.strikeouts,
                            "SB": line.stolen_bases,
                            "AVG": line.avg,
                        },
                    });
                    return serde_json::to_string_pretty(&response)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None));
                }
            }

            // Check pitching lines.
            for line in boxscore
                .away_pitchers
                .iter()
                .chain(boxscore.home_pitchers.iter())
            {
                if line.player_id == args.player_id {
                    let response = json!({
                        "player": {
                            "id": player.id,
                            "name": player.full_name,
                            "position": player.primary_position,
                            "bats": player.bat_side,
                            "throws": player.pitch_hand,
                            "team": player.current_team,
                        },
                        "date": date.format("%Y-%m-%d").to_string(),
                        "game": format!("{} @ {}", boxscore.away_team_name, boxscore.home_team_name),
                        "type": "pitching",
                        "line": {
                            "IP": line.innings_pitched,
                            "H": line.hits,
                            "R": line.runs,
                            "ER": line.earned_runs,
                            "BB": line.walks,
                            "SO": line.strikeouts,
                            "HR": line.home_runs,
                            "P": line.pitches_thrown,
                            "S": line.strikes,
                            "ERA": line.era,
                            "decision": line.decision,
                        },
                    });
                    return serde_json::to_string_pretty(&response)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None));
                }
            }
        }

        // Player not found in any game that day.
        let response = json!({
            "player": {
                "id": player.id,
                "name": player.full_name,
                "position": player.primary_position,
                "bats": player.bat_side,
                "throws": player.pitch_hand,
                "team": player.current_team,
            },
            "date": date.format("%Y-%m-%d").to_string(),
            "status": "no_game",
            "message": format!("{} did not appear in any game on {}", player.full_name, date),
        });
        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Get lineup optimization with platoon-based start/sit recommendations.
    ///
    /// Fetches roster from Fantrax, today's schedule from MLB Stats API,
    /// resolves batter handedness, and runs platoon analysis.
    #[tool(
        name = "optimize_lineup",
        description = "Analyze your fantasy roster against today's probable starters and return \
                       platoon-based start/sit recommendations. Fetches roster from Fantrax, \
                       today's MLB schedule, and each batter's handedness to identify platoon \
                       advantages and disadvantages."
    )]
    async fn optimize_lineup(
        &self,
        Parameters(args): Parameters<OptimizeLineupArgs>,
    ) -> Result<String, ErrorData> {
        let date = parse_date_or_today(args.date.as_deref())?;

        // 1. Fetch the roster from Fantrax (enriched with player names).
        let roster = self
            .state
            .client
            .get_team_rosters_enriched(&args.league_id, &args.period)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let team = roster
            .teams
            .iter()
            .find(|t| t.team_id == args.team_id)
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!(
                        "team '{}' not found in league '{}' for period '{}'",
                        args.team_id, args.league_id, args.period
                    ),
                    None,
                )
            })?;

        // 2. Fetch today's schedule with probable starters.
        let games = self
            .state
            .mlb
            .get_schedule(date)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        // 3. Resolve MLB player info (handedness) for each roster batter.
        //    We search MLB player IDs by name — not ideal but functional until
        //    we have a Fantrax→MLB ID mapping.
        let mut player_pairs = Vec::new();
        let mut lookup_failures = Vec::new();

        for rp in &team.players {
            // Try to find the MLB player by searching cached player IDs.
            // For now, we look up each player from the MLB API by iterating
            // through games to find them, or we try the Fantrax player_id
            // as a potential MLB ID (numeric).
            if let Ok(mlb_id) = rp.player_id.as_str().parse::<u64>() {
                match self.state.mlb.get_player(mlb_id).await {
                    Ok(player) => {
                        player_pairs.push((rp.clone(), player));
                    }
                    Err(e) => {
                        lookup_failures.push(json!({
                            "player": rp.name,
                            "player_id": rp.player_id.as_str(),
                            "error": format!("MLB lookup failed: {e}"),
                        }));
                    }
                }
            } else {
                lookup_failures.push(json!({
                    "player": rp.name,
                    "player_id": rp.player_id.as_str(),
                    "error": "non-numeric player ID — cannot resolve via MLB API",
                }));
            }
        }

        // 4. Run platoon analysis.
        let recommendations = analysis::platoon_recommendations(&player_pairs, &games);

        // 5. Optionally optimize to N slots.
        let final_recs = match args.slots {
            Some(n) => analysis::optimize_lineup(&recommendations, n),
            None => recommendations,
        };

        // 6. Build response.
        let response = json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "league_id": args.league_id,
            "team_id": args.team_id,
            "total_games_today": games.len(),
            "players_analyzed": final_recs.len(),
            "lookup_failures": lookup_failures,
            "recommendations": final_recs.iter().map(|r| {
                json!({
                    "player": r.player_name,
                    "player_id": r.player_id.as_str(),
                    "recommendation": r.verdict.to_string(),
                    "matchup_advantage": r.matchup_advantage.map(|a| a.to_string()),
                    "opposing_pitcher": r.opposing_pitcher,
                    "reason": r.reason,
                })
            }).collect::<Vec<_>>(),
        });

        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }
    /// Generate a morning briefing for one or all leagues.
    ///
    /// Assembles three layers: what happened yesterday, what to do today,
    /// and hot takes from the recommendation ledger.
    #[tool(
        name = "get_briefing",
        description = "Generate a morning fantasy baseball briefing. Covers yesterday's player \
                       performances, today's action items (IL moves, platoon sits, empty slots), \
                       and recent recommendation outcomes. Provide a team_id to identify your \
                       roster. Optionally filter to a single league_id."
    )]
    async fn get_briefing(
        &self,
        Parameters(args): Parameters<BriefingArgs>,
    ) -> Result<String, ErrorData> {
        let date = parse_date_or_today(args.date.as_deref())?;

        // Determine which leagues to brief.
        let leagues: Vec<_> = match &args.league_id {
            Some(id) => self
                .state
                .config
                .leagues
                .iter()
                .filter(|l| l.id == *id)
                .collect(),
            None => self.state.config.leagues.iter().collect(),
        };

        if leagues.is_empty() {
            return Err(ErrorData::invalid_params(
                format!(
                    "no matching league found (configured: {})",
                    self.state
                        .config
                        .leagues
                        .iter()
                        .map(|l| l.id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None,
            ));
        }

        let mut briefings = Vec::new();
        let mut errors = Vec::new();

        for league in &leagues {
            match &args.team_id {
                Some(team_id) => {
                    match briefing::generate_briefing(
                        league,
                        team_id,
                        &args.period,
                        date,
                        &self.state.client,
                        &self.state.mlb,
                        &self.state.db,
                    )
                    .await
                    {
                        Ok(b) => briefings.push(b),
                        Err(e) => errors.push(json!({
                            "league_id": league.id,
                            "error": e.to_string(),
                        })),
                    }
                }
                None => {
                    errors.push(json!({
                        "league_id": league.id,
                        "error": "team_id is required for roster-specific briefing layers; \
                                  skipping this league",
                    }));
                }
            }
        }

        let response = json!({
            "date": date.format("%Y-%m-%d").to_string(),
            "briefings": briefings,
            "errors": errors,
        });

        serde_json::to_string_pretty(&response)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    /// Get MLB Stats API rest-of-season ZiPS projections for batters or pitchers.
    #[tool(
        name = "get_projections",
        description = "Get MLB Stats API rest-of-season ZiPS projections for batters or pitchers. \
                       Results are sorted by WAR descending. Optionally filter by player name \
                       (case-insensitive substring match). Cached for 24 hours."
    )]
    async fn get_projections(
        &self,
        Parameters(args): Parameters<GetProjectionsArgs>,
    ) -> Result<String, ErrorData> {
        let name_filter = args.player_name.as_deref().map(|n| n.to_lowercase());

        if args.player_type.is_batter() {
            let projections = self
                .state
                .projections
                .get_batter_projections()
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            let filtered: Vec<_> = match &name_filter {
                Some(filter) => projections
                    .into_iter()
                    .filter(|p| p.player_name.to_lowercase().contains(filter))
                    .collect(),
                None => projections,
            };

            let response = json!({
                "player_type": "batter",
                "source": self.state.config.projections.source,
                "total": filtered.len(),
                "projections": filtered,
            });

            serde_json::to_string_pretty(&response)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))
        } else {
            let projections = self
                .state
                .projections
                .get_pitcher_projections()
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            let filtered: Vec<_> = match &name_filter {
                Some(filter) => projections
                    .into_iter()
                    .filter(|p| p.player_name.to_lowercase().contains(filter))
                    .collect(),
                None => projections,
            };

            let response = json!({
                "player_type": "pitcher",
                "source": self.state.config.projections.source,
                "total": filtered.len(),
                "projections": filtered,
            });

            serde_json::to_string_pretty(&response)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))
        }
    }
}

#[tool_handler]
impl ServerHandler for FantraxServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Fantasy baseball MCP server for Fantrax and MLB Stats. Provides league standings, \
             roster data, player IDs, league info, probable starters with handedness, player \
             game lines from box scores, platoon-based lineup optimization, morning briefings, \
             and MLB Stats API rest-of-season ZiPS projections. Use `list_leagues` to discover \
             Fantrax leagues, `get_probable_starters` for today's pitching matchups, \
             `get_player_snippet` for a player's most recent stat line, `optimize_lineup` for \
             start/sit recommendations based on platoon matchups, `get_briefing` for a \
             comprehensive morning briefing, and `get_projections` for MLB Stats API WAR-sorted \
             ZiPS projections for batters or pitchers."
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MCP requires every tool's inputSchema to have `type: "object"`.
    /// `Parameters<()>` generates `type: "null"` which breaks Claude Code's
    /// schema validator. This test catches the trap at build time.
    #[test]
    fn all_tool_arg_schemas_are_object_type() {
        fn assert_object_schema<T: schemars::JsonSchema>(name: &str) {
            let schema = schemars::schema_for!(T);
            let obj = serde_json::to_value(&schema).unwrap();
            assert_eq!(
                obj.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "tool arg struct {name} has non-object schema type: {obj}"
            );
        }

        assert_object_schema::<NoArgs>("NoArgs");
        assert_object_schema::<LeagueIdArgs>("LeagueIdArgs");
        assert_object_schema::<RosterArgs>("RosterArgs");
        assert_object_schema::<PlayerIdsArgs>("PlayerIdsArgs");
        assert_object_schema::<LogRecommendationArgs>("LogRecommendationArgs");
        assert_object_schema::<QueryLedgerArgs>("QueryLedgerArgs");
        assert_object_schema::<RecordOutcomeArgs>("RecordOutcomeArgs");
        assert_object_schema::<ProbableStartersArgs>("ProbableStartersArgs");
        assert_object_schema::<PlayerSnippetArgs>("PlayerSnippetArgs");
        assert_object_schema::<OptimizeLineupArgs>("OptimizeLineupArgs");
        assert_object_schema::<BriefingArgs>("BriefingArgs");
        assert_object_schema::<GetProjectionsArgs>("GetProjectionsArgs");
    }
}
