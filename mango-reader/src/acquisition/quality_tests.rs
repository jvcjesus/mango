//! Unit tests for M6.11 Candidate Quality Intelligence & Release Selection.

use crate::acquisition::approval::PersistedCandidate;
use crate::acquisition::quality::*;

fn make_candidate(
    candidate_id: &str,
    provider: &str,
    raw_title: &str,
    match_type: &str,
    is_digital: bool,
    is_trusted: bool,
    uploader: Option<&str>,
    seeders: Option<u32>,
) -> PersistedCandidate {
    PersistedCandidate {
        series_id: "title:frieren".to_string(),
        volume_number: 1.0,
        candidate_id: candidate_id.to_string(),
        provider: provider.to_string(),
        provider_item_id: "item_1".to_string(),
        title: raw_title.to_string(),
        raw_title: raw_title.to_string(),
        match_type: match_type.to_string(),
        size_bytes: Some(150_000_000),
        uploader: uploader.map(|s| s.to_string()),
        is_trusted,
        is_digital,
        score: 80.0,
        score_explanation: "Test score".to_string(),
        download_url: Some("https://example.com/dl".to_string()),
        magnet_url: None,
        info_hash: None,
        seeders,
        leechers: Some(2),
        published_at: Some(1700000000),
    }
}

#[test]
fn test_quality_scoring_digital_and_trusted_bonuses() {
    let cand = make_candidate(
        "cand-nyaa-1",
        "nyaa",
        "[danke-Empire] Frieren v01 (Digital) (2048px)",
        "exact_volume",
        true,
        true,
        Some("danke-Empire"),
        Some(25),
    );

    let eval = evaluate_candidate_quality(&cand, false, None);

    assert_eq!(eval.provider, "nyaa");
    assert!(eval.is_digital);
    assert!(eval.is_trusted);
    assert!(eval.is_quality_group);
    assert_eq!(eval.health_rating, "EXCELLENT");
    assert_eq!(eval.resolution_label.as_deref(), Some("2048px (Ultra-HD)"));

    // Expected score breakdown:
    // Match type: 60
    // Digital: 15
    // Trusted: 10
    // Quality group: 5
    // Resolution (2048px): 5
    // Seeds (>=20): 10
    // Total: 105.0
    assert_eq!(eval.total_score, 105.0);
    assert_eq!(eval.selection_verdict, "ALTERNATIVE"); // Nyaa is alternative when Madokami is evaluated
}

#[test]
fn test_quality_provider_preference_madokami_over_nyaa() {
    let cand_nyaa = make_candidate(
        "cand-nyaa-1",
        "nyaa",
        "Frieren v01 (Digital)",
        "exact_volume",
        true,
        false,
        None,
        Some(10), // +5 seeds
    );

    let cand_madokami = make_candidate(
        "cand-mado-1",
        "madokami",
        "Frieren v01 (Digital).cbz",
        "exact_volume",
        true,
        false,
        None,
        None, // HTTP direct
    );

    let mut evals = vec![
        evaluate_candidate_quality(&cand_nyaa, false, None),
        evaluate_candidate_quality(&cand_madokami, false, None),
    ];

    // Madokami gets +10 for preferred direct HTTP provider
    assert!(evals[1].is_preferred_provider);
    assert_eq!(evals[1].health_rating, "EXCELLENT");

    rank_quality_evaluations(&mut evals);

    // Rank 1 must be Madokami due to provider preference policy
    assert_eq!(evals[0].candidate_id, "cand-mado-1");
    assert_eq!(evals[0].rank, 1);
    assert_eq!(evals[0].selection_verdict, "PREFERRED");

    // Rank 2 is Nyaa
    assert_eq!(evals[1].candidate_id, "cand-nyaa-1");
    assert_eq!(evals[1].rank, 2);
    assert_eq!(evals[1].selection_verdict, "ALTERNATIVE");
}

#[test]
fn test_quality_pre_release_penalty() {
    let clean = make_candidate(
        "cand-clean",
        "madokami",
        "Frieren v01.cbz",
        "exact_volume",
        false,
        false,
        None,
        None,
    );

    let pre_release = make_candidate(
        "cand-pre",
        "madokami",
        "Frieren v01 (PRE).cbz",
        "exact_volume",
        false,
        false,
        None,
        None,
    );

    let mut evals = vec![
        evaluate_candidate_quality(&pre_release, false, None),
        evaluate_candidate_quality(&clean, false, None),
    ];

    assert!(evals[0].is_pre_release);
    // Pre-release gets -20 penalty
    assert!(evals[0].factors.iter().any(|f| f.name == "Pre-Release Penalty"));

    rank_quality_evaluations(&mut evals);

    // Clean release ranks #1 over pre-release
    assert_eq!(evals[0].candidate_id, "cand-clean");
    assert_eq!(evals[1].candidate_id, "cand-pre");
}

#[test]
fn test_no_silent_upgrades_when_volume_is_owned() {
    let high_quality_cand = make_candidate(
        "cand-upgrade",
        "madokami",
        "[danke-Empire] Frieren v01 (Digital) (2048px).cbz",
        "exact_volume",
        true,
        true,
        Some("danke-Empire"),
        None,
    );

    // When is_owned = true:
    let eval = evaluate_candidate_quality(&high_quality_cand, true, None);

    // Strict No Silent Upgrade Invariant:
    assert!(eval.upgrade_evaluation.is_currently_owned);
    assert!(eval.upgrade_evaluation.silent_upgrade_blocked);
    assert!(eval.upgrade_evaluation.upgrade_eligible);
    assert_eq!(eval.selection_verdict, "MANUAL_UPGRADE_ONLY");
    assert!(eval
        .upgrade_evaluation
        .upgrade_justification
        .as_ref()
        .unwrap()
        .contains("manual upgrade"));
}

#[test]
fn test_detect_resolution_label() {
    assert_eq!(
        detect_resolution_label("Dungeon Meshi v01 (2048px)").as_deref(),
        Some("2048px (Ultra-HD)")
    );
    assert_eq!(
        detect_resolution_label("Dungeon Meshi v01 (1920x1080)").as_deref(),
        Some("1920px (Full HD)")
    );
    assert_eq!(
        detect_resolution_label("Dungeon Meshi v01 (1600px)").as_deref(),
        Some("1600px (High-Res)")
    );
    assert_eq!(
        detect_resolution_label("Dungeon Meshi v01 (Digital)").as_deref(),
        Some("Digital Archival")
    );
    assert_eq!(detect_resolution_label("Dungeon Meshi v01"), None);
}
