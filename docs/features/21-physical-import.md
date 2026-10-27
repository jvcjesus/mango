# Physical acquisition and import

Starting state: clean `work`, local/remote 44c0dcf. Existing migrations 001–026.
Tests: UI test/check/build; core release tests/check/Clippy/fmt; local fixtures only.

## Audit before implementation

| Existing gate/path | Decision | Reason |
|---|---|---|
| Lists/Title → AcquisitionDialog → interactive saved plan | REPLACE primary approval semantics | Physical payload selection is not an import assignment |
| Nyaa/source title ranking | WARNING | Useful discovery suggestions, not byte identity |
| source_units filename edition/unit eligibility | WARNING | Parsing cannot describe nested ZIP contents |
| planner catalog missing volumes | compatibility only | May propose content; cannot own a selected payload |
| profile size, seeds, disk/destination/worker locks | KEEP HARD | Resource and filesystem policy |
| metainfo exact path vs backend exact path | REPLACE for physical imports | Backend paths may include/exclude the torrent root; use unique normalized suffix plus size, never IDs |
| transfer range/retry/recreation | KEEP HARD | Proven retrieval machinery |
| pre-transfer CBZ page requirement | MOVE | A valid payload may contain books, not direct pages |
| downloaded archive validation | KEEP HARD | Corruption/traversal/resource limits |
| downloaded bytes → candidate manifest | NEW explicit stage | Inspect actual packaging once |
| filename volume/edition parser | WARNING | Suggest assignments only |
| user assignment | authoritative | Persist before scheduling candidate processing |
| metadata provider resolution | WARNING/fallback | Cannot veto an approved import |
| optimizer, resulting CBZ validation, atomic publisher | KEEP HARD | Reuse the existing processing back half |
| Komga verification and tracker boundaries | KEEP HARD | Publication is not verified ownership |

The reported immediate error originates in the approved-file exact path/size
lookup. That branch returns before the bibliographic resolver; it does not prove
that another volume parser rejected the selection. The old lookup collapses zero
matches and duplicate matches into `ambiguous_files`. A torrent-root namespace
difference, actual path/size change, or indistinguishable duplicates can therefore
produce the same message. Without the live approved/backend manifests, the exact
cause of that TEST instance is unverified. The zero-attempt failure was a selection
failure before transfer. This milestone replaces the authority model: the primary
path retrieves a selected physical payload independently of bibliographic
assignments, inspects it, and persists books and assignments afterward.

## Concrete model

Migration 027 adds payload inspection and candidate rows, keyed by download chapter
and physical inner locator. Existing release/torrent provenance stays in retrieval
JSON and downloads. Payload → inspected → needs review; candidates → needs_review,
queued, processing, ignored. Payload cancellation and cleanup are persisted too.
Assigned candidates use existing independent chapter/download jobs,
then normal processing/publication/verification. User-confirmed typed assignments
are separate SQL columns, not inferred from filenames during restart.

Inspection is bounded to two nested archive levels, 10,000 entries, 64 GiB expanded
bytes per inspection, and a 200:1 compression ratio. No arbitrary recursive unzip.
Inner files are staged under generated names, never source paths. Covers/Extras
are review/ignored material, not invented numbered volumes. Unknown books remain
Needs Review. The outer payload and extracted candidate staging are retained until
explicit cleanup; no sibling cleanup races or repeated outer downloads.

## Implemented authority boundaries

The native shared dialog now approves physical payloads with Acquire & Inspect,
not library numbers. Metainfo exposes outer files only; it never pretends to expose
ZIP internals. Each payload gets a chapter/job key derived from its physical
selection, not its suggested volume. Two v04 alternatives can therefore be
retrieved independently when explicitly selected. Native retrieval calls the
physical payload matcher directly; the bibliographic resolver is compatibility
code, not the primary execution path. Exact source hash corroboration is required
when a durable torrent hash is available. A unique root-relative path/size match
may survive backend-root changes and disposable IDs. Missing, size-changed and
indistinguishable physical files have distinct payload error codes.

`import_payloads` retains outer location, source hash and immutable retrieval
provenance even after queue removal. `import_candidates` retains physical inner
locator, staging path, suggestions, explicit series/kind/number/edition, approval
time, cancellation and processing lease, and the child chapter linkage. Its
library assignment is frozen into the existing retrieval JSON when queued.
Source provenance and assignment columns are separate. No catalog IDs own the
candidate. Suggestions can be edited (including a v01 suggestion assigned as v05).

Downloads exposes Assign & Import, Ignore, guarded Retry Inspection and explicit
staging cleanup. Unambiguous numbered suggestions can be batch-approved; duplicate
alternatives and unnumbered candidates are excluded from that batch. Unknown
numbers stay null, never implicitly become zero. No automatic acquisition policy
is enabled: all imports require explicit assignment approval in this milestone.
Assignment can select an existing target series or create its normal manga row.

A candidate processing lease protects shared staging from cleanup. Restart resets
interrupted leases while preserving assignments. Candidate cancellation is checked
before publication; an atomic publication already in progress can finish safely.
Cancelling a payload does not implicitly cancel already assigned child jobs;
explicit execution cancellation or Clear Queue cancels its appropriate siblings.
Publication/verification outcomes remain in the existing acquisition history.

Outer payloads and extracted books are retained until explicit guarded cleanup;
cleanup requires no unresolved candidate, active/failed child queue or processing
lease. Cancelling an unused payload permits immediate cleanup. Cleanup never
removes canonical books. Staging retention is deliberately conservative; there is
no age-based garbage collector. Pending verification retains its candidate staging.

## Packaging behavior and limits

| Shape | Result |
|---|---|
| A/B: separate CBZ or page ZIP files | Independent physical payloads, one inspected book each |
| C: ZIP containing book archives | Outer retrieved once; inner books staged independently |
| D: image directories | One candidate per image group; numbers are suggestions |
| E: mixed content | Covers/Extras/Samples directories ignored, text ignored, uncertain groups reviewed |
| F: unnumbered book | Candidate exists in Needs Review; explicit assignment required |
| G: two v04 alternatives | Physical choices remain distinct; no downstream number re-resolution |
| H: metadata mismatch | Assigned import uses exact metadata or general/source fallback |
| I: physical mutation | Hash/path/size corroboration fails with a physical-stage error |
| J: corrupt/traversal | Hard inspection/import validation failure |
| K: restart | SQL candidates, assignments, staging and provenance survive |
| L: failing inner book | Failed inner material remains reviewable; healthy siblings import independently |

The expanded-byte budget is **64 GiB per inspection**, including nested reads,
with **10,000 entries**, **200:1** maximum compression ratio, and **two nested
levels beyond the outer archive**. The budget accommodates ordinary large collections. These are finite safety
limits, not catalog assumptions. All file paths and symlinks are checked; generated
staging names prevent extraction through source paths. Duplicate inner file names
are rejected. Plausible image headers are required before a candidate is imported.
Source images and single-book ComicInfo are retained; collection-level ComicInfo
is not blindly duplicated onto every extracted book. Only CBZ publication is added
to the existing pipeline; RAR/CBR/7z/PDF/EPUB remain unsupported.

## Compatibility and remaining acceptance

Legacy APIs and reader/page acquisition remain compatibility paths; their existing
bibliographic resolver and planner utilities are not removed wholesale. The native
Nyaa/TorBox path uses physical payload selection and post-transfer assignments.
Old reader jobs continue their proven page assembly path. Physical-stage imported
books use the same metadata/optimizer/publisher/Komga implementation as before.
Comics provider-not-applicable behavior is unchanged.

Directory heuristics intentionally stay conservative. Extras with unusual names
may appear in review; uncertain books are never automatically published. Bulk
review is limited to 500 newest candidates in this UI. Archive budgets are constants
and are not yet profile settings. CRC validation may read a payload more than once;
outer transfer is not repeated per book. Transfer, inspection, processing and
verification remain separately recoverable, not a new global scheduler.

No deployment or real external mutation was performed. Desktop/mobile and
keyboard/focus checks passed locally with mocked API responses (see below).
Real-provider and runtime-filesystem acceptance remain required in isolated TEST. Use disposable storage/DB and disabled tracker writes. For BLAME!, acquire
its actual outer payload(s), inspect, then approve the actual six candidates or
only v04. Edit one suggestion and verify the assignment persists through restart.
Repeat nested archive/directory, unknown-book, duplicate-alternative, metadata
outage, cancel/retry, collision and corrupt/traversal fixtures. Confirm healthy
siblings continue while unresolved material waits. Finally repeat Witch Hat
selective volumes, Blue Lock v39 and Made in Abyss partial/recreation controls.


## Local validation evidence

Final implementation checks (Node 22, locked Rust dependencies):

| Check | Result |
|---|---|
| `cargo test --release --locked` | 390 passed, 0 failed (starting milestone: 374) |
| `cargo test --release --locked acquisition:: -- --test-threads=1` | 108 passed, 0 failed (starting: 94) |
| `npm run test` | 86 passed, 0 failed (starting: 81) |
| `npm run build` | Passed; real production UI built before Rust release tests |
| `cargo check --locked` | Passed; 19 existing warnings |
| `cargo clippy --all-targets --locked` | Passed; existing 86 binary / 124 test warnings, no new diagnostic messages |
| `npm run check` | Existing 31 errors, 0 warnings in Settings, Title and reliability tests; no additional errors |
| `cargo fmt --all -- --check` | Existing repository formatting failure: 922 hunks across 55 files versus starting 924 across 55 |
| `git diff --check` | Passed |

New regressions cover migration from the real 026 schema, separate physical
alternatives with the same suggested number, inner-number suggestions independent
of outer collection ranges, mixed nested archives and image directories, image
header validation, source ComicInfo preservation, bounded unsafe archives,
idempotent inspection/assignment and assignment recovery. The durable lifecycle
fixture reopens SQLite, edits a suggestion, processes through the real local
publisher with provider/optimizer/Komga disabled, exercises candidate cancellation,
blocks cleanup during a processing lease, and verifies safe final cleanup. Parent
completion waits for active child queues/leases; execution states allow independent
siblings to progress. Restart uses the frozen series/kind/number even if catalog
and chapter display labels change.

Existing Blue Lock v39/selective retrieval/canonical metadata tests pass. Existing
Witch Hat selective collection tests pass. Made in Abyss job recreation, sibling
handle updates, durable restart provenance and partial-transfer controls pass.
BLAME filename/edition tests pass; the new generic physical identity, duplicate
number and explicit-assignment fixtures cover its architecture regression without
title-specific production logic. These are fixture results, not live service
acceptance.

Chromium exercised the actual Svelte dialog and Downloads components with a
local Vite harness and mocked API responses; external requests were blocked.
At 1440px and 390px: one and 40 physical files, one and 40 candidate books, long
unbroken paths, unknown numbering, edited series/number, Ignore, keyboard Tab
containment, Escape, focus return and single approval all passed with no horizontal
overflow. A 390px provider-outage/retry/cancel/stale-response scenario also passed.
This caught and fixed focus loss when Retry replaces the modal body. These checks
do not establish actual transfer, optimizer, filesystem or Komga UI acceptance.

Remaining compatibility debt: imported children reuse the existing cached-local
TorBox worker entry with `source_type=import`; missing/corrupt import staging fails
explicitly and cannot fall through to remote retrieval. Legacy non-native reader
and API planner/resolver code remains isolated by the physical inspection flag.
There is no new unnumbered/special library semantic; choose Volume/Chapter or
Ignore. Failed inner inspection is retained as an annotated review candidate and
must validate before assignment. Retry Inspection conservatively rejects any
already assigned child, including completed children. Cleanup is explicit and
conservative; there is no timed staging garbage collection.

Live TEST acceptance must still check the actual source/backend manifests, outer
transfer count, runtime disk headroom, configured optimizer, metadata outage,
atomic publication/collision, targeted Komga scan and exact ownership, restart
at each stage, and cancellation during real processing. Use only disposable TEST
storage and explicit operator authorization. Tracking a series still grants no
automatic download authorization.

## Exact-work MangaBaka covers

Physical volume processing now propagates `MangaBakaEnrichment.work_cover_url`
from the **same collection work** selected by the edition-aware exact-volume
resolver. General series fallback leaves it unset. No series, first-volume,
nearest-volume or neighbouring-volume artwork is substituted.

Schema audit: the published [MangaBaka OpenAPI snapshot](https://github.com/AtiinLabs/mangabaka-sdk/blob/29e364dcaa03dea721d5df0a649ddbb9cd72c076/OpenAPI/openapi.json)
models `/v1/collections/{id}/works` items as `V1_Work_Default`. Its optional
`images` array defaults to empty and contains `V1_Series_Cover_Image` records;
each record has `image: V1_Series_Image` and optional `work_id`. The shared image
schema provides nullable absolute URI values at `raw.url` and
`x150/x250/x350.{x1,x2,x3}`. Multiple image records and resolutions are possible.
This is a published third-party snapshot of the API specification, not a live
Punpun response. The parser visits only the matched work's image records in API
order, rejects explicit foreign work IDs, and prefers raw, then x350/x250/x150
at the highest available DPI. Existing image-size parsing also supports the
string representation already used by Mango's series image integration. A work's
absence of artwork does not change successful metadata resolution.

Cover requests use the existing unauthenticated pipeline HTTP client, HTTP(S)
URLs, normal bounded redirects, a 12-second request timeout and a 15-second
retrieval/decode deadline. Bodies are limited to 16 MiB even without Content-Length.
JPEG/PNG/WebP are decoded by the narrowly enabled `image` crate; MIME types and URL
extensions are not trusted. Decode limits are 16 million pixels, 16,000 pixels per
axis and 96 MiB decoder allocation. Unsupported formats, missing art, HTTP errors
(including 429), timeouts and unsafe/invalid artwork log a warning and retain the
valid source book. Artwork requests never carry MangaBaka's API token; logs omit
artwork URLs and authorization headers.

The additional page uses `000-cover.jpg`, `.png` or `.webp` according to actual
content. Every original image filename and image byte is retained by the staged
archive rewrite. Existing source covers remain reading pages. ComicInfo page
indices shift once, the injected page is FrontCover index 0, and exactly one root
ComicInfo remains. The reserved name must sort before source pages in both lexical
and Mango's natural order; unusual source ordering or an unowned case-insensitive
name collision causes optional cover enrichment to be skipped without renaming or
overwriting source pages.

`mango-volume-cover.json` stores the injected filename, version and SHA256. A retry
replaces only a hash-verified Mango cover; an unmarked or changed source is never
adopted as disposable. Exactly one injected cover remains, including format changes.
Insertion runs **before the existing RustyLibraryShrinker step** with the configured
normal book policy (defaults 2400/90/10). After that same processing run, ownership
is refreshed if the optimizer changes the injected image's bytes or extension.
An optimizer that strips provenance or changes identity ambiguously causes future
replacement to fail closed. Final ComicInfo generation, ZIP validation, atomic
publication/collision protection and Komga verification keep their existing order.
No migration, acquisition authorization change or existing-library bulk mutation
is introduced. Provider-not-applicable paths remain untouched.

Cover milestone validation: 398 release tests passed (390 starting), including
95 pipeline tests, 13 MangaBaka pipeline tests and six cover tests. The dedicated
acquisition suite passed 108 tests. Eight added regressions cover exact work and
edition selection, absent/foreign artwork and no series/neighbor substitution,
source-page preservation, root/case-insensitive ComicInfo cleanup and page indices,
idempotency/collisions, PNG/WebP/JPEG decoding, HTTP redirects/errors/429, declared
and streamed size limits, timeout, optimizer format conversion and optional-art
failure through the real local publisher. The optimizer-order integration test
uses a recording stub, not the real RustyLibraryShrinker binary.

Frontend: 86 tests passed; production build passed; the same 31 type-check errors
and zero warnings remain. Cargo check passed with the existing 19 warnings;
Clippy passed with the same diagnostic messages (86 binary / 124 test warnings).
Formatting remains the existing 922 hunks across 55 files; diff whitespace checks
pass. No frontend/API workflow or database migration was needed.

TEST deployment and Punpun volumes 1–7 were not exercised from this environment:
it exposes no server credentials, VPN or TCP grants. The guarded deploy helper,
TEST container/image/health, retained Punpun payloads, live work IDs and covers,
and production before/after fingerprints were therefore not inspected. No server
was contacted and no canonical book was replaced. Required next acceptance is to
inspect the existing deploy helper, fingerprint production, back up TEST, deploy
the pushed commit to TEST only, verify health/8788 and the unchanged production
fingerprint, then build and validate all seven volumes from retained payloads in
staging. Confirm exact edition/work IDs, expected cover bytes before optimization,
original pages (accounting for normal optimization), one ComicInfo, valid ZIPs,
optimizer outcome, filenames and permissions. If retained source is absent, stop;
do not reacquire automatically or overwrite canonical books. Any eventual
canonical replacement needs the separately guarded backup/hash/atomic-replacement
procedure described in the task.

Existing canonical books now share exact-work cover enrichment through the
[normalization workflow](16-library-maintenance.md#exact-work-covers-for-existing-books).
That workflow freezes reviewed artwork, preserves source-entry hashes and full
rollback backups, and never sends canonical books through acquisition publication
or a whole-book optimizer. The existing ownership marker optionally retains the
upstream image hash so later maintenance can distinguish new artwork from
optimization changes. Legacy markers remain readable and conservative.
