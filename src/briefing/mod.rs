use std::sync::Arc;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::analysis;
use crate::config::{LeagueConfig, LineupType};
use crate::db::Database;
use crate::fantrax::{FantraxClient, Roster, RosterPlayer};
use crate::mlb::{BatterLine, Boxscore, MlbClient, PitcherLine};
use crate::types::{LeagueId, TeamId};

// ─── Types ─────────────────────────────────────────────────────────────────

/// A complete morning briefing for one league.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Briefing {
    pub league_name: String,
    pub league_id: LeagueId,
    pub league_type: String,
    pub date: NaiveDate,
    pub what_happened: Vec<PlayerSnippet>,
    pub what_to_do: Vec<ActionItem>,
    pub hot_takes: Vec<String>,
    /// Names of roster players that could not be resolved to MLB Stats API
    /// IDs. These players are excluded from boxscore lookups and platoon
    /// analysis rather than reported with fabricated "DNP" lines.
    #[serde(default)]
    pub unresolved_players: Vec<String>,
    /// Human-readable note explaining `unresolved_players`, when non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_note: Option<String>,
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
#[derive(Debug, thiserror::Error)]
pub enum BriefingError {
    #[error("MLB error: {0}")]
    Mlb(#[from] crate::mlb::MlbError),
    #[error("Fantrax error: {0}")]
    Fantrax(#[from] crate::fantrax::FantraxError),
    #[error("briefing error: {0}")]
    Other(String),
}

// ─── Pipeline ──────────────────────────────────────────────────────────────

/// Generate a morning briefing for a single league.
///
/// Pulls yesterday's boxscores, today's schedule, and the current roster
/// to assemble all three briefing layers.
pub async fn generate_briefing(
    league: &LeagueConfig,
    team_id: &TeamId,
    period: &str,
    date: NaiveDate,
    fantrax: &FantraxClient,
    mlb: &MlbClient,
    db: &Arc<Database>,
) -> Result<Briefing, BriefingError> {
    let yesterday = date - chrono::Duration::days(1);

    // Fetch roster once (enriched with player names — the bare roster
    // endpoint returns IDs only) and share between layers.
    let roster = fantrax
        .get_team_rosters_enriched(&league.id, period)
        .await?;

    // ── Layer 1: What happened ─────────────────────────────────────────
    let (what_happened, unresolved_l1) =
        build_what_happened(league, team_id, yesterday, mlb, &roster).await?;

    // ── Layer 2: What to do ────────────────────────────────────────────
    let (what_to_do, unresolved_l2) = build_what_to_do(league, team_id, date, mlb, &roster).await?;

    // ── Layer 3: Hot takes (stub) ──────────────────────────────────────
    let hot_takes = build_hot_takes(&league.id, db).await;

    let league_type = league.league_type.to_string();

    let unresolved_players = merge_unresolved(unresolved_l1, unresolved_l2);
    let unresolved_note = unresolved_note(&unresolved_players);

    Ok(Briefing {
        league_name: league.name.clone(),
        league_id: league.id.clone(),
        league_type,
        date,
        what_happened,
        what_to_do,
        hot_takes,
        unresolved_players,
        unresolved_note,
    })
}

// ─── MLB ID resolution ─────────────────────────────────────────────────────

/// Resolve a Fantrax roster player to an MLB Stats API person ID.
///
/// There is currently no Fantrax→MLB ID crosswalk (issue #11), so this always
/// returns `None`. Fantrax player IDs are alphanumeric strings (e.g. "03pit")
/// in a completely separate ID space from MLB's numeric person IDs. Do NOT
/// "fix" this by parsing the Fantrax ID as a number: an all-digit Fantrax ID
/// would parse successfully and silently fetch a *different* player's MLB
/// record. Callers must treat `None` as "skip and report", never as "DNP".
fn resolve_mlb_id(_player: &RosterPlayer) -> Option<u64> {
    None
}

/// Merge unresolved-player name lists from multiple briefing layers,
/// deduplicating while preserving first-seen order.
fn merge_unresolved(a: Vec<String>, b: Vec<String>) -> Vec<String> {
    let mut merged = a;
    for name in b {
        if !merged.contains(&name) {
            merged.push(name);
        }
    }
    merged
}

/// Build the human-readable annotation for unresolved roster players.
fn unresolved_note(unresolved: &[String]) -> Option<String> {
    match unresolved.len() {
        0 => None,
        1 => Some(
            "1 roster player could not be resolved to an MLB ID \
             (Fantrax→MLB crosswalk pending, see #11)"
                .to_string(),
        ),
        n => Some(format!(
            "{n} roster players could not be resolved to MLB IDs \
             (Fantrax→MLB crosswalk pending, see #11)"
        )),
    }
}

// ─── Layer 1: What happened ────────────────────────────────────────────────

/// Pull yesterday's boxscores and find each roster player's stat line.
///
/// Returns the snippets plus the names of players that could not be resolved
/// to MLB IDs (and were therefore skipped, not reported as "DNP").
async fn build_what_happened(
    league: &LeagueConfig,
    team_id: &TeamId,
    yesterday: NaiveDate,
    mlb: &MlbClient,
    roster: &Roster,
) -> Result<(Vec<PlayerSnippet>, Vec<String>), BriefingError> {
    let team = roster.teams.iter().find(|t| t.team_id == *team_id);
    let players = match team {
        Some(t) => &t.players,
        None => {
            return Err(BriefingError::Other(format!(
                "team '{team_id}' not found in league '{}'",
                league.id
            )));
        }
    };

    // Only fetch boxscores if at least one player is resolvable to an MLB ID
    // — otherwise the fetches can't match anyone.
    let any_resolvable = players
        .iter()
        .any(|rp| !is_injured_status(&rp.roster_status) && resolve_mlb_id(rp).is_some());

    let mut boxscores: Vec<Boxscore> = Vec::new();
    if any_resolvable {
        // Fetch yesterday's schedule and all boxscores.
        let games = mlb.get_schedule(yesterday).await?;
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
    }

    Ok(snippets_from_boxscores(players, &boxscores))
}

/// For each roster player, find their line in yesterday's boxscores.
///
/// Players that cannot be resolved to an MLB ID are skipped and returned in
/// the second tuple element — a missing crosswalk entry must not be reported
/// as "DNP". "DNP" is reserved for resolvable players with no boxscore line.
fn snippets_from_boxscores(
    players: &[RosterPlayer],
    boxscores: &[Boxscore],
) -> (Vec<PlayerSnippet>, Vec<String>) {
    let mut snippets = Vec::new();
    let mut unresolved = Vec::new();

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

        let Some(pid) = resolve_mlb_id(rp) else {
            unresolved.push(rp.name.clone());
            continue;
        };

        // Search boxscores for this player's batting or pitching line.
        let mut found = false;
        for bs in boxscores {
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

        if !found {
            snippets.push(PlayerSnippet {
                player_name: rp.name.clone(),
                line: "DNP".to_string(),
                notable: false,
            });
        }
    }

    (snippets, unresolved)
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
    // Avoid redundant labels like "IL (IL10)" — just use the status directly
    // when it already contains the category.
    let s = status.to_uppercase();
    if s.starts_with("IL") || s.starts_with("DTD") || s.starts_with("OUT") {
        status.to_string()
    } else if s.contains("IL") {
        format!("IL ({status})")
    } else if s.contains("DTD") {
        format!("DTD ({status})")
    } else {
        format!("OUT ({status})")
    }
}

// ─── Layer 2: What to do ───────────────────────────────────────────────────

/// Generate action items based on today's matchups and roster status.
///
/// Returns the action items plus the names of players that could not be
/// resolved to MLB IDs (and were therefore excluded from platoon analysis).
async fn build_what_to_do(
    league: &LeagueConfig,
    team_id: &TeamId,
    today: NaiveDate,
    mlb: &MlbClient,
    roster: &Roster,
) -> Result<(Vec<ActionItem>, Vec<String>), BriefingError> {
    let mut items = Vec::new();
    let mut unresolved = Vec::new();

    let team = roster.teams.iter().find(|t| t.team_id == *team_id);
    let players = match team {
        Some(t) => &t.players,
        None => return Ok((items, unresolved)),
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
            // Run platoon analysis for start/sit recs. Players without a
            // resolvable MLB ID are excluded and reported, not silently
            // dropped.
            let (resolved, not_resolved) = partition_resolvable(players);
            unresolved = not_resolved;

            let mut player_pairs = Vec::new();
            for (rp, mlb_id) in resolved {
                match mlb.get_player(mlb_id).await {
                    Ok(player) => player_pairs.push((rp.clone(), player)),
                    Err(e) => {
                        tracing::warn!(
                            player = %rp.name, mlb_id, error = %e,
                            "MLB player lookup failed; excluding from platoon analysis"
                        );
                        unresolved.push(rp.name.clone());
                    }
                }
            }

            let recs = analysis::platoon_recommendations(&player_pairs, &games);
            for rec in &recs {
                if rec.verdict == analysis::LineupVerdict::Sit {
                    items.push(ActionItem {
                        urgency: Urgency::Soon,
                        action: format!("Consider sitting {}", rec.player_name),
                        reason: rec.reason.clone(),
                    });
                }
            }

            // Flag players with no game today.
            for rec in &recs {
                if rec.verdict == analysis::LineupVerdict::Monitor {
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

    Ok((items, unresolved))
}

/// Partition active (non-injured) roster players by whether they resolve to
/// an MLB ID. Returns (resolved players with their MLB IDs, names of players
/// that could not be resolved).
fn partition_resolvable(players: &[RosterPlayer]) -> (Vec<(&RosterPlayer, u64)>, Vec<String>) {
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for rp in players {
        if is_injured_status(&rp.roster_status) {
            continue;
        }
        match resolve_mlb_id(rp) {
            Some(mlb_id) => resolved.push((rp, mlb_id)),
            None => unresolved.push(rp.name.clone()),
        }
    }
    (resolved, unresolved)
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
async fn build_hot_takes(league_id: &LeagueId, db: &Arc<Database>) -> Vec<String> {
    let recs = db.get_recommendations_async(league_id.clone(), None).await;

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
mod tests;
