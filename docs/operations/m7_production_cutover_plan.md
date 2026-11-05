# Mango M7 Production Cutover & Operational Readiness Plan

**Document Version:** 1.1.0
**Target Release:** Mango M7 (Storage Reliability, Release Intelligence, Edition Resolution, Operations Center)
**Target Environment:** Production (`http://10.0.0.140:8787`)
**Staging / TEST Environment:** TEST (`http://10.0.0.140:8788`)
**Production Container Name:** `mango`
**Production Rollback Image:** `sha256:03969cd442737bf5cb3aac2bf74d62366ffb7e778e2a33e9fbe8d7a6a018d6fa` (`ghcr.io/apixelsnet/mango:full`)
**Production App Directory:** `/srv/apps/manga/mango`
**Production Compose File:** `/srv/apps/manga/mango/compose.yml`
**Production Database Path:** `/srv/apps/manga/mango/data/mango.db`
**TEST Database Path:** `/srv/apps/manga/mango-fork-test/data/mango.db`
**Canonical Media Paths:** `/srv/media/manga` (English) and `/srv/media/manga-chinese` (Chinese)
**Komga Canonical Library ID:** `0RMWEXC3T10TY`

---

## 1. Executive Summary & Production Objectives

The M7 milestone series transforms Mango into an enterprise-grade self-hosted manga acquisition, discovery, and operations control plane:
- **M7.1:** Storage reliability, staging inventory, 9-category hygiene preview, strict 12-factor duplicate verification, and ARM64/AMD64 portable optimizer discovery (`RustyLibraryShrinker 1.0.3`).
- **M7.2:** Unified release availability watcher dashboard, release filtering, and real-time release monitoring for tracked titles (e.g. Kagurabachi Volume 9).
- **M7.3:** MangaBaka edition resolution for ambiguous works (Bungo Stray Dogs, Holy Land, Blade of the Immortal) and OpenTome release-line retargeting (e.g. BLAME! Master Edition).
- **M7.4:** Acquisition Operations Center in UI, integrating active queue, historical backlog management, on-disk publication receipts audit verification, and safety lock observability.
- **M7.5:** Cutover plan, zero-downtime container replacement procedure, and disaster recovery protocol.
- **M7.6:** Safety hardening, storage verification corrections, fail-closed duplicate cleanup preview, and ARM64 optimizer runtime diagnostics.

---

## 2. Pre-Cutover Checklist & Operational Invariants

Prior to initiating any production container or database mutations, verify every item below:

### 2.1 Environmental State
- [ ] **Production Health:** Verify production container `mango` on port 8787 is healthy and serving requests:
  ```bash
  curl -s http://10.0.0.140:8787/api/health | jq .
  ```
- [ ] **Worktree Cleanliness:** Ensure `jvcjesus/mango` on branch `work` is fully tested, built, and committed.
- [ ] **Canonical Media Immutability:** Verify canonical media (`/srv/media/manga` and `/srv/media/manga-chinese`) is untouched. The cutover MUST NEVER alter canonical media files.
- [ ] **Staging Preservation:** Ensure active chapter downloads and staging files are not modified during cutover.
- [ ] **Queue Quiescence:** Ensure no active transfers or acquisitions are inflight (`downloads.paused` verified).

### 2.2 Operational Safety Invariants
- [ ] `acquisition.auto_acquire.enabled` = `false` (Global master switch strictly false).
- [ ] `acquisition.auto_acquire.dry_run` = `true` (Enforce dry-run mode for any automated evaluation).
- [ ] Opted-in titles = `0` (Zero titles configured for auto-acquisition without operator intervention).
- [ ] Real deletion disabled: Staging cleanup preview runs with `execution_enabled = false`.
- [ ] External acquisition backends (TorBox / Madokami) paused during deployment window.

### 2.3 Atomic Database Backup
- [ ] Create an atomic SQLite online backup of the production database (`/srv/apps/manga/mango/data/mango.db`) prior to any deployment:
  ```bash
  sqlite3 /srv/apps/manga/mango/data/mango.db ".backup '/srv/apps/manga/mango/data/mango.db.backup-pre-m7-$(date +%Y%m%d%H%M%S)'"
  ```
- [ ] Verify the integrity of the backup file:
  ```bash
  sqlite3 "/srv/apps/manga/mango/data/mango.db.backup-pre-m7-*" "PRAGMA integrity_check;"
  ```
  Expected output: `ok`.

---

## 3. Database Schema Migration Sequence (001 through 041)

Mango uses SQLite's built-in `PRAGMA user_version` mechanism to track applied migrations in lockstep with `mango-reader/src/db.rs`. There is no auxiliary `schema_migrations` table; the schema version directly equals the number of executed migrations.

The full migration sequence comprises 41 migrations:
1. `001_init.sql` — Initial schema and user authentication
2. `002_categories.sql` — Category hierarchies
3. `003_settings.sql` — Application settings key-value store
4. `004_reader.sql` — Reader progress and session state
5. `005_downloads.sql` — Download queue and status tracking
6. `006_opds.sql` — OPDS catalog feeds
7. `007_sources.sql` — External source mappings
8. `008_plugins.sql` — Plugin configuration
9. `009_auth_tokens.sql` — Token authentication
10. `010_notifications.sql` — Notification history
11. `011_custom_covers.sql` — Custom cover overrides
12. `012_sync_identities.sql` — Tracker identities and external mappings
13. `013_sync_totals.sql` — Canonical tracker totals
14. `014_sync_lists.sql` — Personal tracked list catalog
15. `015_sync_provenance.sql` — Tracker synchronization provenance
16. `016_komga_verification.sql` — Exact ownership verification
17. `017_nyaa_torbox.sql` — Torrent discovery and cloud acquisition
18. `018_collections.sql` — Multi-volume collections and mapping
19. `019_planning.sql` — Approved acquisition plans
20. `020_execution.sql` — Pipeline execution records
21. `021_metadata_provenance.sql` — MangaBaka metadata provenance and ComicInfo policy
22. `022_normalization_history.sql` — Pre-Mango normalization and reversible backups
23. `023_transfer_state.sql` — Durable transfer state and restart recovery
24. `024_durable_magnet_provenance.sql` — Magnet URI and infohash persistence
25. `025_normalization_safety.sql` — Saved normalization previews and recovery fields
26. `026_download_failure_history.sql` — Durable download error logging
27. `027_external_torbox_jobs.sql` — Shared TorBox background job lifecycle
28. `028_nyaa_search_provenance.sql` — Discovery query provenance and candidate caching
29. `029_madokami_acquisitions.sql` — Madokami direct HTTP download tracking
30. `030_reconciliation_schedule.sql` — Periodic tracker reconciliation engine
31. `031_ntfy_notifications.sql` — Deduplicated ntfy alert dispatch
32. `032_mangabaka_collections.sql` — MangaBaka multi-edition collection mapping
33. `033_opentome_release_schedule.sql` — OpenTome release-line retargeting
34. `034_acquisition_recovery_attempts.sql` — Transient acquisition recovery tracking
35. `035_guard_historical_backlog.sql` — Backlog eligibility gating and boundary checks
36. `036_candidate_quality_intelligence.sql` — Multi-factor candidate release scoring
37. `037_title_diagnostics.sql` — Unified series health diagnostics and ownership audits
38. `038_release_availability_watchers.sql` — Release availability monitoring
39. `039_release_monitoring_events.sql` — Release transition logging and deduplication
40. `040_publication_receipts.sql` — Durable on-disk publication verification receipts
41. `041_backlog_acquisitions.sql` — Backlog acquisition lifecycle and operator approvals

### Schema Verification Command:
Verify current schema version:
```bash
sqlite3 /srv/apps/manga/mango/data/mango.db "PRAGMA user_version;"
```
Expected prior to M7: <= 40 (or legacy).
Expected after M7 startup: `41`.

All migrations from 026 to 041 are purely additive (creating new tables or adding nullable/defaulted columns). None drop tables or rewrite existing data, ensuring forward and backward database compatibility.

---

## 4. Production Deployment Procedure

Production deployment uses Docker Compose to ensure correct SELinux volume relabeling (`:Z`), network bindings, and restart policies.

### Step 4.1: Build Production Image
From the source repository with production UI built:
```bash
# 1. Build UI assets
docker run --rm -v /srv/apps/manga/mango-fork-test/src/mango-reader/ui:/ui -w /ui node:22-slim npm run build

# 2. Build Docker container image tagged for production
cd /srv/apps/manga/mango-fork-test/src
docker build -t mango-fork:full -f Dockerfile .
```

### Step 4.2: Verify Pre-Cutover Container Fingerprint
```bash
docker inspect mango --format '{{.Id}} | {{.Image}} | {{.State.StartedAt}}'
```
Record this fingerprint for change-control documentation.

### Step 4.3: Update Production Compose File
In `/srv/apps/manga/mango/compose.yml`, verify service configuration:
```yaml
services:
  mango:
    image: mango-fork:full
    container_name: mango
    restart: unless-stopped
    ports:
      - "8787:8787"
    environment:
      TZ: America/Chicago
      RUST_LOG: info
    volumes:
      - /srv/apps/manga/mango/data:/data:Z
      - /srv/media/manga:/data/local/English:ro
      - /srv/media/manga-chinese:/data/local/Chinese:ro
```

### Step 4.4: Graceful Container Replacement
```bash
cd /srv/apps/manga/mango
docker compose up -d --force-recreate mango
```
This performs an atomic container recreation while preserving `/srv/apps/manga/mango/data` and media volume mounts.

---

## 5. Post-Cutover Sanity Checks & Acceptance Verification

Within 5 minutes of launching the updated production container, verify the following endpoints:

### 5.1 Service Health & Schema Version
```bash
# Health endpoint
curl -s http://10.0.0.140:8787/api/health | jq .
# Expected: {"status": "ok"}

# Database schema version check
sqlite3 /srv/apps/manga/mango/data/mango.db "PRAGMA user_version;"
# Expected: 41
```

### 5.2 M7 Core Endpoints
1. **Staging Inventory & Duplicate Verification:**
   ```bash
   curl -s http://10.0.0.140:8787/api/pipeline/staging/inventory | jq .
   ```
   Verify 9 classification counts and active download safety locks.
2. **Availability Watchers Dashboard:**
   ```bash
   curl -s http://10.0.0.140:8787/api/availability/watchers/dashboard | jq .
   ```
   Verify watcher count, monitoring metrics, and Kagurabachi Volume 9 monitoring status.
3. **Acquisition Operations Summary:**
   ```bash
   curl -s http://10.0.0.140:8787/api/acquisition/operations/summary | jq .
   ```
   Verify `autoAcquireEnabled: false`, `autoAcquireDryRun: true`, `autoAcquireOptedInCount: 0`.
4. **Optimizer Runtime Diagnostics:**
   ```bash
   curl -s http://10.0.0.140:8787/api/pipeline/config | jq .optimizer_status
   ```
   Verify `code: "valid"`, `is_operational: true`, `detected_architecture: "aarch64"`.

### 5.3 Connected Integration Health
- [ ] **Komga Sync:** Verify Komga connection and library ID (`0RMWEXC3T10TY`).
- [ ] **KOMF Connectivity:** Verify KOMF endpoint responsiveness.
- [ ] **OpenTome Catalog:** Verify OpenTome cache is readable and sync runs without error.
- [ ] **TorBox Backend:** Verify TorBox client authenticates and reports available quota.
- [ ] **Madokami Backend:** Verify credentials are valid (no 401s on head checks).
- [ ] **MangaBaka API:** Verify search and collection resolution respond promptly.

---

## 6. Rollback Procedures (Separated: Application vs Database)

The rollback procedure is divided into two distinct levels depending on the failure mode:

### Level A: Application Rollback (Non-Destructive — Standard Procedure)
Used when a bug, UI regression, or runtime issue is found in the new container binary, but the SQLite database is healthy. Because migrations 001–041 are strictly additive, the previous image can run directly against schema version 41 without rolling back the database.

1. **Revert compose image to pinned rollback SHA:**
   Edit `/srv/apps/manga/mango/compose.yml` to specify:
   ```yaml
   image: sha256:03969cd442737bf5cb3aac2bf74d62366ffb7e778e2a33e9fbe8d7a6a018d6fa
   ```
2. **Recreate the container:**
   ```bash
   cd /srv/apps/manga/mango
   docker compose up -d --force-recreate mango
   ```
3. **Verify restored health:**
   ```bash
   curl -s http://10.0.0.140:8787/api/health
   ```

### Level B: Database Rollback (Disaster Recovery Only)
Used ONLY if SQLite database file corruption occurs or an unrecoverable data mutation took place.

1. **Stop the production container:**
   ```bash
   cd /srv/apps/manga/mango
   docker compose stop mango
   ```
2. **Restore pre-cutover database backup:**
   ```bash
   # Move corrupted database to quarantine
   mv /srv/apps/manga/mango/data/mango.db /srv/apps/manga/mango/data/mango.db.corrupted-$(date +%Y%m%d%H%M%S)
   # Restore clean backup
   cp /srv/apps/manga/mango/data/mango.db.backup-pre-m7-* /srv/apps/manga/mango/data/mango.db
   # Verify restored integrity
   sqlite3 /srv/apps/manga/mango/data/mango.db "PRAGMA integrity_check;"
   ```
3. **Restart container:**
   ```bash
   docker compose start mango
   ```
4. **Verify restored state:**
   ```bash
   curl -s http://10.0.0.140:8787/api/health
   ```

---

## 7. Operational Contacts & Monitoring

- **Log Monitoring Command:**
  ```bash
  docker logs -f --tail 100 mango
  ```
- **Error Filtering:**
  ```bash
  docker logs mango 2>&1 | grep -iE 'error|panic|fatal|corrupt'
  ```
- **Staging Directory Inspection:**
  ```bash
  ls -la /srv/media/downloads/mango-test
  ```
