# Source-local acquisition units

The native acquisition architecture is superseded by
[physical acquisition and import](21-physical-import.md). This document retains
historical implementation/acceptance evidence and compatibility behavior.

For current approved-file semantics and the shared Lists/Library workflow, see
[approved source acquisition](20-approved-source-acquisition.md). Strict metadata
prerequisites below describe historical behavior superseded for approved files.

This focused follow-up to [interactive planning](18-interactive-planning-and-identity-override.md)
addresses the isolated TEST zero-unit report. Implementation does not constitute
live acceptance. No deployment, production access, real media or tracker mutation
was performed.

## Root cause

The Nyaa RSS title identifies an edition, not necessarily its volumes. An unnumbered
Master collection is correctly classified `AcquisitionReleaseUnit::Unknown`.
`prepare_plan` asked `included_volumes()` for that title, which returned an empty
list. It deliberately did not borrow standard catalog totals. No source manifest
was inspected, so clear v01–v06 filenames never reached the planner. Override only
changed identity consent; it reused the same empty coverage and therefore could
not make an acquisition approvable. Older executions with collection/unknown unit
identity also could not establish a single requested unit at retrieval.

Lists had a manual Specific Volumes text field, but no discovered-unit or archive
selector. Switching to Specific also cleared the plan and its source controls.
The resulting advice was not backed by a usable source-file selection flow.

## Read-only discovery and identity reuse

The selected release now obtains file evidence within the existing 12-second
provider and 30-second server planning bounds. A matching known TorBox job may
supply a read-only listing, with its torrent hash corroborated. Otherwise Mango
fetches the selected RSS enclosure's public torrent metainfo from the configured
Nyaa origin. It never submits/recreates a job, requests a download link, or queues
content during planning. Only the explicitly selected release is inspected.

The bounded metainfo reader accepts UTF-8 v1 name/path and integer lengths. It
limits the body to 2 MiB, files to 500, nesting to 16 and parsing work to 30,000
nodes. Malformed, duplicate and unsafe paths fail closed. It adds no dependency.
This reader decodes metainfo; it does not introduce another filename parser.

`source_units` reuses `AcquisitionReleaseUnit`, normalized edition evidence,
`SelectedArchive {path,size}`, the existing Nyaa/release parsers and strict TorBox
resolver. Each preview choice carries its typed local unit, edition, archive and
eligibility/evidence. The enclosing candidate retains source-release provenance.
Absolute backend listing paths remain opaque remote identities, never local output
paths; metainfo paths must be relative. Both reject traversal/control characters.
Supported nonzero CBZ/ZIP content files are considered; unsupported files do not
become units. Names such as `v01`, `Vol. 01`, `Volume 01` and established chapter
forms are parsed. The basename takes precedence over collection ranges on parent
directories. A chapter within a volume remains a chapter, not a whole volume.

Known contradictory editions remain ineligible. An absent file edition can be
corroborated by the release/path context. Archive order, IDs, count, aggregate
size and standard catalog totals never establish numbering. Metainfo is preview
evidence: actual backend files must still corroborate the approved path/size and
unit at retrieval.

## Content selection and ownership

For a selected nonstandard or overridden release with file evidence:

- **Complete Series** selects all discovered eligible source-local volumes. Six
  corroborated Master volumes mean six Master acquisition items, independent of
  standard catalog totals. Duplicate alternatives for one number block automatic
  selection; they are not silently excluded or picked by order.
- **Specific Volumes** keeps the manual multi-volume input and exposes discovered
  volume buttons. Clicking Volume 4 means this release's edition-local Volume 4.
  The exact archive chooser resolves duplicate numbered alternatives. A supported
  explicitly selected chapter archive retains its Chapter kind end to end.
- **Missing Volumes** uses only matching edition-qualified ownership. It can use
  physical canonical files with existing durable edition provenance, or an exact
  edition-qualified Komga series. If that evidence is unavailable, it clearly
  requires Complete/Specific; standard ownership is never substituted. Tracker
  totals do not establish edition totals.

Ownership queries reuse `Job::publication_title()` and the existing Komga ownership
service. The cache now includes both manga ID and queried series title: a cached
standard result cannot satisfy a Master query. Invalidating a manga clears all its
namespaces. Execution preflight rechecks edition-qualified ownership and does not
mistake a chapter for an owned volume with the same number.

The UI now retains override consent while switching Complete/Specific/Missing and
regenerates the plan. Changing source/release/edition clears that consent. It shows
**Using source release identity · Series metadata fallback** after confirmation.
The selected source, detected edition and any exact archive choice remain visible.
The volume input and source controls remain accessible in the same dialog. Editing
manual intent invalidates approval until Apply regenerates a plan. Existing cancel,
stale-response, loading, opaque-modal and focus behavior is retained.

## Execution, recovery and safety

Each selected unit freezes its typed `requestedUnit`, `requestedEdition` and exact
`selectedArchive` into its existing candidate JSON. Large preview lists are not
copied into every per-unit candidate. Existing saved-plan alternatives retain the
review evidence. Execution/queue creation uses that frozen typed unit rather than
reparsing a generic display title. The existing retrieval JSON then preserves it
through retry, restart, TorBox recreation, resolution and history.

Remote IDs may change; path/size/unit evidence must not. Duplicate numbered files
need exact selection. Unlabelled/contradictory files remain ineligible even with
identity override. If read-only discovery is unavailable, manual Specific Volumes
can still express intent, but retrieval must corroborate it; no source coverage is
invented. The dialog reports the manifest failure.

Metadata behavior is unchanged: strict exact-work resolution, or explicitly approved
identity fallback through exact MangaBaka work → general MangaBaka series → cached
general AniList → source. Source imagery and edition-qualified canonical/ComicInfo
naming remain intact. No destination, ZIP/archive, atomic publication, duplicate,
worker-lock or partial-transfer protection is relaxed. Existing files still stop
with `destination_conflict`; override never grants replacement permission.

No schema migration, dependency or competing acquisition/edition model was added.
Changes are in acquisition `candidates`, `interactive`, `mod`, new `source_units`,
`service`, `torbox`; pipeline `ownership`; UI `api`, `planning`, planning tests and
`Lists`; and feature documents 18/19.

## Deterministic validation

Fifteen Rust regressions were added: five manifest/parser tests and ten source-local
planner/integration tests. They cover generic edition families, six-volume Complete,
specific v04, standard totals/ownership separation, qualified physical/Komga ownership
and cache separation, duplicate archive selection, consent regeneration, unlabelled
files, chapter kind, saved execution restart, changed IDs/path/size, RSS metainfo and
read-only existing TorBox listings. Two frontend regressions cover discovered choices
and explicit request/override/archive transport. Existing safety and acquisition
regressions remain in the complete suite.

Final validation: **371 release-mode Rust tests passed**, including the 15 new
regressions and existing Witch Hat/Blue Lock/Made in Abyss, TorBox, transfer,
metadata/publication, Unicode and planning lifecycle coverage. The acquisition
subset passed (91 tests); the Made in Abyss reordered-file regression also passed
individually. **80 frontend tests passed**, and the production build passed.
Type checking remains at **35 pre-existing errors in four files**, with no new
error messages. Rust check and all-target Clippy pass with no new warning messages.
Repository formatting remains the existing failure: **924 hunks in 55 files**,
unchanged from the verified starting baseline. Changed code was formatted;
`git diff --check` passes. All integration fixtures are local and use no real
credentials. Desktop checks were not applicable; live TEST acceptance is separate.

## Exact isolated TEST acceptance

Use the separate TEST deployment helper after this implementation phase. Record the
deployed commit. Use disposable database, staging/canonical roots and a TEST Komga
library; keep tracker writes disabled and production paths/credentials inaccessible.
Capture safe outcomes, not tokens, magnets or signed URLs.

1. **BLAME! Master → Complete Series:** Lists → BLAME! → Add to Library → Complete
   Series → explicitly select the reported Master Edition release. Leave override
   off. Verify actual filename evidence yields Master v01–v06 and the dialog shows
   six source-local volumes. Inspect six plan items: each has its own typed volume,
   `requestedEdition: master` and exact archive path/size. Approve and inspect six
   execution/queue identities. Confirm only those selected archives enter Mango,
   qualified metadata/naming and exact TEST Komga results. If the real filenames
   differ, record the actual evidence and fail safely rather than assuming six.
2. **BLAME! Master → Specific Volume 4:** reopen, select the same release and choose
   Specific Volumes → Volume 4 (or enter `4`, Apply). Expect exactly one item and
   the actual v04 archive. Approve; verify five siblings are not retrieved. Restart
   the disposable acquisition and recreate an expired TEST backend job; reordered
   files/new IDs must preserve v04 by exact path/size. Changed path/size must stop.
3. **BLAME! → Use Release Anyway:** use a disposable metadata mock with general
   series data but no exact Master work. Confirm strict metadata resolution fails.
   Replan the same release; choose Use Release Anyway, read/confirm, and verify the
   visible source-identity/fallback state. Choose Complete (six items) or Specific
   v04 (one). Consent must remain active, and final approval stays separate. Verify
   actual fallback confidence, durable consent/history, original cover/page imagery
   and qualified names. No standard totals, publisher or edition date is invented.
4. **Ambiguous duplicate archive:** serve a disposable manifest and backend listing
   with two Master v04 alternatives having distinct paths. Complete and Specific 4
   must both block, including under override. Select one exact numbered archive in
   the chooser; expect one item with its path/size, then only that file retrieved.
   Reorder/change IDs and repeat; alter selected path/size and expect a safe failure.
   Repeat with unlabelled archives: no automatic numbering or approvable fabricated
   unit. Contradictory Standard/Deluxe evidence must also remain blocked.
5. **Witch Hat control:** discover its standard v01–14 collection, request only
   volumes 1/2 and approve in strict mode. Verify separate selective file retrieval,
   untouched siblings, optimizer, metadata, publication and exact TEST Komga ownership.
   Repeat Blue Lock specific v39 and Made in Abyss disposable recovery controls.
6. **Ownership/lifecycle controls:** with only Standard BLAME! v04 owned, Master
   Missing must not subtract it. Add an edition-qualified Master v04 to TEST Komga:
   Missing must subtract only that Master unit. Remove/disable qualified evidence
   and verify actionable Complete/Specific guidance. Hold/fail the manifest mock;
   timeout/error/retry must settle. Cancel/close/Escape during discovery and change
   titles rapidly; no stale response may approve, queue or replace another title.

Remaining limits: public v1 metainfo or an existing matching TorBox listing must be
available for automatic file discovery; magnet-only/new uncached backend jobs are
not created during preview. Missing calculation requires qualified evidence and
cannot identify arbitrary legacy artifacts. Complete/Specific Volumes remain volume
flows; individual chapter archives can be chosen explicitly. Unsupported/unnumbered
archives cannot gain invented local numbers from override. Actual remote path
representations and provider behavior require the separate TEST acceptance above.
