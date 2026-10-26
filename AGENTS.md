# Development guide for the Mango fork

This is the authoritative repository development guide for `jvcjesus/mango`, forked from `apixelsnet/mango`. Apply it throughout this repository. Preserve upstream mergeability and Mango's existing visual identity. This fork is becoming a self-hosted manga acquisition, discovery, and control plane; it does not replace Komga.

## Architecture and product meaning

- Mango handles discovery, source reading, follows, acquisition, native processing, and control.
- Komga is the permanent canonical manga library, library database, and reader. The canonical filesystem and Komga determine physical ownership; Mango's operational SQLite database is not a replacement library authority.
- AniList is the primary personal catalog/status tracker. MangaBaka is the preferred manga metadata/catalog and catalog presentation/cover authority where identity is authoritative, and the secondary personal tracker.
- Keiyoushi/Mihon-compatible extensions provide page/reader sources. Nyaa is a native release-discovery source. TorBox is an external acquisition/download backend.

Intended flow: extensions / sources / Nyaa → Mango discovery/read/follow/acquisition → Mango native processing → canonical manga filesystem → Komga → tracker synchronization.

Conceptual canonical destinations are English Manga at `/srv/media/manga` and Chinese Manga at `/srv/media/manga-chinese`. Use existing configuration (`PipelineConfig.library_dir`, `chinese_library_dir`, and Komga library IDs); do not hardcode deployment paths. Paths must be valid inside the actual runtime/container.

**Library** means physically owned/local canonical content. **Lists** means the personal tracked catalog, independent of ownership. Tracked does not mean owned or permission to download. Reading/tracker status is separate from ownership. Acquiring means an active Mango acquisition. Future Auto-Acquire must be explicit opt-in and default false. Importing a tracker catalog must never unexpectedly download it.

## Repository map and inspection workflow

Before editing any subsystem, inspect its implementation, relevant tests, migrations, and useful recent history; determine whether the abstraction already exists. Start with `git status`, `git log`, `README.md`, `docs/features/`, and `.github/workflows/ci.yml`. The README includes fork-native pipeline and synchronization documentation; some feature documents still describe upstream behavior and historical acceptance results. Verify claims against current code.

| Location | Responsibility |
| --- | --- |
| `mango-reader/` | Rust 2021 server crate; Axum/Tokio, SQLite via rusqlite, embedded web UI |
| `mango-reader/src/main.rs`, `api.rs` | Startup/wiring and HTTP surfaces |
| `mango-reader/src/acquisition/` | Native candidates/providers, Nyaa/TorBox, service, selective collection mapping, profiles, missing-content intelligence, planning and approved execution |
| `mango-reader/src/downloads.rs`, `transfer.rs` | Queue/workers, durable transfer lifecycle, retry/stall/resume and restart recovery |
| `mango-reader/src/pipeline/` | MangaBaka matching, shared metadata policy, optimization, CBZ validation, publication, Komga ownership/verification, existing-library normalization |
| `mango-reader/src/sync/` | Native Komga reading progress → AniList/MangaBaka, identities/mappings, totals, catalog/lists, scheduling and audit |
| `mango-reader/src/trackers/` | Existing upstream reader-driven tracker integrations; inspect alongside native sync to avoid competing writers |
| `mango-reader/src/db.rs`, `mango-reader/migrations/` | Ordered embedded SQLite migrations and operational state |
| `mango-reader/ui/src/routes/` | Svelte 5 surfaces, including Lists, Library, Downloads and Settings |
| `mango-reader/ui/src/lib/`, `src/app.css` | API client, shared components, theme and surface tokens |
| `mango-extension-host/` | Separate JVM/Kotlin extension host; protocol in `protocol/openapi.yaml` |
| `mango-desktop/` | Separate Rust desktop shell crate |

There is no root Cargo workspace manifest: run Rust commands in the relevant crate. Migrations currently run through `025_normalization_safety.sql` and are explicitly included in `src/db.rs`. Relevant groups: 012–015 sync/provenance/lists; 016–020 verification, Nyaa/TorBox, collections, planning/execution; 021 metadata provenance; 022 normalization/history; 023 transfer state; 024 durable magnet provenance; 025 saved normalization previews and durable backup/recovery fields. Add forward-safe migrations and register them in order for future schema changes; preserve user data and never rewrite applied migrations to change existing installations.

Keep changes focused and commits logically isolated. Extend existing abstractions rather than create parallel systems. Avoid unrelated cleanup, gratuitous dependencies, and stylistic rewrites of working systems.

## Acquisition and recovery invariants

Preserve the native pipeline. It already has chapter and first-class volume acquisition, extension/source and Nyaa discovery, TorBox, selective multi-volume acquisition, approved plans, durable history, canonical naming, English/Chinese routing, duplicate protection, metadata enrichment, ComicInfo rewriting, optimization, archive validation, atomic publication, targeted Komga scans, exact ownership verification and recovery. Extend these systems rather than introducing a second pipeline.

**Selection is authoritative at the backend → Mango retrieval boundary.** For one selected volume in a collection, the external backend may cache the collection, but only that selected unit may be retrieved into Mango and processed/published. Never silently retrieve/process/publish the entire collection. Fail closed on ambiguous selection; retain exact durable provenance and deterministic external-file re-resolution. Do not guess or silently substitute a different candidate.

Preserve safe `.part` data and resumable transfers:

- Valid HTTP 206: append/resume safely.
- Range ignored with HTTP 200: safely restart at byte zero.
- HTTP 416 with a valid complete CBZ: accept completion.
- HTTP 416 with incomplete/invalid data: safely reset/recover.

Keep temporary backend/network failures recoverable, retries bounded, stall detection and startup recovery intact, and workers/external jobs deduplicated. Recover expired TorBox jobs from durable acquisition provenance when sufficient evidence exists. Do not clean up a shared external job while incomplete sibling acquisitions depend on it. Preserve approved-plan authorization, idempotent queueing, durable history and exact Komga verification; publication alone does not establish verified ownership.

## Metadata and existing-library maintenance

Use MangaBaka where identity is authoritative; fail closed on ambiguous metadata identity and never invent metadata. Preserve stable MangaBaka/AniList cross-references. Reuse `pipeline/metadata.rs` and existing matching/normalization abstractions across acquisition, maintenance and catalog presentation, including human-readable genre/tag normalization and deduplication.

ComicInfo rewriting must leave exactly one root `ComicInfo.xml`. Validate CBZ contents before canonical publication. Do not reopen volume numbering/Komga ordering without a current reproducible bug: current generated volumes work in Komga. Preserve chapter/true-volume distinctions and do not use source chapter counts as volume totals.

Existing pre-Mango books must be normalizable without redownloading. Extend `pipeline/normalization.rs` and its durable history/rollback machinery. Follow dry run → explicit mutation → safe staging → archive validation → atomic replacement → durable history/rollback. Normalization must be idempotent; never modify the only canonical copy unsafely in place, and use appropriate locking.

Komga's separate **Comics** library must not be forced through MangaBaka. With no applicable provider, report provider-not-applicable/skipped (the existing `SkippedProviderNotApplicable` status), not unmatched/error. Preserve provider extensibility; do not implement a comics provider unless requested. Treat Chinese-library bulk normalization conservatively: English MangaBaka normalization policy is not automatically valid for Chinese Manga and needs explicit policy validation.

## Tracker synchronization

The intended native direction is Komga reading progress → Mango → AniList / MangaBaka. AniList remains primary and MangaBaka secondary. Preserve never-decrease semantics unless explicitly redesigned with strong justification, dry-run support, fail-closed identity resolution and audit history. Avoid simultaneous writers, including the existing reader-driven `trackers/` path and external/legacy services. Do not silently enable scheduled tracker mutation during migration.

## UI and security

Preserve Mango's visual language. Improve Lists, Library, Downloads, Settings, Maintenance and existing notifications/queues before adding dashboard sprawl. Maintenance currently lives within existing UI/API surfaces; do not assume a separate route is needed. Backend state is authoritative; present it instead of recreating business logic in the frontend. Reuse theme/surface tokens and shared components. Preserve accessibility, keyboard behavior, focus, responsive layouts and readable contrast.

Use existing secure configuration mechanisms. Never expose API tokens/secrets in logs, frontend payloads, test snapshots, commits or documentation examples. Never ask the user to paste secrets or commit real credentials.

## Validation commands and evidence

Inspect current package scripts and CI before choosing checks. For applicable implementation work, run Rust formatting, tests and check/lint, frontend tests and production build. Commands from the repository root:

```sh
# UI: CI uses Node 22. Install only when needed.
(cd mango-reader/ui && npm ci --no-audit --no-fund)
(cd mango-reader/ui && npm run test)
(cd mango-reader/ui && npm run check)
(cd mango-reader/ui && npm run build)

# Core: build the UI first so the server embeds the actual production UI.
(cd mango-reader && cargo fmt --all -- --check)
(cd mango-reader && cargo test --release --locked)
(cd mango-reader && cargo check --locked)
# Additional standard Rust lint when applicable; CI currently has no Clippy gate.
(cd mango-reader && cargo clippy --all-targets --locked)

# When the desktop shell is affected (requires platform system libraries):
(cd mango-desktop && cargo check --locked)
```

CI currently runs UI check/build, core release tests and desktop check; it does not run Vitest, rustfmt or Clippy. `mango-reader/build.rs` creates a placeholder `ui/dist/index.html` if none exists; a successful Rust build alone does not validate the real UI. Avoid broad formatting mutations in an unrelated change.

Core regression coverage is largely inline (`#[cfg(test)]`), plus `acquisition/tests.rs` and `sync/tests.rs`; transfer tests use local mocked HTTP behavior. Frontend tests currently live in `ui/src/lib/reliability.test.ts` and run with Vitest. Inspect relevant tests before editing and add deterministic bug regressions where practical. External integrations need deterministic mocks plus a separately documented live acceptance procedure when real services are necessary.

User-reported starting baseline: approximately 314 passing Rust tests, 65 passing frontend tests, passing UI production build, and approximately 45 pre-existing UI source type-check errors. These are contextual figures, not freshly verified results or permanent expected counts. Record actual results; distinguish existing type-check failures from regressions and introduce no additional errors. Documentation-only changes need only lightweight accuracy/path/script checks and `git diff --check`; do not run the implementation suite just to validate prose.

## Deployment boundaries

Normal implementation tasks operate on the repository only. Unless explicitly authorized by the task, do not deploy, SSH into production, modify canonical media, disable legacy services, mutate real trackers, bulk-normalize a real library, manipulate production databases, or require production secrets. Implementation and live acceptance are separate phases.

## Roadmap and completion reports

v1.0 priorities, in order: fix remaining UI regressions; validate/harden existing-library normalization; finalize native tracker sync; validate unattended/background operation; add/audit reconciliation; complete production-readiness acceptance coverage; prepare controlled production migration; burn in the fork; safely retire replaced legacy processing components. Code existence does not complete v1.0: real acceptance evidence and successful production migration are required.

Later roadmap: v1.1 richer tracked/owned/reading/acquiring/auto-acquire semantics; v1.2 missing-content intelligence; v1.3 acquisition profiles; v1.4 source intelligence; v1.5 metadata review; v1.6 richer Downloads lifecycle/history; v1.7 library health; v1.8 update feed; v1.9 notifications; v2.0 a cohesive self-hosted manga automation platform around Mango + Komga + AniList + MangaBaka. Some foundations already exist in code; roadmap labels do not imply those subsystems should be recreated. Komga remains canonical.

Future implementation reports should concisely state root cause/design rationale, files changed, migrations and tests added/changed, Rust/frontend test and frontend build results, pre-existing failures versus regressions, commit SHA(s), final HEAD, whether `origin/main` matches when pushing was requested, worktree status, and exact remaining live-validation steps. Report unrun checks and blocked operations honestly.
