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
