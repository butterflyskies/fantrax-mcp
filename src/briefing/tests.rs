use super::*;
use crate::types::PlayerId;

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

// ─── MLB ID resolution / unresolved-player handling ────────────────────────
//
// Fixture shapes match the live Fantrax roster endpoint (see PRs #9/#10):
// alphanumeric player IDs like "03pit", positions, and roster statuses.

fn fantrax_player(id: &str, name: &str, pos: &str, status: &str) -> RosterPlayer {
    RosterPlayer {
        player_id: PlayerId::new(id),
        name: name.to_string(),
        position: pos.to_string(),
        roster_status: status.to_string(),
    }
}

/// A boxscore containing one away batter with the given MLB person ID.
fn boxscore_with_batter(player_id: u64, name: &str) -> Boxscore {
    Boxscore {
        game_pk: 745804,
        away_team_name: "Los Angeles Dodgers".into(),
        home_team_name: "San Diego Padres".into(),
        away_batters: vec![BatterLine {
            player_id,
            name: name.into(),
            position: "DH".into(),
            at_bats: 4,
            runs: 1,
            hits: 2,
            doubles: 0,
            triples: 0,
            home_runs: 1,
            rbi: 2,
            walks: 0,
            strikeouts: 1,
            stolen_bases: 0,
            avg: ".310".into(),
        }],
        home_batters: vec![],
        away_pitchers: vec![],
        home_pitchers: vec![],
    }
}

#[test]
fn unresolvable_ids_are_skipped_not_reported_as_dnp() {
    // Captured-shape roster: Fantrax alphanumeric IDs, no MLB crosswalk.
    let players = vec![
        fantrax_player("03pit", "Shohei Ohtani", "UT", "ACTIVE"),
        fantrax_player("04ru7", "Manny Machado", "3B", "ACTIVE"),
        fantrax_player("05y7s", "Zack Wheeler", "SP", "ACTIVE"),
    ];

    let (snippets, unresolved) = snippets_from_boxscores(&players, &[]);

    // Zero fabricated DNP lines — unresolvable players must be skipped.
    assert!(
        snippets.iter().all(|s| s.line != "DNP"),
        "unresolvable players must not be reported as DNP: {snippets:?}"
    );
    assert!(snippets.is_empty(), "no snippets expected: {snippets:?}");
    assert_eq!(
        unresolved,
        vec!["Shohei Ohtani", "Manny Machado", "Zack Wheeler"]
    );
}

#[test]
fn injured_players_get_status_lines_not_unresolved() {
    let players = vec![
        fantrax_player("06jk2", "Mike Trout", "OF", "IL10"),
        fantrax_player("03pit", "Shohei Ohtani", "UT", "ACTIVE"),
    ];

    let (snippets, unresolved) = snippets_from_boxscores(&players, &[]);

    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].player_name, "Mike Trout");
    assert_eq!(snippets[0].line, "IL10");
    assert_eq!(unresolved, vec!["Shohei Ohtani"]);
}

/// Regression guard for the parse-Fantrax-as-MLB bug: an all-digit Fantrax ID
/// must NOT be treated as an MLB person ID. Here the Fantrax ID "660271"
/// collides with Shohei Ohtani's MLB ID, and Ohtani has a line in the
/// boxscore — if anyone reintroduces `player_id.parse::<u64>()`, this player
/// gets a snippet from a different player's record and the test fails.
#[test]
fn all_digit_fantrax_id_is_not_parsed_as_mlb_id() {
    let players = vec![fantrax_player("660271", "Luis Arraez", "1B", "ACTIVE")];
    let boxscores = vec![boxscore_with_batter(660271, "Shohei Ohtani")];

    let (snippets, unresolved) = snippets_from_boxscores(&players, &boxscores);

    assert!(
        snippets.is_empty(),
        "all-digit Fantrax ID must not match an MLB boxscore line: {snippets:?}"
    );
    assert_eq!(unresolved, vec!["Luis Arraez"]);
}

/// Same guard for the platoon-analysis path (`build_what_to_do`): no roster
/// player may resolve to an MLB ID via numeric parsing, even when the Fantrax
/// ID is all digits. Injured players are excluded from both lists.
#[test]
fn partition_resolvable_reports_fantrax_ids_including_all_digit_ones() {
    let players = vec![
        fantrax_player("03pit", "Shohei Ohtani", "UT", "ACTIVE"),
        fantrax_player("660271", "Luis Arraez", "1B", "ACTIVE"),
        fantrax_player("06jk2", "Mike Trout", "OF", "IL10"),
    ];

    let (resolved, unresolved) = partition_resolvable(&players);

    assert!(
        resolved.is_empty(),
        "no Fantrax ID should resolve to an MLB ID without a crosswalk"
    );
    assert_eq!(unresolved, vec!["Shohei Ohtani", "Luis Arraez"]);
}

#[test]
fn unresolved_note_counts_and_cites_crosswalk_issue() {
    assert_eq!(unresolved_note(&[]), None);

    let one = unresolved_note(&["Shohei Ohtani".to_string()]).unwrap();
    assert!(
        one.starts_with("1 roster player could not be resolved"),
        "{one}"
    );
    assert!(one.contains("see #11"), "{one}");

    let names: Vec<String> = ["Shohei Ohtani", "Manny Machado", "Zack Wheeler"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let three = unresolved_note(&names).unwrap();
    assert!(
        three.starts_with("3 roster players could not be resolved to MLB IDs"),
        "{three}"
    );
    assert!(
        three.contains("Fantrax→MLB crosswalk pending, see #11"),
        "{three}"
    );
}

// ─── Wire-contract snapshots ────────────────────────────────────────────────
//
// `Briefing` is MCP tool output consumed by an LLM client, so its serialized
// JSON shape is a wire contract. These snapshots pin the camelCase field
// names and structure — including the `unresolved_players`/`unresolved_note`
// fields added for the missing Fantrax→MLB crosswalk (see #11).

/// A deterministic, fully-populated briefing fixture. Dates are pinned; no
/// live API calls.
fn briefing_fixture() -> Briefing {
    Briefing {
        league_name: "Ho Ho Homers".to_string(),
        league_id: LeagueId::new("abc123xyz"),
        league_type: "roto".to_string(),
        date: NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(),
        what_happened: vec![
            PlayerSnippet {
                player_name: "Shohei Ohtani".to_string(),
                line: "2-4, HR, 3 RBI".to_string(),
                notable: true,
            },
            PlayerSnippet {
                player_name: "Mike Trout".to_string(),
                line: "IL10".to_string(),
                notable: false,
            },
        ],
        what_to_do: vec![ActionItem {
            urgency: Urgency::Now,
            action: "Move Mike Trout to IL or bench".to_string(),
            reason: "Mike Trout has status 'IL10' but is in an active lineup slot (OF)".to_string(),
        }],
        hot_takes: vec!["No recent recommendations in the ledger.".to_string()],
        unresolved_players: vec![],
        unresolved_note: None,
    }
}

#[test]
fn briefing_wire_shape_with_unresolved_players() {
    let unresolved_players = vec!["Manny Machado".to_string(), "Zack Wheeler".to_string()];
    let briefing = Briefing {
        unresolved_note: unresolved_note(&unresolved_players),
        unresolved_players,
        ..briefing_fixture()
    };
    insta::assert_json_snapshot!(briefing);
}

#[test]
fn briefing_wire_shape_without_unresolved_players() {
    // `unresolved_players` serializes as an empty array (it is not skipped);
    // `unresolved_note` is omitted entirely when `None`.
    insta::assert_json_snapshot!(briefing_fixture());
}

#[test]
fn merge_unresolved_dedups_preserving_order() {
    let merged = merge_unresolved(
        vec!["Shohei Ohtani".to_string(), "Manny Machado".to_string()],
        vec![
            "Manny Machado".to_string(),
            "Zack Wheeler".to_string(),
            "Shohei Ohtani".to_string(),
        ],
    );
    assert_eq!(
        merged,
        vec!["Shohei Ohtani", "Manny Machado", "Zack Wheeler"]
    );
}
