# Existing-library metadata maintenance

This extends Mango's native normalization service, shared MangaBaka metadata generator,
archive writer, Komga client, Maintenance UI, and normalization journal. It does not
redownload books, optimize images, or change acquisition numbering semantics.

## Supported scope and review

Use Settings → Pipeline → Library Normalization & Maintenance as an administrator.
English Manga is the first acceptance target. The configured Chinese library ID/root
is excluded regardless of display name. Libraries identified as Chinese or Comics,
unavailable libraries, and unknown providers are skipped. If a primary English
Komga library ID is configured, other libraries are excluded. Without that setting,
Manga name/root recognition is conservative fallback classification; inspect the
library selection before approving a mutation. No comics provider or Chinese policy
is supplied.

Identity resolution uses resolved tracker mappings, authoritative MangaBaka Web links,
unique acquisition provenance, and the existing fail-closed title matcher. Explicitly
ambiguous mappings and conflicting book identities are not overridden with guesses.
An established AniList identity must resolve through its MangaBaka cross-reference.
Provider outages/authentication failures count as errors; unavailable identity or
editions count as unresolved; ambiguous identities/editions count as ambiguous.

Maintenance accepts unambiguous volumes only. It uses explicit volume names or existing
ComicInfo Volume metadata, refuses conflicting evidence, and leaves chapter releases
and unknown numbering for manual review. It never substitutes Komga's display ordinal
for a volume number. The existing generated volume/Number policy is unchanged.

The shared generator and taxonomy policy determine proposed fields. Existing useful
fields remain when the provider supplies no replacement; Notes, Translator and other
local contribution fields are retained. Nested fields such as Pages are preserved.
Malformed XML, duplicate child fields, attributed/namespaced top-level fields, and
conflicting ComicInfo documents require manual review. Matching duplicate archive
ComicInfo entries are consolidated to exactly one root `ComicInfo.xml`. The XML parser
is `roxmltree`; it replaces unsafe line-based assumptions in the maintenance path.

## Preview and approval contract

1. `POST /api/pipeline/maintenance/dry-run` with
   `{"libraryId":"test-english-id","seriesId":null}` returns counts, per-book
   identity/status/reason, current hash, proposed field diffs, and `previewId`.
   Canonical media are read only. Mango saves the exact proposed XML and server-resolved
   identity/path/hash in its operational database. Komga pages are fetched completely;
   an API failure aborts the analysis rather than returning an incomplete success.
2. Review the book's diff disclosure and reason in Maintenance. Counts are book counts
   (matched overlaps would-change/unchanged); skipped is separate from errors.
3. `POST /api/pipeline/maintenance/normalize` selects 1–25 distinct `bookIds` from
   the saved `previewId`. `dryRun` defaults to true and `approved` defaults to false.
   A real mutation requires both `"dryRun":false` and `"approved":true`.
   Example body: `{"previewId":"returned-preview-id","bookIds":["test-book-id"],"dryRun":false,"approved":true}`.
   Old requests containing arbitrary paths/metadata are rejected. Previews expire
   after 24 hours; obtain another preview if files or library configuration change.
4. The server rechecks current library policy, series membership and actual Komga
   filesystem URL, requiring a CBZ within the library root. Komga and Mango must see
   the same absolute book paths (plain paths or local `file:` URLs); there is no guessed display-name path or automatic
   container-path translation. Symlink resolution must remain within the root.
5. Mutations have separate destructive confirmations. A file changed since preview
   fails closed. The exact reviewed provider XML is frozen; execution does not fetch
   a silently different proposal. Independent failures are recorded and do not stop
   other valid books in the batch. Inspect results/history, then preview again.

## Replacement, journal and rollback

Normalization uses a canonicalized per-file lock shared by all accounts/services in the process, stages on
the same filesystem, validates every ZIP entry to EOF/CRC, and compares every
non-ComicInfo entry's name/order/content hash. Exactly one root ComicInfo must remain.
The staged file is synced and revalidated immediately before atomic rename. Existing
permissions are retained. Linux/Unix directory sync makes the rename durable.

Before replacement, a complete original archive is copied with exclusive creation,
synced and checksum-verified. A durable `prepared` record stores both hashes and the
backup/staging locations. Critical journal/rollback-intent writes temporarily use SQLite
synchronous FULL, then restore the connection's usual setting before releasing its lock.
History reads recover the replacement/status crash window
from hashes: prepared + original → failed (not published), prepared + normalized →
normalized; rollback's corresponding states resolve to normalized/reverted. A different
hash causes a conflict rather than an overwrite. Active operations are not recovered
by concurrent history readers.

Rollback requires a normalized journal record, an unchanged current normalized hash,
and a verified full-file backup. It stages, validates and atomically restores that
backup, preserving the original archive byte-for-byte. Repeated rollback is harmless.
Legacy history rows lacking complete backups cannot offer safe automatic rollback;
the UI explains that limitation. A second unchanged analysis is Already Normalized
and does not rewrite the archive.

Hidden `.original.backup` backups (with a non-book extension) are deliberately retained beside books. Interrupted
operations may also leave hidden staging files or backups made before journal insert.
Do not delete backups required by history. There is no automatic retention/garbage
collection policy yet: budget temporary and backup disk space and review orphan
cleanup manually in the isolated test environment. Keep one Mango process writing
maintenance and pause other filesystem writers for acceptance; locks coordinate
Mango workers, not unrelated programs. Network filesystems/Windows replacement and
crash durability need platform-specific acceptance before bulk use.

## Komga refresh

After publication/rollback, the existing Komga client requests targeted book analysis
(or series analysis when no book ID is present). Three bounded attempts are made.
Failure leaves the successfully published book and backup intact, records
`refreshPending`, and exposes Retry Komga metadata refresh in the existing history UI:
`POST /api/pipeline/maintenance/refresh/{historyId}`. Restart retains pending work;
retry is explicit, not an unbounded background worker. HTTP acknowledgement is not
proof that Komga finished importing ComicInfo: verify the resulting fields in Komga
before promoting acceptance. No re-download is used to handle refresh failure.

## Controlled live TEST acceptance (not production)

Implementation tests use temporary fixtures and local mocked services. The following
real-service acceptance is still required, on a separately authorized isolated test
Mango/Komga instance using disposable **copies**, never production media or databases:

1. Back up the test Mango database. Create English Manga, Comics and Chinese test
   libraries. Configure the test English/Chinese IDs, MangaBaka access and Komga via
   Mango's existing secure Settings flow. Do not put tokens in logs, shell history,
   screenshots or reports. Use one test Mango writer; stop other test file writers.
2. Copy a small sample of known English volumes (including fractional volume,
   missing metadata, useful legacy fields, nested Pages, taxonomy aliases and
   identical duplicate ComicInfo entries). Include separate ambiguous identity,
   conflicting metadata, chapter, unknown-volume and invalid-archive fixtures.
   Record baseline SHA-256, ZIP entry order, non-metadata hashes and permissions.
   Confirm Mango's runtime can read/write the same filesystem URLs Komga returns,
   and that free space covers originals, staging and retained backups.
3. Run whole-library DRY RUN. Verify considered/would-change/unchanged/unresolved/
   ambiguous/error counts, identity, each proposed diff, and reasons against the
   known sample. Check Award Winning, Slice of Life, Sci-Fi, the existing Boys Love
   spelling and Football → Soccer deduplication. Confirm all baseline hashes remain
   unchanged and no download/acquisition is created.
4. Preview Comics and Chinese libraries, including a generically named Chinese
   library selected by configured ID. Every book must be skipped, error count zero,
   and media hashes unchanged. No book should become a mutation candidate.
5. Normalize **one** reviewed eligible English book through the destructive
   confirmation. Inspect journal status and retained backup. Verify all page/extra
   bytes/order and permissions, exactly one root ComicInfo, expected metadata,
   unchanged established Volume/Number semantics, and targeted Komga analysis.
   Wait for Komga and inspect its resulting metadata and reading order.
6. Preview again with the same provider data: the book must be Already Normalized,
   with no new replacement. Verify canonical hash and modification time unchanged.
7. In test copies only, change another candidate after its preview. Submit a batch
   with that stale book and an independent valid book. Verify one fails durably,
   the valid book succeeds, and the stale file is not overwritten. Run at most 25
   books per batch and review history between batches.
8. Make only the test Komga analyze endpoint temporarily fail (leave read endpoints
   working, e.g. through a test reverse-proxy rule). Normalize one book. Verify
   publication succeeds, three refresh attempts are bounded and pending status is
   visible. Restart test Mango, restore that endpoint and use Retry; confirm Komga
   eventually observes the metadata without another file replacement/download.
9. Revert an unchanged normalized test book through confirmation. Verify the exact
   baseline SHA-256, all original metadata/entries and permissions, durable reverted
   history, and Komga's restored metadata after refresh. Try repeat revert (no-op).
   Change a different normalized test file, attempt revert, and confirm conflict
   rejection preserves it. Corrupt a disposable copy of a backup and confirm the
   canonical file is not replaced. Keep the uncorrupted backup available.
10. Record build commit, fixture/provider IDs (no credentials), counts, hashes,
    expected/actual Komga fields, restart/refresh evidence and failures. Stop on any
    ambiguity, unexpected mutation or mismatch. Approve broader test batches only
    after this evidence passes. Production migration is a separate authorized phase.

### Exact-work covers for existing books

Canonical normalization now reviews optional exact-work MangaBaka artwork alongside
metadata. A book whose ComicInfo is already correct can have an `Exact work cover`
diff. Analysis downloads and validates the matched work's image without modifying
the canonical CBZ. Validated bytes and their hash are frozen server-side in the
existing saved preview JSON; they are not returned in the UI response. Preview
execution survives restart/provider outages and cannot fetch different artwork
than the operator reviewed. Expiry remains 24 hours; artwork is bounded to 16 MiB
per book (base64 in SQLite), and previews are saved one book at a time to bound
memory. No migration is required; old metadata-only previews remain readable.

Execution uses the same normalization staging, full-file backup, original hash
check, durable journal, atomic replacement and rollback. Every non-managed entry
must retain its original hash: only ComicInfo and provably Mango-owned injected
cover/marker entries may change. Cover insertion uses the existing atomic cover
writer. The normal acquisition publisher and its collision policy are not used.
There is **no optimizer pass over canonical pages**. A rollback restores the exact
original archive, including its original metadata and cover state.

`Pipeline::repair_canonical_cbz()` now delegates to this same normalization engine
instead of maintaining a separate rewrite path. It records backup/history and
retains the existing library scan behavior; batch normalization performs the normal
targeted Komga refresh. Repair also checks the canonical hash across metadata
resolution and before replacement.

Known edition labels from the canonical series/ComicInfo select the edition-aware
collection resolver; conflicting labels fail closed. Edition namespaces are
retained. There is no cover substitution from a series, another volume or another
edition. Missing artwork and temporary download/validation errors do not by
themselves produce `NeedsUpdate`; valid owned artwork is `AlreadyNormalized`.
Unowned reserved filenames or invalid ownership markers require review and leave
the canonical file unchanged.

The existing version-1 ownership marker gains an optional `source_sha256` field.
Its original `sha256` still proves ownership of the current injected image; the
source hash identifies the upstream image before optimization. New acquisitions
retain that source hash across optimization. A changed upstream hash can update
only a current hash-verified owned cover. Legacy valid markers lacking an upstream
hash are conservatively retained: their optimized bytes cannot establish whether
upstream art changed. Such books need a future explicit cover-replacement review
if an upstream update is desired. No alternate provenance mechanism is added.

Local validation for this extension: 405 release tests passed, including 102
pipeline tests (18 normalization, six cover and 13 MangaBaka tests) and 108
acquisition tests. Seven new regressions exercise non-destructive cover-only
preview/execution, source-page and ComicInfo preservation, exact rollback,
frozen artwork across restart/outage, stale-file refusal, edition/volume selection,
changed upstream artwork, absent/invalid/unavailable artwork, collision/ownership
refusal, legacy previews/markers, and the canonical repair entry point itself.
Cargo check and Clippy pass with existing warning counts (19 check; 86 binary /
124 test Clippy warnings). Formatting retains existing repository debt: 921 hunks
across 55 files versus 922 at the starting commit. Diff whitespace checks pass.
Frontend tests/build were not rerun because no frontend code or public response
shape changed. All integration calls used local fixtures. No server was accessed
or deployed; Goodnight Punpun volumes 1–7 still require later operator acceptance
through the saved-preview normalization workflow, without reacquisition.

### Komga and Mango filesystem namespaces

Normalization uses the selected Komga library root as the source namespace and
Mango's existing `pipeline.library_dir` as the local English canonical root (the
same destination used by acquisition/publication). The existing `book_path`
resolver strips the selected Komga root by path components, joins the relative
book path onto the configured local root, then resolves and checks the local path.
The Komga root itself need not exist inside Mango's container. Nested directories,
spaces/punctuation and encoded `file:` URLs are preserved; already-local paths
inside the configured root are also accepted. No mount discovery, basename search,
new mapping registry, configuration key or compose change is introduced.

The same resolver handles identity samples, dry-run analysis and execution's live
book revalidation. Traversal (including encoded URI dot segments), unrelated root
prefixes and symlink escapes are rejected; the final target must be a CBZ confined
to the configured root. Execution reloads that root and compares the resolved path
to the frozen preview, so changing a mapping requires a new preview even if an
identical book exists at the new destination. Hash, staging, journal, backup,
rollback and atomic replacement checks remain unchanged. Dry runs do not modify
canonical files. Chinese/Comics provider exclusions still run before local-root
resolution. Installations without a configured publication destination retain the
legacy same-namespace Komga-root fallback; different mounts require the correct
Mango-visible `pipeline.library_dir`. Re-run analysis after changing configuration.

Local namespace-fix validation: 22 focused normalization tests and all 409 release
tests pass. Four new regressions cover mapped-root dry-run/diff/execution/rollback,
changed-root refusal despite identical book contents, nested spaces/punctuation
and `file:` URLs, traversal/unrelated-prefix rejection, and symlink escape refusal.
Cargo check and Clippy pass with unchanged warnings; existing formatting debt
remains 921 hunks across 55 files, and diff whitespace checks pass. No server was
accessed or deployed. Live acceptance should deploy this fix, verify the English
canonical destination setting, and repeat the Manga/Punpun dry-run; verify all
seven books reach exact-work metadata/cover analysis without canonical changes.
