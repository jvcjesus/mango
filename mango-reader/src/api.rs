//! The HTTP API under `/api`. JSON in and out; errors are `{"error": {"code", "message"}}`.
//! Everything else the server is asked for is the web UI.

use crate::{
    auth::{self, Account, AccountChange, Auth, Invite, Who},
    db::Db,
    downloads::{Downloads, QueueView},
    error::{AppError, Result},
    export, files,
    host::{Host, Solver},
    images::{Image, Images},
    import::{self, Importer, Preview},
    library::{self, Bulk, Category, LibraryView},
    migrate::{self, Comparison, Outcome},
    opds, settings,
    spaces::Spaces,
    store::{CatalogItem, CatalogQuery, Installed, StoreInfo, Stores},
    titles::{Browse, Listing, ReaderView, TitleView, Titles},
    trackers::{self, Change, Credentials, Kind, Pick, Track, Trackers},
    ui,
    updates::{Status, Updates, UpdatesView},
    version::{self, Releases},
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, RawQuery, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use flate2::{Compression, write::GzEncoder};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{io::Write, sync::Arc};

/// For answers whose address changes when their content does.
const FOREVER: &str = "private, max-age=31536000, immutable";
/// Text shorter than this goes out as it is: gzip would save next to nothing on it.
const GZIP_FROM: usize = 1024;

/// What a request works with: the library of the account it comes from, and what
/// all accounts share. `spaces.rs` keeps one per account.
#[derive(Clone)]
pub struct AppState {
    /// The account asking.
    pub me: Who,
    pub db: Db,
    pub auth: Arc<Auth>,
    pub releases: Arc<Releases>,
    pub host: Arc<Host>,
    pub stores: Stores,
    pub titles: Titles,
    pub images: Images,
    pub downloads: Downloads,
    pub updates: Updates,
    pub importer: Importer,
    pub trackers: Trackers,
    pub pipeline: Arc<crate::pipeline::Pipeline>,
    pub sync: Arc<crate::sync::service::SyncService>,
    pub acquisition: Arc<crate::acquisition::service::AcquisitionService>,
    pub notification: Arc<crate::notification::NotificationService>,
    pub opentome: Arc<crate::opentome::service::OpenTomeService>,
    /// The local source's folder, shown in Settings so the owner knows where to put files.
    pub local_dir: String,
}

/// The library of whoever is asking. `auth::require` put it there.
pub struct Me(pub AppState);

impl<S: Send + Sync> FromRequestParts<S> for Me {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self> {
        parts
            .extensions
            .remove::<AppState>()
            .map(Me)
            .ok_or_else(auth::signed_out)
    }
}

pub fn router(state: Spaces) -> Router {
    // Everything here wants a signed-in browser; `/auth` below is what works without one.
    let private = Router::new()
        .route("/health", get(health))
        .route("/version", get(about))
        .route("/version/check", post(check_version))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/users", get(list_users).post(add_user))
        .route("/users/{id}", put(edit_user).delete(remove_user))
        .route("/invites", get(list_invites).post(add_invite))
        .route("/invites/{id}", delete(remove_invite))
        .route("/solver", get(get_solver).put(put_solver))
        .route("/stores", get(list_stores).post(add_store))
        .route("/stores/refresh", post(refresh_stores))
        .route("/stores/{id}", delete(remove_store))
        .route("/stores/{id}/refresh", post(refresh_store))
        .route("/stores/{id}/accept-key", post(accept_key))
        .route("/catalog", get(catalog))
        .route("/catalog/{store}/{pkg}", get(catalog_item))
        .route("/extensions", get(installed))
        .route("/extensions/update-all", post(update_all))
        .route("/extensions/{pkg}", delete(remove_extension))
        .route("/extensions/{pkg}/install", post(install))
        .route("/sources", get(sources))
        .route("/sources/{id}", put(change_source))
        .route(
            "/sources/{id}/preferences",
            get(source_preferences).put(set_source_preference),
        )
        .route("/sources/{id}/verify", post(verify_source))
        .route("/sources/{id}/filters", get(source_filters))
        .route("/sources/{id}/{kind}", get(browse))
        .route("/manga/{id}", get(title))
        .route("/manga/{id}/refresh", post(refresh_title))
        .route("/manga/{id}/ownership", get(manga_ownership))
        .route(
            "/manga/{id}/ownership/refresh",
            post(refresh_manga_ownership),
        )
        .route(
            "/manga/{id}/refresh-ownership",
            post(refresh_manga_ownership),
        )
        .route(
            "/manga/{id}/follow",
            get(get_manga_follow).post(post_manga_follow),
        )
        .route("/manga/{id}/unfollow", post(post_manga_unfollow))
        .route("/manga/{id}/reconciliation", get(get_manga_reconciliation))
        .route("/manga/{id}/library-state", get(get_manga_library_state))
        .route("/manga/{id}/reconcile", post(post_manga_reconcile))
        .route("/manga/{id}/retry-metadata", post(post_manga_retry_metadata))
        .route(
            "/reconciliation/followed",
            get(get_followed_reconciliations).post(post_run_followed_reconciliations),
        )
        .route("/manga/{id}/volumes/{volume}/candidates", get(get_manga_volume_candidates))
        .route("/manga/{id}/volumes-manager", get(get_manga_volumes_manager))
        .route("/titles/{id}/volumes-manager", get(get_manga_volumes_manager))
        .route("/manga/{id}/volumes-manager/plan", post(post_manga_volumes_manager_plan))
        .route("/titles/{id}/volumes-manager/plan", post(post_manga_volumes_manager_plan))
        .route("/manga/{id}/volumes-manager/acquire", post(post_manga_volumes_manager_acquire))
        .route("/titles/{id}/volumes-manager/acquire", post(post_manga_volumes_manager_acquire))
        .route(
            "/manga/{id}/volumes/{volume}/candidates/inspect",
            get(get_manga_volume_candidates_inspect),
        )
        .route(
            "/manga/{id}/volumes/{volume}/compare-candidates",
            get(get_manga_volume_candidates_compare),
        )
        .route("/manga/{id}/volumes/{volume}/candidates/{candidate_id}/acquire", post(post_manga_volume_candidate_acquire))
        .route("/manga/{id}/volumes/{volume}/candidates/{candidate_id}/review", get(get_manga_volume_candidate_review))
        .route("/manga/{id}/volumes/{volume}/discover", post(post_manga_volume_discover))
        .route("/manga/{id}/discover-missing", post(post_manga_discover_missing))
        .route("/manga/{id}/discovery", get(get_manga_series_discovery))
        .route("/notifications/config", get(get_notification_config).post(post_notification_config))
        .route("/notifications/test", post(post_notification_test))
        .route("/notifications/outbox", get(get_notification_outbox))
        .route("/opentome/status", get(get_opentome_status))
        .route("/opentome/update", post(post_opentome_update))
        .route("/opentome/config", post(post_opentome_config))
        .route("/manga/{id}/opentome", get(get_manga_opentome))
        .route("/manga/{id}/opentome/remap", post(post_manga_opentome_remap))
        .route("/manga/{id}/release-discovery", get(get_manga_release_discovery))
        .route("/manga/{id}/release-discovery/check", post(post_manga_release_discovery_check))
        .route(
            "/manga/{id}/release-discovery/targets/{target_id}/retry",
            post(post_manga_release_discovery_retry),
        )
        .route(
            "/manga/{id}/release-discovery/targets/{target_id}/evaluate-auto",
            post(post_manga_release_discovery_evaluate_auto),
        )
        .route(
            "/manga/{id}/auto-acquire",
            get(get_manga_auto_acquire).put(put_manga_auto_acquire),
        )
        .route(
            "/settings/auto-acquire",
            get(get_auto_acquire_settings).put(put_auto_acquire_settings),
        )
        .route("/catalog/mangabaka/search", get(get_catalog_mangabaka_search))
        .route("/catalog/mangabaka/{id}/add", post(post_catalog_mangabaka_add))
        .route("/manga/{id}/convert-to-catalog", post(post_manga_convert_to_catalog))
        .route("/manga/{id}/revert-to-source", post(post_manga_revert_to_source))
        .route("/manga/{id}/detach-source", post(post_manga_detach_source))
        .route("/manga/{id}/attach-source/{source_id}", post(post_manga_attach_source))
        .route("/manga/{id}/mangabaka", put(put_manga_mangabaka))
        .route("/manga/{id}/diagnostics", get(get_manga_diagnostics))
        .route("/titles/{id}/diagnostics", get(get_manga_diagnostics))
        .route("/manga/{id}/timeline", get(get_series_timeline_endpoint))
        .route("/titles/{id}/timeline", get(get_series_timeline_endpoint))
        .route("/manga/{id}/gaps", get(get_series_gaps_endpoint))
        .route("/titles/{id}/gaps", get(get_series_gaps_endpoint))
        .route(
            "/manga/{id}/mapping/manual",
            post(post_manga_mapping_manual).delete(delete_manga_mapping_manual),
        )
        .route(
            "/catalog/mangabaka/{id}/collections",
            get(get_catalog_mangabaka_collections),
        )
        .route(
            "/titles/{id}/mangabaka/collections",
            get(get_title_mangabaka_collections),
        )
        .route(
            "/manga/{id}/mangabaka/collections",
            get(get_title_mangabaka_collections),
        )
        .route(
            "/titles/{id}/mangabaka/collection",
            post(post_title_mangabaka_collection),
        )
        .route(
            "/manga/{id}/mangabaka/collection",
            post(post_title_mangabaka_collection),
        )
        .route(
            "/titles/{id}/opentome/lines",
            get(get_title_opentome_lines),
        )
        .route(
            "/manga/{id}/opentome/lines",
            get(get_title_opentome_lines),
        )
        .route(
            "/titles/{id}/opentome/line",
            post(post_title_opentome_line),
        )
        .route(
            "/manga/{id}/opentome/line",
            post(post_title_opentome_line),
        )
        .route("/releases/calendar", get(get_releases_calendar))
        .route("/system/integrity", get(get_system_integrity))
        .route("/notifications/history", get(get_notifications_history))
        .route(
            "/notifications/events/{id}/resolve",
            post(post_notification_event_resolve),
        )
        .route("/availability/watchers", get(get_availability_watchers))
        .route(
            "/availability/watchers/dashboard",
            get(get_availability_watchers_dashboard),
        )
        .route(
            "/availability/watchers/summary",
            get(get_availability_watchers_summary),
        )
        .route(
            "/availability/watchers/bulk-action",
            post(post_availability_watchers_bulk_action),
        )
        .route(
            "/availability/watchers/{id}/refresh",
            post(post_availability_watcher_refresh),
        )
        .route("/watchers", get(get_availability_watchers))
        .route("/watchers/dashboard", get(get_availability_watchers_dashboard))
        .route("/watchers/summary", get(get_availability_watchers_summary))
        .route(
            "/watchers/bulk-action",
            post(post_availability_watchers_bulk_action),
        )
        .route(
            "/watchers/{id}/refresh",
            post(post_availability_watcher_refresh),
        )
        .route(
            "/titles/{id}/availability",
            get(get_title_availability).patch(patch_title_availability),
        )
        .route(
            "/titles/{id}/availability/refresh",
            post(post_title_availability_refresh),
        )
        .route(
            "/manga/{id}/availability",
            get(get_title_availability).patch(patch_title_availability),
        )
        .route(
            "/manga/{id}/availability/refresh",
            post(post_title_availability_refresh),
        )
        .route("/manga/{id}/cover", get(cover))
        .route("/manga/{id}/library", put(set_library))
        .route("/manga/{id}/reader", put(set_reader))
        .route("/manga/{id}/chapters", put(mark_chapters))
        .route("/chapters/{id}", get(reader))
        .route("/chapters/{id}/pages", get(pages))
        .route("/chapters/{id}/pages/{index}", get(page))
        .route("/chapters/{id}/progress", put(progress))
        .route("/library", get(library))
        .route("/library/bulk", post(bulk))
        .route("/library/receipts", get(get_library_receipts))
        .route("/library/receipts/{id}/verify", post(post_receipt_verify))
        .route("/library/receipts/{id}/rescan", post(post_receipt_rescan))
        .route("/library/receipts/{id}/retry-metadata", post(post_receipt_retry_metadata))
        .route("/titles/{id}/receipts", get(get_title_receipts))
        .route("/manga/{id}/receipts", get(get_title_receipts))
        .route("/backlog/jobs", get(get_all_backlog_jobs))
        .route("/backlog/jobs/{id}/cancel", post(post_backlog_job_cancel))
        .route("/backlog/jobs/{id}/execute", post(post_backlog_job_execute_by_id))
        .route("/titles/{id}/backlog/plan", post(post_manga_backlog_plan))
        .route("/titles/{id}/backlog/execute", post(post_manga_backlog_execute))
        .route("/titles/{id}/backlog/jobs", get(get_manga_backlog_jobs))
        .route("/manga/{id}/backlog/plan", post(post_manga_backlog_plan))
        .route("/manga/{id}/backlog/execute", post(post_manga_backlog_execute))
        .route("/manga/{id}/backlog/jobs", get(get_manga_backlog_jobs))
        .route(
            "/categories",
            get(list_categories)
                .post(add_category)
                .put(order_categories),
        )
        .route(
            "/categories/{id}",
            put(rename_category).delete(remove_category),
        )
        .route("/manga/{id}/categories", put(set_categories))
        .route("/history", get(history).delete(clear_history))
        .route("/history/{chapter}", delete(forget))
        .route("/updates", get(updates))
        .route("/updates/run", post(run_updates))
        .route(
            "/downloads",
            get(downloads).post(queue_downloads).delete(clear_downloads),
        )
        .route("/downloads/order", put(order_downloads))
        .route("/downloads/pause", post(pause_downloads))
        .route("/downloads/resume", post(resume_downloads))
        .route("/downloads/{chapter}", delete(cancel_download))
        .route("/downloads/{chapter}/retry", post(retry_download))
        .route("/downloads/{chapter}/recover", post(recover_download))
        .route("/downloads/{chapter}/recovery-info", get(get_download_recovery_info))
        .route("/downloads/recovery-summary", get(get_downloads_recovery_summary))
        .route(
            "/downloads/{chapter}/retry-metadata",
            post(retry_metadata_download),
        )
        .route(
            "/downloads/{chapter}/retry-komga",
            post(retry_komga_download),
        )
        .route("/chapters/{id}/download", delete(delete_download))
        .route("/manga/{id}/auto-download", put(set_auto_download))
        .route(
            "/manga/{id}/migrate/{to}",
            get(compare_titles).post(migrate_title),
        )
        .route(
            "/import",
            get(import_status)
                .post(start_import)
                .layer(DefaultBodyLimit::max(import::MAX_UPLOAD)),
        )
        .route(
            "/import/preview",
            post(preview_import).layer(DefaultBodyLimit::max(import::MAX_UPLOAD)),
        )
        .route("/export", get(export_library))
        .route("/export/files", get(export_files))
        .route("/export/files/size", get(files_size))
        .route("/trackers", get(list_trackers))
        .route("/trackers/{id}/authorize", post(authorize_tracker))
        .route(
            "/trackers/{id}/login",
            post(login_tracker).delete(logout_tracker),
        )
        .route("/trackers/{id}/search", get(search_tracker))
        .route("/manga/{id}/tracks", get(tracks))
        .route(
            "/manga/{id}/tracks/{tracker}",
            post(bind_track).put(edit_track).delete(unbind_track),
        )
        .route("/manga/{id}/tracks/{tracker}/refresh", post(refresh_track))
        .route(
            "/pipeline/config",
            get(get_pipeline_config).put(put_pipeline_config),
        )
        .route("/pipeline/validate-paths", post(validate_pipeline_paths))
        .route("/pipeline/test-mangabaka", post(test_pipeline_mangabaka))
        .route("/pipeline/test-komga", post(test_pipeline_komga))
        .route(
            "/pipeline/discover-komga-libraries",
            post(discover_pipeline_komga_libraries),
        )
        .route(
            "/pipeline/retry-komga/{chapter}",
            post(retry_komga_download),
        )
        .route("/pipeline/repair-cbz", post(repair_pipeline_cbz))
        .route("/pipeline/naming/audit", post(post_naming_audit))
        .route("/pipeline/naming/audit/latest", get(get_naming_audit_latest))
        .route("/pipeline/naming/apply", post(post_naming_apply))
        .route("/pipeline/naming/rollback", post(post_naming_rollback))
        .route("/pipeline/naming/backups", get(get_naming_backups))
        .route(
            "/pipeline/maintenance/dry-run",
            post(post_maintenance_dry_run),
        )
        .route(
            "/pipeline/maintenance/normalize",
            post(post_maintenance_normalize),
        )
        .route(
            "/pipeline/maintenance/history",
            get(get_maintenance_history),
        )
        .route(
            "/pipeline/maintenance/refresh/{id}",
            post(post_maintenance_refresh),
        )
        .route(
            "/pipeline/maintenance/rollback/{id}",
            post(post_maintenance_rollback),
        )
        .route("/pipeline/staging/inventory", get(get_staging_inventory))
        .route("/storage/staging-inventory", get(get_staging_inventory))
        .route("/storage/optimizer-status", get(get_storage_optimizer_status))
        .route("/storage/duplicates", get(get_storage_duplicates))
        .route("/dashboard/summary", get(get_dashboard_summary))
        .route("/pipeline/staging/preview", post(post_staging_cleanup_preview))
        .route(
            "/imports/payload/{id}/cleanup",
            post(cleanup_import_payload),
        )
        .route("/imports/payload/{id}/retry", post(retry_import_inspection))
        .route("/imports", get(get_import_candidates))
        .route("/imports/{id}/assignment", post(assign_import_candidate))
        .route("/acquisition/sources", get(get_acquisition_sources))
        .route("/acquisition/plan-by-title", post(post_plan_by_title))
        .route(
            "/acquisition/operations/summary",
            get(get_acquisition_operations_summary),
        )
        .route("/pipeline/receipts", get(get_library_receipts))
        .route("/pipeline/receipts/{id}/verify", post(post_receipt_verify))
        .route("/pipeline/events", get(get_pipeline_events))
        .route(
            "/titles/{id}/acquisition-releases",
            get(get_title_acquisition_releases),
        )
        .route("/titles/{id}/acquire", post(post_title_acquire))
        .route(
            "/titles/{id}/acquisition-inspect-collection",
            post(post_title_acquisition_inspect_collection),
        )
        .route(
            "/titles/{id}/acquire-collection",
            post(post_title_acquire_collection),
        )
        .route(
            "/settings/acquisition",
            get(get_acquisition_settings).put(put_acquisition_settings),
        )
        .route(
            "/settings/acquisition/torbox/test",
            post(test_torbox_connection),
        )
        .route("/acquisition/history", get(get_acquisition_history))
        .route(
            "/settings/profiles",
            get(get_acquisition_profiles).post(post_create_acquisition_profile),
        )
        .route(
            "/settings/profiles/{id}",
            get(get_acquisition_profile)
                .put(put_update_acquisition_profile)
                .delete(delete_acquisition_profile),
        )
        .route(
            "/settings/profiles/{id}/duplicate",
            post(post_duplicate_acquisition_profile),
        )
        .route(
            "/titles/{id}/profile",
            get(get_title_acquisition_profile).put(put_title_acquisition_profile),
        )
        .route(
            "/titles/{id}/missing-content",
            get(get_title_missing_content),
        )
        .route(
            "/titles/{id}/plan",
            get(get_title_plan).post(post_title_plan),
        )
        .route(
            "/titles/{id}/executions",
            get(get_title_executions).post(post_create_execution),
        )
        .route(
            "/acquisition/executions/{id}",
            get(get_acquisition_execution),
        )
        .route(
            "/acquisition/executions/{id}/start",
            post(post_start_execution),
        )
        .route(
            "/acquisition/executions/{id}/cancel",
            post(post_cancel_execution),
        )
        .route(
            "/acquisition/executions/{id}/retry",
            post(post_retry_execution),
        )
        .route("/acquisition/missing", get(get_global_missing))
        .route("/acquisition/plan-all", post(post_plan_all))
        .route(
            "/acquisition/sources/health",
            get(get_acquisition_sources_health),
        )
        .route("/sync/dry-run", post(sync_dry_run))
        .route("/sync/run", post(sync_run))
        .route("/sync/status", get(sync_status))
        .route("/sync/history", get(sync_history))
        .route(
            "/sync/mappings",
            get(get_sync_mappings).put(put_sync_mapping),
        )
        .route(
            "/sync/mappings/{series_id}/{tracker}",
            delete(delete_sync_mapping),
        )
        .route("/sync/mappings/manual", post(post_sync_manual_mapping))
        .route("/sync/mappings/reset", post(post_sync_reset_mapping))
        .route(
            "/sync/diagnostics/unresolved",
            get(get_sync_unresolved_diagnostics),
        )
        .route(
            "/sync/schedule",
            get(get_sync_schedule).put(put_sync_schedule),
        )
        .route(
            "/catalog/tracked",
            get(get_tracked_catalog).post(post_tracked_catalog),
        )
        .route(
            "/catalog/tracked/{id}/auto-acquire",
            put(put_tracked_auto_acquire),
        )
        .route("/catalog/tracked/{id}/tracked", put(put_tracked_status))
        .route("/lists", get(get_lists))
        .route("/lists/refresh", post(post_lists_refresh))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require));
    let api = Router::new()
        .nest("/auth", auth::routes())
        .merge(private)
        .fallback(no_route)
        .layer(middleware::from_fn(same_site_only))
        .layer(middleware::from_fn(kept_nowhere))
        .with_state(state.clone());
    Router::new()
        .nest("/api", api)
        .nest("/opds", opds::routes(state.clone()))
        .fallback(ui::serve)
        // The inner one first: an answer that is not sent again is not gzipped either.
        .layer(middleware::from_fn(sent_once))
        .layer(middleware::from_fn(gzipped))
        .layer(middleware::from_fn_with_state(state, known_host_only))
}

/// An answer marked `no-cache` is asked for again every time it is wanted: the page, a
/// feed, a cover without its version. It gets a mark made from its content, and a caller
/// that shows the same mark is told that what it holds still stands, with nothing sent
/// after. Such answers are whole in memory already.
async fn sent_once(request: Request, next: Next) -> Response {
    let got = request.method() == Method::GET;
    let held = request.headers().get(header::IF_NONE_MATCH).cloned();
    let response = next.run(request).await;
    let asked_again = response
        .headers()
        .get(header::CACHE_CONTROL)
        .is_some_and(|kept| kept == "no-cache");
    if !got || !asked_again || response.status() != StatusCode::OK {
        return response;
    }
    let (mut head, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, usize::MAX).await else {
        return AppError::internal("an answer broke off").into_response();
    };
    // A weak mark: the gzipped answer and the plain one carry the same.
    let mark = format!("W/\"{}\"", hex::encode(&Sha256::digest(&bytes)[..8]));
    if let Ok(value) = HeaderValue::from_str(&mark) {
        head.headers.insert(header::ETAG, value);
    }
    if held
        .as_ref()
        .and_then(|held| held.to_str().ok())
        .is_some_and(|held| names(held, &mark))
    {
        head.status = StatusCode::NOT_MODIFIED;
        head.headers.remove(header::CONTENT_LENGTH);
        return Response::from_parts(head, Body::empty());
    }
    Response::from_parts(head, Body::from(bytes))
}

/// Whether a caller's `If-None-Match` names this mark. Weak or strong is the same here.
fn names(held: &str, mark: &str) -> bool {
    fn bare(mark: &str) -> &str {
        mark.trim().trim_start_matches("W/")
    }
    held.split(',')
        .any(|one| one.trim() == "*" || bare(one) == bare(mark))
}

/// Text of some size goes out gzipped to a caller that reads gzip: the page's script and
/// styles, JSON, the feeds. Pictures and archives are packed already and pass untouched.
async fn gzipped(request: Request, next: Next) -> Response {
    let reads = reads_gzip(request.headers());
    let mut response = next.run(request).await;
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|kind| kind.to_str().ok())
        .unwrap_or("");
    let text = kind.starts_with("text/") || kind.contains("json") || kind.contains("xml");
    if !text || response.headers().contains_key(header::CONTENT_ENCODING) {
        return response;
    }
    // A cache between Mango and its callers has to keep the two forms apart.
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if !reads {
        return response;
    }
    let (mut head, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, usize::MAX).await else {
        return AppError::internal("an answer broke off").into_response();
    };
    if bytes.len() < GZIP_FROM {
        return Response::from_parts(head, Body::from(bytes));
    }
    let plain = bytes.clone();
    let packed = tokio::task::spawn_blocking(move || {
        let mut gzip = GzEncoder::new(Vec::with_capacity(plain.len() / 4), Compression::default());
        gzip.write_all(&plain)?;
        gzip.finish()
    })
    .await;
    match packed {
        Ok(Ok(packed)) => {
            head.headers
                .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            head.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(head, Body::from(packed))
        }
        _ => Response::from_parts(head, Body::from(bytes)),
    }
}

/// Whether a caller says it reads gzip, and does not take it back with `q=0`.
fn reads_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let named = parts
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("gzip"));
            named
                && !parts.any(|part| {
                    part.strip_prefix("q=").and_then(|q| q.parse::<f32>().ok()) == Some(0.0)
                })
        })
}

/// Refuses a call that names Mango by a host it does not go by; see `Auth::answers`.
async fn known_host_only(State(server): State<Spaces>, request: Request, next: Next) -> Response {
    if !server.auth.answers(request.headers()) {
        let why = "Mango does not go by that name. Open it by its address, or add the name to MANGO_HOSTS.";
        return AppError::new(403, "unknown_host", why).into_response();
    }
    next.run(request).await
}

/// An answer of the API is one account's own and may differ the next moment. No browser
/// or proxy keeps a copy, unless the answer itself says how long it holds, as pictures do.
async fn kept_nowhere(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

/// Refuses changes that a page on another site sends through the owner's browser.
/// The session cookie is not sent along with them from another site either.
async fn same_site_only(request: Request, next: Next) -> Response {
    let changes_something = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if changes_something && from_elsewhere(request.headers()) {
        return AppError::new(
            403,
            "cross_site",
            "Requests from other sites are not accepted",
        )
        .into_response();
    }
    next.run(request).await
}

/// Whether a request was sent by a page that is not Mango's own. Browsers label such
/// requests, but only where the address is HTTPS or this machine's own. Without the
/// label, what tells is the page the request says it comes from.
fn from_elsewhere(headers: &HeaderMap) -> bool {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    match header("sec-fetch-site") {
        Some(site) => site != "same-origin" && site != "none",
        None => header("origin").is_some_and(|origin| {
            let from = origin.split_once("://").map_or(origin, |(_, host)| host);
            // A proxy may put its own name for Mango in `Host`, and the one the browser used beside it.
            !["host", "x-forwarded-host"]
                .into_iter()
                .filter_map(header)
                .any(|host| host.eq_ignore_ascii_case(from))
        }),
    }
}

async fn no_route() -> AppError {
    AppError::not_found("No such endpoint")
}

fn done() -> Json<Value> {
    Json(json!({ "ok": true }))
}

fn picture(image: Image, cache: &'static str) -> Response {
    let headers = [
        (header::CONTENT_TYPE, image.content_type),
        (header::CACHE_CONTROL, cache),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ];
    (headers, image.bytes).into_response()
}

async fn health(Me(app): Me) -> Json<Value> {
    let anyone_installs = app.stores.anyone_installs();
    Json(
        json!({ "version": version::VERSION, "host": app.host.status(), "localDir": app.local_dir, "anyoneInstalls": anyone_installs }),
    )
}

/// This Mango's version and what the release list said when it was last asked.
async fn about(Me(app): Me) -> Json<version::View> {
    Json(app.releases.view())
}

/// Asks the release list now.
async fn check_version(Me(app): Me) -> Result<Json<version::View>> {
    Ok(Json(app.releases.check().await?))
}

async fn get_settings(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(Value::Object(
        app.db.call(|conn| settings::all(conn)).await?,
    )))
}

/// Stores the keys given; a null removes one. Keys not named are left alone.
async fn put_settings(Me(app): Me, Json(values): Json<Map<String, Value>>) -> Result<Json<Value>> {
    app.db
        .call(move |conn| settings::put(conn, &values))
        .await?;
    Ok(done())
}

// ---------- accounts ----------

async fn list_users(Me(app): Me) -> Result<Json<Value>> {
    app.me.admin()?;
    Ok(Json(json!({ "users": app.auth.list() })))
}

#[derive(Deserialize)]
struct NewUser {
    name: String,
    password: String,
    #[serde(default)]
    admin: bool,
}

/// A new account starts with an empty library and no stores.
async fn add_user(
    State(server): State<Spaces>,
    Me(app): Me,
    Json(body): Json<NewUser>,
) -> Result<Json<Account>> {
    app.me.admin()?;
    let account = app.auth.add(&body.name, body.password, body.admin).await?;
    if let Err(why) = server.create(account.id) {
        let _ = app.auth.remove(account.id).await;
        return Err(why);
    }
    Ok(Json(account))
}

async fn edit_user(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(change): Json<AccountChange>,
) -> Result<Json<Account>> {
    app.me.admin()?;
    Ok(Json(app.auth.edit(id, change).await?))
}

/// Removes an account with its library, its stores, its downloads and its tracker sign-ins.
async fn remove_user(
    State(server): State<Spaces>,
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    if id == app.me.id {
        return Err(AppError::bad_request(
            "That is the account you are signed in with",
        ));
    }
    app.auth.remove(id).await?;
    server.close(id).await;
    Ok(done())
}

// ---------- invitations ----------

async fn list_invites(Me(app): Me) -> Result<Json<Value>> {
    app.me.admin()?;
    Ok(Json(json!({ "invites": app.auth.invites().await? })))
}

#[derive(Deserialize)]
struct NewInvite {
    /// Who it is for. Only admins see it.
    #[serde(default)]
    note: String,
    /// How many days it holds. Left out, a week.
    days: Option<i64>,
}

/// Makes an invitation. The answer carries its code, this once.
async fn add_invite(Me(app): Me, Json(body): Json<NewInvite>) -> Result<Json<Invite>> {
    app.me.admin()?;
    Ok(Json(app.auth.invite(&body.note, body.days).await?))
}

async fn remove_invite(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    app.me.admin()?;
    app.auth.uninvite(id).await?;
    Ok(done())
}

// ---------- the solver ----------

async fn get_solver(Me(app): Me) -> Result<Json<Solver>> {
    app.me.admin()?;
    Ok(Json(app.host.solver()))
}

/// Where the sources send a Cloudflare check to be passed. The extension host is
/// restarted to learn of it, at its next call.
async fn put_solver(
    State(server): State<Spaces>,
    Me(app): Me,
    Json(solver): Json<Solver>,
) -> Result<Json<Solver>> {
    app.me.admin()?;
    let solver = solver.checked()?;
    let kept = serde_json::to_value(&solver).map_err(|e| AppError::internal(e.to_string()))?;
    server
        .shared()
        .call(move |conn| settings::set(conn, Solver::KEY, &kept))
        .await?;
    app.host.set_solver(solver.clone()).await;
    Ok(Json(solver))
}

// ---------- stores ----------

async fn list_stores(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(json!({ "stores": app.stores.list().await? })))
}

#[derive(Deserialize)]
struct NewStore {
    url: String,
}

async fn add_store(Me(app): Me, Json(body): Json<NewStore>) -> Result<Json<StoreInfo>> {
    Ok(Json(app.stores.add(&body.url).await?))
}

async fn refresh_stores(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(json!({ "stores": app.stores.refresh_all().await? })))
}

async fn refresh_store(Me(app): Me, Path(id): Path<i64>) -> Result<Json<StoreInfo>> {
    Ok(Json(app.stores.refresh(id).await?))
}

async fn accept_key(Me(app): Me, Path(id): Path<i64>) -> Result<Json<StoreInfo>> {
    Ok(Json(app.stores.accept_key(id).await?))
}

async fn remove_store(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    app.stores.remove_store(id).await?;
    Ok(Json(json!({ "removed": true })))
}

// ---------- catalog ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct CatalogParams {
    /// Text to find in extension and source names.
    q: String,
    /// Languages, comma separated; any of them.
    lang: String,
    /// Any of safe, mixed, nsfw, comma separated. Left out: safe and mixed.
    content: String,
    /// installed, available or updates.
    state: Option<String>,
    store: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn catalog(Me(app): Me, Query(params): Query<CatalogParams>) -> Result<Json<Value>> {
    let words = |list: &str| {
        list.split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut levels = Vec::new();
    for word in words(&params.content) {
        match word.as_str() {
            "safe" => levels.extend([0, 1]),
            "mixed" => levels.push(2),
            "nsfw" => levels.push(3),
            _ => return Err(AppError::bad_request("content takes safe, mixed and nsfw")),
        }
    }
    if params
        .state
        .as_deref()
        .is_some_and(|state| !matches!(state, "installed" | "available" | "updates"))
    {
        return Err(AppError::bad_request(
            "state takes installed, available or updates",
        ));
    }
    let query = CatalogQuery {
        text: params.q,
        langs: words(&params.lang),
        levels,
        state: params.state,
        store: params.store,
        limit: params.limit.unwrap_or(60),
        offset: params.offset.unwrap_or(0),
    };
    let (items, total) = app.stores.catalog(query).await?;
    Ok(Json(json!({ "items": items, "total": total })))
}

async fn catalog_item(
    Me(app): Me,
    Path((store, pkg)): Path<(i64, String)>,
) -> Result<Json<CatalogItem>> {
    Ok(Json(app.stores.catalog_item(store, pkg).await?))
}

// ---------- installed extensions and their sources ----------

async fn installed(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(json!({ "extensions": app.stores.installed().await? })))
}

#[derive(Deserialize)]
struct InstallParams {
    /// The store to install from, when more than one lists the extension.
    store: Option<i64>,
}

/// Installs, or updates to the store's version.
async fn install(
    Me(app): Me,
    Path(pkg): Path<String>,
    Query(params): Query<InstallParams>,
) -> Result<Json<Installed>> {
    Ok(Json(
        app.stores
            .install(&pkg, params.store, app.stores.open_to(&app.me))
            .await?,
    ))
}

async fn update_all(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(
        json!({ "results": app.stores.update_all(app.stores.open_to(&app.me)).await? }),
    ))
}

async fn remove_extension(Me(app): Me, Path(pkg): Path<String>) -> Result<Json<Value>> {
    app.stores.remove(&pkg).await?;
    Ok(Json(json!({ "removed": true })))
}

async fn sources(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(json!({ "sources": app.stores.sources().await? })))
}

#[derive(Deserialize)]
struct SourceChange {
    pinned: Option<bool>,
    hidden: Option<bool>,
}

async fn change_source(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<SourceChange>,
) -> Result<Json<Value>> {
    app.stores.set_source(id, body.pinned, body.hidden).await?;
    Ok(done())
}

/// What a source lets its user set. The host keeps one set per source for every account,
/// a sign-in to the source's site among them: where extensions are an admin's to bring
/// in, the settings are an admin's to see and change.
async fn source_preferences(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    if !app.stores.open_to(&app.me) {
        return Ok(Json(json!({ "preferences": [] })));
    }
    Ok(Json(app.stores.preferences(id).await?))
}

/// Changes one of them: `{position, key, value}`. Answers with the list as it is afterwards.
async fn set_source_preference(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(change): Json<Value>,
) -> Result<Json<Value>> {
    if !app.stores.open_to(&app.me) {
        app.me.admin()?;
    }
    Ok(Json(app.stores.set_preference(id, &change).await?))
}

/// Verifies whether the source credentials and connection authenticate against the remote service.
async fn verify_source(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::store::SourceVerifyResponse>> {
    if !app.stores.open_to(&app.me) {
        app.me.admin()?;
    }
    Ok(Json(app.stores.verify_source(id).await?))
}

// ---------- browse and titles ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct BrowseParams {
    page: Option<i64>,
    /// The search text. Only `search` reads it.
    q: String,
    /// The filters set to something else than the source starts them at: a JSON list of
    /// `{position, state}`. Only `search` reads it.
    filters: Option<String>,
}

/// `kind` is popular, latest or search.
async fn browse(
    Me(app): Me,
    Path((id, kind)): Path<(i64, String)>,
    Query(params): Query<BrowseParams>,
) -> Result<Json<Listing>> {
    let kind = Browse::parse(&kind).ok_or_else(|| AppError::not_found("No such endpoint"))?;
    let filters: Vec<Value> = match params.filters.as_deref() {
        Some(text) if !text.is_empty() => serde_json::from_str(text)
            .map_err(|_| AppError::bad_request("filters is not a JSON list"))?,
        _ => Vec::new(),
    };
    Ok(Json(
        app.titles
            .browse(id, kind, params.page.unwrap_or(1), &params.q, &filters)
            .await?,
    ))
}

/// What a source's search can be narrowed by: `{filters}`, each named by its place in the list.
async fn source_filters(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(app.titles.filters(id).await?))
}

async fn title(Me(app): Me, Path(id): Path<i64>) -> Result<Json<TitleView>> {
    Ok(Json(app.titles.get(id).await?))
}

async fn refresh_title(Me(app): Me, Path(id): Path<i64>) -> Result<Json<TitleView>> {
    app.pipeline.ownership.invalidate(id);
    Ok(Json(app.titles.refresh(id).await?))
}

async fn manga_ownership(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::pipeline::ownership::TitleOwnership>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .get_title_ownership(id, &title_view.title)
        .await;
    Ok(Json(ownership))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FollowResponse {
    followed: bool,
}

async fn get_manga_follow(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<FollowResponse>> {
    let title_view = app.titles.get(id).await?;
    let followed = app
        .db
        .call(move |conn| crate::sync::reconciliation::is_series_followed(conn, Some(id), &title_view.title))
        .await?;
    Ok(Json(FollowResponse { followed }))
}

async fn post_manga_follow(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<FollowResponse>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .get_title_ownership(id, &title_view.title)
        .await;
    let komga_series_id = ownership.series_id.clone();
    let title = title_view.title.clone();

    app.db
        .call(move |conn| {
            crate::sync::reconciliation::follow_series(
                conn,
                &title,
                komga_series_id.as_deref(),
                Some(id),
            )
        })
        .await?;
    Ok(Json(FollowResponse { followed: true }))
}

async fn post_manga_unfollow(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<FollowResponse>> {
    let title_view = app.titles.get(id).await?;
    let title = title_view.title.clone();

    app.db
        .call(move |conn| {
            crate::sync::reconciliation::unfollow_series(conn, Some(id), &title)
        })
        .await?;
    Ok(Json(FollowResponse { followed: false }))
}

async fn get_manga_reconciliation(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Option<crate::sync::reconciliation::ReconciliationSummary>>> {
    let summary = app
        .db
        .call(move |conn| crate::sync::reconciliation::get_reconciliation_for_manga(conn, id))
        .await?;
    Ok(Json(summary))
}

async fn get_manga_library_state(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::sync::reconciliation::CanonicalLibraryReleasesSummary>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .get_title_ownership(id, &title_view.title)
        .await;
    let ot_schedule = if app.opentome.is_enabled().await {
        app.opentome.get_series_schedule(id).await.ok().flatten()
    } else {
        None
    };
    let today = app.opentome.current_date_str().await;
    let summary = app
        .db
        .call(move |conn| {
            crate::sync::reconciliation::resolve_canonical_library_state_with_ownership(
                conn,
                id,
                ot_schedule.as_ref(),
                "en",
                &today,
                Some(&ownership),
            )
        })
        .await?;
    Ok(Json(summary))
}

async fn post_manga_reconcile(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::sync::reconciliation::ReconciliationSummary>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .get_title_ownership(id, &title_view.title)
        .await;

    let target = crate::sync::reconciliation::ReconciliationTarget {
        manga_id: Some(id),
        komga_series_id: ownership.series_id,
        title: title_view.title,
        edition: None,
    };

    let summary = app.sync.run_reconciliation(target).await?;
    Ok(Json(summary))
}

async fn get_followed_reconciliations(
    Me(app): Me,
) -> Result<Json<Vec<crate::sync::reconciliation::ReconciliationSummary>>> {
    let list = app
        .db
        .call(|conn| crate::sync::reconciliation::list_reconciliations(conn))
        .await?;
    Ok(Json(list))
}

async fn post_run_followed_reconciliations(
    Me(app): Me,
) -> Result<Json<Vec<crate::sync::reconciliation::ReconciliationSummary>>> {
    app.me.admin()?;
    let list = app.sync.run_reconciliation_all_followed().await?;
    Ok(Json(list))
}

async fn get_manga_volumes_manager(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::acquisition::volume_manager::VolumeManagerSummary>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .get_title_ownership(id, &title_view.title)
        .await;
    let res = app
        .db
        .call(move |conn| {
            crate::acquisition::volume_manager::get_volume_manager_summary_with_ownership(
                conn,
                id,
                Some(&ownership),
            )
        })
        .await?;
    Ok(Json(res))
}

async fn post_manga_volumes_manager_plan(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<crate::acquisition::volume_manager::VolumeManagerBatchPlanRequest>,
) -> Result<Json<crate::acquisition::volume_manager::VolumeManagerBatchPlanResponse>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .get_title_ownership(id, &title_view.title)
        .await;
    let profile = app.acquisition.get_manga_profile(id).await.unwrap_or_default();
    let res = app
        .db
        .call(move |conn| {
            crate::acquisition::volume_manager::plan_volume_manager_batch(
                conn,
                id,
                &payload,
                &profile,
                Some(&ownership),
            )
        })
        .await?;
    Ok(Json(res))
}

async fn post_manga_volumes_manager_acquire(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<crate::acquisition::volume_manager::VolumeManagerBatchAcquireRequest>,
) -> Result<Json<crate::acquisition::volume_manager::VolumeManagerBatchAcquireResponse>> {
    let user_id = if app.me.id > 0 { Some(app.me.id) } else { None };
    let res = crate::acquisition::volume_manager::acquire_volume_manager_batch(
        &app.db,
        &app.acquisition,
        &app.titles,
        id,
        payload,
        user_id,
    )
    .await?;
    Ok(Json(res))
}

async fn get_manga_volume_candidates(
    Me(app): Me,
    Path((id, volume)): Path<(i64, f64)>,
) -> Result<Json<Option<crate::acquisition::discovery::VolumeDiscoveryResult>>> {
    let series_id = app
        .db
        .call(move |conn| {
            if let Some(sum) = crate::sync::reconciliation::get_reconciliation_for_manga(conn, id)? {
                Ok(Some(sum.series_id))
            } else {
                Ok(None)
            }
        })
        .await?;

    let Some(s_id) = series_id else {
        return Ok(Json(None));
    };

    let res = app
        .db
        .call(move |conn| crate::acquisition::discovery::get_volume_discovery(conn, &s_id, volume))
        .await?;

    Ok(Json(res))
}

async fn get_manga_volume_candidate_review(
    Me(app): Me,
    Path((id, volume, candidate_id)): Path<(i64, f64, String)>,
) -> Result<Json<crate::acquisition::approval::ManualAcquisitionReviewDto>> {
    let res = app
        .acquisition
        .request_manual_acquisition_review(id, volume, &candidate_id, Some(app.me.id))
        .await?;
    Ok(Json(res))
}

async fn post_manga_volume_candidate_acquire(
    Me(app): Me,
    Path((id, volume, candidate_id)): Path<(i64, f64, String)>,
    Json(payload): Json<Option<crate::acquisition::approval::ApproveCandidateRequest>>,
) -> Result<Json<crate::acquisition::approval::ApproveCandidateResponse>> {
    let mut req = payload.unwrap_or_default();
    req.user_id = Some(app.me.id);
    let res = app
        .acquisition
        .approve_candidate(id, volume, &candidate_id, req)
        .await?;
    Ok(Json(res))
}

async fn get_manga_series_discovery(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::acquisition::discovery::VolumeDiscoveryResult>>> {
    let series_id = app
        .db
        .call(move |conn| {
            if let Some(sum) = crate::sync::reconciliation::get_reconciliation_for_manga(conn, id)? {
                Ok(Some(sum.series_id))
            } else {
                Ok(None)
            }
        })
        .await?;

    let Some(s_id) = series_id else {
        return Ok(Json(Vec::new()));
    };

    let results = app
        .db
        .call(move |conn| crate::acquisition::discovery::list_series_discovery(conn, &s_id))
        .await?;

    Ok(Json(results))
}

async fn post_manga_volume_discover(
    Me(app): Me,
    Path((id, volume)): Path<(i64, f64)>,
) -> Result<Json<crate::acquisition::discovery::VolumeDiscoveryResult>> {
    let existing = app
        .db
        .call(move |conn| crate::sync::reconciliation::get_reconciliation_for_manga(conn, id))
        .await?;

    let series_id = if let Some(rec) = existing {
        rec.series_id
    } else {
        let title_view = app.titles.get(id).await?;
        let ownership = app
            .pipeline
            .ownership
            .get_title_ownership(id, &title_view.title)
            .await;
        let target = crate::sync::reconciliation::ReconciliationTarget {
            manga_id: Some(id),
            komga_series_id: ownership.series_id,
            title: title_view.title,
            edition: None,
        };
        let summary = app.sync.run_reconciliation(target).await?;
        summary.series_id
    };

    let result = crate::acquisition::discovery::discover_for_volume(
        &app.db,
        app.acquisition.http.clone(),
        &series_id,
        volume,
    )
    .await?;

    Ok(Json(result))
}

async fn post_manga_discover_missing(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::acquisition::discovery::SeriesDiscoverySummary>> {
    let existing = app
        .db
        .call(move |conn| crate::sync::reconciliation::get_reconciliation_for_manga(conn, id))
        .await?;

    let series_id = if let Some(rec) = existing {
        rec.series_id
    } else {
        let title_view = app.titles.get(id).await?;
        let ownership = app
            .pipeline
            .ownership
            .get_title_ownership(id, &title_view.title)
            .await;
        let target = crate::sync::reconciliation::ReconciliationTarget {
            manga_id: Some(id),
            komga_series_id: ownership.series_id,
            title: title_view.title,
            edition: None,
        };
        let summary = app.sync.run_reconciliation(target).await?;
        summary.series_id
    };

    let summary = crate::acquisition::discovery::discover_missing_for_series(
        &app.db,
        app.acquisition.http.clone(),
        &series_id,
    )
    .await?;

    Ok(Json(summary))
}

async fn refresh_manga_ownership(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::pipeline::ownership::TitleOwnership>> {
    let title_view = app.titles.get(id).await?;
    let ownership = app
        .pipeline
        .ownership
        .refresh_title(id, &title_view.title)
        .await;
    Ok(Json(ownership))
}

/// The address carries a version, so with one the answer never changes.
async fn cover(Me(app): Me, Path(id): Path<i64>, RawQuery(version): RawQuery) -> Result<Response> {
    let image = app.images.cover(id).await?;
    Ok(picture(
        image,
        if version.is_some() {
            FOREVER
        } else {
            "no-cache"
        },
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryChange {
    in_library: bool,
}

async fn set_library(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<LibraryChange>,
) -> Result<Json<Value>> {
    app.titles.set_library(id, body.in_library).await?;
    Ok(done())
}

#[derive(Deserialize)]
struct ReaderChange {
    mode: Option<String>,
    fit: Option<String>,
    /// Every other reader setting the title keeps for itself, as the UI shapes them.
    prefs: Option<Value>,
}

async fn set_reader(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<ReaderChange>,
) -> Result<Json<Value>> {
    app.titles
        .set_reader(id, body.mode, body.fit, body.prefs)
        .await?;
    Ok(done())
}

#[derive(Deserialize)]
struct MarkChange {
    /// Left out: every chapter of the title.
    ids: Option<Vec<i64>>,
    /// Read or not, bookmarked or not: at least one of the two.
    read: Option<bool>,
    bookmark: Option<bool>,
}

async fn mark_chapters(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<MarkChange>,
) -> Result<Json<Value>> {
    if body.read.is_none() && body.bookmark.is_none() {
        return Err(AppError::bad_request(
            "Say what to change: read or bookmark",
        ));
    }
    if let Some(on) = body.bookmark {
        app.titles.bookmark(id, body.ids.clone(), on).await?;
    }
    if let Some(read) = body.read {
        app.titles.mark(id, body.ids, read).await?;
        if read {
            app.trackers.read_title(id);
        }
    }
    Ok(done())
}

// ---------- reader ----------

async fn reader(Me(app): Me, Path(id): Path<i64>) -> Result<Json<ReaderView>> {
    Ok(Json(app.titles.reader(id).await?))
}

async fn pages(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    Ok(Json(json!({ "count": app.images.page_count(id).await? })))
}

async fn page(Me(app): Me, Path((id, index)): Path<(i64, i64)>) -> Result<Response> {
    Ok(picture(app.images.page(id, index).await?, FOREVER))
}

#[derive(Deserialize)]
struct Progress {
    page: i64,
    /// Set to mark the chapter finished or not. Left out, it keeps its state.
    read: Option<bool>,
}

async fn progress(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<Progress>,
) -> Result<Json<Value>> {
    app.titles.progress(id, body.page, body.read).await?;
    if body.read == Some(true) {
        app.downloads.finished_reading(id).await;
        app.trackers.read_chapter(id);
    }
    Ok(done())
}

// ---------- updates and downloads ----------

/// New chapters of library titles, newest first, with the failures and state of the last run.
async fn updates(Me(app): Me, Query(slice): Query<Slice>) -> Result<Json<UpdatesView>> {
    Ok(Json(app.updates.view(slice.limit.unwrap_or(200)).await?))
}

/// Starts a library update, unless one is running. Answers at once.
async fn run_updates(Me(app): Me) -> Json<Status> {
    Json(app.updates.start())
}

async fn downloads(Me(app): Me) -> Result<Json<QueueView>> {
    Ok(Json(app.downloads.list().await?))
}

#[derive(Deserialize)]
struct ToQueue {
    /// Chapter ids.
    #[serde(default)]
    chapters: Vec<i64>,
    /// Title ids: their unread chapters are queued.
    #[serde(default)]
    titles: Vec<i64>,
}

async fn queue_downloads(Me(app): Me, Json(body): Json<ToQueue>) -> Result<Json<Value>> {
    let mut queued = app.downloads.enqueue(body.chapters).await?;
    if !body.titles.is_empty() {
        queued += app.downloads.enqueue_unread(body.titles).await?;
    }
    Ok(Json(json!({ "queued": queued })))
}

async fn clear_downloads(Me(app): Me) -> Result<Json<Value>> {
    app.downloads.cancel(None).await?;
    app.db.call(|c|{c.execute("UPDATE import_payloads SET state='cancelled' WHERE state!='cleaned'",[])?;c.execute("UPDATE import_candidates SET cancel_requested=1,state=CASE WHEN state='processing' THEN state ELSE 'ignored' END WHERE state IN ('needs_review','queued','processing')",[])?;Ok(())}).await?;
    Ok(done())
}

async fn cancel_download(Me(app): Me, Path(chapter): Path<i64>) -> Result<Json<Value>> {
    app.downloads.cancel(Some(chapter)).await?;
    app.db.call(move |conn| {
        conn.execute("UPDATE import_payloads SET state='cancelled' WHERE chapter_id=?1",[chapter])?;
        conn.execute("UPDATE import_candidates SET state='ignored' WHERE payload_chapter=?1 AND state='needs_review'",[chapter])?;
        conn.execute("UPDATE import_candidates SET cancel_requested=1,state=CASE WHEN state='processing' THEN state ELSE 'ignored' END WHERE child_chapter=?1",[chapter])?;
        Ok(())
    }).await?;
    let _ = crate::acquisition::physical::cleanup(&app.db, chapter, app.downloads.dir()).await;
    Ok(done())
}

async fn retry_download(Me(app): Me, Path(chapter): Path<i64>) -> Result<Json<Value>> {
    app.downloads.retry(chapter).await?;
    Ok(done())
}

async fn retry_metadata_download(Me(app): Me, Path(chapter): Path<i64>) -> Result<Json<Value>> {
    let msg = app.downloads.retry_metadata(chapter).await?;
    Ok(Json(serde_json::json!({"message": msg})))
}

async fn post_manga_retry_metadata(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    let chapters = app
        .db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id FROM chapters c
                 LEFT JOIN downloads d ON d.chapter_id = c.id
                 LEFT JOIN metadata_handoffs h ON h.chapter_id = c.id
                 WHERE c.manga_id = ?1
                   AND (h.state IN ('needs_review', 'failed', 'rejected', 'unavailable', 'waiting_komga')
                        OR d.state IN ('metadata_review', 'metadata_failed', 'verifying'))
                 ORDER BY c.chapter_number ASC",
            )?;
            let rows = stmt
                .query_map([id], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;

    let mut succeeded = 0;
    let mut failed = 0;
    for chapter in chapters {
        match app.downloads.retry_metadata(chapter).await {
            Ok(_) => succeeded += 1,
            Err(e) => {
                tracing::warn!("Failed to retry metadata for chapter {chapter}: {e}");
                failed += 1;
            }
        }
    }

    Ok(Json(json!({
        "succeeded": succeeded,
        "failed": failed,
    })))
}

async fn retry_komga_download(Me(app): Me, Path(chapter): Path<i64>) -> Result<Json<Value>> {
    let msg = app.downloads.retry_komga(chapter).await?;
    Ok(Json(json!({ "message": msg })))
}

async fn order_downloads(Me(app): Me, Json(body): Json<Ids>) -> Result<Json<Value>> {
    app.downloads.reorder(body.ids).await?;
    Ok(done())
}

async fn pause_downloads(Me(app): Me) -> Result<Json<Value>> {
    app.downloads.set_paused(true).await?;
    Ok(done())
}

async fn resume_downloads(Me(app): Me) -> Result<Json<Value>> {
    app.downloads.set_paused(false).await?;
    Ok(done())
}

/// Deletes a downloaded chapter's file.
async fn delete_download(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    app.downloads.delete(id).await?;
    Ok(done())
}

#[derive(Deserialize)]
struct Switch {
    on: bool,
}

async fn set_auto_download(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<Switch>,
) -> Result<Json<Value>> {
    app.titles.set_auto_download(id, body.on).await?;
    Ok(done())
}

// ---------- migration ----------

/// A title beside the one it would move to, and what would go along. The other title's
/// source is asked for its chapters first, so the two are compared as they are today.
async fn compare_titles(Me(app): Me, Path((id, to)): Path<(i64, i64)>) -> Result<Json<Comparison>> {
    let target = app.titles.get(to).await?;
    if id != to {
        app.titles.fetch(to, !target.initialized, true).await?;
    }
    Ok(Json(
        app.db
            .call(move |conn| migrate::compare(conn, id, to))
            .await?,
    ))
}

/// Moves a title's place in the library, its progress, categories and tracker links to
/// another title, or copies them. Its source is only asked when it never was.
async fn migrate_title(
    Me(app): Me,
    Path((id, to)): Path<(i64, i64)>,
    Json(options): Json<migrate::Options>,
) -> Result<Json<Outcome>> {
    let target = app.titles.get(to).await?;
    if id != to && target.chapters_at.is_none() {
        app.titles.fetch(to, !target.initialized, true).await?;
    }
    Ok(Json(
        app.db
            .call(move |conn| migrate::apply(conn, id, to, &options))
            .await?,
    ))
}

// ---------- import ----------

/// What a Mihon or Suwayomi backup holds. The body is the `.tachibk` file itself.
async fn preview_import(Me(app): Me, file: Bytes) -> Result<Json<Preview>> {
    Ok(Json(app.importer.preview(file.to_vec()).await?))
}

/// Starts importing the backup in the body. Answers at once; `GET /import` follows it.
async fn start_import(Me(app): Me, file: Bytes) -> Result<Json<import::Status>> {
    Ok(Json(app.importer.start(file.to_vec())?))
}

async fn import_status(Me(app): Me) -> Json<import::Status> {
    Json(app.importer.status())
}

/// The library as a `.tachibk` that Mihon, Suwayomi and Mango itself read.
async fn export_library(Me(app): Me) -> Result<Response> {
    let file = app.db.call(|conn| export::write(conn)).await?;
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", export::file_name()),
        ),
        (header::CACHE_CONTROL, "no-store".to_owned()),
    ];
    Ok((headers, file).into_response())
}

/// Every downloaded chapter as one zip, laid out as a local source reads it.
async fn export_files(Me(app): Me) -> Result<Response> {
    let titles = files::plan(&app.db, &app.images, app.downloads.dir()).await?;
    if titles.is_empty() {
        return Err(AppError::new(
            404,
            "nothing_downloaded",
            "No chapter is downloaded, so there are no files to export.",
        ));
    }
    let headers = [
        (header::CONTENT_TYPE, "application/zip".to_owned()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", files::file_name()),
        ),
        (header::CACHE_CONTROL, "no-store".to_owned()),
    ];
    Ok((headers, files::send(titles)).into_response())
}

async fn files_size(Me(app): Me) -> Result<Json<files::Size>> {
    Ok(Json(files::size(
        &files::plan(&app.db, &app.images, app.downloads.dir()).await?,
    )))
}

// ---------- trackers ----------

fn tracker(id: i64) -> Result<Kind> {
    Kind::from_id(id).ok_or_else(|| AppError::not_found("No such tracker"))
}

async fn list_trackers(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(json!({ "trackers": app.trackers.list().await? })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Authorize {
    client_id: String,
    /// Where the tracker sends the browser back to: Mango's own address.
    redirect: String,
}

/// Starts a sign-in that goes through the tracker's own page.
async fn authorize_tracker(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<Authorize>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({ "url": app.trackers.authorize(tracker(id)?, body.client_id, body.redirect).await? }),
    ))
}

async fn login_tracker(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<Credentials>,
) -> Result<Json<Value>> {
    app.trackers.login(tracker(id)?, body).await?;
    Ok(done())
}

async fn logout_tracker(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    app.trackers.logout(tracker(id)?).await?;
    Ok(done())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Wanted {
    q: String,
}

async fn search_tracker(
    Me(app): Me,
    Path(id): Path<i64>,
    Query(wanted): Query<Wanted>,
) -> Result<Json<Value>> {
    Ok(Json(
        json!({ "results": app.trackers.search(tracker(id)?, &wanted.q).await? }),
    ))
}

/// The trackers and this title's links to them. Asks no tracker.
async fn tracks(Me(app): Me, Path(id): Path<i64>) -> Result<Json<trackers::View>> {
    Ok(Json(app.trackers.view(id).await?))
}

async fn bind_track(
    Me(app): Me,
    Path((id, kind)): Path<(i64, i64)>,
    Json(pick): Json<Pick>,
) -> Result<Json<Track>> {
    Ok(Json(app.trackers.bind(id, tracker(kind)?, pick).await?))
}

async fn edit_track(
    Me(app): Me,
    Path((id, kind)): Path<(i64, i64)>,
    Json(change): Json<Change>,
) -> Result<Json<Track>> {
    Ok(Json(app.trackers.edit(id, tracker(kind)?, change).await?))
}

async fn refresh_track(Me(app): Me, Path((id, kind)): Path<(i64, i64)>) -> Result<Json<Track>> {
    Ok(Json(app.trackers.refresh(id, tracker(kind)?).await?))
}

async fn unbind_track(Me(app): Me, Path((id, kind)): Path<(i64, i64)>) -> Result<Json<Value>> {
    app.trackers.unbind(id, tracker(kind)?).await?;
    Ok(done())
}

// ---------- library, categories, history ----------

/// Every library title with its counts, the categories, and where to go on reading.
async fn library(Me(app): Me) -> Result<Json<LibraryView>> {
    Ok(Json(app.db.call(|conn| library::read(conn)).await?))
}

#[derive(Deserialize)]
struct BulkChange {
    /// Title ids.
    ids: Vec<i64>,
    /// read, unread, remove, categories, or download (their unread chapters).
    action: String,
    #[serde(default)]
    categories: Vec<i64>,
    /// With `categories`: take the titles out of every other category.
    #[serde(default)]
    replace: bool,
}

async fn bulk(Me(app): Me, Json(body): Json<BulkChange>) -> Result<Json<Value>> {
    let action = match body.action.as_str() {
        "download" => {
            app.downloads.enqueue_unread(body.ids).await?;
            return Ok(done());
        }
        "read" => Bulk::Read,
        "unread" => Bulk::Unread,
        "remove" => Bulk::Remove,
        "categories" => Bulk::Categories {
            ids: body.categories,
            replace: body.replace,
        },
        _ => {
            return Err(AppError::bad_request(
                "action takes read, unread, remove or categories",
            ));
        }
    };
    let (ids, read) = (body.ids.clone(), matches!(action, Bulk::Read));
    app.db
        .call(move |conn| library::bulk(conn, &body.ids, &action))
        .await?;
    if read {
        ids.into_iter().for_each(|id| app.trackers.read_title(id));
    }
    Ok(done())
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

async fn list_categories(Me(app): Me) -> Result<Json<Value>> {
    Ok(Json(
        json!({ "categories": app.db.call(|conn| library::categories(conn)).await? }),
    ))
}

async fn add_category(Me(app): Me, Json(body): Json<Named>) -> Result<Json<Category>> {
    Ok(Json(
        app.db
            .call(move |conn| library::add_category(conn, &body.name))
            .await?,
    ))
}

async fn rename_category(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<Named>,
) -> Result<Json<Value>> {
    app.db
        .call(move |conn| library::rename_category(conn, id, &body.name))
        .await?;
    Ok(done())
}

async fn remove_category(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    app.db
        .call(move |conn| library::remove_category(conn, id))
        .await?;
    Ok(done())
}

#[derive(Deserialize)]
struct Ids {
    ids: Vec<i64>,
}

async fn order_categories(Me(app): Me, Json(body): Json<Ids>) -> Result<Json<Value>> {
    app.db
        .call(move |conn| library::order_categories(conn, &body.ids))
        .await?;
    Ok(done())
}

async fn set_categories(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<Ids>,
) -> Result<Json<Value>> {
    app.db
        .call(move |conn| library::set_categories(conn, id, &body.ids))
        .await?;
    Ok(done())
}

#[derive(Deserialize)]
struct Slice {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn history(Me(app): Me, Query(slice): Query<Slice>) -> Result<Json<Value>> {
    let entries = app
        .db
        .call(move |conn| {
            library::history(conn, slice.limit.unwrap_or(100), slice.offset.unwrap_or(0))
        })
        .await?;
    Ok(Json(json!({ "entries": entries })))
}

async fn forget(Me(app): Me, Path(chapter): Path<i64>) -> Result<Json<Value>> {
    app.db
        .call(move |conn| library::forget(conn, Some(chapter)))
        .await?;
    Ok(done())
}

async fn clear_history(Me(app): Me) -> Result<Json<Value>> {
    app.db.call(|conn| library::forget(conn, None)).await?;
    Ok(done())
}

// ---------- pipeline ----------

async fn get_pipeline_config(Me(app): Me) -> Result<Json<crate::pipeline::PipelineConfig>> {
    app.me.admin()?;
    let config = app.pipeline.load_config().await?;
    Ok(Json(config))
}

async fn put_pipeline_config(
    Me(app): Me,
    Json(config): Json<crate::pipeline::PipelineConfig>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    app.pipeline.save_config(config).await?;
    Ok(done())
}

#[derive(Deserialize)]
struct ValidatePipelinePathsPayload {
    library_dir: Option<String>,
    chinese_library_dir: Option<String>,
    optimizer_bin: Option<String>,
}

async fn validate_pipeline_paths(
    Me(app): Me,
    Json(payload): Json<ValidatePipelinePathsPayload>,
) -> Result<Json<crate::pipeline::ValidatePathsResponse>> {
    app.me.admin()?;
    let res = app
        .pipeline
        .validate_paths(
            payload.library_dir.as_deref(),
            payload.chinese_library_dir.as_deref(),
            payload.optimizer_bin.as_deref(),
        )
        .await;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct TestPipelinePayload {
    url: Option<String>,
    token: Option<String>,
}

async fn test_pipeline_mangabaka(
    Me(app): Me,
    Json(payload): Json<TestPipelinePayload>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    match app
        .pipeline
        .test_mangabaka(payload.url, payload.token)
        .await
    {
        Ok(msg) => {
            let _ = app
                .db
                .call(|conn| crate::db::set_state(conn, "pipeline.mangabaka_validated", "valid"))
                .await;
            Ok(Json(json!({ "message": msg, "validated": true })))
        }
        Err(e) => {
            let err_msg = e.message.clone();
            let _ = app
                .db
                .call(move |conn| {
                    crate::db::set_state(
                        conn,
                        "pipeline.mangabaka_validated",
                        &format!("error: {err_msg}"),
                    )
                })
                .await;
            Err(e)
        }
    }
}

async fn test_pipeline_komga(
    Me(app): Me,
    Json(payload): Json<TestPipelinePayload>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    let msg = app.pipeline.test_komga(payload.url, payload.token).await?;
    Ok(Json(json!({ "message": msg })))
}

async fn discover_pipeline_komga_libraries(
    Me(app): Me,
    Json(payload): Json<TestPipelinePayload>,
) -> Result<Json<Vec<crate::pipeline::komga::KomgaLibrary>>> {
    app.me.admin()?;
    let libs = app
        .pipeline
        .discover_komga_libraries(payload.url, payload.token)
        .await?;
    Ok(Json(libs))
}

async fn get_pipeline_events(Me(app): Me) -> Result<Json<Vec<crate::pipeline::PipelineEvent>>> {
    app.me.admin()?;
    Ok(Json(app.pipeline.recent_events()))
}

async fn repair_pipeline_cbz(
    Me(app): Me,
    Json(payload): Json<crate::pipeline::RepairCbzRequest>,
) -> Result<Json<crate::pipeline::RepairOutcome>> {
    app.me.admin()?;
    let path = if let Some(ref p) = payload.path {
        std::path::PathBuf::from(p)
    } else if let (Some(ref series), Some(volume)) = (&payload.series, payload.volume) {
        let (cfg, _, _) = app.pipeline.load_internal_config().await?;
        let lib_dir = cfg.library_dir.as_deref().unwrap_or("/srv/media/manga");
        let rel_name = crate::pipeline::canonical_release_name(series, true, volume);
        std::path::PathBuf::from(lib_dir)
            .join(series)
            .join(format!("{rel_name}.cbz"))
    } else {
        return Err(AppError::bad_request(
            "Must provide either 'path' or both 'series' and 'volume'",
        ));
    };

    let outcome = app
        .pipeline
        .repair_canonical_cbz(
            &path,
            payload.series.as_deref(),
            payload.volume,
            payload.anilist_id,
            payload.mangabaka_id,
        )
        .await?;

    Ok(Json(outcome))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceDryRunRequest {
    pub library_id: String,
    pub series_id: Option<String>,
}

async fn post_maintenance_dry_run(
    Me(app): Me,
    Json(payload): Json<MaintenanceDryRunRequest>,
) -> Result<Json<crate::pipeline::normalization::LibraryAnalysisSummary>> {
    app.me.admin()?;
    let (cfg, mb_token, komga_api_key) = app.pipeline.load_internal_config().await?;

    let komga_client = crate::pipeline::komga::KomgaClient::new(
        app.trackers.http().clone(),
        cfg.komga_url,
        komga_api_key,
    );
    let mb_client = crate::pipeline::mangabaka::MangaBakaClient::new(
        app.trackers.http().clone(),
        cfg.mangabaka_url,
        mb_token,
    );

    let summary = app
        .pipeline
        .normalization
        .analyze_library(
            &komga_client,
            &mb_client,
            &payload.library_id,
            payload.series_id.as_deref(),
            None,
        )
        .await?;

    Ok(Json(summary))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceNormalizeRequest {
    pub preview_id: String,
    pub book_ids: Vec<String>,
    #[serde(default)]
    pub approved: bool,
    #[serde(default = "maintenance_default_dry_run")]
    pub dry_run: bool,
}

fn maintenance_default_dry_run() -> bool {
    true
}

async fn post_maintenance_normalize(
    Me(app): Me,
    Json(payload): Json<MaintenanceNormalizeRequest>,
) -> Result<Json<crate::pipeline::normalization::NormalizationExecutionResult>> {
    app.me.admin()?;
    let (cfg, _, key) = app.pipeline.load_internal_config().await?;
    let komga =
        crate::pipeline::komga::KomgaClient::new(app.trackers.http().clone(), cfg.komga_url, key);
    Ok(Json(
        app.pipeline
            .normalization
            .execute_preview(
                &payload.preview_id,
                &payload.book_ids,
                payload.approved,
                payload.dry_run,
                &komga,
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct MaintenanceHistoryQuery {
    limit: Option<usize>,
}

async fn get_maintenance_history(
    Me(app): Me,
    Query(query): Query<MaintenanceHistoryQuery>,
) -> Result<Json<Vec<crate::pipeline::normalization::NormalizationHistoryRecord>>> {
    app.me.admin()?;
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let records = app.pipeline.normalization.list_history(limit).await?;
    Ok(Json(records))
}

async fn post_maintenance_refresh(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<crate::pipeline::normalization::NormalizationHistoryRecord>> {
    app.me.admin()?;
    let (cfg, _, key) = app.pipeline.load_internal_config().await?;
    let komga =
        crate::pipeline::komga::KomgaClient::new(app.trackers.http().clone(), cfg.komga_url, key);
    let rec = app
        .pipeline
        .normalization
        .get_history_record(&id)
        .await?
        .ok_or_else(|| AppError::not_found("History record not found"))?;
    app.pipeline
        .normalization
        .refresh_record(&rec, &komga)
        .await?;
    Ok(Json(
        app.pipeline
            .normalization
            .get_history_record(&id)
            .await?
            .unwrap(),
    ))
}

async fn post_maintenance_rollback(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<crate::pipeline::normalization::NormalizationHistoryRecord>> {
    app.me.admin()?;
    let (cfg, _, komga_api_key) = app
        .pipeline
        .load_internal_config()
        .await
        .unwrap_or_default();

    let komga_client = crate::pipeline::komga::KomgaClient::new(
        app.trackers.http().clone(),
        cfg.komga_url,
        komga_api_key,
    );

    let rec = app
        .pipeline
        .normalization
        .rollback_normalization(&id, Some(&komga_client))
        .await?;

    Ok(Json(rec))
}

async fn get_staging_inventory(
    Me(app): Me,
) -> Result<Json<crate::pipeline::staging::StagingInventoryReport>> {
    app.me.admin()?;
    let config = app.pipeline.load_config().await?;
    let downloads_dir = app.downloads.dir();
    let library_dir = config.library_dir.as_ref().map(std::path::PathBuf::from);
    let inventory = crate::pipeline::staging::generate_staging_inventory(
        &app.db,
        downloads_dir,
        library_dir.as_deref(),
    )
    .await?;
    Ok(Json(inventory))
}

async fn post_staging_cleanup_preview(
    Me(app): Me,
) -> Result<Json<crate::pipeline::staging::CleanupManifest>> {
    app.me.admin()?;
    let config = app.pipeline.load_config().await?;
    let downloads_dir = app.downloads.dir();
    let library_dir = config.library_dir.as_ref().map(std::path::PathBuf::from);
    let manifest = crate::pipeline::staging::generate_cleanup_manifest(
        &app.db,
        downloads_dir,
        library_dir.as_deref(),
    )
    .await?;
    Ok(Json(manifest))
}

async fn get_storage_optimizer_status(
    Me(app): Me,
) -> Result<Json<crate::pipeline::duplicates::StorageOptimizerStatus>> {
    app.me.admin()?;
    let config = app.pipeline.load_config().await?;
    let status = crate::pipeline::duplicates::get_storage_optimizer_status(config.optimizer_bin.as_deref()).await;
    Ok(Json(status))
}

async fn get_storage_duplicates(
    Me(app): Me,
) -> Result<Json<crate::pipeline::duplicates::LibraryDuplicateReport>> {
    app.me.admin()?;
    let report = app
        .db
        .call(|conn| crate::pipeline::duplicates::scan_library_duplicates(conn))
        .await?;
    Ok(Json(report))
}

async fn get_dashboard_summary(
    Me(app): Me,
) -> Result<Json<crate::acquisition::dashboard::DashboardSummaryDto>> {
    app.me.admin()?;
    let config = app.pipeline.load_config().await?;
    let downloads_dir = app.downloads.dir();
    let summary = crate::acquisition::dashboard::get_dashboard_summary(
        &app.db,
        downloads_dir,
        config.optimizer_bin.as_deref(),
    )
    .await?;
    Ok(Json(summary))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionSourceOption {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub available: bool,
    pub supports_volumes: bool,
    pub supports_chapters: bool,
}

async fn get_acquisition_sources(Me(app): Me) -> Result<Json<Vec<AcquisitionSourceOption>>> {
    let (cfg, _) = app.acquisition.load_internal_config().await?;
    let sources = vec![
        AcquisitionSourceOption {
            id: "auto".to_string(),
            name: "Auto (Recommended)".to_string(),
            description:
                "Automatically selects the best source based on quality, seeders, and profile rules"
                    .to_string(),
            is_default: true,
            available: true,
            supports_volumes: true,
            supports_chapters: true,
        },
        AcquisitionSourceOption {
            id: "nyaa".to_string(),
            name: "Nyaa".to_string(),
            description:
                "Native BitTorrent release discovery via TorBox debrid acquisition backend"
                    .to_string(),
            is_default: false,
            available: cfg.nyaa_enabled,
            supports_volumes: true,
            supports_chapters: true,
        },
        AcquisitionSourceOption {
            id: "mangadot".to_string(),
            name: "MangaDot".to_string(),
            description: "Direct Tachiyomi/Mihon chapter source with inferred volume numbering"
                .to_string(),
            is_default: false,
            available: true,
            supports_volumes: false,
            supports_chapters: true,
        },
        AcquisitionSourceOption {
            id: "keiyoushi".to_string(),
            name: "Keiyoushi Extensions".to_string(),
            description: "Installed Keiyoushi extensions for chapter-level reader releases"
                .to_string(),
            is_default: false,
            available: false,
            supports_volumes: false,
            supports_chapters: true,
        },
        AcquisitionSourceOption {
            id: "madokami".to_string(),
            name: "Madokami".to_string(),
            description: "Direct HTTP acquisition provider for discrete manga volumes with safe Range resume"
                .to_string(),
            is_default: false,
            available: cfg.madokami_enabled && cfg.has_madokami_credentials,
            supports_volumes: true,
            supports_chapters: false,
        },
    ];
    Ok(Json(sources))
}

async fn post_plan_by_title(
    Me(app): Me,
    Json(payload): Json<crate::acquisition::interactive::PlanningRequest>,
) -> Result<Json<crate::acquisition::interactive::InteractivePlan>> {
    Ok(Json(app.acquisition.plan_interactive(payload).await?))
}

// ---------- acquisition (Nyaa + TorBox) ----------

#[derive(Deserialize)]
struct AcquisitionReleasesQuery {
    unit_type: Option<String>,
    unit_number: Option<f64>,
    sort: Option<String>,
    filter_type: Option<String>,
    filter_ownership: Option<String>,
    filter_uploader: Option<String>,
    min_seeders: Option<u32>,
}

async fn get_title_acquisition_releases(
    Me(app): Me,
    Path(id): Path<i64>,
    Query(query): Query<AcquisitionReleasesQuery>,
) -> Result<Json<Vec<crate::acquisition::AcquisitionCandidate>>> {
    let target_unit = match (query.unit_type.as_deref(), query.unit_number) {
        (Some("volume"), Some(num)) => {
            Some(crate::acquisition::AcquisitionReleaseUnit::Volume(num))
        }
        (Some("chapter"), Some(num)) => {
            Some(crate::acquisition::AcquisitionReleaseUnit::Chapter(num))
        }
        _ => None,
    };
    let candidates = app
        .acquisition
        .search_candidates(
            id,
            target_unit,
            query.sort,
            query.filter_type,
            query.filter_ownership,
            query.filter_uploader,
            query.min_seeders,
        )
        .await?;
    Ok(Json(candidates))
}

async fn post_title_acquire(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<crate::acquisition::service::AcquireReleaseRequest>,
) -> Result<Json<serde_json::Value>> {
    let chapter_id = app
        .acquisition
        .acquire_release_with_identity(
            id,
            payload.release,
            payload.target_unit,
            payload.requested_edition,
            payload.selected_archive,
        )
        .await?;
    Ok(Json(json!({
        "success": true,
        "chapterId": chapter_id
    })))
}

async fn post_title_acquisition_inspect_collection(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(release): Json<crate::acquisition::AcquisitionRelease>,
) -> Result<Json<crate::acquisition::CollectionInspection>> {
    let inspection = app.acquisition.inspect_collection(id, release).await?;
    Ok(Json(inspection))
}

async fn post_title_acquire_collection(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<crate::acquisition::AcquireCollectionRequest>,
) -> Result<Json<serde_json::Value>> {
    let chapter_ids = app.acquisition.acquire_collection(id, payload).await?;
    Ok(Json(json!({
        "success": true,
        "queuedCount": chapter_ids.len(),
        "chapterIds": chapter_ids,
    })))
}

async fn get_acquisition_settings(
    Me(app): Me,
) -> Result<Json<crate::acquisition::AcquisitionConfig>> {
    let config = app.acquisition.load_config().await?;
    Ok(Json(config))
}

async fn put_acquisition_settings(
    Me(app): Me,
    Json(patch): Json<crate::acquisition::service::AcquisitionConfigPatch>,
) -> Result<Json<crate::acquisition::AcquisitionConfig>> {
    app.me.admin()?;
    app.acquisition.save_config(patch).await?;
    let config = app.acquisition.load_config().await?;
    Ok(Json(config))
}

#[derive(Deserialize)]
struct TestTorBoxPayload {
    url: Option<String>,
    token: Option<String>,
}

async fn test_torbox_connection(
    Me(app): Me,
    Json(payload): Json<TestTorBoxPayload>,
) -> Result<Json<serde_json::Value>> {
    app.me.admin()?;
    let msg = app
        .acquisition
        .test_torbox(payload.url, payload.token)
        .await?;
    Ok(Json(json!({
        "success": true,
        "message": msg
    })))
}

#[derive(Deserialize)]
struct AcquisitionHistoryQuery {
    manga_id: Option<i64>,
    limit: Option<i64>,
}

async fn get_acquisition_history(
    Me(app): Me,
    Query(query): Query<AcquisitionHistoryQuery>,
) -> Result<Json<Vec<crate::acquisition::AcquisitionHistoryRecord>>> {
    let history = app
        .acquisition
        .list_history(query.manga_id, query.limit)
        .await?;
    Ok(Json(history))
}

async fn get_acquisition_profiles(
    Me(app): Me,
) -> Result<Json<Vec<crate::acquisition::profiles::AcquisitionProfile>>> {
    let profiles = app.acquisition.list_profiles().await?;
    Ok(Json(profiles))
}

async fn post_create_acquisition_profile(
    Me(app): Me,
    Json(profile): Json<crate::acquisition::profiles::AcquisitionProfile>,
) -> Result<Json<crate::acquisition::profiles::AcquisitionProfile>> {
    let created = app.acquisition.create_profile(profile).await?;
    Ok(Json(created))
}

async fn get_acquisition_profile(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<Option<crate::acquisition::profiles::AcquisitionProfile>>> {
    let profile = app.acquisition.get_profile(id).await?;
    Ok(Json(profile))
}

async fn put_update_acquisition_profile(
    Me(app): Me,
    Path(id): Path<String>,
    Json(patch): Json<crate::acquisition::profiles::ProfilePatch>,
) -> Result<Json<crate::acquisition::profiles::AcquisitionProfile>> {
    let updated = app.acquisition.update_profile(id, patch).await?;
    Ok(Json(updated))
}

async fn delete_acquisition_profile(Me(app): Me, Path(id): Path<String>) -> Result<Json<bool>> {
    app.acquisition.delete_profile(id).await?;
    Ok(Json(true))
}

#[derive(Deserialize)]
struct DuplicateProfilePayload {
    name: Option<String>,
}

async fn post_duplicate_acquisition_profile(
    Me(app): Me,
    Path(id): Path<String>,
    Json(payload): Json<Option<DuplicateProfilePayload>>,
) -> Result<Json<crate::acquisition::profiles::AcquisitionProfile>> {
    let name = payload.and_then(|p| p.name);
    let dup = app.acquisition.duplicate_profile(id, name).await?;
    Ok(Json(dup))
}

#[derive(Deserialize)]
struct TitleProfilePayload {
    #[serde(rename = "profileId")]
    profile_id: Option<String>,
}

async fn get_title_acquisition_profile(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::acquisition::profiles::AcquisitionProfile>> {
    let prof = app.acquisition.get_manga_profile(id).await?;
    Ok(Json(prof))
}

async fn put_title_acquisition_profile(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<TitleProfilePayload>,
) -> Result<Json<crate::acquisition::profiles::AcquisitionProfile>> {
    app.acquisition
        .set_manga_profile(id, payload.profile_id)
        .await?;
    let prof = app.acquisition.get_manga_profile(id).await?;
    Ok(Json(prof))
}

async fn get_title_missing_content(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::acquisition::missing::MissingContentSummary>> {
    let summary = app.acquisition.get_title_missing_content(id).await?;
    Ok(Json(summary))
}

#[derive(Deserialize, Default)]
struct TitlePlanPayload {
    #[serde(default)]
    #[serde(rename = "dryRun")]
    dry_run: Option<bool>,
}

async fn post_title_plan(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<Option<TitlePlanPayload>>,
) -> Result<Json<crate::acquisition::planner::AcquisitionPlan>> {
    let dry_run = payload.and_then(|p| p.dry_run).unwrap_or(true);
    let plan = app
        .acquisition
        .plan_title_missing_content(id, dry_run)
        .await?;
    Ok(Json(plan))
}

async fn get_title_plan(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Option<crate::acquisition::planner::AcquisitionPlan>>> {
    let plan = app.acquisition.get_title_plan(id).await?;
    Ok(Json(plan))
}

async fn get_global_missing(
    Me(app): Me,
) -> Result<Json<Vec<crate::acquisition::missing::MissingContentSummary>>> {
    let summaries = app.acquisition.get_global_missing_content().await?;
    Ok(Json(summaries))
}

async fn post_plan_all(
    Me(app): Me,
) -> Result<Json<crate::acquisition::planner::GlobalPlanSummary>> {
    let summary = app.acquisition.plan_all_missing_content().await?;
    Ok(Json(summary))
}

async fn get_acquisition_sources_health(
    Me(app): Me,
) -> Result<Json<Vec<crate::acquisition::intelligence::SourceHealthReport>>> {
    let reports = app.acquisition.get_source_health_reports().await?;
    Ok(Json(reports))
}

async fn post_create_execution(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<crate::acquisition::execution::ApprovePlanRequest>,
) -> Result<Json<crate::acquisition::execution::AcquisitionExecution>> {
    let execution = app
        .acquisition
        .approve_and_create_execution(id, payload)
        .await?;
    let started = app.acquisition.start_execution(execution.id).await?;
    Ok(Json(started))
}

async fn get_title_executions(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::acquisition::execution::AcquisitionExecution>>> {
    let list = app.acquisition.get_title_executions(id).await?;
    Ok(Json(list))
}

async fn get_acquisition_execution(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<crate::acquisition::execution::AcquisitionExecution>> {
    let exec = app.acquisition.get_execution(&id).await?;
    match exec {
        Some(e) => Ok(Json(e)),
        None => Err(AppError::not_found("Execution not found")),
    }
}

async fn post_start_execution(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<crate::acquisition::execution::AcquisitionExecution>> {
    let exec = app.acquisition.start_execution(id).await?;
    Ok(Json(exec))
}

async fn post_cancel_execution(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    app.acquisition
        .cancel_execution(&id, "Cancelled by user via API")
        .await?;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn post_retry_execution(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<crate::acquisition::execution::AcquisitionExecution>> {
    let exec = app.acquisition.retry_execution(&id).await?;
    Ok(Json(exec))
}

#[derive(Deserialize, Default)]
struct SyncRunPayload {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    library_id: Option<String>,
}

async fn sync_dry_run(
    Me(app): Me,
    Json(payload): Json<Option<SyncRunPayload>>,
) -> Result<Json<crate::sync::model::SyncSummary>> {
    let payload = payload.unwrap_or_default();
    let target = payload
        .target
        .as_deref()
        .and_then(crate::sync::model::SyncTarget::parse);
    let summary = app.sync.run_sync(true, target, payload.library_id).await?;
    Ok(Json(summary))
}

async fn sync_run(
    Me(app): Me,
    Json(payload): Json<Option<SyncRunPayload>>,
) -> Result<Json<crate::sync::model::SyncSummary>> {
    app.me.admin()?;
    let payload = payload.unwrap_or_default();
    let target = payload
        .target
        .as_deref()
        .and_then(crate::sync::model::SyncTarget::parse);
    let summary = app.sync.run_sync(false, target, payload.library_id).await?;
    Ok(Json(summary))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusResponse {
    pub running: bool,
    pub komga_enabled: bool,
    pub has_komga_api_key: bool,
    pub mangabaka_enabled: bool,
    pub has_mangabaka_token: bool,
    pub mangabaka_validated: Option<bool>,
}

async fn sync_status(Me(app): Me) -> Result<Json<SyncStatusResponse>> {
    let (cfg, _mb_token, _komga_key) = app.pipeline.load_internal_config().await?;
    Ok(Json(SyncStatusResponse {
        running: app.sync.is_running(),
        komga_enabled: cfg.komga_enabled,
        has_komga_api_key: cfg.has_komga_api_key,
        mangabaka_enabled: cfg.mangabaka_enabled,
        has_mangabaka_token: cfg.has_mangabaka_token,
        mangabaka_validated: cfg.mangabaka_validated,
    }))
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

async fn sync_history(
    Me(app): Me,
    Query(query): Query<LimitQuery>,
) -> Result<Json<Vec<crate::sync::model::SyncRecord>>> {
    let limit = query.limit.unwrap_or(100);
    let records = app.sync.history(limit).await?;
    Ok(Json(records))
}

#[derive(Deserialize)]
struct MappingsQuery {
    tracker: Option<String>,
}

async fn get_sync_mappings(
    Me(app): Me,
    Query(query): Query<MappingsQuery>,
) -> Result<Json<Vec<crate::sync::model::TrackerMapping>>> {
    let trk = query
        .tracker
        .as_deref()
        .and_then(crate::sync::model::SyncTarget::parse);
    let mappings = app
        .db
        .call(move |conn| crate::sync::mapping::list_mappings(conn, trk))
        .await?;
    Ok(Json(mappings))
}

async fn put_sync_mapping(
    Me(app): Me,
    Json(mapping): Json<crate::sync::model::TrackerMapping>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    app.db
        .call(move |conn| crate::sync::mapping::save_mapping(conn, &mapping))
        .await?;
    Ok(Json(true))
}

async fn delete_sync_mapping(
    Me(app): Me,
    Path((series_id, tracker_str)): Path<(String, String)>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    let trk = crate::sync::model::SyncTarget::parse(&tracker_str)
        .ok_or_else(|| AppError::bad_request(format!("Invalid tracker: {tracker_str}")))?;
    let deleted = app
        .db
        .call(move |conn| crate::sync::mapping::delete_mapping(conn, &series_id, trk))
        .await?;
    Ok(Json(deleted))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualMappingPayload {
    komga_series_id: String,
    tracker: String,
    tracker_id: String,
    edition_id: Option<String>,
    tracker_title: Option<String>,
}

async fn post_sync_manual_mapping(
    Me(app): Me,
    Json(payload): Json<ManualMappingPayload>,
) -> Result<Json<crate::sync::model::TrackerMapping>> {
    app.me.admin()?;
    let trk = crate::sync::model::SyncTarget::parse(&payload.tracker)
        .ok_or_else(|| AppError::bad_request(format!("Invalid tracker: {}", payload.tracker)))?;
    let mapping = app
        .db
        .call(move |conn| {
            crate::sync::mapping::set_manual_override(
                conn,
                &payload.komga_series_id,
                trk,
                &payload.tracker_id,
                payload.edition_id.as_deref(),
                payload.tracker_title.as_deref(),
            )
        })
        .await?;
    Ok(Json(mapping))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetMappingPayload {
    komga_series_id: String,
    tracker: String,
}

async fn post_sync_reset_mapping(
    Me(app): Me,
    Json(payload): Json<ResetMappingPayload>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    let trk = crate::sync::model::SyncTarget::parse(&payload.tracker)
        .ok_or_else(|| AppError::bad_request(format!("Invalid tracker: {}", payload.tracker)))?;
    app.db
        .call(move |conn| {
            crate::sync::mapping::clear_manual_override(conn, &payload.komga_series_id, trk)
        })
        .await?;
    Ok(Json(true))
}

async fn get_sync_unresolved_diagnostics(
    Me(app): Me,
) -> Result<Json<Vec<crate::sync::model::UnresolvedTitleDiagnostic>>> {
    let unres = app
        .db
        .call(|conn| crate::sync::mapping::list_unresolved_mappings(conn))
        .await?;
    let diagnostics: Vec<crate::sync::model::UnresolvedTitleDiagnostic> = unres
        .into_iter()
        .map(|m| crate::sync::model::UnresolvedTitleDiagnostic {
            komga_series_id: m.komga_series_id.clone(),
            komga_title: m
                .tracker_title
                .clone()
                .unwrap_or_else(|| m.komga_series_id.clone()),
            tracker: m.tracker,
            reason: match m.confidence {
                crate::sync::model::MappingConfidence::Ambiguous => {
                    "Multiple candidates found; manual selection required".to_string()
                }
                crate::sync::model::MappingConfidence::NeedsMapping => {
                    "Series found but specific edition mapping is missing".to_string()
                }
                _ => "Unresolved mapping".to_string(),
            },
            candidates: vec![],
            confidence: m.confidence,
            source: m.mapping_source,
        })
        .collect();
    Ok(Json(diagnostics))
}

async fn get_sync_schedule(
    Me(app): Me,
) -> Result<Json<crate::sync::scheduler::SyncScheduleConfig>> {
    let cfg = app
        .db
        .call(|conn| Ok(crate::sync::scheduler::get_schedule_config(conn)))
        .await?;
    Ok(Json(cfg))
}

async fn put_sync_schedule(
    Me(app): Me,
    Json(cfg): Json<crate::sync::scheduler::SyncScheduleConfig>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    app.db
        .call(move |conn| crate::sync::scheduler::set_schedule_config(conn, &cfg))
        .await?;
    Ok(Json(true))
}

async fn get_tracked_catalog(
    Me(app): Me,
) -> Result<Json<Vec<crate::sync::catalog::TrackedCatalogEntry>>> {
    let entries = app
        .db
        .call(|conn| crate::sync::catalog::list_tracked(conn))
        .await?;
    Ok(Json(entries))
}

#[derive(Deserialize)]
struct AddTrackedPayload {
    title: String,
    #[serde(default = "default_origin")]
    origin: String,
    komga_series_id: Option<String>,
    manga_id: Option<i64>,
}

fn default_origin() -> String {
    "mango".to_string()
}

async fn post_tracked_catalog(
    Me(app): Me,
    Json(payload): Json<AddTrackedPayload>,
) -> Result<Json<crate::sync::catalog::TrackedCatalogEntry>> {
    let entry = app
        .db
        .call(move |conn| {
            crate::sync::catalog::add_or_update_tracked(
                conn,
                &payload.title,
                &payload.origin,
                payload.komga_series_id.as_deref(),
                payload.manga_id,
            )
        })
        .await?;
    Ok(Json(entry))
}

#[derive(Deserialize)]
struct ToggleFlagPayload {
    enabled: bool,
}

async fn put_tracked_auto_acquire(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<ToggleFlagPayload>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    app.db
        .call(move |conn| crate::sync::catalog::set_auto_acquire(conn, id, payload.enabled))
        .await?;
    Ok(Json(true))
}

async fn put_tracked_status(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<ToggleFlagPayload>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    app.db
        .call(move |conn| crate::sync::catalog::set_tracked(conn, id, payload.enabled))
        .await?;
    Ok(Json(true))
}

async fn get_lists(
    Me(app): Me,
    Query(query): Query<crate::sync::lists::ListsQuery>,
) -> Result<Json<crate::sync::lists::UnifiedCatalogResponse>> {
    let (cfg, _, komga_api_key) = app
        .pipeline
        .load_internal_config()
        .await
        .unwrap_or_default();
    let komga_client = crate::pipeline::komga::KomgaClient::new(
        app.trackers.http().clone(),
        cfg.komga_url,
        komga_api_key,
    );

    let komga_progress = crate::sync::komga_reader::fetch_all_series_progress(
        &komga_client,
        cfg.komga_library_id.as_deref(),
    )
    .await
    .unwrap_or_default();

    let res = app
        .db
        .call(move |conn| crate::sync::lists::build_unified_catalog(conn, &komga_progress, &query))
        .await?;

    Ok(Json(res))
}

async fn post_lists_refresh(
    Me(app): Me,
    Json(payload): Json<crate::sync::lists::RefreshListsPayload>,
) -> Result<Json<crate::sync::lists::ListsRefreshReport>> {
    let report = crate::sync::lists::refresh_tracker_lists(
        &app.db,
        &app.trackers,
        &app.pipeline,
        payload.tracker.as_deref(),
    )
    .await?;

    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    header::HeaderName::from_static(name),
                    value.parse().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn requests_from_other_sites_are_told_apart() {
        // Where the browser labels a request, the label decides.
        assert!(from_elsewhere(&headers(&[(
            "sec-fetch-site",
            "cross-site"
        )])));
        assert!(from_elsewhere(&headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://mango.example.org"),
            ("host", "mango.example.org")
        ])));
        assert!(!from_elsewhere(&headers(&[(
            "sec-fetch-site",
            "same-origin"
        )])));
        assert!(!from_elsewhere(&headers(&[("sec-fetch-site", "none")])));

        // Over plain HTTP there is no label, and the page it comes from has to be Mango's own.
        let at = ("host", "10.0.0.1:8787");
        assert!(!from_elsewhere(&headers(&[
            ("origin", "http://10.0.0.1:8787"),
            at
        ])));
        assert!(from_elsewhere(&headers(&[
            ("origin", "http://other.example.org"),
            at
        ])));
        assert!(
            from_elsewhere(&headers(&[("origin", "http://10.0.0.1:3000"), at])),
            "another port is another site's page"
        );
        assert!(from_elsewhere(&headers(&[("origin", "null"), at])));
        let proxied = [
            ("origin", "http://mango.example.org"),
            ("host", "127.0.0.1:8787"),
            ("x-forwarded-host", "mango.example.org"),
        ];
        assert!(
            !from_elsewhere(&headers(&proxied)),
            "behind a proxy that names Mango its own way"
        );

        // A reading app or a script sends neither, and is not a page at all.
        assert!(!from_elsewhere(&headers(&[at])));
    }

    #[test]
    fn gzip_goes_to_who_reads_it() {
        assert!(reads_gzip(&headers(&[(
            "accept-encoding",
            "gzip, deflate, br"
        )])));
        assert!(reads_gzip(&headers(&[(
            "accept-encoding",
            "br;q=1.0, GZIP;q=0.5"
        )])));
        assert!(!reads_gzip(&headers(&[(
            "accept-encoding",
            "br, gzip;q=0"
        )])));
        assert!(!reads_gzip(&headers(&[("accept-encoding", "identity")])));
        assert!(!reads_gzip(&headers(&[])));
    }

    #[test]
    fn a_mark_is_known_again() {
        let mark = "W/\"abc\"";
        assert!(names("W/\"abc\"", mark));
        assert!(
            names("\"other\", \"abc\"", mark),
            "one of several, and strong for weak"
        );
        assert!(names("*", mark));
        assert!(!names("\"abd\"", mark));
        assert!(!names("", mark));
    }
}

#[cfg(test)]
mod maintenance_request_tests {
    use super::*;
    #[test]
    fn mutation_requires_explicit_flags_and_saved_preview() {
        let req: MaintenanceNormalizeRequest = serde_json::from_value(serde_json::json!({
            "previewId":"preview", "bookIds":["book"]
        }))
        .unwrap();
        assert!(req.dry_run);
        assert!(!req.approved);
        assert!(
            serde_json::from_value::<MaintenanceNormalizeRequest>(serde_json::json!({
                "items":[{"canonicalPath":"/arbitrary.cbz","mangabakaId":1}], "dryRun":false
            }))
            .is_err()
        );
    }
}

async fn get_import_candidates(
    Me(app): Me,
) -> Result<Json<Vec<crate::acquisition::physical::Candidate>>> {
    Ok(Json(crate::acquisition::physical::list(&app.db).await?))
}
async fn assign_import_candidate(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(body): Json<crate::acquisition::physical::Assignment>,
) -> Result<Json<Value>> {
    crate::acquisition::physical::assign(&app.db, id, body, app.downloads.dir()).await?;
    app.downloads.wake().notify_one();
    Ok(Json(json!({"ok":true})))
}

async fn retry_import_inspection(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    crate::acquisition::physical::retry_inspection(&app.db, id).await?;
    app.downloads.wake().notify_one();
    Ok(Json(json!({"ok":true})))
}

async fn cleanup_import_payload(Me(app): Me, Path(id): Path<i64>) -> Result<Json<Value>> {
    crate::acquisition::physical::cleanup(&app.db, id, app.downloads.dir()).await?;
    Ok(Json(json!({"ok":true})))
}

async fn get_notification_config(
    Me(app): Me,
) -> Result<Json<crate::notification::NtfyConfigDto>> {
    app.me.admin()?;
    let config = app.notification.get_config().await?;
    Ok(Json(config))
}

async fn post_notification_config(
    Me(app): Me,
    Json(payload): Json<crate::notification::service::UpdateNtfyConfigRequest>,
) -> Result<Json<crate::notification::NtfyConfigDto>> {
    app.me.admin()?;
    let updated = app.notification.save_config(payload).await?;
    Ok(Json(updated))
}

async fn post_notification_test(
    Me(app): Me,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    app.me.admin()?;
    let req: Option<crate::notification::service::TestNotificationRequest> = if body.is_empty() {
        None
    } else {
        serde_json::from_slice(&body).ok()
    };
    match app.notification.send_test(req).await {
        Ok(()) => Ok(Json(json!({
            "success": true,
            "message": "Test notification sent successfully"
        }))),
        Err(e) => Ok(Json(json!({
            "success": false,
            "error": e.to_string()
        }))),
    }
}

async fn get_notification_outbox(
    Me(app): Me,
) -> Result<Json<Vec<crate::notification::OutboxEntry>>> {
    app.me.admin()?;
    let entries = app.notification.get_outbox(50).await?;
    Ok(Json(entries))
}

async fn get_opentome_status(
    Me(app): Me,
) -> Result<Json<crate::opentome::updater::OpenTomeStatus>> {
    let status = app.opentome.status().await?;
    Ok(Json(status))
}

#[derive(Deserialize, Default)]
struct OpenTomeUpdatePayload {
    force: Option<bool>,
}

async fn post_opentome_update(
    Me(app): Me,
    payload: Option<Json<OpenTomeUpdatePayload>>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    let force = payload.and_then(|p| p.force).unwrap_or(false);
    let updated = app.opentome.check_for_updates(force).await?;
    Ok(Json(json!({
        "success": true,
        "updated": updated
    })))
}

#[derive(Deserialize)]
struct OpenTomeConfigPayload {
    enabled: bool,
}

async fn post_opentome_config(
    Me(app): Me,
    Json(payload): Json<OpenTomeConfigPayload>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    app.opentome.set_enabled(payload.enabled).await?;
    Ok(Json(json!({"ok": true})))
}

async fn get_manga_opentome(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Option<crate::opentome::catalog::OpenTomeSeriesSchedule>>> {
    let schedule = app.opentome.get_series_schedule(id).await?;
    Ok(Json(schedule))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenTomeRemapPayload {
    work_id: Option<String>,
    action: String,
}

async fn post_manga_opentome_remap(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<OpenTomeRemapPayload>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    app.opentome.remap_manga(id, payload.work_id, &payload.action).await?;
    Ok(Json(json!({"ok": true})))
}

async fn get_manga_release_discovery(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::acquisition::release_discovery::ReleaseDiscoveryTarget>>> {
    let targets = app.opentome.evaluate_manga_release_discovery(id).await?;
    Ok(Json(targets))
}

async fn post_manga_release_discovery_check(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::acquisition::release_discovery::ReleaseDiscoveryTarget>>> {
    app.me.admin()?;
    let targets = app.opentome.evaluate_manga_release_discovery(id).await?;
    for t in &targets {
        if t.status == "candidate_available" {
            let _ = crate::acquisition::auto_acquire::evaluate_target_for_auto_acquire(
                &app.db,
                &app.acquisition,
                t.id,
            )
            .await;
        }
    }
    let updated_targets = app
        .db
        .call(move |conn| crate::acquisition::release_discovery::list_targets_for_manga(conn, id))
        .await?;
    Ok(Json(updated_targets))
}

async fn post_manga_release_discovery_retry(
    Me(app): Me,
    Path((_id, target_id)): Path<(i64, i64)>,
) -> Result<Json<Option<crate::acquisition::release_discovery::ReleaseDiscoveryTarget>>> {
    app.me.admin()?;
    // Reset status to releases_today to force re-evaluation
    let cur_time = crate::db::now();
    app.db
        .call(move |conn| {
            conn.execute(
                "UPDATE release_discovery_targets
                 SET status = 'releases_today', next_attempt_at = 0, updated_at = ?1
                 WHERE id = ?2 AND status IN ('not_found_retry_wait', 'dormant', 'mapping_review_required')",
                [cur_time, target_id],
            )?;
            Ok(())
        })
        .await?;

    let res = crate::acquisition::release_discovery::process_target_discovery(
        &app.db,
        app.opentome.http().clone(),
        target_id,
    )
    .await?;

    if let Some(ref t) = res {
        if t.status == "candidate_available" {
            let _ = crate::acquisition::auto_acquire::evaluate_target_for_auto_acquire(
                &app.db,
                &app.acquisition,
                t.id,
            )
            .await;
        }
    }

    let updated = app
        .db
        .call(move |conn| crate::acquisition::release_discovery::get_target_by_id(conn, target_id))
        .await?;
    Ok(Json(updated))
}

async fn post_manga_release_discovery_evaluate_auto(
    Me(app): Me,
    Path((_id, target_id)): Path<(i64, i64)>,
) -> Result<Json<crate::acquisition::auto_acquire::AutoAcquireDecision>> {
    let decision = crate::acquisition::auto_acquire::evaluate_target_for_auto_acquire(
        &app.db,
        &app.acquisition,
        target_id,
    )
    .await?;
    Ok(Json(decision))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MangaAutoAcquireResponse {
    enabled: bool,
    activated_at: Option<i64>,
}

#[derive(Deserialize)]
struct SetAutoAcquirePayload {
    #[serde(alias = "autoAcquire", alias = "auto_acquire")]
    enabled: bool,
}

async fn get_manga_auto_acquire(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<MangaAutoAcquireResponse>> {
    let (enabled, activated_at) = app
        .db
        .call(move |conn| crate::acquisition::auto_acquire::get_title_auto_acquire(conn, id))
        .await?;
    Ok(Json(MangaAutoAcquireResponse {
        enabled,
        activated_at,
    }))
}

async fn put_manga_auto_acquire(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<SetAutoAcquirePayload>,
) -> Result<Json<MangaAutoAcquireResponse>> {
    app.me.admin()?;
    app.db
        .call(move |conn| {
            crate::acquisition::auto_acquire::set_title_auto_acquire(conn, id, payload.enabled)
        })
        .await?;

    let (enabled, activated_at) = app
        .db
        .call(move |conn| crate::acquisition::auto_acquire::get_title_auto_acquire(conn, id))
        .await?;
    Ok(Json(MangaAutoAcquireResponse {
        enabled,
        activated_at,
    }))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoAcquireSettingsDto {
    enabled: bool,
    dry_run: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchAutoAcquireSettingsDto {
    enabled: Option<bool>,
    dry_run: Option<bool>,
}

async fn get_auto_acquire_settings(Me(app): Me) -> Result<Json<AutoAcquireSettingsDto>> {
    let (enabled, dry_run) = app
        .db
        .call(|conn| {
            Ok((
                crate::acquisition::auto_acquire::is_global_auto_acquire_enabled(conn),
                crate::acquisition::auto_acquire::is_auto_acquire_dry_run(conn),
            ))
        })
        .await?;
    Ok(Json(AutoAcquireSettingsDto { enabled, dry_run }))
}

async fn put_auto_acquire_settings(
    Me(app): Me,
    Json(payload): Json<PatchAutoAcquireSettingsDto>,
) -> Result<Json<AutoAcquireSettingsDto>> {
    app.me.admin()?;
    app.db
        .call(move |conn| {
            if let Some(en) = payload.enabled {
                crate::acquisition::auto_acquire::set_global_auto_acquire(conn, en)?;
            }
            if let Some(dr) = payload.dry_run {
                crate::acquisition::auto_acquire::set_auto_acquire_dry_run(conn, dr)?;
            }
            Ok(())
        })
        .await?;

    let (enabled, dry_run) = app
        .db
        .call(|conn| {
            Ok((
                crate::acquisition::auto_acquire::is_global_auto_acquire_enabled(conn),
                crate::acquisition::auto_acquire::is_auto_acquire_dry_run(conn),
            ))
        })
        .await?;
    Ok(Json(AutoAcquireSettingsDto { enabled, dry_run }))
}

#[derive(Deserialize)]
struct CatalogSearchQuery {
    q: String,
}

async fn get_catalog_mangabaka_search(
    Me(app): Me,
    Query(query): Query<CatalogSearchQuery>,
) -> Result<Json<Vec<crate::catalog::MangaBakaSearchResult>>> {
    let (cfg, mb_token, _) = app.pipeline.load_internal_config().await?;
    let mb_client = crate::pipeline::mangabaka::MangaBakaClient::new(
        app.trackers.http().clone(),
        cfg.mangabaka_url,
        mb_token,
    );
    let results = crate::catalog::search_mangabaka_catalog(&mb_client, &app.db, &query.q).await?;
    Ok(Json(results))
}

async fn post_catalog_mangabaka_add(
    Me(app): Me,
    Path(id): Path<i64>,
    payload: Option<Json<crate::catalog::AddMangaBakaPayload>>,
) -> Result<Json<crate::catalog::AddMangaBakaResponse>> {
    app.me.admin()?;
    let convert_existing = payload.and_then(|p| p.convert_existing).unwrap_or(false);
    let res = crate::catalog::add_mangabaka_catalog_title(&app, id, convert_existing).await?;
    Ok(Json(res))
}

async fn post_manga_convert_to_catalog(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::catalog::ConvertToCatalogResponse>> {
    app.me.admin()?;
    let res = crate::catalog::convert_to_catalog_title(&app, id).await?;
    Ok(Json(res))
}

async fn post_manga_revert_to_source(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::catalog::RevertToSourceResponse>> {
    app.me.admin()?;
    let res = crate::catalog::revert_to_source_title(&app, id).await?;
    Ok(Json(res))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachSourcePayload {
    pub source_id: Option<i64>,
}

async fn post_manga_detach_source(
    Me(app): Me,
    Path(id): Path<i64>,
    payload: Option<Json<DetachSourcePayload>>,
) -> Result<Json<crate::catalog::DetachSourceResponse>> {
    app.me.admin()?;
    let specific = payload.and_then(|p| p.0.source_id);
    let res = crate::catalog::detach_source_from_catalog(&app, id, specific).await?;
    Ok(Json(res))
}

async fn post_manga_attach_source(
    Me(app): Me,
    Path((catalog_id, source_id)): Path<(i64, i64)>,
) -> Result<Json<crate::catalog::ConvertToCatalogResponse>> {
    app.me.admin()?;
    let res = crate::catalog::attach_source_to_catalog(&app, catalog_id, source_id).await?;
    Ok(Json(res))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MangaBakaLinkPayload {
    pub mangabaka_id: Option<i64>,
    pub input: Option<String>,
}

async fn put_manga_mangabaka(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<MangaBakaLinkPayload>,
) -> Result<Json<crate::sync::reconciliation::ReconciliationSummary>> {
    use rusqlite::OptionalExtension;
    app.me.admin()?;
    let mb_id = if let Some(mid) = payload.mangabaka_id {
        Some(mid)
    } else if let Some(ref inp) = payload.input {
        crate::catalog::parse_mangabaka_id_input(inp)
    } else {
        None
    };

    let mb_id = mb_id.ok_or_else(|| AppError::bad_request("Valid MangaBaka ID or URL is required"))?;

    let (title, komga_series_id) = app
        .db
        .call(move |conn| {
            conn.execute(
                "UPDATE manga SET mangabaka_id = ?1 WHERE id = ?2",
                rusqlite::params![mb_id, id],
            )?;

            let title: Option<String> = conn
                .query_row(
                    "SELECT title FROM manga WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;

            let k_id: Option<String> = conn
                .query_row(
                    "SELECT komga_series_id FROM tracked_catalog WHERE manga_id = ?1 LIMIT 1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .unwrap_or(None);

            if let Some(ref kid) = k_id {
                conn.execute(
                    "INSERT INTO tracker_mappings (komga_series_id, tracker, tracker_id, updated_at, mapping_source, confidence)
                     VALUES (?1, 'mangabaka', ?2, ?3, 'manual', 'resolved')
                     ON CONFLICT(komga_series_id, tracker) DO UPDATE SET
                        tracker_id = excluded.tracker_id,
                        confidence = 'resolved',
                        updated_at = excluded.updated_at",
                    rusqlite::params![kid, mb_id.to_string(), crate::db::now()],
                )?;
            }

            Ok((title.unwrap_or_default(), k_id))
        })
        .await?;

    let target = crate::sync::reconciliation::ReconciliationTarget {
        manga_id: Some(id),
        komga_series_id,
        title,
        edition: None,
    };
    let summary = app.sync.run_reconciliation(target).await?;
    Ok(Json(summary))
}

// -------------------------------------------------------------
// M6.8.3 Observability, Diagnostics & Control UX Handlers
// -------------------------------------------------------------

async fn get_manga_diagnostics(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::sync::diagnostics::TitleDiagnostics>> {
    let diag = app
        .db
        .call(move |conn| crate::sync::diagnostics::get_title_diagnostics(conn, id))
        .await?;
    Ok(Json(diag))
}

async fn get_series_timeline_endpoint(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::sync::timeline::SeriesTimelineDto>> {
    let timeline = app
        .db
        .call(move |conn| crate::sync::timeline::get_series_timeline(conn, id))
        .await?;
    Ok(Json(timeline))
}

async fn get_series_gaps_endpoint(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::sync::timeline::SeriesGapAnalysisDto>> {
    let gaps = app
        .db
        .call(move |conn| crate::sync::timeline::get_series_gap_analysis(conn, id))
        .await?;
    Ok(Json(gaps))
}

async fn get_manga_volume_candidates_inspect(
    Me(app): Me,
    Path((id, volume)): Path<(i64, f64)>,
) -> Result<Json<crate::sync::diagnostics::VolumeCandidateInspectionResult>> {
    use rusqlite::OptionalExtension;
    let series_id = app
        .db
        .call(move |conn| {
            if let Some(sum) = crate::sync::reconciliation::get_reconciliation_for_manga(conn, id)? {
                Ok(Some(sum.series_id))
            } else {
                let title: Option<String> = conn
                    .query_row("SELECT title FROM manga WHERE id = ?1", [id], |r| r.get(0))
                    .optional()?;
                Ok(title.as_deref().map(|t| format!("title:{}", crate::pipeline::mangabaka::normalize_title(t))))
            }
        })
        .await?;

    let Some(s_id) = series_id else {
        return Err(AppError::not_found(format!("Manga {id} not found")));
    };

    let profile = app.acquisition.get_manga_profile(id).await.unwrap_or_default();

    let s_id_for_hashes = s_id.clone();
    let hashes: Vec<String> = app
        .db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT info_hash FROM discovered_release_candidates
                 WHERE (series_id = ?1 OR series_id = 'manga:' || ?1)
                   AND ABS(volume_number - ?2) < 0.001
                   AND info_hash IS NOT NULL",
            )?;
            let rows = stmt.query_map(rusqlite::params![s_id_for_hashes, volume], |r| {
                r.get::<_, String>(0)
            })?;
            Ok(rows.flatten().collect())
        })
        .await?;

    let cache_map = if let Some(torbox) = app.acquisition.get_torbox_client().await {
        torbox.check_cached_torrents(&hashes).await
    } else {
        std::collections::HashMap::new()
    };

    let res = app
        .db
        .call(move |conn| {
            crate::sync::diagnostics::inspect_volume_candidates(
                conn,
                &s_id,
                volume,
                &profile,
                Some(&cache_map),
            )
        })
        .await?;

    Ok(Json(res))
}

async fn get_manga_volume_candidates_compare(
    Me(app): Me,
    Path((id, volume)): Path<(i64, f64)>,
) -> Result<Json<crate::acquisition::quality::CandidateComparisonMatrix>> {
    use rusqlite::OptionalExtension;
    let series_id = app
        .db
        .call(move |conn| {
            if let Some(sum) = crate::sync::reconciliation::get_reconciliation_for_manga(conn, id)? {
                Ok(Some(sum.series_id))
            } else {
                let title: Option<String> = conn
                    .query_row("SELECT title FROM manga WHERE id = ?1", [id], |r| r.get(0))
                    .optional()?;
                Ok(title.as_deref().map(|t| format!("title:{}", crate::pipeline::mangabaka::normalize_title(t))))
            }
        })
        .await?;

    let Some(s_id) = series_id else {
        return Err(AppError::not_found(format!("Manga {id} not found")));
    };

    let profile = app.acquisition.get_manga_profile(id).await.unwrap_or_default();

    let s_id_for_hashes = s_id.clone();
    let hashes: Vec<String> = app
        .db
        .call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT info_hash FROM discovered_release_candidates
                 WHERE (series_id = ?1 OR series_id = 'manga:' || ?1)
                   AND ABS(volume_number - ?2) < 0.001
                   AND info_hash IS NOT NULL",
            )?;
            let rows = stmt.query_map(rusqlite::params![s_id_for_hashes, volume], |r| {
                r.get::<_, String>(0)
            })?;
            Ok(rows.flatten().collect())
        })
        .await?;

    let cache_map = if let Some(torbox) = app.acquisition.get_torbox_client().await {
        torbox.check_cached_torrents(&hashes).await
    } else {
        std::collections::HashMap::new()
    };

    let res = app
        .db
        .call(move |conn| {
            crate::sync::diagnostics::compare_volume_candidates(
                conn,
                &s_id,
                volume,
                &profile,
                Some(&cache_map),
            )
        })
        .await?;

    Ok(Json(res))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TitleManualMappingPayload {
    mangabaka_id: Option<i64>,
    collection_id: Option<String>,
    edition: Option<String>,
}

async fn post_manga_mapping_manual(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<TitleManualMappingPayload>,
) -> Result<Json<crate::sync::reconciliation::ReconciliationSummary>> {
    app.me.admin()?;

    let (title, komga_series_id) = app
        .db
        .call(move |conn| {
            crate::sync::reconciliation::set_manual_series_and_collection(
                conn,
                id,
                payload.mangabaka_id,
                payload.collection_id.as_deref(),
                payload.edition.as_deref(),
            )
        })
        .await?;

    let target = crate::sync::reconciliation::ReconciliationTarget {
        manga_id: Some(id),
        komga_series_id,
        title,
        edition: None,
    };
    let summary = app.sync.run_reconciliation(target).await?;
    Ok(Json(summary))
}

async fn delete_manga_mapping_manual(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::sync::reconciliation::ReconciliationSummary>> {
    app.me.admin()?;

    let (title, komga_series_id) = app
        .db
        .call(move |conn| {
            crate::sync::reconciliation::clear_manual_series_and_collection(conn, id)
        })
        .await?;

    let target = crate::sync::reconciliation::ReconciliationTarget {
        manga_id: Some(id),
        komga_series_id,
        title,
        edition: None,
    };
    let summary = app.sync.run_reconciliation(target).await?;
    Ok(Json(summary))
}

async fn get_catalog_mangabaka_collections(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::pipeline::mangabaka::MangaBakaCollectionInfo>>> {
    let (cfg, mb_token, _) = app.pipeline.load_internal_config().await?;
    let mb_client = crate::pipeline::mangabaka::MangaBakaClient::new(
        app.trackers.http().clone(),
        cfg.mangabaka_url,
        mb_token,
    );
    let collections = mb_client.list_series_collections(id).await?;
    Ok(Json(collections))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TitleMangaBakaCollectionsDto {
    manga_id: i64,
    mangabaka_id: Option<i64>,
    current_collection_id: Option<String>,
    current_edition: Option<String>,
    status: String,
    collections: Vec<crate::pipeline::mangabaka::MangaBakaCollectionInfo>,
}

async fn get_title_mangabaka_collections(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<TitleMangaBakaCollectionsDto>> {
    let (mb_id, current_coll_id, current_ed): (Option<i64>, Option<String>, Option<String>) = app
        .db
        .call(move |conn| {
            let res = conn
                .query_row(
                    "SELECT COALESCE(sr.mangabaka_id, m.mangabaka_id), sr.collection_id, sr.edition
                     FROM manga m
                     LEFT JOIN series_reconciliations sr ON sr.manga_id = m.id
                     WHERE m.id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            Ok(res.unwrap_or((None, None, None)))
        })
        .await?;

    let Some(mangabaka_id) = mb_id else {
        return Ok(Json(TitleMangaBakaCollectionsDto {
            manga_id: id,
            mangabaka_id: None,
            current_collection_id: None,
            current_edition: None,
            status: "no_mangabaka_id".to_string(),
            collections: Vec::new(),
        }));
    };

    let (cfg, mb_token, _) = app.pipeline.load_internal_config().await?;
    let mb_client = crate::pipeline::mangabaka::MangaBakaClient::new(
        app.trackers.http().clone(),
        cfg.mangabaka_url,
        mb_token,
    );
    let collections = mb_client.list_series_collections(mangabaka_id).await?;

    let status = if collections.is_empty() {
        "collection_unavailable".to_string()
    } else if current_coll_id.is_some() {
        "resolved".to_string()
    } else if collections.len() == 1 {
        "single_collection".to_string()
    } else {
        "ambiguous".to_string()
    };

    Ok(Json(TitleMangaBakaCollectionsDto {
        manga_id: id,
        mangabaka_id: Some(mangabaka_id),
        current_collection_id: current_coll_id,
        current_edition: current_ed,
        status,
        collections,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetTitleCollectionPayload {
    collection_id: String,
    edition: Option<String>,
}

async fn post_title_mangabaka_collection(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<SetTitleCollectionPayload>,
) -> Result<Json<crate::sync::reconciliation::ReconciliationSummary>> {
    app.me.admin()?;

    let (title, komga_series_id) = app
        .db
        .call(move |conn| {
            crate::sync::reconciliation::set_manual_series_and_collection(
                conn,
                id,
                None,
                Some(&payload.collection_id),
                payload.edition.as_deref(),
            )
        })
        .await?;

    let target = crate::sync::reconciliation::ReconciliationTarget {
        manga_id: Some(id),
        komga_series_id,
        title,
        edition: None,
    };
    let summary = app.sync.run_reconciliation(target).await?;
    Ok(Json(summary))
}

async fn get_title_opentome_lines(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<crate::opentome::service::TitleOpenTomeLinesResponse>> {
    let lines_resp = app.opentome.get_manga_opentome_lines(id).await?;
    Ok(Json(lines_resp))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetTitleOpenTomeLinePayload {
    gcd_series_id: i64,
}

async fn post_title_opentome_line(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<SetTitleOpenTomeLinePayload>,
) -> Result<Json<Option<crate::opentome::catalog::OpenTomeSeriesSchedule>>> {
    app.me.admin()?;
    let schedule = app
        .opentome
        .set_manga_opentome_line(id, payload.gcd_series_id)
        .await?;
    Ok(Json(schedule))
}

#[derive(Deserialize)]
struct ReleaseCalendarQuery {
    market: Option<String>,
}

async fn get_releases_calendar(
    Me(app): Me,
    Query(query): Query<ReleaseCalendarQuery>,
) -> Result<Json<Vec<crate::sync::reconciliation::ReleaseCalendarEntryDto>>> {
    let items = app
        .db
        .call(move |conn| {
            crate::sync::reconciliation::get_release_calendar(conn, query.market.as_deref())
        })
        .await?;
    Ok(Json(items))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemIntegrityQuery {
    severity: Option<String>,
    category: Option<String>,
    search: Option<String>,
}

async fn get_system_integrity(
    Me(app): Me,
    Query(query): Query<SystemIntegrityQuery>,
) -> Result<Json<crate::sync::integrity::LibraryIntegrityReport>> {
    app.me.admin()?;
    let downloads_dir = app.downloads.dir().to_path_buf();
    let mut report = app
        .db
        .call(move |conn| {
            crate::sync::integrity::audit_library_integrity(conn, Some(&downloads_dir))
        })
        .await?;
    if query.severity.is_some() || query.category.is_some() || query.search.is_some() {
        report.filter(
            query.severity.as_deref(),
            query.category.as_deref(),
            query.search.as_deref(),
        );
    }
    Ok(Json(report))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotificationsHistoryQuery {
    limit: Option<usize>,
    active_only: Option<bool>,
}

async fn get_notifications_history(
    Me(app): Me,
    Query(query): Query<NotificationsHistoryQuery>,
) -> Result<Json<Vec<crate::sync::reconciliation::NotificationEventHistoryDto>>> {
    let limit = query.limit.unwrap_or(50);
    let active_only = query.active_only.unwrap_or(false);
    let events = app
        .db
        .call(move |conn| {
            crate::sync::reconciliation::list_notification_events(conn, limit, active_only)
        })
        .await?;
    Ok(Json(events))
}

async fn post_notification_event_resolve(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<bool>> {
    app.me.admin()?;
    let resolved = app
        .db
        .call(move |conn| crate::sync::reconciliation::resolve_notification_event(conn, id))
        .await?;
    Ok(Json(resolved))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailabilityWatchersQuery {
    manga_id: Option<i64>,
    series_id: Option<String>,
    state: Option<String>,
}

async fn get_availability_watchers(
    Me(app): Me,
    Query(query): Query<AvailabilityWatchersQuery>,
) -> Result<Json<Vec<crate::acquisition::watcher::ReleaseAvailabilityWatcher>>> {
    app.me.admin()?;
    let watchers = app
        .db
        .call(move |conn| {
            let mut sql = "SELECT id, series_id, manga_id, series_title, volume_number, mangabaka_id, collection_id,
                                  work_id, market, edition, release_date, is_released, is_owned, monitoring_enabled,
                                  notify_on_available, state, last_attempt_at, last_checked_at, next_check_at,
                                  check_interval_secs, consecutive_failures, last_provider_checked, last_check_status,
                                  last_error, top_candidate_id, top_candidate_provider, top_candidate_title,
                                  top_candidate_fingerprint, candidate_count, notified_at, notification_event_id,
                                  created_at, updated_at
                           FROM release_availability_watchers
                           WHERE 1=1".to_string();
            let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

            if let Some(mid) = query.manga_id {
                sql.push_str(" AND manga_id = ?");
                params_vec.push(mid.into());
            }
            if let Some(ref sid) = query.series_id {
                sql.push_str(" AND series_id = ?");
                params_vec.push(sid.clone().into());
            }
            if let Some(ref st) = query.state {
                sql.push_str(" AND state = ?");
                params_vec.push(st.clone().into());
            }
            sql.push_str(" ORDER BY manga_id ASC, volume_number ASC");

            let mut stmt = conn.prepare(&sql).map_err(AppError::from)?;
            let params_slice: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(&params_slice[..], |r| {
                let state_str: String = r.get(15)?;
                let state = std::str::FromStr::from_str(&state_str).unwrap_or(crate::acquisition::watcher::WatcherState::ReleasedAwaitingProvider);
                Ok(crate::acquisition::watcher::ReleaseAvailabilityWatcher {
                    id: r.get(0)?,
                    series_id: r.get(1)?,
                    manga_id: r.get(2)?,
                    series_title: r.get(3)?,
                    volume_number: r.get(4)?,
                    mangabaka_id: r.get(5)?,
                    collection_id: r.get(6)?,
                    work_id: r.get(7)?,
                    market: r.get(8)?,
                    edition: r.get(9)?,
                    release_date: r.get(10)?,
                    is_released: r.get::<_, i64>(11)? != 0,
                    is_owned: r.get::<_, i64>(12)? != 0,
                    monitoring_enabled: r.get::<_, i64>(13)? != 0,
                    notify_on_available: r.get::<_, i64>(14)? != 0,
                    state,
                    last_attempt_at: r.get(16)?,
                    last_checked_at: r.get(17)?,
                    next_check_at: r.get(18)?,
                    check_interval_secs: r.get(19)?,
                    consecutive_failures: r.get(20)?,
                    last_provider_checked: r.get(21)?,
                    last_check_status: r.get(22)?,
                    last_error: r.get(23)?,
                    top_candidate_id: r.get(24)?,
                    top_candidate_provider: r.get(25)?,
                    top_candidate_title: r.get(26)?,
                    top_candidate_fingerprint: r.get(27)?,
                    candidate_count: r.get(28)?,
                    notified_at: r.get(29)?,
                    notification_event_id: r.get(30)?,
                    created_at: r.get(31)?,
                    updated_at: r.get(32)?,
                })
            }).map_err(AppError::from)?;

            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_err(AppError::from)?);
            }
            Ok(list)
        })
        .await?;
    Ok(Json(watchers))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TitleAvailabilityResponse {
    settings: crate::acquisition::watcher::TitleMonitoringSettings,
    watchers: Vec<crate::acquisition::watcher::ReleaseAvailabilityWatcher>,
}

async fn get_title_availability(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<TitleAvailabilityResponse>> {
    let settings = app
        .db
        .call(move |conn| crate::acquisition::watcher::get_title_monitoring_settings(conn, id))
        .await?;

    let series_id = format!("manga:{id}");
    let watchers = app
        .db
        .call(move |conn| crate::acquisition::watcher::list_watchers_for_series(conn, &series_id))
        .await?;

    Ok(Json(TitleAvailabilityResponse { settings, watchers }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchTitleAvailabilityRequest {
    monitoring_enabled: Option<bool>,
    notify_enabled: Option<bool>,
}

async fn patch_title_availability(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<PatchTitleAvailabilityRequest>,
) -> Result<Json<crate::acquisition::watcher::TitleMonitoringSettings>> {
    app.me.admin()?;
    let settings = app
        .db
        .call(move |conn| {
            let current = crate::acquisition::watcher::get_title_monitoring_settings(conn, id)?;
            let new_monitoring = payload.monitoring_enabled.unwrap_or(current.release_monitoring_enabled);
            let new_notify = payload.notify_enabled.unwrap_or(current.release_notify_enabled);
            crate::acquisition::watcher::set_title_monitoring_settings(conn, id, new_monitoring, new_notify)
        })
        .await?;
    Ok(Json(settings))
}

async fn post_title_availability_refresh(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::acquisition::watcher::ReleaseAvailabilityWatcher>>> {
    app.me.admin()?;
    let http = reqwest::Client::new();
    let res = crate::acquisition::watcher::refresh_title_watchers(&app.db, http, id).await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchersDashboardQuery {
    state: Option<String>,
    query: Option<String>,
}

async fn get_availability_watchers_dashboard(
    Me(app): Me,
    Query(query): Query<WatchersDashboardQuery>,
) -> Result<Json<crate::acquisition::watcher::WatchersDashboardResponse>> {
    app.me.admin()?;
    let resp = app
        .db
        .call(move |conn| {
            crate::acquisition::watcher::list_all_watchers(
                conn,
                query.state.as_deref(),
                query.query.as_deref(),
            )
        })
        .await?;
    Ok(Json(resp))
}

async fn get_availability_watchers_summary(
    Me(app): Me,
) -> Result<Json<crate::acquisition::watcher::WatchersSummary>> {
    app.me.admin()?;
    let resp = app
        .db
        .call(move |conn| {
            let dash = crate::acquisition::watcher::list_all_watchers(conn, None, None)?;
            Ok(dash.summary)
        })
        .await?;
    Ok(Json(resp))
}

async fn post_availability_watchers_bulk_action(
    Me(app): Me,
    Json(payload): Json<crate::acquisition::watcher::WatchersBulkActionRequest>,
) -> Result<Json<crate::acquisition::watcher::WatchersBulkActionResponse>> {
    app.me.admin()?;
    let resp = app
        .db
        .call(move |conn| {
            crate::acquisition::watcher::execute_watchers_bulk_action(
                conn,
                &payload.action,
                payload.watcher_ids.as_deref(),
            )
        })
        .await?;
    Ok(Json(resp))
}

async fn post_availability_watcher_refresh(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Option<crate::acquisition::watcher::ReleaseAvailabilityWatcher>>> {
    app.me.admin()?;
    let http = reqwest::Client::new();
    let res = crate::acquisition::watcher::refresh_watcher_by_id(&app.db, http, id).await?;
    Ok(Json(res))
}

#[derive(Deserialize)]
struct ListReceiptsQuery {
    limit: Option<usize>,
}

async fn get_library_receipts(
    Me(app): Me,
    Query(query): Query<ListReceiptsQuery>,
) -> Result<Json<Vec<crate::pipeline::receipts::PublicationReceipt>>> {
    app.me.admin()?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let receipts = app
        .db
        .call(move |conn| crate::pipeline::receipts::list_publication_receipts(conn, limit))
        .await?;
    Ok(Json(receipts))
}

async fn get_title_receipts(
    Me(app): Me,
    Path(id): Path<i64>,
) -> Result<Json<Vec<crate::pipeline::receipts::PublicationReceipt>>> {
    app.me.admin()?;
    let receipts = app
        .db
        .call(move |conn| crate::pipeline::receipts::list_receipts_for_manga(conn, id))
        .await?;
    Ok(Json(receipts))
}

async fn post_receipt_verify(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<crate::pipeline::receipts::PublicationReceiptVerificationResult>> {
    app.me.admin()?;
    let result = app
        .db
        .call(move |conn| crate::pipeline::receipts::verify_publication_receipt_on_disk(conn, &id))
        .await?;
    Ok(Json(result))
}

async fn post_receipt_rescan(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    let receipt_id = id.clone();
    let receipt = app
        .db
        .call(move |conn| crate::pipeline::receipts::get_publication_receipt(conn, &receipt_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Receipt {id} not found")))?;

    let (cfg, _, komga_key) = app.pipeline.load_internal_config().await?;

    if !cfg.komga_enabled || !cfg.komga_scan_enabled {
        return Err(AppError::bad_request("Komga scan is disabled in configuration"));
    }

    let komga_client = crate::pipeline::komga::KomgaClient::new(app.pipeline.http.clone(), cfg.komga_url.clone(), komga_key);
    let target_lib = receipt.komga_library_id.as_deref().or(cfg.komga_library_id.as_deref());

    komga_client.trigger_scan(target_lib).await.map_err(|e| {
        AppError::internal(format!("Failed to trigger Komga scan: {}", e.message))
    })?;

    let canonical = receipt.canonical_path.clone();
    let lib_str = target_lib.map(|s| s.to_string());
    app.db.call(move |conn| {
        crate::pipeline::receipts::update_receipt_komga_status(conn, &canonical, true, lib_str.as_deref())
    }).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "receiptId": receipt.id,
        "canonicalPath": receipt.canonical_path,
        "komgaLibraryId": target_lib,
    })))
}

async fn post_receipt_retry_metadata(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    app.me.admin()?;
    let receipt_id = id.clone();
    let receipt = app
        .db
        .call(move |conn| crate::pipeline::receipts::get_publication_receipt(conn, &receipt_id))
        .await?
        .ok_or_else(|| AppError::not_found(format!("Receipt {id} not found")))?;

    let path = std::path::PathBuf::from(&receipt.canonical_path);

    let res = app
        .pipeline
        .repair_canonical_cbz(
            &path,
            Some(&receipt.series_title),
            receipt.volume_number,
            None,
            None,
        )
        .await;

    let canonical = receipt.canonical_path.clone();
    match res {
        Ok(outcome) => {
            app.db.call(move |conn| {
                crate::pipeline::receipts::update_receipt_metadata_status(conn, &canonical, "complete", None)
            }).await?;
            Ok(Json(serde_json::json!({
                "success": true,
                "receiptId": receipt.id,
                "repaired": true,
                "outcome": outcome,
            })))
        }
        Err(e) => {
            let err_msg = e.message.clone();
            app.db.call(move |conn| {
                crate::pipeline::receipts::update_receipt_metadata_status(conn, &canonical, "failed", Some(&err_msg))
            }).await?;
            Err(e)
        }
    }
}

async fn recover_download(Me(app): Me, Path(chapter): Path<i64>) -> Result<Json<Value>> {
    app.me.admin()?;
    // Check if canonical publication receipt or published file already exists
    let receipt = app
        .db
        .call(move |conn| crate::pipeline::receipts::get_publication_receipt_by_chapter(conn, chapter))
        .await?;

    if let Some(r) = receipt {
        let path = std::path::PathBuf::from(&r.canonical_path);
        if path.is_file() && crate::pipeline::publisher::verify_cbz_integrity(&path).is_ok() {
            let p_str = r.canonical_path.clone();
            app.db.call(move |conn| {
                conn.execute(
                    "UPDATE downloads SET published_path = ?2, state = 'verifying', error = NULL WHERE chapter_id = ?1",
                    rusqlite::params![chapter, p_str],
                )?;
                crate::acquisition::service::persist_publication_provenance(conn, chapter)?;
                Ok(())
            }).await?;
            app.downloads.wake();
            return Ok(Json(serde_json::json!({
                "recovered": true,
                "strategy": "existing_publication_receipt",
                "canonicalPath": r.canonical_path,
                "state": "verifying",
            })));
        }
    }

    // Otherwise standard retry
    app.downloads.retry(chapter).await?;
    Ok(Json(serde_json::json!({
        "recovered": true,
        "strategy": "retry_download",
        "chapterId": chapter,
    })))
}

async fn get_download_recovery_info(
    Me(app): Me,
    Path(chapter): Path<i64>,
) -> Result<Json<crate::pipeline::recovery::DownloadRecoveryInfo>> {
    app.me.admin()?;
    let info = app
        .db
        .call(move |conn| crate::pipeline::recovery::analyze_download_recovery(conn, chapter))
        .await?;
    Ok(Json(info))
}

async fn get_downloads_recovery_summary(
    Me(app): Me,
) -> Result<Json<crate::pipeline::recovery::DownloadRecoverySummary>> {
    app.me.admin()?;
    let summary = app
        .db
        .call(move |conn| crate::pipeline::recovery::list_all_recovery_info(conn))
        .await?;
    Ok(Json(summary))
}

// -----------------------------------------------------------------------------
// Historical Backlog Acquisition (M6.10)
// -----------------------------------------------------------------------------

async fn post_manga_backlog_plan(
    Me(app): Me,
    Path(id): Path<i64>,
    Json(payload): Json<crate::acquisition::backlog::PlanBacklogRequest>,
) -> Result<Json<crate::acquisition::backlog::BacklogJobDto>> {
    let job = app.acquisition.plan_backlog_acquisition(id, payload).await?;
    Ok(Json(job))
}

async fn post_manga_backlog_execute(
    Me(app): Me,
    Path(_id): Path<i64>,
    Json(payload): Json<crate::acquisition::backlog::ExecuteBacklogRequest>,
) -> Result<Json<crate::acquisition::backlog::ExecuteBacklogResponse>> {
    let job_id = payload
        .job_id
        .clone()
        .ok_or_else(|| AppError::bad_request("jobId is required in execution payload"))?;
    let res = app.acquisition.execute_backlog_job(&job_id, payload).await?;
    Ok(Json(res))
}

async fn post_backlog_job_execute_by_id(
    Me(app): Me,
    Path(id): Path<String>,
    Json(mut payload): Json<crate::acquisition::backlog::ExecuteBacklogRequest>,
) -> Result<Json<crate::acquisition::backlog::ExecuteBacklogResponse>> {
    payload.job_id = Some(id.clone());
    let res = app.acquisition.execute_backlog_job(&id, payload).await?;
    Ok(Json(res))
}

async fn get_manga_backlog_jobs(
    Me(app): Me,
    Path(id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<crate::acquisition::backlog::BacklogJobDto>>> {
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(20);
    let jobs = app.acquisition.get_manga_backlog_jobs(id, limit).await?;
    Ok(Json(jobs))
}

async fn get_all_backlog_jobs(
    Me(app): Me,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<crate::acquisition::backlog::BacklogJobDto>>> {
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50);
    let jobs = app.acquisition.get_all_backlog_jobs(limit).await?;
    Ok(Json(jobs))
}

async fn post_backlog_job_cancel(
    Me(app): Me,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let cancelled = app.acquisition.cancel_backlog_job(&id).await?;
    Ok(Json(serde_json::json!({ "cancelled": cancelled })))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionOperationsSummary {
    pub queue_total: usize,
    pub queue_paused: bool,
    pub backlog_jobs_total: usize,
    pub backlog_jobs_pending: usize,
    pub backlog_jobs_running: usize,
    pub receipts_total: usize,
    pub receipts_verified: usize,
    pub receipts_unverified: usize,
    pub auto_acquire_enabled: bool,
    pub auto_acquire_dry_run: bool,
    pub auto_acquire_opted_in_count: usize,
    pub free_disk_bytes: u64,
}

async fn get_acquisition_operations_summary(
    Me(app): Me,
) -> Result<Json<AcquisitionOperationsSummary>> {
    let q_view = app.downloads.list().await?;
    let queue_total = q_view.queue.len();
    let queue_paused = q_view.paused;

    let (b_total, b_pending, b_running, r_total, r_verified, r_unverified, auto_en, auto_dry, opt_count) = app
        .db
        .call(|conn| {
            let b_total: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM backlog_acquisition_jobs",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let b_pending: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM backlog_acquisition_jobs WHERE status = 'planned'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let b_running: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM backlog_acquisition_jobs WHERE status = 'executing'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);

            let r_total: usize = conn
                .query_row("SELECT COUNT(*) FROM publication_receipts", [], |r| {
                    r.get(0)
                })
                .unwrap_or(0);
            let r_verified: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM publication_receipts WHERE verified_ownership = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let r_unverified = r_total.saturating_sub(r_verified);

            let auto_en: bool = crate::settings::get::<bool>(conn, "acquisition.auto_acquire.enabled")
                .unwrap_or(false);
            let auto_dry: bool = crate::settings::get::<bool>(conn, "acquisition.auto_acquire.dry_run")
                .unwrap_or(true);
            let opt_count: usize = conn
                .query_row("SELECT COUNT(*) FROM manga WHERE auto_acquire = 1", [], |r| {
                    r.get(0)
                })
                .unwrap_or(0);

            Ok((
                b_total,
                b_pending,
                b_running,
                r_total,
                r_verified,
                r_unverified,
                auto_en,
                auto_dry,
                opt_count,
            ))
        })
        .await?;

    let free_disk_bytes = crate::downloads::free_space(app.downloads.dir()).unwrap_or(0);

    Ok(Json(AcquisitionOperationsSummary {
        queue_total,
        queue_paused,
        backlog_jobs_total: b_total,
        backlog_jobs_pending: b_pending,
        backlog_jobs_running: b_running,
        receipts_total: r_total,
        receipts_verified: r_verified,
        receipts_unverified: r_unverified,
        auto_acquire_enabled: auto_en,
        auto_acquire_dry_run: auto_dry,
        auto_acquire_opted_in_count: opt_count,
        free_disk_bytes,
    }))
}

async fn post_naming_audit(
    Me(app): Me,
    Json(req): Json<crate::pipeline::naming_audit::NamingAuditRequest>,
) -> Result<Json<crate::pipeline::naming_audit::NamingAuditReport>> {
    app.me.admin()?;
    let (cfg, _, komga_key) = app.pipeline.load_internal_config().await?;
    if !cfg.komga_enabled {
        return Err(AppError::bad_request("Komga integration is disabled"));
    }
    let komga_client = crate::pipeline::komga::KomgaClient::new(
        app.pipeline.http.clone(),
        cfg.komga_url.clone(),
        komga_key,
    );
    let target_lib = req.library_id.as_deref().or(cfg.komga_library_id.as_deref());
    let library_id = target_lib.ok_or_else(|| AppError::bad_request("Komga library ID is not configured"))?;

    let report = crate::pipeline::naming_audit::execute_naming_audit(
        &komga_client,
        library_id,
        req.series_id.as_deref(),
    ).await?;

    let report_clone = report.clone();
    app.db.call(move |conn| {
        crate::pipeline::naming_audit::save_naming_audit(conn, &report_clone)
    }).await?;

    Ok(Json(report))
}

async fn get_naming_audit_latest(
    Me(app): Me,
) -> Result<Json<Option<crate::pipeline::naming_audit::NamingAuditReport>>> {
    app.me.admin()?;
    let report = app.db.call(|conn| {
        crate::pipeline::naming_audit::get_latest_naming_audit(conn)
    }).await?;
    Ok(Json(report))
}

async fn post_naming_apply(
    Me(app): Me,
    Json(req): Json<crate::pipeline::naming_audit::NamingApplyRequest>,
) -> Result<Json<crate::pipeline::naming_audit::NamingCorrectionResult>> {
    app.me.admin()?;
    let (cfg, _, komga_key) = app.pipeline.load_internal_config().await?;
    if !cfg.komga_enabled {
        return Err(AppError::bad_request("Komga integration is disabled"));
    }
    let komga_client = crate::pipeline::komga::KomgaClient::new(
        app.pipeline.http.clone(),
        cfg.komga_url.clone(),
        komga_key,
    );

    let audit_id = req.audit_id.clone();
    let report = app.db.call(move |conn| {
        crate::pipeline::naming_audit::get_naming_audit_by_id(conn, &audit_id)
    }).await?.ok_or_else(|| AppError::not_found("Audit report not found"))?;

    let backup_id = {
        let mut random = [0u8; 16];
        let _ = getrandom::getrandom(&mut random);
        format!("batch_{}", hex::encode(random))
    };

    let filter_set: Option<std::collections::HashSet<String>> = req.book_ids.map(|ids| ids.into_iter().collect());

    let items_to_process: Vec<crate::pipeline::naming_audit::NamingAuditItem> = report.items.into_iter()
        .filter(|item| {
            if item.is_already_canonical {
                return false;
            }
            if let Some(ref set) = filter_set {
                set.contains(&item.book_id)
            } else {
                item.is_safe
            }
        })
        .collect();

    let total_requested = items_to_process.len();
    if req.dry_run {
        return Ok(Json(crate::pipeline::naming_audit::NamingCorrectionResult {
            backup_id,
            is_dry_run: true,
            total_requested,
            applied_count: 0,
            verified_count: 0,
            failed_count: 0,
            errors: Vec::new(),
        }));
    }

    let mut applied_count = 0;
    let mut verified_count = 0;
    let mut failed_count = 0;
    let mut errors = Vec::new();

    for item in items_to_process {
        // 1. Fetch current book from Komga for reliable backup
        let current_book = match komga_client.get_book(&item.book_id).await {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("Failed to fetch book {}: {}", item.book_id, e.message));
                failed_count += 1;
                continue;
            }
        };

        let orig_meta = current_book.metadata;
        let orig_title = orig_meta.as_ref().and_then(|m| m.title.clone());
        let orig_number = orig_meta.as_ref().and_then(|m| m.number.clone());
        let orig_number_sort = orig_meta.as_ref().and_then(|m| m.number_sort);

        // 2. Insert durable backup record before making any changes
        let b_id = backup_id.clone();
        let it = item.clone();
        let ot = orig_title.clone();
        let on = orig_number.clone();
        let ons = orig_number_sort;
        let cbz_up = req.update_cbz;

        if let Err(e) = app.db.call(move |conn| {
            crate::pipeline::naming_audit::record_backup_entry(
                conn,
                &b_id,
                &it.book_id,
                &it.series_id,
                &it.series_title,
                ot.as_deref(),
                on.as_deref(),
                ons,
                &it.proposed_title,
                &it.proposed_number,
                it.proposed_number_sort,
                cbz_up,
            )
        }).await {
            errors.push(format!("Failed to store backup for book {}: {e}", item.book_id));
            failed_count += 1;
            continue;
        }

        // 3. Apply PATCH to Komga
        let patch = crate::pipeline::komga::KomgaBookMetadataUpdate {
            title: Some(item.proposed_title.clone()),
            title_lock: Some(true),
            number: Some(item.proposed_number.clone()),
            number_lock: Some(true),
            number_sort: Some(item.proposed_number_sort),
            number_sort_lock: Some(true),
        };

        if let Err(e) = komga_client.update_book_metadata(&item.book_id, &patch).await {
            errors.push(format!("Komga PATCH failed for book {}: {}", item.book_id, e.message));
            failed_count += 1;
            let b_id = backup_id.clone();
            let b_book_id = item.book_id.clone();
            let _ = app.db.call(move |conn| {
                crate::pipeline::naming_audit::update_backup_status(conn, &b_id, &b_book_id, "failed")
            }).await;
            continue;
        }

        applied_count += 1;

        // 4. Verify the update in Komga
        match komga_client.get_book(&item.book_id).await {
            Ok(verified_book) => {
                let matches_title = verified_book.metadata.as_ref()
                    .and_then(|m| m.title.as_deref()) == Some(&item.proposed_title);
                if matches_title {
                    verified_count += 1;
                    let b_id = backup_id.clone();
                    let b_book_id = item.book_id.clone();
                    let _ = app.db.call(move |conn| {
                        crate::pipeline::naming_audit::update_backup_status(conn, &b_id, &b_book_id, "verified")
                    }).await;
                } else {
                    errors.push(format!("Post-verification title mismatch for book {}", item.book_id));
                }
            }
            Err(e) => {
                errors.push(format!("Post-verification get failed for book {}: {}", item.book_id, e.message));
            }
        }
    }

    Ok(Json(crate::pipeline::naming_audit::NamingCorrectionResult {
        backup_id,
        is_dry_run: false,
        total_requested,
        applied_count,
        verified_count,
        failed_count,
        errors,
    }))
}

async fn post_naming_rollback(
    Me(app): Me,
    Json(req): Json<crate::pipeline::naming_audit::NamingRollbackRequest>,
) -> Result<Json<crate::pipeline::naming_audit::NamingRollbackResult>> {
    app.me.admin()?;
    let (cfg, _, komga_key) = app.pipeline.load_internal_config().await?;
    if !cfg.komga_enabled {
        return Err(AppError::bad_request("Komga integration is disabled"));
    }
    let komga_client = crate::pipeline::komga::KomgaClient::new(
        app.pipeline.http.clone(),
        cfg.komga_url.clone(),
        komga_key,
    );

    let backup_id = req.backup_id.clone();
    let filter = req.book_ids.clone();
    let rows = app.db.call(move |conn| {
        crate::pipeline::naming_audit::get_backups_for_rollback(conn, &backup_id, filter.as_deref())
    }).await?;

    let total_requested = rows.len();
    let mut rolled_back_count = 0;
    let mut failed_count = 0;
    let mut errors = Vec::new();

    for row in rows {
        let patch = crate::pipeline::komga::KomgaBookMetadataUpdate {
            title: row.original_title.clone(),
            title_lock: Some(false),
            number: row.original_number.clone(),
            number_lock: Some(false),
            number_sort: row.original_number_sort,
            number_sort_lock: Some(false),
        };

        if let Err(e) = komga_client.update_book_metadata(&row.book_id, &patch).await {
            errors.push(format!("Rollback failed for book {}: {}", row.book_id, e.message));
            failed_count += 1;
            continue;
        }

        let row_id = row.id;
        let _ = app.db.call(move |conn| {
            crate::pipeline::naming_audit::mark_backup_rolled_back(conn, row_id)
        }).await;

        rolled_back_count += 1;
    }

    Ok(Json(crate::pipeline::naming_audit::NamingRollbackResult {
        backup_id: req.backup_id,
        total_requested,
        rolled_back_count,
        failed_count,
        errors,
    }))
}

async fn get_naming_backups(
    Me(app): Me,
) -> Result<Json<Vec<crate::pipeline::naming_audit::NamingCorrectionBackup>>> {
    app.me.admin()?;
    let backups = app.db.call(|conn| {
        crate::pipeline::naming_audit::list_naming_backups(conn)
    }).await?;
    Ok(Json(backups))
}


