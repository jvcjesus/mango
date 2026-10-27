# Edition-aware acquisition and file resolution

The native acquisition architecture is superseded by
[physical acquisition and import](21-physical-import.md). This document retains
historical implementation/acceptance evidence and compatibility behavior.

For current approved-file semantics and the shared Lists/Library workflow, see
[approved source acquisition](20-approved-source-acquisition.md). Strict metadata
prerequisites below describe historical behavior superseded for approved files.

The interactive planner and override limitations recorded here describe milestone
52641b0; see [interactive planning and identity override](18-interactive-planning-and-identity-override.md) for the subsequent implementation.

Acquisition numbering is edition-local. A request for Standard Volume 4 never
implicitly becomes Master/Omnibus Volume 2 or 4. No conversion table is inferred
from archive count, ordering, release size, or tracker totals.

## Identity audit and storage

The catalog row supplies Mango's stable manga ID/title; native sync mappings
supply MangaBaka/AniList cross references to metadata resolution. AniList is a
tracker/series cross reference, not an edition authority. Nyaa supplies raw release
title, source ID, hash, size and parsed release coverage. Normalized candidates
already have `edition`, `series_identity`, mapping provenance and JSON provider
metadata. Plans and execution items already freeze those candidates in JSON.

The reported BLAME!-class title contains no volume/chapter marker: Nyaa classifies
such a release as `Unknown`. The current Title UI passes that release unit into
the acquire endpoint. Previously the service accepted it with the release label
and numeric sentinel -1; queue loading reparsed it as a chapter (the first
year in a year-bearing label can become the chapter number), so volume-labelled
files could never satisfy that requested chapter. The resolver then discarded the
requested-unit error whenever raw_title existed and returned generic multi-file
ambiguity. A supplied Volume N could also lose its real failure through the same
fallback. The actual six TEST filenames were not supplied, so this code audit does
not prove which filename markers were present in that live job. The repair requires
an explicit unit for an unnumbered release and retains failures instead of guessing.

Previously `edition` came from variant classification, which did not identify
Master/Deluxe/Omnibus editions. Direct and collection downloads only retained
raw_title, collection_name, numeric chapter/volume, size, job ID and job-local
external_file_id. Collection queueing could replace the original release title
with a per-file or synthesized label. The worker reparsed display names rather
than restoring a frozen requested unit. History retained series/provider/work
references and requested-unit text but no selected path or edition intent.

The existing normalized edition field and provider metadata now carry source
edition evidence and requested edition intent through plan snapshots. Edition
changes during live candidate revalidation require replanning. Planner/preflight
reject a release edition incompatible with the requested context.

Migration **026_retrieval_identity.sql** adds one JSON provenance column to
`downloads` and `acquisition_history`. Existing plan snapshots cannot cover direct
or collection queue rows, and a job-local numeric file ID cannot safely identify
a file after recreation. JSON contains catalog series title, requested normalized
edition, edition-local requested unit, original release title, and (after selection)
exact archive path/size. Existing foreign keys provide stable Manga IDs; existing
metadata history fields preserve provider series/work IDs. No parallel catalog
or edition database is introduced. History copies provenance while the queue row
still exists. Old rows derive conservative context from catalog/collection/raw
labels; old numeric IDs alone are never trusted.

## Edition evidence

One shared normalization/evidence module handles Standard, Master, Deluxe,
Omnibus, Collector's, Perfect, Full Color, 2-in-1, 3-in-1 and Revised markers.
Absence of a marker means unknown, not authoritative Standard. Ordinary unmarked
releases retain the existing ordinary-volume interpretation. Nonstandard release
evidence requires explicit matching edition intent or a matching catalog title;
clicking a release does not authorize conversion of standard missing-volume IDs.
Unknown provider edition names remain distinct normalized names.

MangaBaka collection `edition` metadata is authoritative for metadata enrichment.
Its existing collection/work resolver now selects the requested edition rather
than always Standard. It must uniquely match the English digital collection (or
its existing non-print fallback) and requested work sequence. Nonstandard editions
do not inherit a standard-series volume total when their collection count is
missing. Authoritative cross-edition volume conversion is not implemented.

Nonstandard acquisitions use a separate edition-qualified canonical series name,
ComicInfo Series, staging folder and Komga verification title, preventing a
Master Volume 4 from replacing an existing Standard Volume 4. No existing media
is renamed or migrated. Tracker progress writers are unchanged.

## Resolution and errors

1. Restore durable requested edition/unit and original release context.
2. Reject conflicting edition intent before retrieving bytes. Existing unsupported
   archive guards still apply.
3. Enumerate supported CBZ/ZIP candidates. Parse explicit volume/chapter identity
   from names/paths; do not conflate chapter, collection or fractional-volume IDs.
4. Require matching edition evidence and requested unit. A single unlabelled file
   may use an explicit single-unit source release as corroboration; file count by
   itself never establishes a unit.
5. If selected path/size is persisted, require that exact path and nonzero size.
   A recreated job may assign a new ID; it must still corroborate the selection.
   Otherwise a unique exact original name/path may disambiguate eligible files.
6. Retrieve only one uniquely matching archive. An ID is corroborating context,
   never an override. Torrent aggregate size is not a per-file tie breaker.

Zero matching labelled units uses existing `requested_unit_not_found`; conflicting
editions use `edition_mismatch`; unlabelled/duplicate alternatives use
`ambiguous_files`. Direct requests require an explicit single volume/chapter when
the release itself does not identify one. Downloads displays the existing error
message with requested edition/unit and concise candidate counts.

Debug tracing records safe bounded series/release context, job identity, each
candidate filename/path, parsed unit/edition, and acceptance/rejection reason.
Normal warnings contain job identity and concise final failure. URLs, magnet
strings and query-bearing values are redacted from provider-text diagnostics;
no request headers, credentials or signed retrieval URLs are included.

## Explicit selection API groundwork and limitations

The existing `POST /api/titles/{id}/acquire` accepts optional `requestedEdition`
and `selectedArchive: { "path": "exact/path.cbz", "size": 123456 }`, alongside
`release` and `targetUnit: { "type": "volume", "value": 4 }`. Existing collection
requests accept optional `requestedEdition`. Omitted fields remain compatible.

Requeueing preserves an existing selected path/size. Changing the unit, edition,
source release, selected archive or torrent identity after selection is rejected
with `selection_conflict`, preserving partial bytes rather than mixing archives.

A selected path can disambiguate multiple files already corroborated against the
requested edition/unit. It cannot override an edition warning, assign a unit to
an otherwise unidentifiable file, accept a size change, or silently substitute a
renamed file after recreation. That stricter fallback is intentional. It remains
backend groundwork: the existing acquisition dialog has no edition picker, file
chooser, or diagnostic-candidate listing endpoint. No new screen is introduced.
Ownership/duplicate checks remain conservative: standard-volume ownership or an
active shared hash can block a separate edition under the same catalog row. This
milestone does not introduce edition-specific personal-tracker totals or a general
coexisting-edition ownership UI.

Plans derive requested edition from catalog context and freeze it in existing
provider JSON. A generic edition-picker/provider-collection UI and authoritative
cross-edition mapping require separate work; standard tracker totals must not be
repurposed as nonstandard edition numbering.

## Separate isolated TEST acceptance (not performed by this implementation)

Before any later authorized TEST deployment, use a TEST-only SQLite database,
staging/canonical directory, Komga library and backend configuration. Confirm all
paths/IDs point to disposable TEST resources and tracker writes are disabled.
Production, real canonical media and the live database must remain inaccessible.
Record the deployed commit and safe debug diagnostics; never export tokens,
magnets or signed URLs. Deployment itself needs separate authorization.

1. **BLAME! Master Edition:** discover the reported 2016–2017 digital release in
   isolated TEST. Inspect its actual file listing; do not infer six units from six
   archives. Using the existing acquire endpoint and authenticated TEST session,
   request `requestedEdition: "Master Edition"`, `targetUnit: volume 4`. Expect
   only a uniquely labelled Master v04 archive to enter Mango. If actual files do
   not prove v04, expect actionable failure and retain the safe listing evidence.
   Inspect TEST provenance JSON, ComicInfo Series/Number, edition-specific path,
   MangaBaka Master collection/work (if available) and targeted Komga verification.
   Verify no Standard v04 or sibling archive was retrieved/published. Restart
   TEST, retry, and simulate remote expiry using disposable jobs: new IDs must
   resolve the same path/size. If the provider lacks an authoritative Master
   collection, expect existing fail-closed metadata handling, not Standard data.
2. **Witch Hat Atelier:** inspect the v01–14,082–095 release. Select only Volumes 1
   and 2 using the existing collection flow. Verify two independent queue units,
   exact corresponding archive retrieval, untouched siblings, standard MangaBaka
   work metadata, archive integrity, canonical names and Komga verification. Retry
   a disposable expired shared job; confirm incomplete siblings update job IDs,
   file IDs clear/re-resolve, and cleanup waits for incomplete siblings. Keep any
   valid `.part` and verify Range 206 resume/200 restart/416 handling as applicable.
3. **Edition mismatch:** on the TEST catalog title, submit the same Master release
   with `requestedEdition: "Standard Edition"`, Volume 4. Expect HTTP 422
   `edition_mismatch` before queueing/submission; no Mango retrieval, publication,
   or tracker mutation. Repeat with `selectedArchive` supplied: selection must not
   bypass the mismatch. Confirm the UI/API explanation identifies the conflict.

Unit/integration tests use synthetic files and local HTTP/SQLite fixtures. They
establish the generic code behavior, not successful live BLAME! acceptance.

## Repository validation evidence

The completed implementation passed 338 release-mode Rust tests, including 15 new
regressions. Targeted resolver (9), edition/provenance (2), Unicode (1), captured
logging (1), restart/history (1) and MangaBaka edition (1) tests also passed.
`cargo check --locked` and `cargo clippy --all-targets --locked` passed; comparison
with the original checkout found no new Clippy warning messages. Local HTTP tests
require socket permission; the initial restricted-sandbox run failed to bind its
mock listeners, and the permitted rerun passed without production credentials.

Frontend tests passed (69), production build passed, and type checking reported
45 errors in the same four unchanged frontend files. Formatting remains a baseline
failure: both original checkout and final code report 925 hunks across the same
55 files. New/changed code was formatted without repository-wide cleanup.
`git diff --check` passed. No deployment or live acceptance was performed.
