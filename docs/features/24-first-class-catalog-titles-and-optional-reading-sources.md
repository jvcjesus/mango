# First-Class Catalog Titles & Optional Reading Sources

Milestone M6.7 establishes this permanent architectural principle:

> **A manga exists in Mango because the user cares about that work — not because a website happens to provide chapters for it.**

Previously, putting a manga into Mango required installing a Keiyoushi/Mihon reading extension (e.g., MangaDot, MangaDex), querying web chapters, and scraping hundreds or thousands of chapter rows into SQLite. Milestone M6.7 elevates Mango's existing catalog title capability (`source_id = 0`) into a first-class user workflow where reading sources are completely optional.

---

## 1. Architectural Model & Domain Separation

Mango treats these subsystems as independent concerns:

```text
MangaBaka (Catalog Identity)
   ↓
Catalog Manga (source_id = 0)
   ├── MangaBaka Identity (MangaBaka #id)
   ├── Personal Tracking (AniList / local status)
   ├── OpenTome Release Intelligence (en/ja schedules)
   ├── Komga Physical Ownership (canonical media on disk)
   ├── Nyaa / Madokami Discovery (candidate releases)
   ├── TorBox Acquisition (approved execution)
   ├── KOMF Metadata (post-publication enrichment)
   └── OPTIONAL Reading Source (MangaDot, MangaDex, etc.)
          └── Scraped chapters & reader pages
```

### Definitions:

1. **Catalog Manga:**
   A Mango manga record (`manga.source_id = 0`, `url = 'catalog:{mangabaka_id}'`) that exists independently of any reading extension. It stores author, title, description, cover, and status without scraping web chapters. It is immune to extension scraper failures, URL rot, and scraper background jobs.

2. **Reading-Source Manga / Binding:**
   An optional extension-backed content binding (`source_id > 0`) providing web chapters, scanlator attribution, and reader page navigation. A catalog title can exist without a reading source, or can have a reading source bound via `source_manga_id`.

3. **Catalog Identity:**
   MangaBaka-backed authoritative work identity (`mangabaka_id`). Exact MangaBaka identities are stored directly on the manga row at creation, bypassing fuzzy title matching ambiguity (e.g. distinguishing main series from pilot one-shots).

4. **Personal Tracking:**
   AniList and local personal catalog tracking status (`Reading`, `Planning`, `Completed`, `Paused`, `Dropped`). A work can exist in Mango without an AniList entry, and tracking does not imply ownership or permission to download.

5. **Physical Ownership:**
   Komga and the canonical manga filesystem (`/srv/media/manga`). Physical media ownership is verified strictly by canonical CBZ files on disk in Komga, independent of whether chapters exist in Mango's operational database.

6. **Release Intelligence:**
   OpenTome provides publisher schedules, market-accurate volume counts, and upcoming release dates.

7. **Acquisition Discovery:**
   Nyaa and Madokami provide candidate releases for missing physical volumes. Acquisition remains manual and guarded (M6.4) and executes via TorBox only upon explicit user approval.

---

## 2. MangaBaka Direct-Add Workflow

Users can add any manga directly from MangaBaka:
1. Navigate to **Lists** → click **[ + Add Manga ]**.
2. Search by title, MangaBaka series ID (e.g. `83`), or URL (`https://mangabaka.com/series/83`).
3. View disambiguating metadata: canonical title, aliases, authors, status, total volumes, and genres.
4. Click **[ Add to Mango ]**:
   - Creates a catalog manga (`source_id = 0`, `in_library = 1`, `mangabaka_id = {id}`).
   - Direct-add creates **0 extension chapters**.
   - Initiates initial Komga ownership check and OpenTome release mapping.
   - Idempotent: adding an already existing work opens or offers conversion rather than creating duplicate titles.
   - Navigates directly to the full `/manga/:id` title page.

---

## 3. Title Page & Chapter UI Cleanup

When viewing a catalog title (`source_id = 0`) with zero web chapters:
- Meaningless counters (`0 Chapters`, `0 Unread`, `0 Saved`) are completely suppressed.
- Empty chapter tables, uploader filter dropdowns, and download buttons are hidden.
- The reader dock at the bottom is hidden when no chapters exist.
- Instead, a clean **Reading** section is displayed:
  - If standalone: `"No reading source attached. This title is managed as a catalog-native work for library ownership, release intelligence, and acquisition."`
  - If bound to a source manga: displays the attached source name, a button to view scraped chapters, and options to revert or detach.

---

## 4. Reversible Source-to-Catalog Conversion

Existing titles with scraped chapters (such as Blue Lock with ~370 MangaDot chapters) can be safely converted into catalog-native titles:
- **Non-destructive:** The old source manga and all scraped chapters remain safely stored in SQLite. Setting `in_library = 0` on the source manga removes clutter from Library/Lists without deleting data.
- **Durable linkages:** The catalog manga stores `source_manga_id`, and the source manga stores `catalog_manga_id`.
- **Associations migrated:** Komga reconciliations, OpenTome mappings, tracked catalog entries, candidate approvals, and acquisition history are re-pointed to the new catalog manga.
- **100% Reversible Rollback:** Users can click **Revert to Source Primary** at any time. This sets `in_library = 1` on the source manga, restores primary navigation to the source manga, and re-points reconciliations back to the source ID without losing a single chapter.
- **Physical media safety:** Canonical CBZs in `/srv/media/manga` are never touched or modified during conversion or rollback.
