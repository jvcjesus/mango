# Canonical Ownership Semantics and Release Intelligence

This feature establishes a single canonical definition of physical ownership and unifies title-level library and release presentation.

## 1. Single Canonical Authority for Physical Ownership

Physical ownership has exactly one definition throughout Mango: **a logical volume is owned if and only if verified physical media exists in the canonical Komga library.**

- Operational acquisition records, queued downloads, or retrieval states are not physical media. A volume in acquisition or processing is acquiring, not owned.
- If physical CBZ media exists in Komga, that volume is unconditionally owned. Upstream metadata review states, KOMF retries, or enrichment warnings never revoke or decrement verified physical ownership.
- The UI never contradicts itself: the physical ownership count and volume ranges match across all badges, summaries, and details.

## 2. Market-Accurate Denominator Semantics

When evaluating physical collection completeness:

- The denominator `released_volumes_count` is strictly drawn from the target acquisition market (typically English `en`).
- Japanese release schedules (`ja`) or other upstream schedules represent catalog release intelligence only; they are strictly secondary and never become the denominator for an English physical library.
- For example, if Japanese GCD reports 16 volumes released and English OpenTome reports 14 volumes released, an English collection with 14 verified volumes is `14 / 14 released volumes owned · Up to date`. The missing count is `0`, not `2`.

## 3. Upcoming Releases vs Missing Volumes

Upcoming volumes with release dates strictly in the future are classified as **upcoming**:

- Upcoming volumes are displayed with their release date (e.g. `Next: Vol. 9 · Nov 3, 2026`).
- Upcoming volumes are never counted in `missing_released_count` or treated as missing physical media.
- On the calendar release date (`today`), if verified media is absent, the volume automatically transitions to released/missing and becomes eligible for release discovery.

## 4. Up to Date vs Complete

A healthy library whose owned volumes equal the released count is presented with precise lifecycle awareness:

- **Ongoing series:** When all currently released volumes are owned and the series is ongoing, the library status is `✓ Up to date` (`{owned} / {released} released volumes owned · Up to date`). The primary action is neutral/none; no misleading "Discover releases" CTA is shown when there are zero missing released volumes.
- **Completed series:** Only when the series is authoritatively completed by the author/publisher and all released volumes are owned does the status present `✓ Library complete`.
- **Secondary schedules:** In ongoing series where Japanese GCD has no upcoming volume scheduled, the secondary schedule is formatted as `JA · {total} released` without falsely appending `Complete`.

## 5. Unified Ownership and Release Module

Title view (`Title.svelte`) unifies all Komga ownership, OpenTome schedules, and acquisition intelligence into a single `Library & Releases` card:

- Redundant, conflicting standalone ownership panels are retired.
- Under `Advanced actions`, an expandable `Ownership & Provenance Details` block provides full technical auditability: matched Komga series ID, library ID, owned volume ranges, missing volume ranges, and backend provenance explanations.
- If series identity is ambiguous, authoritative volume metrics and missing counts are suppressed in favor of a clear `Resolve Identity` action.
