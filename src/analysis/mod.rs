use std::fmt;

use serde::{Deserialize, Serialize};

use crate::fantrax::RosterPlayer;
use crate::mlb::{Game, PitcherInfo, Player};

// ─── Types ─────────────────────────────────────────────────────────────────

/// Start/sit recommendation for a single batter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSitRecommendation {
    pub player_name: String,
    pub player_id: String,
    pub recommendation: Recommendation,
    pub reason: String,
    pub opposing_pitcher: Option<String>,
    pub matchup_advantage: Option<MatchupAdvantage>,
}

/// Start, sit, or monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Recommendation {
    Start,
    Sit,
    Monitor,
}

impl fmt::Display for Recommendation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start => write!(f, "start"),
            Self::Sit => write!(f, "sit"),
            Self::Monitor => write!(f, "monitor"),
        }
    }
}

/// How the batter-pitcher matchup breaks down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MatchupAdvantage {
    /// Batter has the platoon edge (e.g. RHB vs LHP).
    Platoon,
    /// Same-side matchup (e.g. RHB vs RHP).
    Disadvantage,
    /// Switch hitter or no handedness data available.
    Neutral,
}

impl fmt::Display for MatchupAdvantage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Platoon => write!(f, "platoon"),
            Self::Disadvantage => write!(f, "disadvantage"),
            Self::Neutral => write!(f, "neutral"),
        }
    }
}

/// Stub result for roster gap analysis.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryGap {
    pub category: String,
    pub severity: String,
    pub description: String,
}

// ─── Platoon logic ─────────────────────────────────────────────────────────

/// Determine the matchup advantage for a batter against a pitcher.
///
/// - Switch hitters (`bat_side == "S"`) are always `Neutral`.
/// - A batter facing same-side pitching (R vs R, L vs L) is `Disadvantage`.
/// - Opposite-side matchup (R vs L, L vs R) is `Platoon`.
/// - Unknown handedness results in `Neutral`.
pub fn matchup_advantage(bat_side: &str, pitch_hand: &str) -> MatchupAdvantage {
    match (bat_side, pitch_hand) {
        ("S", _) | (_, "S") => MatchupAdvantage::Neutral,
        ("R", "L") | ("L", "R") => MatchupAdvantage::Platoon,
        ("R", "R") | ("L", "L") => MatchupAdvantage::Disadvantage,
        _ => MatchupAdvantage::Neutral,
    }
}

/// Determine whether a position is a pitching position (not a batter).
fn is_pitcher_position(pos: &str) -> bool {
    matches!(pos.to_uppercase().as_str(), "SP" | "RP" | "P")
}

/// Given resolved player info and today's games with probable starters, produce
/// start/sit recommendations based on platoon matchups.
///
/// # Arguments
/// * `player_info` - MLB player data paired with Fantrax roster entries,
///   containing handedness and current team.
/// * `games` - Today's MLB schedule with probable starters.
///
/// Returns one recommendation per non-pitcher in `player_info`.
pub fn platoon_recommendations(
    player_info: &[(RosterPlayer, Player)],
    games: &[Game],
) -> Vec<StartSitRecommendation> {
    let mut recs = Vec::new();

    for (roster_player, mlb_player) in player_info {
        // Skip pitchers — platoon logic is for batters only.
        if is_pitcher_position(&roster_player.position) {
            continue;
        }

        // Find which game this batter's team is in today.
        let game = find_game_for_team(games, mlb_player.current_team_id);

        let (opposing_pitcher, matchup, reason) = match game {
            Some((_g, pitcher_hand, pitcher_name)) => {
                let advantage = matchup_advantage(&mlb_player.bat_side, &pitcher_hand);
                let reason = format_platoon_reason(
                    &mlb_player.full_name,
                    &mlb_player.bat_side,
                    &pitcher_hand,
                    &pitcher_name,
                    advantage,
                );
                (Some(pitcher_name), Some(advantage), reason)
            }
            None => (
                None,
                None,
                format!(
                    "{}: no game found for team today — check if they have a day off",
                    mlb_player.full_name
                ),
            ),
        };

        let recommendation = match matchup {
            Some(MatchupAdvantage::Platoon) => Recommendation::Start,
            Some(MatchupAdvantage::Disadvantage) => Recommendation::Sit,
            Some(MatchupAdvantage::Neutral) => Recommendation::Start,
            None => Recommendation::Monitor,
        };

        recs.push(StartSitRecommendation {
            player_name: mlb_player.full_name.clone(),
            player_id: roster_player.player_id.clone(),
            recommendation,
            reason,
            opposing_pitcher,
            matchup_advantage: matchup,
        });
    }

    recs
}

/// Find the game a player's team is in, returning the opposing pitcher's hand
/// and name. Uses numeric team ID for reliable matching.
fn find_game_for_team(games: &[Game], team_id: Option<u64>) -> Option<(&Game, String, String)> {
    let tid = team_id?;

    /// Extract opposing pitcher info given the opposing team's probable pitcher.
    fn pitcher_info(pitcher: &Option<PitcherInfo>) -> (String, String) {
        match pitcher {
            Some(p) => {
                let hand = p.pitch_hand.clone().unwrap_or_else(|| "Unknown".into());
                (hand, p.full_name.clone())
            }
            None => ("Unknown".into(), "TBD".into()),
        }
    }

    for game in games {
        if game.away_team.id == tid {
            let (hand, name) = pitcher_info(&game.home_probable_pitcher);
            return Some((game, hand, name));
        }
        if game.home_team.id == tid {
            let (hand, name) = pitcher_info(&game.away_probable_pitcher);
            return Some((game, hand, name));
        }
    }

    None
}

fn format_platoon_reason(
    batter: &str,
    bat_side: &str,
    pitch_hand: &str,
    pitcher_name: &str,
    advantage: MatchupAdvantage,
) -> String {
    let bat_label = match bat_side {
        "R" => "RHB",
        "L" => "LHB",
        "S" => "switch-hitter",
        _ => "unknown bat side",
    };
    let pitch_label = match pitch_hand {
        "R" => "RHP",
        "L" => "LHP",
        _ => "unknown hand",
    };

    match advantage {
        MatchupAdvantage::Platoon => {
            format!(
                "{batter} ({bat_label}) vs {pitcher_name} ({pitch_label}) — platoon advantage, \
                 start with confidence"
            )
        }
        MatchupAdvantage::Disadvantage => {
            format!(
                "{batter} ({bat_label}) vs {pitcher_name} ({pitch_label}) — same-side matchup, \
                 consider sitting if you have alternatives"
            )
        }
        MatchupAdvantage::Neutral => {
            format!(
                "{batter} ({bat_label}) vs {pitcher_name} ({pitch_label}) — switch hitter, \
                 no platoon concern"
            )
        }
    }
}

// ─── Lineup optimizer ──────────────────────────────────────────────────────

/// Rank batters by platoon advantage for lineup optimization.
///
/// This is a first-pass optimizer: it sorts eligible batters by matchup
/// quality (platoon advantage > neutral > disadvantage) and recommends
/// the top N for a given number of slots.
///
/// Future iterations will incorporate recent performance and projections.
pub fn optimize_lineup(
    recommendations: &[StartSitRecommendation],
    slots: usize,
) -> Vec<StartSitRecommendation> {
    let mut sorted: Vec<_> = recommendations.to_vec();

    sorted.sort_by(|a, b| {
        let a_score = matchup_score(a.matchup_advantage);
        let b_score = matchup_score(b.matchup_advantage);
        // Higher score = better matchup = sort first.
        b_score.cmp(&a_score)
    });

    sorted.truncate(slots);
    sorted
}

/// Assign a numeric score to a matchup advantage for sorting.
fn matchup_score(advantage: Option<MatchupAdvantage>) -> u8 {
    match advantage {
        Some(MatchupAdvantage::Platoon) => 3,
        Some(MatchupAdvantage::Neutral) => 2,
        Some(MatchupAdvantage::Disadvantage) => 1,
        None => 0,
    }
}

// ─── Roster gap analysis (stub) ────────────────────────────────────────────

/// Placeholder for roster gap analysis.
///
/// Given standings and scoring rules, this will identify which categories the
/// team is weak in and return a ranked list of category gaps. Currently returns
/// a stub response.
#[allow(dead_code)]
pub fn analyze_roster_gaps(_league_id: &str, _team_id: &str) -> Vec<CategoryGap> {
    vec![CategoryGap {
        category: "stub".into(),
        severity: "info".into(),
        description: "Roster gap analysis not yet implemented — will analyze \
                      category strengths/weaknesses vs standings."
            .into(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_hitter_is_neutral() {
        assert_eq!(matchup_advantage("S", "R"), MatchupAdvantage::Neutral);
        assert_eq!(matchup_advantage("S", "L"), MatchupAdvantage::Neutral);
    }

    #[test]
    fn opposite_hand_is_platoon() {
        assert_eq!(matchup_advantage("R", "L"), MatchupAdvantage::Platoon);
        assert_eq!(matchup_advantage("L", "R"), MatchupAdvantage::Platoon);
    }

    #[test]
    fn same_hand_is_disadvantage() {
        assert_eq!(matchup_advantage("R", "R"), MatchupAdvantage::Disadvantage);
        assert_eq!(matchup_advantage("L", "L"), MatchupAdvantage::Disadvantage);
    }

    #[test]
    fn unknown_hand_is_neutral() {
        assert_eq!(matchup_advantage("R", "Unknown"), MatchupAdvantage::Neutral);
        assert_eq!(matchup_advantage("Unknown", "R"), MatchupAdvantage::Neutral);
    }

    #[test]
    fn pitcher_position_detection() {
        assert!(is_pitcher_position("SP"));
        assert!(is_pitcher_position("RP"));
        assert!(is_pitcher_position("P"));
        assert!(!is_pitcher_position("SS"));
        assert!(!is_pitcher_position("OF"));
        assert!(!is_pitcher_position("1B"));
    }

    #[test]
    fn optimize_lineup_limits_to_slots() {
        let recs = vec![
            StartSitRecommendation {
                player_name: "A".into(),
                player_id: "1".into(),
                recommendation: Recommendation::Sit,
                reason: String::new(),
                opposing_pitcher: None,
                matchup_advantage: Some(MatchupAdvantage::Disadvantage),
            },
            StartSitRecommendation {
                player_name: "B".into(),
                player_id: "2".into(),
                recommendation: Recommendation::Start,
                reason: String::new(),
                opposing_pitcher: None,
                matchup_advantage: Some(MatchupAdvantage::Platoon),
            },
            StartSitRecommendation {
                player_name: "C".into(),
                player_id: "3".into(),
                recommendation: Recommendation::Start,
                reason: String::new(),
                opposing_pitcher: None,
                matchup_advantage: Some(MatchupAdvantage::Neutral),
            },
        ];

        let top2 = optimize_lineup(&recs, 2);
        assert_eq!(top2.len(), 2);
        // Platoon advantage should be first.
        assert_eq!(top2[0].player_name, "B");
        // Neutral should be second.
        assert_eq!(top2[1].player_name, "C");
    }

    #[test]
    fn roster_gaps_stub_returns_something() {
        let gaps = analyze_roster_gaps("league1", "team1");
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].category, "stub");
    }
}
