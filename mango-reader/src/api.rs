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
        .route("/manga/{id}/reconcile", post(post_manga_reconcile))
        .route("/reconciliation/followed", get(get_followed_reconciliations))
        .route("/manga/{id}/volumes/{volume}/candidates", get(get_manga_volume_candidates))
        .route("/manga/{id}/volumes/{volume}/discover", post(post_manga_volume_discover))
        .route("/manga/{id}/discover-missing", post(post_manga_discover_missing))
        .route("/manga/{id}/discovery", get(get_manga_series_discovery))
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
        .route(
            "/imports/payload/{id}/cleanup",
            post(cleanup_import_payload),
        )
        .route("/imports/payload/{id}/retry", post(retry_import_inspection))
        .route("/imports", get(get_import_candidates))
        .route("/imports/{id}/assignment", post(assign_import_candidate))
        .route("/acquisition/sources", get(get_acquisition_sources))
        .route("/acquisition/plan-by-title", post(post_plan_by_title))
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
            description: "Digital manga provider (read-only archive index)".to_string(),
            is_default: false,
            available: false,
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
    let dry_run = payload.and_then(|p| p.dry_run).unwrap_or(false);
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
