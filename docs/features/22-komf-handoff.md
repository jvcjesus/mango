# Mango → KOMF metadata handoff

The optional acquisition handoff is disabled by default. Enable
`pipeline.komf_enabled` and configure `pipeline.komf_url` in the existing Pipeline
settings after configuring Komga and the English target library. The URL must be
reachable from Mango's runtime/container; it may include a reverse-proxy prefix.
No password/token is added to this integration. URL credentials, queries and
fragments are rejected. The handoff does not change production deployment or KOMF.

Mango keeps acquisition, physical construction, atomic publication and identity
provenance. KOMF receives the final Komga metadata/artwork operation. Existing
Mango enrichment needed for canonical construction/identity remains in place
before publication. There is no new MangaBaka lookup or archive writer here.

## Publication and exact book identity

After publication and durable provenance, Mango requests the existing targeted
Komga scan. When handoff is enabled, exact-book polling replaces the boolean-only
verification step for English volumes. Search results are discovery, not mutation
authority. Every candidate series must belong to the expected configured library.
Mango follows all pages and requires a unique book with the exact release unit
and parsed number. Conflicting book library, series or explicit metadata numbering
fails closed. Chapters are currently recorded for review rather than handed off.

Strong evidence is either the Komga filesystem URL resolving to the exact
published canonical CBZ or a unique authoritative persisted Komga series mapping
plus exact unit/number when no file URL is available. Reported contradictory file
paths are never overridden by number matching. The filesystem comparison reuses
normalization's canonical-root resolver, including arbitrary host/container root
translation, traversal rejection and symlink confinement. It canonicalizes paths
for comparison only; it never changes a CBZ. Missing or duplicate candidates do
not result in a KOMF request. Chinese destinations never receive an automatic
English handoff.

MangaBaka identity comes from acquisition_history for that chapter/publication,
then the resolved MangaBaka tracker mapping for the exact Komga series. Existing
resolved mappings can supply an otherwise absent ID. Conflicting positive IDs,
ambiguous confidence, invalid IDs or ambiguous persisted metadata status require
review. Title search is never used to acquire a MangaBaka identity.

## Exact KOMF protocol

The contract was verified against jvcjesus/komf commit
63ad97c9f741ea45bf65b570c03b69e4583dea6a, specifically MetadataRoutes, JobRoutes,
KomfIdentifyBookRequest and KomfMetadataJob. Only this mutation endpoint is used:

```http
POST {pipeline.komf_url}/api/komga/metadata/identify/book
```

```json
{"bookId":"EXACT_BOOK_ID","provider":"MANGA_BAKA","providerSeriesId":"AUTHORITATIVE_SERIES_ID","libraryId":"EXPECTED_LIBRARY_ID"}
```

Mango stores `jobId` and polls `GET /api/jobs/{jobId}`. The returned `id` and
`seriesId` must match the persisted job and Komga series. RUNNING is pending,
FAILED is terminal failure and COMPLETED is terminal job success. Missing,
unrecognized or mismatched responses and HTTP failures are unknown, never success.
Each request has a 15-second timeout. Polling uses the existing bounded pipeline
verification policy/backoff (60 seconds by default). Redirects and automatic POST
retries are disabled. Server response bodies/errors are not copied into messages,
which avoids leaking arbitrary credential-bearing error text.

KOMF COMPLETED confirms the metadata job, not necessarily artwork: the existing
KOMF contract treats some artwork failures as optional. Metadata patch and cover
upload are separate non-atomic operations. Mango does not claim independent cover
verification or blindly resubmit merely to repair a cover. No series identify,
series analyze, book analyze or post-KOMF scan is invoked by this handoff.

## Durable delivery and recovery

Migration 028 adds metadata_handoffs, keyed by chapter_id. It records the canonical
publication path, state/message, KOMF URL, exact Komga book/series/library IDs,
authoritative MangaBaka ID and KOMF job ID. MangaBaka work/collection provenance
continues to live in existing acquisition_history fields; no parallel identity
mapping database is introduced. The handoff is exposed on existing queue and
acquisition-history API records as metadataHandoff.

States:

| State | Meaning and recovery |
| --- | --- |
| waiting_komga | Published; exact book not visible. Existing verification worker retries indexing/targeted scan. |
| needs_review | Ambiguous/missing identity, unsupported unit, or conflicting book context. No POST. |
| submitting | Persisted before POST. A crash here is uncertain; automatic resubmission is blocked. |
| pending | A job ID is durable. Resume polling that ID. |
| unknown + job ID | Timeout/unavailable/unrecognized status. Retain ID and poll again; no new POST. |
| unknown without job ID | Request may have reached KOMF. Reconcile externally; resubmission is blocked. |
| unavailable | Connection establishment failed before submission. Explicit metadata retry is allowed. |
| rejected | Explicit validation/authentication/route rejection. Explicit metadata retry is allowed. |
| failed | Known terminal job failure. Only explicit metadata retry may create another job. |
| succeeded | Idempotent completion; never POST again for that publication. |

HTTP 408, 5xx, redirects and malformed successful submission responses are
uncertain. Only definite validation/auth/route rejection codes are classified as
rejected. A durable compare-and-set claim prevents concurrent duplicate POSTs.
The submitting marker intentionally favors avoiding duplication over automatic
recovery in the crash window before the response ID is saved.

Published-file restart recovery resumes a known job before requesting another
scan or resolving/downloading any source. It does not redownload, optimize or
republish the CBZ. Terminal/review failures remain published and are parked
separately from physical acquisition failure; pending jobs resume through bounded
metadata-only polling. Publication history remains durable after queue completion.

The Downloads UI exposes metadata status and a retry/check-job action:

```http
POST /api/downloads/{chapter}/retry-metadata
```

This reads the existing publication, optionally checks archive integrity read-only,
and retries/resumes only handoff. Generic retry and retry-Komga actions redirect
to this path when a handoff record exists. Known jobs are always polled first;
uncertain submissions without an ID cannot be cleared by pressing retry. A
failed job may have applied partial metadata, so explicit retry is an intentional
repeat, not an assumed atomic rollback. Correct missing/conflicting authoritative
provenance before retrying a Needs Review item.

## Boundaries and remaining limitations

The handoff never opens a canonical file for writing, injects ComicInfo/artwork,
renames files, optimizes pages or deletes valid media. It cannot invalidate physical
publication because metadata failed. Existing construction and approved manual
normalization remain separate capabilities; this change does not disable them.
KOMF owns final Komga metadata/artwork, with no later Mango analyze/refresh action
from this workflow. The feature is opt-in and English-volume-only. No authenticated
KOMF proxy contract is implemented; deployments requiring authentication need an
explicit extension rather than credentials embedded in URLs.

Metadata-only requests can run until the bounded poll expires. Inspect persisted
state after a client disconnect. An unknown submission without a job ID requires
operator reconciliation in KOMF; no unsafe reset/resubmit button is provided.
The existing disabled-feature ownership path is retained for compatibility and
continues to use its prior boolean verification API.

Tests use local mock HTTP servers and temporary archives/databases. They cover
publication/scan ordering, exact payload/volume isolation, delayed indexing,
zero/multiple books, identity failures, missing/failed/unknown jobs, response loss,
explicit retry, real Downloads-worker restart, no duplicate submissions,
concurrent claims, arbitrary filesystem roots/nested punctuation and escapes,
library/unit conflicts, and unchanged archive bytes. Production deployment and
live acceptance are separate; nothing is enabled automatically by this commit.

## Validation

This change adds 17 deterministic handoff tests. The full locked release suite
passes 426 tests, including existing acquisition, Komga verification, normalization
and sync tests. UI tests pass 86 tests and the UI production build succeeds.
`cargo check --locked` and `cargo clippy --all-targets --locked` pass; clippy has
warnings including two new argument-count lints for the explicit publication/
resolution context and a new collapsible-if lint, in addition to existing warnings.
Repository-wide formatting remains at the
pre-existing 921 diff hunks; the new module and touched code were formatted.
UI type checking retains the existing 31 errors across three files and introduces
none in the new controls. No production deployment or live service mutation was
performed, and the KOMF repository was not modified.
