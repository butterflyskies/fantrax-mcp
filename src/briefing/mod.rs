use std::sync::Arc;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::analysis;
use crate::config::{LeagueConfig, LineupType};
use crate::db::Database;
use crate::fantrax::{FantraxClient, Roster};
use crate::mlb::{BatterLine, Boxscore, MlbClient, PitcherLine};

// ─── Types ─────────────────────────────────────────────────────────────────

/// A complete morning briefing for one league.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Briefing {
    pub league_name: String,
    pub league_id: String,
    pub league_type: String,
    pub date: NaiveDate,
    pub what_happened: Vec<PlayerSnippet>,
    pub what_to_do: Vec<ActionItem>,
    pub hot_takes: Vec<String>,
}

/// One player's stat line (or status) from yesterday.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerSnippet {
    pub player_name: String,
    /// Human-readable line: "2-4, HR, 3 RBI" or "DNP" or "IL10 (oblique)".
    pub line: String,
    /// Flag standout performances.
    pub notable: bool,
}

/// Something the manager should act on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub urgency: Urgency,
    pub action: String,
    pub reason: String,
}

/// How urgently something needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Urgency {
    Now,
    Soon,
    Monitor,
}

// ─── Error type ────────────────────────────────────────────────────────────

/// Errors from the briefing pipeline.
#[derive(Debug)]
pub enum BriefingError {
    Mlb(crate::mlb::MlbError),
    Fantrax(crate::fantrax::FantraxError),
    Other(String),
}

impl std::fmt::Display for BriefingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mlb(e) => write!(f, "MLB error: {e}"),
            Self::Fantrax(e) => write!(f, "Fantrax error: {e}"),
            Self::Other(msg) => write!(f, "briefing error: {msg}"),
        }
    }
}

impl std::error::Error for BriefingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mlb(e) => Some(e),
            Self::Fantrax(e) => Some(e),
            Self::Other(_) => None,
        }
    }
}

impl From<crate::mlb::MlbError> for BriefingError {
    fn from(e: crate::mlb::MlbError) -> Self {
        Self::Mlb(e)
    }
}

impl From<crate::fantrax::FantraxError> for BriefingError {
    fn from(e: crate::fantrax::FantraxError) -> Self {
        Self::Fantrax(e)
    }
}

// ─── Pipeline ──────────────────────────────────────────────────────────────

/// Generate a morning briefing for a single league.
///
/// Pulls yesterday's boxscores, today's schedule, and the current roster
/// to assemble all three briefing layers.
pub async fn generate_briefing(
    league: &LeagueConfig,
    team_id: &str,
    period: &str,
    date: NaiveDate,
    fantrax: &FantraxClient,
    mlb: &MlbClient,
    db: &Arc<Database>,
) -> Result<Briefing, BriefingError> {
    let yesterday = date - chrono::Duration::days(1);

    // Fetch roster once and share between layers.
    let roster = fantrax.get_team_rosters(&league.id, period).await?;

    // ── Layer 1: What happened ─────────────────────────────────────────
    let what_happened = build_what_happened(league, team_id, yesterday, mlb, &roster).await?;

    // ── Layer 2: What to do ────────────────────────────────────────────
    let what_to_do = build_what_to_do(league, team_id, date, mlb, &roster).await?;

    // ── Layer 3: Hot takes (stub) ──────────────────────────────────────
    let hot_takes = build_hot_takes(&league.id, db);

    let league_type = league.league_type.to_string();

    Ok(Briefing {
        league_name: league.name.clone(),
        league_id: league.id.clone(),
        league_type,
        date,
        what_happened,
        what_to_do,
        hot_takes,
    })
}

// ─── Layer 1: What happened ────────────────────────────────────────────────

/// Pull yesterday's boxscores and find each roster player's stat line.
async fn build_what_happened(
    league: &LeagueConfig,
    team_id: &str,
    yesterday: NaiveDate,
    mlb: &MlbClient,
    roster: &Roster,
) -> Result<Vec<PlayerSnippet>, BriefingError> {
    let team = roster.teams.iter().find(|t| t.team_id == team_id);
    let players = match team {
        Some(t) => &t.players,
        None => {
            return Err(BriefingError::Other(format!(
                "team '{team_id}' not found in league '{}'",
                league.id
            )));
        }
    };

    // Fetch yesterday's schedule and all boxscores.
    let games = mlb.get_schedule(yesterday).await?;
    let mut boxscores: Vec<Boxscore> = Vec::new();
    for game in &games {
        // Only fetch boxscores for completed or in-progress games.
        if game.status == "Final" || game.status == "Live" {
            let is_final = game.status == "Final";
            match mlb.get_boxscore(game.game_pk, is_final).await {
                Ok(bs) => boxscores.push(bs),
                Err(e) => {
                    tracing::warn!(game_pk = game.game_pk, error = %e, "skipping boxscore");
                }
            }
        }
    }

    // For each roster player, find their line in yesterday's boxscores.
    let mut snippets = Vec::new();
    for rp in players {
        // Check injury status first.
        if is_injured_status(&rp.roster_status) {
            snippets.push(PlayerSnippet {
                player_name: rp.name.clone(),
                line: format_injury_line(&rp.roster_status),
                notable: false,
            });
            continue;
        }

        // Try to parse player_id as MLB numeric ID.
        let mlb_id: Option<u64> = rp.player_id.parse().ok();

        let mut found = false;
        if let Some(pid) = mlb_id {
            // Search boxscores for this player's batting line.
            for bs in &boxscores {
                if let Some(line) = find_batter_line(bs, pid) {
                    let (formatted, notable) = format_batter_snippet(&line);
                    snippets.push(PlayerSnippet {
                        player_name: rp.name.clone(),
                        line: formatted,
                        notable,
                    });
                    found = true;
                    break;
                }
                if let Some(line) = find_pitcher_line(bs, pid) {
                    let (formatted, notable) = format_pitcher_snippet(&line);
                    snippets.push(PlayerSnippet {
                        player_name: rp.name.clone(),
                        line: formatted,
                        notable,
                    });
                    found = true;
                    break;
                }
            }
        }

        if !found {
            snippets.push(PlayerSnippet {
                player_name: rp.name.clone(),
                line: "DNP".to_string(),
                notable: false,
            });
        }
    }

    Ok(snippets)
}

fn find_batter_line(boxscore: &Boxscore, player_id: u64) -> Option<BatterLine> {
    boxscore
        .away_batters
        .iter()
        .chain(boxscore.home_batters.iter())
        .find(|b| b.player_id == player_id)
        .cloned()
}

fn find_pitcher_line(boxscore: &Boxscore, player_id: u64) -> Option<PitcherLine> {
    boxscore
        .away_pitchers
        .iter()
        .chain(boxscore.home_pitchers.iter())
        .find(|p| p.player_id == player_id)
        .cloned()
}

/// Format a batter's box score line into a human-readable snippet.
///
/// Returns (formatted_line, is_notable).
fn format_batter_snippet(line: &BatterLine) -> (String, bool) {
    let mut parts = Vec::new();

    // Hits-AB
    parts.push(format!("{}-{}", line.hits, line.at_bats));

    // Extra-base hits
    if line.home_runs > 0 {
        if line.home_runs == 1 {
            parts.push("HR".to_string());
        } else {
            parts.push(format!("{} HR", line.home_runs));
        }
    }
    if line.doubles > 0 {
        if line.doubles == 1 {
            parts.push("2B".to_string());
        } else {
            parts.push(format!("{} 2B", line.doubles));
        }
    }
    if line.triples > 0 {
        if line.triples == 1 {
            parts.push("3B".to_string());
        } else {
            parts.push(format!("{} 3B", line.triples));
        }
    }

    // Counting stats
    if line.rbi > 0 {
        parts.push(format!("{} RBI", line.rbi));
    }
    if line.runs > 0 {
        parts.push(format!("{} R", line.runs));
    }
    if line.walks > 0 {
        parts.push(format!("{} BB", line.walks));
    }
    if line.stolen_bases > 0 {
        parts.push(format!("{} SB", line.stolen_bases));
    }

    // Notable: 3+ hits, HR, or 3+ RBI.
    let notable = line.hits >= 3 || line.home_runs > 0 || line.rbi >= 3 || line.stolen_bases >= 2;

    (parts.join(", "), notable)
}

/// Format a pitcher's box score line into a human-readable snippet.
///
/// Returns (formatted_line, is_notable).
fn format_pitcher_snippet(line: &PitcherLine) -> (String, bool) {
    let mut parts = Vec::new();

    // IP and decision
    parts.push(format!("{} IP", line.innings_pitched));

    if let Some(ref decision) = line.decision {
        parts.push(format!("({})", decision));
    }

    // Key stats
    parts.push(format!("{} K", line.strikeouts));
    parts.push(format!("{} H", line.hits));
    parts.push(format!("{} ER", line.earned_runs));

    if line.walks > 0 {
        parts.push(format!("{} BB", line.walks));
    }
    if line.home_runs > 0 {
        parts.push(format!("{} HR", line.home_runs));
    }

    // Notable: 10+ K, QS (6+ IP, 3 or fewer ER), or CGSO-ish (8+ IP, 0 ER).
    let ip_float = parse_ip(&line.innings_pitched);
    let notable =
        line.strikeouts >= 10 || (ip_float >= 6.0 && line.earned_runs <= 3) || ip_float >= 8.0;

    (parts.join(", "), notable)
}

/// Parse innings pitched string like "6.2" into a float.
/// MLB uses "6.1" = 6 1/3, "6.2" = 6 2/3.
fn parse_ip(ip: &str) -> f64 {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() == 2 {
        let whole: f64 = parts[0].parse().unwrap_or(0.0);
        let frac: f64 = parts[1].parse().unwrap_or(0.0);
        if frac > 2.0 {
            tracing::warn!(
                innings_pitched = ip,
                "unexpected fractional part > 2 in innings pitched; treating as raw number"
            );
            return ip.parse().unwrap_or(whole);
        }
        whole + frac / 3.0
    } else {
        ip.parse().unwrap_or(0.0)
    }
}

fn is_injured_status(status: &str) -> bool {
    let s = status.to_uppercase();
    s.contains("IL") || s.contains("DL") || s.contains("DTD") || s.contains("OUT")
}

fn format_injury_line(status: &str) -> String {
    let s = status.to_uppercase();
    if s.contains("IL") {
        format!("IL ({})", status)
    } else if s.contains("DTD") {
        format!("DTD ({})", status)
    } else {
        format!("OUT ({})", status)
    }
}

// ─── Layer 2: What to do ───────────────────────────────────────────────────

/// Generate action items based on today's matchups and roster status.
async fn build_what_to_do(
    league: &LeagueConfig,
    team_id: &str,
    today: NaiveDate,
    mlb: &MlbClient,
    roster: &Roster,
) -> Result<Vec<ActionItem>, BriefingError> {
    let mut items = Vec::new();

    let team = roster.teams.iter().find(|t| t.team_id == team_id);
    let players = match team {
        Some(t) => &t.players,
        None => return Ok(items),
    };

    let games = mlb.get_schedule(today).await?;

    // ── Check for injured players still in active slots ─────────────
    for rp in players {
        if is_injured_status(&rp.roster_status) && !is_bench_or_il_slot(&rp.position) {
            items.push(ActionItem {
                urgency: Urgency::Now,
                action: format!("Move {} to IL or bench", rp.name),
                reason: format!(
                    "{} has status '{}' but is in an active lineup slot ({})",
                    rp.name, rp.roster_status, rp.position
                ),
            });
        }
    }

    // ── Lineup advice depends on league type ───────────────────────
    match league.lineup {
        LineupType::Daily => {
            // Run platoon analysis for start/sit recs.
            let mut player_pairs = Vec::new();
            for rp in players {
                if is_injured_status(&rp.roster_status) {
                    continue;
                }
                if let Ok(mlb_id) = rp.player_id.parse::<u64>()
                    && let Ok(player) = mlb.get_player(mlb_id).await
                {
                    player_pairs.push((rp.clone(), player));
                }
            }

            let recs = analysis::platoon_recommendations(&player_pairs, &games);
            for rec in &recs {
                if rec.recommendation == analysis::Recommendation::Sit {
                    items.push(ActionItem {
                        urgency: Urgency::Soon,
                        action: format!("Consider sitting {}", rec.player_name),
                        reason: rec.reason.clone(),
                    });
                }
            }

            // Flag players with no game today.
            for rec in &recs {
                if rec.recommendation == analysis::Recommendation::Monitor {
                    items.push(ActionItem {
                        urgency: Urgency::Monitor,
                        action: format!("Check {} — may not have a game today", rec.player_name),
                        reason: rec.reason.clone(),
                    });
                }
            }
        }
        LineupType::Weekly => {
            // Weekly outlook: flag schedule density.
            items.push(ActionItem {
                urgency: Urgency::Monitor,
                action: "Review weekly matchup schedule".to_string(),
                reason: "Weekly lineup locks — check how many games each player has this week"
                    .to_string(),
            });
        }
        LineupType::Bestball | LineupType::Worstball => {
            // Best ball: just flag injuries, lineups are auto-set.
            for rp in players {
                if is_injured_status(&rp.roster_status) {
                    items.push(ActionItem {
                        urgency: Urgency::Monitor,
                        action: format!("Monitor {} injury status", rp.name),
                        reason: format!(
                            "{} is {} — may need to pick up a replacement",
                            rp.name, rp.roster_status
                        ),
                    });
                }
            }
        }
    }

    Ok(items)
}

/// Check if a position string represents a bench or IL slot.
fn is_bench_or_il_slot(position: &str) -> bool {
    let p = position.to_uppercase();
    p.contains("BN") || p.contains("BENCH") || p.contains("IL") || p.contains("DL")
}

// ─── Layer 3: Hot takes (stub) ─────────────────────────────────────────────

/// Surface recent recommendation ledger entries and their outcomes.
///
/// This is a placeholder for editorial commentary that will be LLM-generated
/// in a future iteration.
fn build_hot_takes(league_id: &str, db: &Arc<Database>) -> Vec<String> {
    let recs = db.get_recommendations(league_id, None);

    // Only look at the 10 most recent entries.
    let recent: Vec<_> = recs.into_iter().take(10).collect();

    let mut takes = Vec::new();
    for rec in &recent {
        let outcome_str = rec.outcome.as_deref().unwrap_or("pending");
        takes.push(format!(
            "[{}] {} by {} — {} ({})",
            rec.recommendation_type, rec.players, rec.agent_id, rec.reasoning, outcome_str,
        ));
    }

    if takes.is_empty() {
        takes.push("No recent recommendations in the ledger.".to_string());
    }

    takes
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batter_snippet_basic() {
        let line = BatterLine {
            player_id: 1,
            name: "Test Player".into(),
            position: "SS".into(),
            at_bats: 4,
            runs: 1,
            hits: 2,
            doubles: 1,
            triples: 0,
            home_runs: 0,
            rbi: 1,
            walks: 0,
            strikeouts: 1,
            stolen_bases: 0,
            avg: ".300".into(),
        };
        let (formatted, notable) = format_batter_snippet(&line);
        assert_eq!(formatted, "2-4, 2B, 1 RBI, 1 R");
        assert!(!notable);
    }

    #[test]
    fn batter_snippet_notable_hr() {
        let line = BatterLine {
            player_id: 1,
            name: "Slugger".into(),
            position: "1B".into(),
            at_bats: 4,
            runs: 2,
            hits: 2,
            doubles: 0,
            triples: 0,
            home_runs: 1,
            rbi: 3,
            walks: 1,
            strikeouts: 1,
            stolen_bases: 0,
            avg: ".280".into(),
        };
        let (formatted, notable) = format_batter_snippet(&line);
        assert!(formatted.contains("HR"));
        assert!(formatted.contains("3 RBI"));
        assert!(notable);
    }

    #[test]
    fn batter_snippet_notable_3hits() {
        let line = BatterLine {
            player_id: 1,
            name: "Contact".into(),
            position: "CF".into(),
            at_bats: 5,
            runs: 2,
            hits: 3,
            doubles: 1,
            triples: 0,
            home_runs: 0,
            rbi: 1,
            walks: 0,
            strikeouts: 0,
            stolen_bases: 1,
            avg: ".310".into(),
        };
        let (_, notable) = format_batter_snippet(&line);
        assert!(notable);
    }

    #[test]
    fn pitcher_snippet_quality_start() {
        let line = PitcherLine {
            player_id: 1,
            name: "Ace".into(),
            innings_pitched: "7.0".into(),
            hits: 5,
            runs: 2,
            earned_runs: 2,
            walks: 1,
            strikeouts: 8,
            home_runs: 0,
            pitches_thrown: 98,
            strikes: 65,
            era: "2.50".into(),
            decision: Some("W".into()),
        };
        let (formatted, notable) = format_pitcher_snippet(&line);
        assert!(formatted.contains("7.0 IP"));
        assert!(formatted.contains("8 K"));
        assert!(notable); // QS: 7 IP, 2 ER
    }

    #[test]
    fn pitcher_snippet_10k() {
        let line = PitcherLine {
            player_id: 1,
            name: "Strikeout King".into(),
            innings_pitched: "6.0".into(),
            hits: 4,
            runs: 3,
            earned_runs: 3,
            walks: 2,
            strikeouts: 11,
            home_runs: 1,
            pitches_thrown: 110,
            strikes: 72,
            era: "3.20".into(),
            decision: None,
        };
        let (_, notable) = format_pitcher_snippet(&line);
        assert!(notable); // 11 K
    }

    #[test]
    fn parse_ip_works() {
        assert!((parse_ip("6.0") - 6.0).abs() < 0.01);
        assert!((parse_ip("6.1") - 6.333).abs() < 0.01);
        assert!((parse_ip("6.2") - 6.666).abs() < 0.01);
    }

    #[test]
    fn injury_status_detection() {
        assert!(is_injured_status("IL10"));
        assert!(is_injured_status("IL60"));
        assert!(is_injured_status("DL"));
        assert!(is_injured_status("DTD"));
        assert!(is_injured_status("OUT"));
        assert!(!is_injured_status("Active"));
        assert!(!is_injured_status("Healthy"));
    }

    #[test]
    fn bench_slot_detection() {
        assert!(is_bench_or_il_slot("BN"));
        assert!(is_bench_or_il_slot("Bench"));
        assert!(is_bench_or_il_slot("IL"));
        assert!(!is_bench_or_il_slot("SS"));
        assert!(!is_bench_or_il_slot("OF"));
    }
}
