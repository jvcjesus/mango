# Approved source acquisition and a shared title workflow

Source acquisition certainty and metadata certainty are separate. This follow-up
supersedes the strict bibliographic prerequisites in feature documents 17–19 for
explicitly approved source files. It does not authorize deployment or external
mutation.

## Audit and consolidation

Lists used the saved by-title planner, Library/detail had a separate missing-content
modal and direct collection/single-release queueing. These paths differed in
approval, edition handling and archive selection. Library's collection and plan
modals and their queueing handlers have been removed from the UI. Both routes now
mount `AcquisitionDialog`, which owns discovery, scopes, source/file choices,
confirmation, saved-plan approval and execution. Nyaa browsing/filtering remains;
its acquire/collection actions open this same dialog with the source preselected.
Execution inspection/retry/cancel and Downloads remain lifecycle surfaces.

Lists remains the personal catalog, Library remains physical/local ownership,
and tracking never grants download authorization. Existing tracking, ownership,
profiles and Comics provider-not-applicable behavior are retained. The shared
workflow still uses the existing by-title manga resolution and tracked catalog
link; it introduces no second ownership or title database. Legacy API endpoints
remain for compatibility; this is UI convergence, not their removal.

## File authority and metadata

An approved exact archive path/nonzero size is authoritative for recovery. Resolution
requires a unique matching supported safe archive. It ignores disposable backend
IDs and does not require another parser to prove the same bibliographic unit.
Changed/missing/duplicate path-size matches fail safely. The typed library unit,
edition, torrent/source provenance, selection and metadata policy remain frozen in
existing plan/execution/retrieval JSON. No migration is added.

The shared dialog supports a concrete file list and Select All. Automatic numbered
selection remains conservative about duplicate alternatives. Explicit file selection
can choose either alternative, and explicitly confirmed unnumbered files can receive
selection-order library assignments. Those assignments are recorded as such and
isolated in a source-qualified namespace; they are not bibliographic numbering or
standard-edition equivalences. The approval preview shows the assigned units.

Use Release Anyway confirms source-file selection despite unresolved catalog
identity. It does not grant replacement permission. A concrete approved archive
permits best-available metadata automatically: exact MangaBaka work when available,
otherwise general MangaBaka, cached AniList, then source information. Existing
fallback code is reused, including source imagery preservation and accurate metadata
scope. Valid transfers with approved source selection do not fail solely because
exact metadata lookup failed. Legacy unresolved acquisitions without concrete source
selection still need a deterministic selection before they can proceed.

Hard blockers remain file ambiguity/change, unsupported/corrupt archive, unsafe paths,
profile/size limits, transfer failures, destination conflicts, duplicate/worker locks,
and unsafe/failed atomic publication. Catalog totals, edition/work matching and
numbering correspondence are metadata warnings once file identity is approved.
No replacement workflow, tracker writer or separate acquisition pipeline is added.

## UI quality and validation boundaries

The shared dialog uses Mango surface/ink tokens, a bounded scroll panel, wrapped
scope controls and long filenames, keyboard focus containment, Escape/cancel,
retry and stale-response protection. The source list exposes actual paths/sizes;
metadata policy is explanatory rather than internal resolver terminology. The Nyaa
browser, ownership/tracker surfaces, execution inspection and Downloads are retained.
Browser screenshots and live external acceptance were not performed; responsive and
keyboard behavior require the isolated checks below.

Regression coverage includes exact approved archive recovery despite parser
disagreement, changed IDs, changed size rejection, concrete multi-file and single-file
planning, confirmed unnumbered assignments, stale selections, shared dialog surface
contracts and multi-file API transport. Existing acquisition, integrity, publication,
metadata, transfer and normalization regressions remain part of the full suite.

## Isolated TEST acceptance

Deploy separately with the TEST helper, disposable database/library roots and Komga,
and disabled tracker writes. Record the deployed SHA. No production paths or secrets.

1. BLAME! Master: select its actual release in Lists, select all six actual supported
   files, review each path/size/assigned unit, approve. Verify six independent items,
   only those files retrieved, safe publication, exact metadata or accurately reported
   fallback, preserved images, edition/source-qualified names and TEST Komga results.
2. Open the same title in Library/detail and use its Nyaa collection/acquire action.
   Verify the identical dialog and source preselection. Select only the actual v04,
   approve and verify only that exact file is retrieved. Repeat from Lists and compare
   normalized file/edition/unit identities, ignoring generated plan IDs/timestamps.
3. Disable exact work metadata using a disposable mock. Confirm useful warnings and
   general-series/source fallback; confirm a valid selected book still publishes.
   Exercise Use Release Anyway for an unnumbered supported file, inspect the explicit
   library assignment and source namespace. No exact bibliographic facts are invented.
4. Supply two possible v04 files. Automatic unit selection must block; selecting one
   exact file must succeed. Reorder/change backend IDs, then change path/size: IDs
   must not matter, changed path/size must fail. Corrupt/traversal archives and a
   different pre-existing canonical file must still fail without replacement.
5. Witch Hat: select only v01/v02 from the collection. Verify untouched siblings,
   metadata/optimizer/ComicInfo/publication and exact Komga ownership. Repeat Blue
   Lock v39 selective retrieval and Made in Abyss partial/restart/recreation controls.
6. Check Lists and Library at phone/desktop widths, long names, 1/40 files, empty
   discovery, provider failure, unknown metadata, owned/unowned/partial titles and
   active/failed execution. Check focus, Escape, cancel, retry and stale responses.
   Downloads must show the resulting queue/history rather than another discovery UI.

Remaining limits: preview file discovery requires public metainfo or a known matching
backend listing; it never creates remote jobs. Unnumbered files need explicit consent
and library assignments; later bibliographic repair remains separate. Live provider,
filesystem and visual acceptance must be recorded in TEST, not inferred from tests.

## Recorded implementation checks

374 release-mode Rust tests passed; the focused acquisition subset is included.
81 frontend tests passed and the production build passed. Type checking reports
31 existing errors in three files (down from 35), zero warnings and no new error
messages. Rust check and all-target Clippy pass with no new warning messages.
Formatting remains the unchanged 924-hunk/55-file baseline; diff checks pass.
Three Rust regressions and one multi-file frontend transport regression were added;
the shared modal surface tests now verify both entry points. Tests use local mocks
and disposable storage, not real external accounts. No desktop changes required
desktop validation. No live/visual acceptance is claimed.
