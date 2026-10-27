# Interactive planning, edition intent and identity override

The native acquisition architecture is superseded by
[physical acquisition and import](21-physical-import.md). This document retains
historical implementation/acceptance evidence and compatibility behavior.

For current approved-file semantics and the shared Lists/Library workflow, see
[approved source acquisition](20-approved-source-acquisition.md). Strict metadata
prerequisites below describe historical behavior superseded for approved files.

This milestone extends the frozen plan/execution and retrieval JSON from migration
026. It does not deploy Mango, access production or authorize filesystem replacement.
See [edition-aware retrieval](17-edition-aware-acquisition.md) for the underlying
file-resolution rules. The planner UI limitations in that earlier milestone are
superseded below.

## Diagnosed planner lifecycle and identity faults

The Lists dialog already had `try/finally`, but it awaited HTTP requests with no
bounded lifecycle. Nyaa send/body reads could remain pending indefinitely; there
was no browser cancellation/deadline, request generation token or stale-response
check. Loading source choices was also awaited before starting discovery. Failed
Nyaa queries were swallowed into empty results by the older service path.

Additional concrete contract faults prevented a usable settled plan:

- The by-title endpoint ignored source, content mode and requested units.
- It used `tracked_catalog.id` as `manga.id`, even though those are separate tables.
- It returned an unsaved dry-run plan, which could not subsequently be approved.
- Lists rendered fields absent from `AcquisitionPlan`/normalized candidates,
  including `smartScore.total`, `releaseTitle`, `expectedUnits`, `unresolvedUnits`
  and `warnings`. A successful response could therefore throw during rendering.
- Closing/changing the modal did not invalidate pending requests. Older responses
  could replace a newer title/plan or clear its loading state.

These are code-level root causes, reproduced with deterministic lifecycle tests.
No live request trace was collected for the reported Ravages of Time screenshot;
its particular slow provider/network event is not asserted as live acceptance.

Known source edition was separately lost because normalized candidates described
source edition while requested edition came only from the catalog title. Lists
sent neither edition intent nor a selected release. The planner rejected Master
releases against unspecified catalog intent, and execution faithfully persisted
that unspecified request. The resolver itself was correctly failing closed.

## Bounded planning and Auto

`acquisition/interactive.rs` uses the existing acquisition plan and normalized
candidate models; the response adds discovered candidates, known editions and
source diagnostics. There is no second edition/catalog database.

The endpoint now finds/creates a real acquisition manga row, links tracked catalog
state to its manga ID, honors source/content/unit input, discovers candidates and
saves a reviewable plan. Saving a preview never queues a download. Approval and
execution remain separate, explicit operations; the dialog starts the approved
execution only after approval succeeds. Repeat approval of one saved plan returns
its existing execution.

Interactive provider/ownership work is bounded at **12 seconds**, and the whole
planning operation at **30 seconds**. Nyaa requests use the shared provider bound,
including body reads. A synchronous coverage guard rejects oversized/invalid
provider ranges before ranking can expand them. User volume lists are limited to
500 valid edition-local units.

Auto queries Nyaa once and can continue with locally known, explicitly
volume-labelled reader/MangaDot candidates. An unavailable or timed-out Nyaa
provider is retained in diagnostics when another source supplies a viable plan;
without viable alternatives it returns an actionable discovery failure. Successful
empty responses remain no-results, not provider errors. This does not implement
new extension-host network discovery; unsupported interactive sources are not
advertised as available volume providers.

The browser has a **35-second** deadline as an outer guard, AbortController and a
request generation token. It settles success, no-results, provider/validation
errors and timeout. Cancel/close/Escape/unmount abort the transport and invalidate
old results. Changing source/content/edition invalidates the previous plan; stale
results cannot replace current state. Retry uses a fresh request without reopening
the modal. Source-choice loading is independent and bounded. Server operations may
finish a bounded unused preview after client cancellation; cancellation never
approves or enqueues it.

## Edition selection and frozen intent

The compact edition control appears when discovered edition evidence matters.
Choices come from detected/provider evidence; an unmarked release is not presented
as an invented authoritative Standard Edition. Ordinary acquisitions keep the
usual strict flow, with source/identity choices under a compact details section.

Explicit selection of a confidently identified Master release supplies normalized
`requestedEdition: master`. That intent is present in the saved plan candidate,
approved execution snapshot, per-unit queue identity, retry/restart/recreated job,
metadata, qualified canonical namespace and publication history. The dialog also
shows the selected known edition. Changing edition regenerates the plan. Provider
results that fail the strict catalog-title gate remain available only as review candidates, marked
`requiresIdentityOverride`; automatic planning excludes them and preflight
requires explicit override consent. Existing known aliases remain available to
strict ranking.

Unnumbered collections require explicit edition-local requested volumes. Standard
tracker totals are never reused as Master/Omnibus totals. A complete nonstandard
release can use its own explicit coverage; absent that evidence, the plan requests
specific units. This also applies to every overridden release, including an
unknown edition: use explicit requested units or source coverage, never standard
catalog totals. A planned requested unit is intent, not invented source coverage:
archive retrieval must still corroborate it.

Source identity confidence is evaluated independently of health/availability.
A known cached release does not require an identity override merely because its
availability needs review; live seed/profile and acquisition-size limits still
apply. Local reader fallback uses the shared release classifier, so a chapter
labelled as belonging to Volume 4 cannot become a whole Volume 4 acquisition.

New interactive collections freeze independent per-volume execution items. Legacy
plan/execution shape is retained. Destination language is carried from the frozen
candidate into retrieval JSON so an acquisition catalog row without an extension
source cannot lose its configured language during queue recovery.

## Use Release Anyway

The existing planner exposes **Use Release Anyway** for an explicitly selected
release. A confirmation names the series, release, detected edition, unverified
catalog edition and series-level fallback policy. Confirming regenerates a plan;
it does not enqueue it. Final approval must separately send `overrideConfirmed`.
The backend rejects approval of an override plan without that explicit consent.

Override can accept uncertainty about catalog/edition association and permit
series/source metadata when an exact edition/work cannot be verified. It does not
bypass an explicit known edition conflict. It cannot convert chapter to volume or
standard to omnibus numbering.

It cannot bypass archive type restrictions, parsed unit/file identity, selected
path/size corroboration, corrupt ZIP data/checksums, unsafe entry paths/symlinks,
destination/language/disk checks, worker locks, idempotency, atomic publication or
existing-file collision protection. Seed/profile integrity and inferred
chapter-to-volume prohibitions remain enforced.

Unknown source editions use a source-release-qualified namespace under override,
not the ordinary Standard namespace. Known Master/Deluxe/etc. retain the existing
edition-qualified naming policy, ComicInfo Series and Komga verification title.

## Metadata and source authority

Strict acquisitions use authoritative exact edition/work metadata. A missing
exact identity produces `identity_unverified`, with an explanation to review Use
Release Anyway. Normal edition mismatch protection remains in the resolver.

For a confirmed override, the order is:

1. Exact confidently matched MangaBaka edition/work metadata.
2. General MangaBaka series metadata: unique local cache first, then a bounded
   public lookup using a stable ID or unique exact series match.
3. General descriptive AniList data when available in the local tracker cache.
4. Source/release information and existing descriptive ComicInfo fields.

Fallback never invents a work/collection ID, edition publication date, page count,
publisher or standard-volume total. AniList supplies general description/genres
where available; it is not an edition authority. No tracker progress is written.

Catalog cover images are never substituted for source images. ComicInfo rewriting
preserves source page/cover entries with valid image indices and retains missing
source descriptive tags in override mode. Actual archive image count controls
PageCount. Existing optimizer policy remains active; it may optimize source image
encoding, but does not replace it with generic catalog artwork.

Retrieval JSON extends the existing provenance with `identityOverride`,
`overrideApprovedAt`, `metadataConfidence` and language. Confidence distinguishes
`exact_edition`, `series_fallback:mangabaka`, `series_fallback:anilist` and source
identity. Consent is separately retained in the frozen candidate/execution and
queue/history JSON. ComicInfo Notes identify override/source provenance. Fallback
is never reported as an exact MangaBaka edition match.

An end-to-end test exposed an older completion ordering gap: the pipeline could
delete downloads before worker history copied retrieval JSON. Publication now
snapshots provenance into existing acquisition history before cleanup, within the
publication-state transaction. Completion/recovery settle that durable row before
removing queue state. Worker history uses the returned canonical destination,
not the staging path, and updates the captured lifecycle row idempotently.

No schema migration is needed: these are backward-compatible optional fields in
migration 026's JSON and existing history/provider fields. Old JSON defaults to
strict behavior. A later accepted attempt does not rewrite the metadata/provenance
of an earlier rejected attempt.

For the follow-up source-local unit discovery and graphical archive chooser, see
[source-local acquisition units](19-source-local-acquisition-units.md). The archive
chooser limitations below describe this milestone's historical state.

## Archive choice and collision behavior

Ambiguous multi-file releases still fail with an actionable request for exact file
selection. There is no archive chooser UI in this milestone. The by-title API can
carry the existing `selectedArchive: {path,size}` representation with one selected
release and one unit. It is frozen into the plan, approved item, downloads and
history. Actual TorBox listing must corroborate the path, nonzero size, unit and
edition; stale/changed selections fail closed. Unlabelled files are not assigned
volume numbers merely by selecting a position. No ordering/count fallback exists.

Requeueing the same selected artifact preserves partial-transfer identity. A
confirmed metadata-override upgrade can retain the same artifact, but changing its
unit, source, selected path/size or torrent identity is still rejected.

Different existing canonical files stop with `destination_conflict`. Identity
consent is not **Replace Existing File** permission; no replacement action/API is
introduced. Existing maintenance/replacement permissions remain separate. Identical
artifacts are no-ops. Publication uses unique verified same-filesystem staging and
an atomic no-replace hard link, closing the prior exists/rename race. Filesystems
that cannot support that operation fail closed; verify compatibility in TEST.
No canonical media is renamed or replaced by this implementation task.

## Deterministic validation

Final validation: 356 release-mode Rust tests passed; 18 focused Rust regressions
passed across planning, approval/restart, Auto fallback, metadata, source content,
selection and integrity/publication. Frontend tests passed (78); the production UI
build passed. Type checking reports 35 pre-existing errors in four files, down
from the freshly observed 45; diagnostic comparison found no new messages.
`cargo check --locked` and all-target Clippy pass with no new warning messages.
Formatting remains a pre-existing repository failure (924 hunks in 55 files,
versus the previous 925-hunk/55-file baseline); changed code was formatted without
broad cleanup. `git diff --check` passes. Tests use local HTTP/SQLite/archive
fixtures and require no production credentials.

Files changed: `src/acquisition/{edition,execution,interactive,mod,nyaa,service,
tests,torbox}.rs`, `src/{api,downloads}.rs`, `src/pipeline/{mangabaka,metadata,mod,
publisher}.rs`, `ui/src/lib/{api,planning,planning.test}.ts`, `ui/src/routes/Lists.svelte`
(all under `mango-reader/`), and feature documents 17/18. No dependency or SQL schema
migration was added. Eighteen Rust tests and nine frontend tests were added;
existing regression assertions were retained.

The tests cover planner terminal states, cancellation and stale responses, retry,
source errors/timeouts with viable Auto fallback, distinct catalog/manga IDs,
known Master intent, frozen approval/consent, queue and restart provenance,
metadata fallback confidence, source cover/pages, explicit selected archive
persistence/staleness, strict mismatches, unsupported/corrupt/unsafe archives,
no silent replacement and concurrent publication. The complete suite retains the
Witch Hat/Blue Lock/Made in Abyss, TorBox recreation/sibling, .part/Range/200/206/416,
optimizer, ComicInfo, Komga, worker and Unicode regressions.

## Separate isolated TEST acceptance

No live acceptance or deployment was performed. Use the separate TEST deployment
helper only after this implementation phase. TEST must use disposable SQLite,
staging/canonical roots and Komga library IDs, with tracker writes disabled and
production paths/credentials inaccessible. Record the deployed commit. Capture
safe diagnostic summaries, never tokens, magnets or signed download URLs.

1. **BLAME! Master normal:** Lists → BLAME! → Add to Library → Specific Volumes `4`.
   Discover and explicitly select the reported Master Edition release; verify the
   compact edition is Master and leave override off. Approve. Inspect the saved
   plan, frozen item and queue JSON for `requestedEdition: master`, Volume 4 and
   strict mode. Only a corroborated Master v04 archive may enter Mango. Verify
   authoritative Master work metadata, qualified canonical/ComicInfo series and
   exact Komga verification. Inspect history after queue cleanup. Restart/retry a
   disposable expired TorBox job and confirm path/size identity survives new IDs.
   If actual filenames do not prove v04, expect actionable file-selection failure.
2. **BLAME! override:** point TEST metadata at a disposable mock that identifies
   the series but has no Master collection (or use an equivalent real unrepresented
   edition). Strict mode must stop at `identity_unverified`. Reopen/review the same
   release and Volume 4, choose Use Release Anyway, read the named confirmation,
   confirm, then separately approve. Verify consent/time and actual fallback scope
   in frozen/queue/history provenance. No standard totals/date/work facts may be
   invented. Check original cover/page imagery, group/source notes, qualified
   namespace and Komga result. Retry the failed same-artifact job without deleting
   partial data. Also test MB unavailable + cached AniList general data, then no
   descriptive provider data: confidence must accurately degrade to source identity.
3. **Genuine mismatch:** send/select Standard intent against the Master release
   with Volume 4, including `useReleaseAnyway: true`. Expect `edition_mismatch`;
   no retrieval/publication. Override must not act as an edition conversion.
4. **Ambiguous files:** use a disposable torrent/mock with two supported Master
   v04 alternatives. Strict and override automatic resolution must both fail;
   neither file order nor count selects one. Through the existing planning API,
   supply one exact `selectedArchive` path/size for one selected release/unit,
   review/approve and verify only that archive is retrieved. Recreate the remote
   job with reordered/new IDs: unchanged path/size succeeds, changed path/size
   fails. Unlabelled identity still fails. A graphical chooser remains future work.
5. **Existing collision/integrity:** pre-place a different disposable canonical
   file, then attempt override to that destination. Expect destination_conflict
   and unchanged existing bytes. Replacement remains a separate permission; do not
   perform it here. Corrupt, traversal-path and unsupported archives must fail in
   both modes. Confirm atomic no-replace support on TEST's actual filesystem.
6. **Witch Hat control:** discover the v01–14 collection, select only Volumes 1/2,
   leave strict mode on. Verify separate requested execution units, exact archive
   retrieval, untouched siblings, existing metadata/optimizer/Komga behavior and
   standard naming. Follow with selective Volume 39 from a Blue Lock-style fixture.
7. **Lists Auto:** open The Ravages of Time → Add to Library → Auto. Verify ready,
   no-results or actionable failure by the server/browser bounds, never an endless
   spinner. Change content/source/edition rapidly; older results must not replace
   the current plan. Approve a ready plan and confirm a saved execution starts once.
8. **Provider timeout/error:** use a disposable TEST RSS mock that responds 502,
   then one that holds the response beyond 12 seconds. With known local volume
   candidates, Auto should return those plus a Nyaa diagnostic. Without alternatives
   it should show a retryable failure. Restore the mock and click Try Again without
   reopening: a new plan should settle. Unsupported sources should be unavailable.
9. **Cancel during discovery:** hold the RSS mock, open the modal and cancel/close
   or press Escape. Loading must stop, focus must return, and no execution/download
   may be created. Immediately open another title and release the old response:
   it must not replace the new title/plan. Repeat by switching content/edition and
   by navigating away. Preserve the opaque modal, focus trap and keyboard behavior.

Remaining limits: no graphical archive chooser, no authoritative cross-edition
conversion, no file replacement workflow, no new remote extension discovery and
no live TEST evidence. Interactive plans have bounded coverage. Cancellation may
leave an unused saved preview, never an approved acquisition. The exact external
provider behavior and filesystem compatibility must be accepted separately in TEST.
