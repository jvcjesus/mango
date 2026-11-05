# Mango M7 Production Cutover & Operational Readiness Plan

**Document Version:** 1.0.0  
**Target Release:** Mango M7 (Storage Reliability, Release Intelligence, Edition Resolution, Operations Center)  
**Target Environment:** Production (`http://10.0.0.140:8787`)  
**Staging / TEST Environment:** TEST (`http://10.0.0.140:8788`)  
**Production Container:** `mango`  
**Production Rollback Image:** `sha256:03969cd442737bf5cb3aac2bf74d62366ffb7e778e2a33e9fbe8d7a6a018d6fa`  
**Canonical Media Path:** `/srv/media/manga`  

---

## 1. Executive Summary & Production Objectives

The M7 milestone series transforms Mango into an enterprise-grade self-hosted manga acquisition, discovery, and operations control plane:
- **M7.1:** Storage reliability, staging inventory, 9-category hygiene preview, and ARM64/AMD64 portable optimizer symlinks (`RustyLibraryShrinker 1.0.3`).
- **M7.2:** Unified release availability watcher dashboard, filtering, and real-time release monitoring for tracked titles (e.g. Kagurabachi Volume 9).
- **M7.3:** MangaBaka edition resolution for ambiguous works (Bungo Stray Dogs, Holy Land, Blade of the Immortal) and OpenTome release-line retargeting (e.g. BLAME! Master Edition).
- **M7.4:** Acquisition Operations Center in UI, integrating active queue, historical backlog management, on-disk publication receipts audit verification, and safety lock observability.
- **M7.5:** Cutover plan, zero-downtime container replacement procedure, and disaster recovery protocol.

---

## 2. Pre-Cutover Checklist & Invariants

Prior to initiating any production container or database mutations, verify every item below:

### 2.1 Environmental State
- [ ] **Production Health:** Verify production container `mango` on port 8787 is healthy and serving requests.
- [ ] **Worktree Cleanliness:** Ensure `jvcjesus/mango` on branch `work` is fully tested, built, and committed.
- [ ] **Canonical Media Integrity:** Ensure `/srv/media/manga` is backed up or snapshots are current. The cutover MUST NOT alter canonical media files.
- [ ] **Staging Preservation:** Ensure active chapter downloads and staging files are not modified during cutover.

### 2.2 Operational Safety Invariants
- [ ] `acquisition.auto_acquire.enabled` = `false` (Global master switch default false).
- [ ] `acquisition.auto_acquire.dry_run` = `true` (Enforce dry-run mode for any automated evaluation).
- [ ] Opted-in titles = `0` (Zero titles configured for auto-acquisition without operator intervention).
- [ ] TorBox and Madokami execution disabled during deployment window.

### 2.3 Database Backup
- [ ] Create an atomic SQLite backup of the production database (`mango.db`) prior to any migration:
  ```bash
  sqlite3 /srv/apps/manga/data/mango.db ".backup '/srv/apps/manga/data/mango.db.backup-pre-m7-$(date +%Y%m%d%H%M%S)'"
  ```
- [ ] Verify the integrity of the backup file:
  ```bash
  sqlite3 "/srv/apps/manga/data/mango.db.backup-pre-m7-*" "PRAGMA integrity_check;"
  ```

---

## 3. Database Migration Sequence

Mango's embedded migration engine runs automatically upon container startup. Migrations up through `025_normalization_safety.sql` are forward-safe and idempotent:
- **020:** Release planning, candidate tracking, and execution tables.
- **021:** Metadata provenance and ComicInfo enrichment tracking.
- **022:** Normalization history and reversible mutation tracking.
- **023:** Durable transfer state and recovery records.
- **024:** Durable magnet provenance and torrent tracker hashes.
- **025:** Saved normalization previews and durable backup/recovery records.

### Verification of Schema Compatibility:
Execute a read-only query on the production database to check applied migrations:
```bash
sqlite3 /srv/apps/manga/data/mango.db "SELECT version, name, applied_at FROM schema_migrations ORDER BY version DESC LIMIT 5;"
```
Expected result: Version 25 is applied or ready to apply safely with zero column drops or irreversible schema transforms.

---

## 4. Step-by-Step Container Replacement Procedure

Production container replacement must follow a controlled, zero-data-loss procedure.

### Step 4.1: Build Production Image
From the repository root with production UI pre-built in `mango-reader/ui/dist`:
```bash
# 1. Build UI assets
cd /srv/apps/manga/mango-fork-test/src/mango-reader/ui
npm run build

# 2. Build Docker container image tagged for production
cd /srv/apps/manga/mango-fork-test/src
docker build -t mango:m7-prod -f Dockerfile .
```

### Step 4.2: Verify Pre-Cutover Fingerprints
Record the existing production container ID and image SHA:
```bash
docker inspect mango --format '{{.Id}} {{.Image}} {{.State.Status}}'
```

### Step 4.3: Perform Graceful Shutdown
Stop the running production container gracefully to ensure WAL checkpointing in SQLite:
```bash
docker stop -t 30 mango
```

### Step 4.4: Recreate Container with M7 Production Image
Launch the new container preserving all volume mounts, environment variables, network ports (8787), and user permissions:
```bash
docker rename mango mango-backup-pre-m7

docker run -d \
  --name mango \
  --restart unless-stopped \
  -p 8787:8000 \
  -v /srv/apps/manga/data:/root/.local/share/mango \
  -v /srv/media/manga:/srv/media/manga \
  -v /srv/media/manga-chinese:/srv/media/manga-chinese \
  -v /srv/apps/manga/staging:/staging \
  -e RUST_LOG=info \
  mango:m7-prod
```

---

## 5. Post-Cutover Sanity Checks & Acceptance Verification

Within 5 minutes of launching the M7 production container, verify the following endpoints and UI flows:

### 5.1 Service Health
```bash
curl -s http://10.0.0.140:8787/api/health | jq .
```
Expected: `{"status": "ok"}`

### 5.2 M7 New Endpoints
1. **Staging Inventory:**
   ```bash
   curl -s http://10.0.0.140:8787/api/pipeline/staging/inventory | jq .
   ```
   Verify 9 classification counts and active download safety locks.
2. **Availability Watchers Dashboard:**
   ```bash
   curl -s http://10.0.0.140:8787/api/availability/watchers/dashboard | jq .
   ```
   Verify watcher count, available/monitoring metrics, and Kagurabachi Volume 9 monitoring status.
3. **Acquisition Operations Summary:**
   ```bash
   curl -s http://10.0.0.140:8787/api/acquisition/operations/summary | jq .
   ```
   Verify `autoAcquireEnabled: false`, `autoAcquireDryRun: true`, `autoAcquireOptedInCount: 0`.
4. **MangaBaka Edition Collections:**
   ```bash
   curl -s http://10.0.0.140:8787/api/titles/1/mangabaka/collections | jq .
   ```
   Verify fallback and edition collections list.

### 5.3 Connected Integration Health
- [ ] **Komga Connectivity:** Verify Komga sync status and library ID configuration.
- [ ] **OpenTome Catalog:** Verify OpenTome cache is readable and sync runs without error.
- [ ] **TorBox Backend:** Verify TorBox client authenticates and reports available quota.
- [ ] **Madokami Backend:** Verify credentials are valid (no 401s on head checks).
- [ ] **MangaBaka API:** Verify search and collection resolution respond promptly.

---

## 6. Rollback Procedure (Emergency Recovery)

If any fatal regressions, unexpected database locks, or canonical corruption risks occur:

1. **Stop the M7 Container:**
   ```bash
   docker stop -t 10 mango
   docker rm mango
   ```

2. **Restore Previous Container:**
   ```bash
   docker start mango-backup-pre-m7
   # Or relaunch from pinned SHA:
   docker run -d \
     --name mango \
     --restart unless-stopped \
     -p 8787:8000 \
     -v /srv/apps/manga/data:/root/.local/share/mango \
     -v /srv/media/manga:/srv/media/manga \
     -v /srv/media/manga-chinese:/srv/media/manga-chinese \
     -v /srv/apps/manga/staging:/staging \
     sha256:03969cd442737bf5cb3aac2bf74d62366ffb7e778e2a33e9fbe8d7a6a018d6fa
   ```

3. **Database Restore (Only if SQLite database was corrupted):**
   ```bash
   docker stop mango
   cp /srv/apps/manga/data/mango.db.backup-pre-m7-* /srv/apps/manga/data/mango.db
   docker start mango
   ```

4. **Verify Restored Container:**
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
  ls -la /srv/apps/manga/staging
  ```
