//! Unit tests for M6.11 / M8.4 Candidate Quality Intelligence & Release Selection.

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
        "[danke-Empire] Frieren v01 (Digital) (2048px).cbz",
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
    assert_eq!(eval.format, "CBZ");
    assert_eq!(eval.edition, "Digital");
    assert!(eval.size_per_page_kb.is_some());

    // Expected normalized two-stage score breakdown:
    // Quality (95.0 * 0.35 = 33.25)
    // Reliability (80.0 * 0.25 = 20.0)
    // Preference (100.0 * 0.20 = 20.0)
    // Convenience (50.0 * 0.15 = 7.5)
    // Storage (100.0 * 0.05 = 5.0)
    // Final: 85.8
    assert_eq!(eval.total_score, 85.8);
    let rb = eval.ranking_breakdown.as_ref().unwrap();
    assert_eq!(rb.confidence_percent, 80);
    assert_eq!(rb.evidence_coverage_percent, 80);
    assert!(!rb.verified_evidence.is_empty());
    assert!(!rb.heuristic_estimates.is_empty());
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
    assert_eq!(evals[0].selection_verdict, "RECOMMENDED");

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

#[test]
fn test_scoring_penalties_chapter_size_language() {
    // 1. Chapter-only penalty (-50)
    let cand_ch = make_candidate(
        "cand-ch",
        "nyaa",
        "Frieren ch. 01.cbz",
        "chapter",
        false,
        false,
        None,
        Some(10),
    );
    let eval_ch = evaluate_candidate_quality(&cand_ch, false, None);
    assert!(eval_ch.factors.iter().any(|f| f.points == -50.0 && f.name == "Match Type"));
    assert!(eval_ch.warning_badges.contains(&"Chapter only".to_string()));
    assert_eq!(eval_ch.selection_verdict, "BLOCKED");

    // 2. Unwanted language penalty (-50)
    let cand_fr = make_candidate(
        "cand-fr",
        "nyaa",
        "[FR] Frieren v01.cbz",
        "exact_volume",
        false,
        false,
        None,
        Some(10),
    );
    let eval_fr = evaluate_candidate_quality(&cand_fr, false, None);
    assert!(eval_fr.factors.iter().any(|f| f.name == "Unwanted Language" && f.points == -50.0));
    assert!(eval_fr.warning_badges.iter().any(|b| b.contains("French")));
    assert_eq!(eval_fr.selection_verdict, "BLOCKED");

    // 3. Extreme size anomaly penalty (-30)
    let mut cand_huge = make_candidate(
        "cand-huge",
        "nyaa",
        "Frieren v01.cbz",
        "exact_volume",
        false,
        false,
        None,
        Some(10),
    );
    cand_huge.size_bytes = Some(4_200_000_000); // 4.2 GB
    let eval_huge = evaluate_candidate_quality(&cand_huge, false, None);
    assert!(eval_huge.factors.iter().any(|f| f.name == "Extreme Size Anomaly" && f.points == -30.0));
    assert!(eval_huge.warning_badges.contains(&"Size anomaly".to_string()));
    assert!(eval_huge.rejection_reason.as_ref().unwrap().contains("exceeds expected volume size"));

    // 4. Format CBR warning
    let cand_cbr = make_candidate(
        "cand-cbr",
        "nyaa",
        "Frieren v01.cbr",
        "exact_volume",
        false,
        false,
        None,
        Some(10),
    );
    let eval_cbr = evaluate_candidate_quality(&cand_cbr, false, None);
    assert_eq!(eval_cbr.format, "CBR");
    assert!(eval_cbr.warning_badges.iter().any(|b| b.contains("CBR")));
    assert!(eval_cbr.rejection_reason.as_ref().unwrap().contains("requires conversion"));
}

#[test]
fn test_deduplicate_candidate_evaluations() {
    let cand_mado = make_candidate(
        "cand-mado",
        "madokami",
        "[danke-Empire] Frieren v01.cbz",
        "exact_volume",
        true,
        true,
        Some("danke-Empire"),
        None,
    );
    let cand_nyaa = make_candidate(
        "cand-nyaa",
        "nyaa",
        "[danke-Empire] Frieren v01 [CBZ]",
        "exact_volume",
        true,
        true,
        Some("danke-Empire"),
        Some(15),
    );

    let candidates = vec![cand_mado.clone(), cand_nyaa.clone()];
    let evals = vec![
        evaluate_candidate_quality(&cand_mado, false, None),
        evaluate_candidate_quality(&cand_nyaa, false, None),
    ];

    let (deduped, dup_count) = deduplicate_candidate_evaluations(evals, &candidates);

    assert_eq!(dup_count, 1);
    assert_eq!(deduped.len(), 1);
    let primary = &deduped[0];
    assert_eq!(primary.candidate_id, "cand-mado"); // Madokami preferred
    assert_eq!(primary.alternate_sources.len(), 1);
    assert_eq!(primary.alternate_sources[0].candidate_id, "cand-nyaa");
    assert_eq!(primary.alternate_sources[0].provider, "nyaa");
}

#[test]
fn test_side_by_side_comparison_matrix() {
    let cand_mado = make_candidate(
        "cand-mado",
        "madokami",
        "[danke-Empire] Frieren v01.cbz",
        "exact_volume",
        true,
        true,
        Some("danke-Empire"),
        None,
    );
    let cand_nyaa = make_candidate(
        "cand-nyaa",
        "nyaa",
        "Frieren v01.cbr",
        "exact_volume",
        false,
        false,
        None,
        Some(1),
    );

    let candidates = vec![cand_mado.clone(), cand_nyaa.clone()];
    let evals = vec![
        evaluate_candidate_quality(&cand_mado, false, None),
        evaluate_candidate_quality(&cand_nyaa, false, None),
    ];

    let matrix = build_candidate_comparison_matrix("title:frieren", 1.0, &evals, &candidates, 0);

    assert_eq!(matrix.candidates.len(), 2);
    assert!(matrix.preference_explanation.contains("Candidate #1"));
    assert!(matrix.preference_explanation.contains("preferred over Candidate #2"));

    // Check advantages and disadvantages
    let first = &matrix.candidates[0];
    assert!(first.advantages.iter().any(|a| a.contains("Direct HTTP")));
    assert!(first.advantages.iter().any(|a| a.contains("digital")));

    let second = &matrix.candidates[1];
    assert!(second.disadvantages.iter().any(|d| d.contains("conversion") || d.contains("BitTorrent")));
}
