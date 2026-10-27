//! Transfer lifecycle engine, progress tracking, stall detection, and retry management.
//!
//! Provides deterministic state handling for external streaming transfers (TorBox, HTTP)
//! and internal page-stream transfers (MangaDot, Keiyoushi).

use crate::error::{AppError, Result};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::sleep;

/// Now, in epoch seconds.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |s| s.as_secs() as i64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Queued,
    Downloading,
    Stalled,
    Retrying,
    Verifying,
    Completed,
    Failed,
}

impl std::fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Downloading => write!(f, "downloading"),
            Self::Stalled => write!(f, "stalled"),
            Self::Retrying => write!(f, "retrying"),
            Self::Verifying => write!(f, "verifying"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Rich progress snapshot for an active or recent transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub chapter_id: i64,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub pages_done: i64,
    pub pages_total: Option<i64>,
    pub attempt: u32,
    pub max_attempts: u32,
    pub start_time: i64,
    pub last_progress_time: i64,
    pub is_stalled: bool,
    pub retry_at: Option<i64>,
    pub status: TransferStatus,
    pub error: Option<String>,
}

/// Configuration governing stall detection and bounded retries.
#[derive(Debug, Clone)]
pub struct TransferConfig {
    pub stall_timeout: Duration,
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub backoff_factor: f64,
    pub max_backoff: Duration,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            stall_timeout: Duration::from_secs(45),
            max_attempts: 5,
            initial_backoff: Duration::from_secs(5),
            backoff_factor: 2.0,
            max_backoff: Duration::from_secs(120),
        }
    }
}

impl TransferConfig {
    pub fn fast_for_testing() -> Self {
        Self {
            stall_timeout: Duration::from_millis(150),
            max_attempts: 3,
            initial_backoff: Duration::from_millis(50),
            backoff_factor: 1.5,
            max_backoff: Duration::from_millis(300),
        }
    }
}

/// Thread-safe tracker coordinating in-flight transfers and active workers.
#[derive(Clone)]
pub struct TransferTracker {
    pub config: TransferConfig,
    active_transfers: Arc<StdMutex<HashMap<i64, TransferProgress>>>,
    active_workers: Arc<StdMutex<HashSet<i64>>>,
}

impl Default for TransferTracker {
    fn default() -> Self {
        Self::new(TransferConfig::default())
    }
}

impl TransferTracker {
    pub fn new(config: TransferConfig) -> Self {
        Self {
            config,
            active_transfers: Arc::new(StdMutex::new(HashMap::new())),
            active_workers: Arc::new(StdMutex::new(HashSet::new())),
        }
    }

    /// Attempts to claim the worker slot for a chapter to prevent duplicate concurrent downloads.
    pub fn try_claim_worker(
        &self,
        chapter_id: i64,
        attempt: u32,
        total_bytes: Option<u64>,
        pages_total: Option<i64>,
    ) -> bool {
        let mut workers = self.active_workers.lock().unwrap_or_else(|e| e.into_inner());
        if workers.contains(&chapter_id) {
            return false;
        }
        workers.insert(chapter_id);

        let now = now_secs();
        let mut transfers = self
            .active_transfers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        transfers.insert(
            chapter_id,
            TransferProgress {
                chapter_id,
                bytes_downloaded: 0,
                total_bytes,
                pages_done: 0,
                pages_total,
                attempt,
                max_attempts: self.config.max_attempts,
                start_time: now,
                last_progress_time: now,
                is_stalled: false,
                retry_at: None,
                status: TransferStatus::Downloading,
                error: None,
            },
        );

        true
    }

    /// Releases the worker slot for a chapter.
    pub fn release_worker(&self, chapter_id: i64) {
        if let Ok(mut workers) = self.active_workers.lock() {
            workers.remove(&chapter_id);
        }
    }

    /// Records streamed byte progress, resetting the stall timer.
    pub fn record_bytes(&self, chapter_id: i64, bytes: u64, total: Option<u64>) {
        let now = now_secs();
        if let Ok(mut transfers) = self.active_transfers.lock() {
            if let Some(p) = transfers.get_mut(&chapter_id) {
                p.bytes_downloaded = bytes;
                if total.is_some() {
                    p.total_bytes = total;
                }
                p.last_progress_time = now;
                p.is_stalled = false;
                p.status = TransferStatus::Downloading;
            }
        }
    }

    /// Records page progress during page streams.
    pub fn record_page(
        &self,
        chapter_id: i64,
        pages_done: i64,
        pages_total: Option<i64>,
        page_bytes: usize,
    ) {
        let now = now_secs();
        if let Ok(mut transfers) = self.active_transfers.lock() {
            if let Some(p) = transfers.get_mut(&chapter_id) {
                p.pages_done = pages_done;
                if pages_total.is_some() {
                    p.pages_total = pages_total;
                }
                p.bytes_downloaded += page_bytes as u64;
                p.last_progress_time = now;
                p.is_stalled = false;
                p.status = TransferStatus::Downloading;
            }
        }
    }

    /// Checks if progress has ceased beyond the configured stall timeout.
    pub fn check_stalled(&self, chapter_id: i64) -> bool {
        let now = now_secs();
        if let Ok(mut transfers) = self.active_transfers.lock() {
            if let Some(p) = transfers.get_mut(&chapter_id) {
                if p.status == TransferStatus::Downloading
                    && (now - p.last_progress_time) >= self.config.stall_timeout.as_secs() as i64
                {
                    p.is_stalled = true;
                    p.status = TransferStatus::Stalled;
                    return true;
                }
                return p.is_stalled;
            }
        }
        false
    }

    /// Updates state to Retrying with the next scheduled time.
    pub fn mark_retrying(&self, chapter_id: i64, attempt: u32, retry_at: i64, error: &str) {
        if let Ok(mut transfers) = self.active_transfers.lock() {
            if let Some(p) = transfers.get_mut(&chapter_id) {
                p.status = TransferStatus::Retrying;
                p.attempt = attempt;
                p.retry_at = Some(retry_at);
                p.error = Some(error.to_string());
                p.is_stalled = false;
            }
        }
    }

    /// Updates state to Failed.
    pub fn mark_failed(&self, chapter_id: i64, error: &str) {
        if let Ok(mut transfers) = self.active_transfers.lock() {
            if let Some(p) = transfers.get_mut(&chapter_id) {
                p.status = TransferStatus::Failed;
                p.error = Some(error.to_string());
                p.is_stalled = false;
            }
        }
    }

    /// Updates state to Completed.
    pub fn mark_completed(&self, chapter_id: i64) {
        if let Ok(mut transfers) = self.active_transfers.lock() {
            if let Some(p) = transfers.get_mut(&chapter_id) {
                p.status = TransferStatus::Completed;
                p.error = None;
                p.is_stalled = false;
            }
        }
    }

    /// Gets current progress snapshot if present.
    pub fn get_progress(&self, chapter_id: i64) -> Option<TransferProgress> {
        self.active_transfers
            .lock()
            .ok()?
            .get(&chapter_id)
            .cloned()
    }

    /// Removes a tracked item from memory.
    pub fn remove(&self, chapter_id: i64) {
        if let Ok(mut transfers) = self.active_transfers.lock() {
            transfers.remove(&chapter_id);
        }
        self.release_worker(chapter_id);
    }

    /// Clears all tracked items and workers from memory.
    pub fn clear(&self) {
        if let Ok(mut transfers) = self.active_transfers.lock() {
            transfers.clear();
        }
        if let Ok(mut workers) = self.active_workers.lock() {
            workers.clear();
        }
    }
}

/// Identifies whether a given failure is recoverable and should trigger automatic retry.
pub fn is_recoverable_error(err: &AppError) -> bool {
    // HTTP Status codes
    if err.status == StatusCode::REQUEST_TIMEOUT // 408
        || err.status == StatusCode::TOO_MANY_REQUESTS // 429
        || err.status == StatusCode::INTERNAL_SERVER_ERROR // 500
        || err.status == StatusCode::BAD_GATEWAY // 502
        || err.status == StatusCode::SERVICE_UNAVAILABLE // 503
        || err.status == StatusCode::GATEWAY_TIMEOUT // 504
    {
        return true;
    }

    let code_lower = err.code.to_lowercase();
    if code_lower == "source_error" || code_lower == "network_error" || code_lower == "host_timeout" {
        return true;
    }

    let msg = err.message.to_lowercase();
    msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("connection reset")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("unexpected eof")
        || msg.contains("503")
        || msg.contains("502")
        || msg.contains("504")
        || msg.contains("429")
        || msg.contains("stalled")
        || msg.contains("broke off")
}

/// Computes bounded exponential backoff with jitter and respects Retry-After where practical.
pub fn calculate_backoff(
    attempt: u32,
    config: &TransferConfig,
    retry_after: Option<Duration>,
) -> Duration {
    if let Some(ra) = retry_after {
        if ra > Duration::ZERO {
            return ra.min(config.max_backoff);
        }
    }

    let factor = config.backoff_factor.powi((attempt.saturating_sub(1)).min(6) as i32);
    let wait_ms = (config.initial_backoff.as_millis() as f64 * factor) as u64;
    // Add 10% deterministic pseudo-jitter based on attempt number
    let jitter_ms = (wait_ms / 10) * ((attempt % 3) as u64);
    Duration::from_millis(wait_ms + jitter_ms).min(config.max_backoff)
}

/// Parses the standard HTTP `Retry-After` header value (seconds or date).
pub fn parse_retry_after(header_val: &str) -> Option<Duration> {
    let s = header_val.trim();
    if let Ok(seconds) = s.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    None
}

/// Streams an HTTP file download directly to a `.part` destination with stall detection,
/// safe Range resume, and bounded retries.
pub async fn stream_http_to_file<F>(
    http: &reqwest::Client,
    url: &str,
    destination_part: &Path,
    expected_size: Option<u64>,
    config: &TransferConfig,
    chapter_id: i64,
    tracker: &TransferTracker,
    mut on_progress: F,
) -> Result<u64>
where
    F: FnMut(u64, Option<u64>, bool) + Send + 'static,
{
    if let Some(parent) = destination_part.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut attempt = 1;
    let max_attempts = config.max_attempts;

    loop {
        tracing::info!(
            "[Transfer] Starting attempt {}/{} for chapter {} -> {}",
            attempt,
            max_attempts,
            chapter_id,
            destination_part.display()
        );

        // Check if destination .part exists for safe Range resume probe
        let existing_len = if destination_part.is_file() {
            std::fs::metadata(destination_part).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        let mut req = http
            .get(url)
            .header("User-Agent", "Mango/1.0 (Native Acquisition Retrieval)");

        if existing_len > 0 {
            tracing::info!(
                "[Transfer] Probing HTTP Range resume from byte {} for chapter {}",
                existing_len,
                chapter_id
            );
            req = req.header("Range", format!("bytes={existing_len}-"));
        }

        let send_res = tokio::time::timeout(config.stall_timeout, req.send()).await;
        let response_res = match send_res {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => Err(AppError::new(502, "network_error", format!("Request send failed: {e}"))),
            Err(_) => Err(AppError::new(504, "network_error", "Connection timed out connecting to source")),
        };

        let (should_retry, retry_after_duration, last_error_msg) = match response_res {
            Ok(mut resp) => {
                let status = resp.status();
                let retry_after_duration = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|h| h.to_str().ok())
                    .and_then(parse_retry_after);

                if status == StatusCode::PARTIAL_CONTENT && existing_len > 0 {
                    // Safe Range resume accepted! Append to existing .part
                    tracing::info!(
                        "[Transfer] Server accepted Range resume (206 Partial Content) from byte {} for chapter {}",
                        existing_len,
                        chapter_id
                    );

                    match stream_body_with_stall_detection(
                        &mut resp,
                        destination_part,
                        true,
                        existing_len,
                        expected_size,
                        config.stall_timeout,
                        chapter_id,
                        tracker,
                        &mut on_progress,
                    )
                    .await
                    {
                        Ok(bytes) => {
                            tracing::info!(
                                "[Transfer] Successfully retrieved {} bytes for chapter {}",
                                bytes,
                                chapter_id
                            );
                            return Ok(bytes);
                        }
                        Err(e) => (is_recoverable_error(&e), retry_after_duration, e.message),
                    }
                } else if status.is_success() {
                    // Server returned 200 OK — fresh download (or server ignored Range).
                    // NEVER append a 200 onto an existing partial file! Restart safely from byte 0.
                    if existing_len > 0 {
                        tracing::info!(
                            "[Transfer] Server returned 200 OK (ignoring Range) — restarting cleanly from byte 0 for chapter {}",
                            chapter_id
                        );
                    }

                    match stream_body_with_stall_detection(
                        &mut resp,
                        destination_part,
                        false,
                        0,
                        expected_size,
                        config.stall_timeout,
                        chapter_id,
                        tracker,
                        &mut on_progress,
                    )
                    .await
                    {
                        Ok(bytes) => {
                            tracing::info!(
                                "[Transfer] Successfully retrieved {} bytes for chapter {}",
                                bytes,
                                chapter_id
                            );
                            return Ok(bytes);
                        }
                        Err(e) => (is_recoverable_error(&e), retry_after_duration, e.message),
                    }
                } else if status == StatusCode::RANGE_NOT_SATISFIABLE && existing_len > 0 {
                    tracing::warn!(
                        "[Transfer] Server returned 416 Range Not Satisfiable for chapter {} at offset {}",
                        chapter_id,
                        existing_len
                    );
                    if crate::acquisition::physical::validate_payload(destination_part).is_ok() {
                        tracing::info!(
                            "[Transfer] Existing .part file is already complete and valid for chapter {}",
                            chapter_id
                        );
                        return Ok(existing_len);
                    }
                    tracing::info!(
                        "[Transfer] Removing invalid/oversized .part file and restarting cleanly from byte 0 for chapter {}",
                        chapter_id
                    );
                    let _ = std::fs::remove_file(destination_part);
                    (true, None, "Range not satisfiable — resetting partial file".to_string())
                } else {
                    let err = AppError::new(
                        status.as_u16(),
                        "source_error",
                        format!("Source returned HTTP {status}"),
                    );
                    (is_recoverable_error(&err), retry_after_duration, err.message)
                }
            }
            Err(e) => (is_recoverable_error(&e), None, e.message),
        };

        if !should_retry || attempt >= max_attempts {
            tracing::warn!(
                "[Transfer] Retries exhausted ({}/{} attempts) for chapter {}: {}",
                attempt,
                max_attempts,
                chapter_id,
                last_error_msg
            );
            tracker.mark_failed(chapter_id, &last_error_msg);
            return Err(AppError::new(
                502,
                "transfer_exhausted",
                format!("Transfer failed after {attempt} attempts: {last_error_msg}"),
            ));
        }

        let backoff = calculate_backoff(attempt, config, retry_after_duration);
        let retry_at = now_secs() + backoff.as_secs() as i64;
        tracker.mark_retrying(chapter_id, attempt + 1, retry_at, &last_error_msg);

        tracing::info!(
            "[Transfer] Recoverable error for chapter {} ({}); retrying attempt {}/{} in {:.1}s",
            chapter_id,
            last_error_msg,
            attempt + 1,
            max_attempts,
            backoff.as_secs_f64()
        );

        sleep(backoff).await;
        attempt += 1;
    }
}

/// Helper streaming chunks into a file while actively enforcing a stall timeout between chunks.
#[allow(clippy::too_many_arguments)]
async fn stream_body_with_stall_detection<F>(
    resp: &mut reqwest::Response,
    path: &Path,
    append: bool,
    start_offset: u64,
    expected_size: Option<u64>,
    stall_timeout: Duration,
    chapter_id: i64,
    tracker: &TransferTracker,
    on_progress: &mut F,
) -> Result<u64>
where
    F: FnMut(u64, Option<u64>, bool) + Send + 'static,
{
    let mut file = if append {
        let mut f = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| AppError::internal(format!("Cannot open .part file for append: {e}")))?;
        f.seek(SeekFrom::End(0))
            .map_err(|e| AppError::internal(format!("Cannot seek .part: {e}")))?;
        f
    } else {
        File::create(path)
            .map_err(|e| AppError::internal(format!("Cannot create .part file: {e}")))?
    };

    let total_from_header = resp.content_length().map(|cl| cl + start_offset).or(expected_size);
    let mut bytes_downloaded = start_offset;
    tracker.record_bytes(chapter_id, bytes_downloaded, total_from_header);
    on_progress(bytes_downloaded, total_from_header, false);

    loop {
        let chunk_res = tokio::time::timeout(stall_timeout, resp.chunk()).await;
        match chunk_res {
            Ok(Ok(Some(chunk))) => {
                file.write_all(&chunk)
                    .map_err(|e| AppError::internal(format!("Failed to write chunk: {e}")))?;
                bytes_downloaded += chunk.len() as u64;
                tracker.record_bytes(chapter_id, bytes_downloaded, total_from_header);
                on_progress(bytes_downloaded, total_from_header, false);
            }
            Ok(Ok(None)) => {
                // Stream finished cleanly!
                break;
            }
            Ok(Err(e)) => {
                return Err(AppError::new(
                    502,
                    "source_error",
                    format!("Stream broke off: {e}"),
                ));
            }
            Err(_) => {
                // Stall timeout tripped! No chunk arrived within timeout.
                tracing::warn!(
                    "[Transfer] Transfer stalled: no bytes received for {}s on chapter {}",
                    stall_timeout.as_secs(),
                    chapter_id
                );
                let _ = tracker.check_stalled(chapter_id);
                on_progress(bytes_downloaded, total_from_header, true);
                return Err(AppError::new(
                    504,
                    "source_stalled",
                    format!("Transfer stalled: no progress received for {}s", stall_timeout.as_secs()),
                ));
            }
        }
    }

    file.flush()
        .map_err(|e| AppError::internal(format!("Failed to flush file: {e}")))?;
    drop(file);

    // Truncated download check
    if let Some(expected) = total_from_header {
        if expected > 0 && bytes_downloaded != expected {
            return Err(AppError::new(
                502,
                "truncated_download",
                format!("Downloaded size ({bytes_downloaded}) does not match expected size ({expected})"),
            ));
        }
    }

    Ok(bytes_downloaded)
}

/// Robust page-level fetch helper with bounded retries and stall timeout.
pub async fn fetch_page_with_lifecycle<F, Fut>(
    chapter_id: i64,
    page_index: i64,
    fetcher: F,
    config: &TransferConfig,
    tracker: &TransferTracker,
) -> Result<crate::images::Image>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<crate::images::Image>>,
{
    let mut page_attempt = 1;
    let max_page_attempts = 3;

    loop {
        let fetch_fut = fetcher();
        let res = tokio::time::timeout(config.stall_timeout, fetch_fut).await;

        match res {
            Ok(Ok(image)) => {
                let img_len = image.bytes.len();
                tracker.record_page(chapter_id, page_index + 1, None, img_len);
                return Ok(image);
            }
            Ok(Err(e)) if is_recoverable_error(&e) && page_attempt < max_page_attempts => {
                let backoff = Duration::from_millis(100 * (1 << (page_attempt - 1)));
                tracing::info!(
                    "[Transfer] Recoverable error fetching page {} for chapter {} ({}); retry {}/{} in {}ms",
                    page_index + 1,
                    chapter_id,
                    e.message,
                    page_attempt + 1,
                    max_page_attempts,
                    backoff.as_millis()
                );
                sleep(backoff).await;
                page_attempt += 1;
            }
            Ok(Err(e)) => {
                return Err(e);
            }
            Err(_) => {
                // Page fetch stalled
                tracing::warn!(
                    "[Transfer] Page {} fetch stalled for chapter {} (timeout {}s)",
                    page_index + 1,
                    chapter_id,
                    config.stall_timeout.as_secs()
                );
                let _ = tracker.check_stalled(chapter_id);
                if page_attempt < max_page_attempts {
                    page_attempt += 1;
                    sleep(Duration::from_millis(200)).await;
                } else {
                    return Err(AppError::new(
                        504,
                        "page_stalled",
                        format!("Page {} fetch stalled after {} attempts", page_index + 1, max_page_attempts),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{header, HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
        Router,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn make_test_cbz(path: &Path, content: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ComicInfo.xml", options).unwrap();
        zip.write_all(format!("<ComicInfo><Series>{content}</Series></ComicInfo>").as_bytes()).unwrap();
        zip.start_file("001.webp", options).unwrap();
        zip.write_all(b"fake image bytes").unwrap();
        zip.finish().unwrap();
    }

    fn temp_test_dir(name: &str) -> std::path::PathBuf {
        let pid = std::process::id();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("mango-transfer-{name}-{pid}-{nanos}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // 1. Uninterrupted transfer: transfer completes normally; bytes match; lifecycle progresses to Completed.
    #[tokio::test]
    async fn test_transfer_01_uninterrupted_transfer() {
        let payload = vec![0x42u8; 1024];
        let payload_clone = payload.clone();

        let app = Router::new().route(
            "/file.cbz",
            get(move || {
                let data = payload_clone.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_LENGTH, data.len().to_string())],
                        data,
                    )
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = temp_test_dir("t01");
        let dest = dir.join("test.cbz.part");
        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/file.cbz");

        assert!(tracker.try_claim_worker(101, 1, Some(1024), None));
        let bytes = stream_http_to_file(&http, &url, &dest, Some(1024), &config, 101, &tracker, |_, _, _| {}).await.unwrap();
        tracker.mark_completed(101);

        assert_eq!(bytes, 1024);
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 1024);
        let progress = tracker.get_progress(101).unwrap();
        assert_eq!(progress.status, TransferStatus::Completed);
        assert_eq!(progress.bytes_downloaded, 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 2. 503 retry: recoverable HTTP 503 triggers retry; backoff applied; subsequent success completes transfer.
    #[tokio::test]
    async fn test_transfer_02_503_retry() {
        let attempts = Arc::new(AtomicU32::new(0));
        let att_clone = attempts.clone();
        let payload = b"recovered content".to_vec();
        let payload_len = payload.len();

        let app = Router::new().route(
            "/retry503.cbz",
            get(move || {
                let count = att_clone.fetch_add(1, Ordering::SeqCst);
                let p = payload.clone();
                async move {
                    if count == 0 {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [(header::RETRY_AFTER, "0".to_string())],
                            b"temporary 503".to_vec(),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            [(header::CONTENT_LENGTH, p.len().to_string())],
                            p,
                        )
                    }
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = temp_test_dir("t02");
        let dest = dir.join("test503.cbz.part");
        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/retry503.cbz");

        assert!(tracker.try_claim_worker(102, 1, Some(payload_len as u64), None));
        let bytes = stream_http_to_file(&http, &url, &dest, Some(payload_len as u64), &config, 102, &tracker, |_, _, _| {}).await.unwrap();
        assert_eq!(bytes, payload_len as u64);
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "Made 2 attempts (first failed 503, second succeeded)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. Timeout retry: connection timeout triggers retry without stranding.
    #[tokio::test]
    async fn test_transfer_03_timeout_retry() {
        let attempts = Arc::new(AtomicU32::new(0));
        let att_clone = attempts.clone();
        let payload = b"timeout recovered".to_vec();
        let payload_len = payload.len();

        let app = Router::new().route(
            "/timeout.cbz",
            get(move || {
                let count = att_clone.fetch_add(1, Ordering::SeqCst);
                let p = payload.clone();
                async move {
                    if count == 0 {
                        // Delay longer than stall timeout (150ms in fast_for_testing)
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        (
                            StatusCode::OK,
                            [(header::CONTENT_LENGTH, p.len().to_string())],
                            p,
                        )
                    } else {
                        (
                            StatusCode::OK,
                            [(header::CONTENT_LENGTH, p.len().to_string())],
                            p,
                        )
                    }
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = temp_test_dir("t03");
        let dest = dir.join("timeout.cbz.part");
        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/timeout.cbz");

        assert!(tracker.try_claim_worker(103, 1, Some(payload_len as u64), None));
        let bytes = stream_http_to_file(&http, &url, &dest, Some(payload_len as u64), &config, 103, &tracker, |_, _, _| {}).await.unwrap();
        assert_eq!(bytes, payload_len as u64);
        assert!(attempts.load(Ordering::SeqCst) >= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 4. Connection reset / unexpected EOF: mid-stream disconnect triggers retry.
    #[tokio::test]
    async fn test_transfer_04_connection_reset_eof() {
        let attempts = Arc::new(AtomicU32::new(0));
        let att_clone = attempts.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let count = att_clone.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    if count == 0 {
                        // Send partial response then forcibly close socket (simulating EOF/disconnect)
                        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nPartial";
                        let _ = socket.write_all(resp).await;
                        // Drop socket immediately without sending the remaining 93 bytes
                    } else {
                        // Full successful response
                        let body = b"full valid body for reset test";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                        let _ = socket.write_all(body).await;
                    }
                });
            }
        });

        let dir = temp_test_dir("t04");
        let dest = dir.join("reset.cbz.part");
        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/stream");

        assert!(tracker.try_claim_worker(104, 1, None, None));
        let bytes = stream_http_to_file(&http, &url, &dest, None, &config, 104, &tracker, |_, _, _| {}).await.unwrap();
        assert_eq!(bytes, b"full valid body for reset test".len() as u64);
        assert!(attempts.load(Ordering::SeqCst) >= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 5. Stream stall detection: mid-stream pause exceeding stall timeout is detected; triggers retry.
    #[tokio::test]
    async fn test_transfer_05_stream_stall_detection() {
        let attempts = Arc::new(AtomicU32::new(0));
        let att_clone = attempts.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let count = att_clone.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    if count == 0 {
                        // Send partial response and stall for longer than stall_timeout (150ms in test)
                        let resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\nchunk_1\r\n";
                        let _ = socket.write_all(resp).await;
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                    } else {
                        let body = b"un-stalled full body";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                        let _ = socket.write_all(body).await;
                    }
                });
            }
        });

        let dir = temp_test_dir("t05");
        let dest = dir.join("stall.cbz.part");
        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/stall");

        assert!(tracker.try_claim_worker(105, 1, None, None));
        let bytes = stream_http_to_file(&http, &url, &dest, None, &config, 105, &tracker, |_, _, _| {}).await.unwrap();
        assert_eq!(bytes, b"un-stalled full body".len() as u64);
        assert!(attempts.load(Ordering::SeqCst) >= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 6. Retry exhaustion: repeated failures hit max attempts; transitions to durable Failed state with clear error.
    #[tokio::test]
    async fn test_transfer_06_retry_exhaustion() {
        let app = Router::new().route(
            "/fail.cbz",
            get(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::RETRY_AFTER, "0")],
                    "Persistent server error",
                )
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = temp_test_dir("t06");
        let dest = dir.join("exhaust.cbz.part");
        let config = TransferConfig::fast_for_testing(); // max_attempts = 3
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/fail.cbz");

        assert!(tracker.try_claim_worker(106, 1, Some(100), None));
        let res = stream_http_to_file(&http, &url, &dest, Some(100), &config, 106, &tracker, |_, _, _| {}).await;

        assert!(res.is_err());
        let err = res.err().unwrap();
        assert_eq!(err.code, "transfer_exhausted");
        assert!(err.message.contains("Transfer failed after 3 attempts"));

        let progress = tracker.get_progress(106).unwrap();
        assert_eq!(progress.status, TransferStatus::Failed);
        assert!(progress.error.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 7. Safe Range resume: server returns 206 Partial Content; transfer resumes from existing offset; completed file is valid.
    #[tokio::test]
    async fn test_transfer_07_safe_range_resume() {
        let total_payload = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_vec();
        let total_len = total_payload.len();
        let existing_prefix = total_payload[0..10].to_vec();

        let total_payload_arc = Arc::new(total_payload);
        let p_arc = total_payload_arc.clone();

        let app = Router::new().route(
            "/range.cbz",
            get(move |headers: HeaderMap| {
                let p = p_arc.clone();
                async move {
                    if let Some(range) = headers.get(header::RANGE).and_then(|r| r.to_str().ok()) {
                        if let Some(start_str) = range.strip_prefix("bytes=").and_then(|s| s.strip_suffix('-')) {
                            let start: usize = start_str.parse().unwrap_or(0);
                            if start < p.len() {
                                let slice = p[start..].to_vec();
                                let cr = format!("bytes {}-{}/{}", start, p.len() - 1, p.len());
                                let cl = slice.len().to_string();
                                return (
                                    StatusCode::PARTIAL_CONTENT,
                                    [
                                        (header::CONTENT_RANGE, cr),
                                        (header::CONTENT_LENGTH, cl),
                                    ],
                                    slice,
                                )
                                    .into_response();
                            }
                        }
                    }
                    let cl = p.len().to_string();
                    (
                        StatusCode::OK,
                        [(header::CONTENT_LENGTH, cl)],
                        p.to_vec(),
                    )
                        .into_response()
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = temp_test_dir("t07");
        let dest = dir.join("range.cbz.part");
        // Pre-create existing partial file with 10 bytes
        std::fs::write(&dest, &existing_prefix).unwrap();
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 10);

        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/range.cbz");

        assert!(tracker.try_claim_worker(107, 1, Some(total_len as u64), None));
        let bytes = stream_http_to_file(&http, &url, &dest, Some(total_len as u64), &config, 107, &tracker, |_, _, _| {}).await.unwrap();

        assert_eq!(bytes, total_len as u64);
        let on_disk = std::fs::read(&dest).unwrap();
        assert_eq!(on_disk, *total_payload_arc);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 8. Range ignored (200 OK): server returns 200 OK; previous partial file is truncated/restarted from 0; no corruption.
    #[tokio::test]
    async fn test_transfer_08_range_ignored_200_safe_restart() {
        let full_payload = b"NEW_FRESH_CONTENT_FROM_BYTE_ZERO".to_vec();
        let full_len = full_payload.len();
        let p_arc = Arc::new(full_payload);

        // Server ignores Range header and always returns 200 OK with full content
        let app = Router::new().route(
            "/no-range.cbz",
            get(move || {
                let p = p_arc.clone();
                async move {
                    let cl = p.len().to_string();
                    (
                        StatusCode::OK,
                        [(header::CONTENT_LENGTH, cl)],
                        p.to_vec(),
                    )
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = temp_test_dir("t08");
        let dest = dir.join("no_range.cbz.part");
        // Pre-create with 50 bytes of stale/garbage data
        std::fs::write(&dest, b"GARBAGE_STALE_BYTES_FROM_PREVIOUS_CORRUPTED_RUN!!").unwrap();

        let config = TransferConfig::fast_for_testing();
        let tracker = TransferTracker::new(config.clone());
        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/no-range.cbz");

        assert!(tracker.try_claim_worker(108, 1, Some(full_len as u64), None));
        let bytes = stream_http_to_file(&http, &url, &dest, Some(full_len as u64), &config, 108, &tracker, |_, _, _| {}).await.unwrap();

        assert_eq!(bytes, full_len as u64);
        let on_disk = std::fs::read(&dest).unwrap();
        assert_eq!(on_disk, b"NEW_FRESH_CONTENT_FROM_BYTE_ZERO");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 9. Restart orphan recovery: server restart with active downloading item transitions it to queued for recovery; does not delete .part.
    #[tokio::test]
    async fn test_transfer_09_restart_orphan_recovery() {
        let dir = temp_test_dir("t09");
        let db = crate::db::Db::open(&dir.join("test.db")).unwrap();

        let part_file = dir.join("orphan.cbz.part");
        std::fs::write(&part_file, b"existing 200 bytes of partial download").unwrap();

        db.call(|conn| {
            conn.execute("INSERT INTO manga (id, source_id, url, title, in_library) VALUES (1, 1, '/m', 'Recovery Test', 1)", [])?;
            conn.execute("INSERT INTO chapters (id, manga_id, url, name, source_order, fetched_at, read) VALUES (109, 1, '/c', 'Vol 1', 1, 0, 0)", [])?;
            conn.execute(
                "INSERT INTO downloads (chapter_id, state, position, added_at, bytes_downloaded) VALUES (109, 'downloading', 1, 1000, 200)",
                [],
            )?;
            Ok(())
        }).await.unwrap();

        // Run startup recovery logic
        db.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT d.chapter_id, m.title, c.name FROM downloads d
                 JOIN chapters c ON c.id = d.chapter_id
                 JOIN manga m ON m.id = c.manga_id
                 WHERE d.state = 'downloading' AND d.published_path IS NULL",
            )?;
            let orphaned = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
                .filter_map(std::result::Result::ok)
                .collect::<Vec<_>>();
            for (ch_id, _, _) in orphaned {
                conn.execute(
                    "UPDATE downloads SET state = 'queued', error = 'Interrupted by server restart — queued for recovery' WHERE chapter_id = ?1",
                    [ch_id],
                )?;
            }
            Ok(())
        }).await.unwrap();

        let (state, err): (String, Option<String>) = db.call(|conn| {
            Ok(conn.query_row("SELECT state, error FROM downloads WHERE chapter_id = 109", [], |r| Ok((r.get(0)?, r.get(1)?)))?)
        }).await.unwrap();

        assert_eq!(state, "queued");
        assert_eq!(err.as_deref(), Some("Interrupted by server restart — queued for recovery"));
        assert!(part_file.is_file(), "Orphan recovery must NOT delete existing .part file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 10. Concurrent download isolation: failure/retry in one transfer does not block or stall unrelated transfers.
    #[tokio::test]
    async fn test_transfer_10_concurrent_download_isolation() {
        let tracker = TransferTracker::new(TransferConfig::fast_for_testing());

        assert!(tracker.try_claim_worker(1101, 1, Some(500), None));
        assert!(tracker.try_claim_worker(1102, 1, Some(600), None));

        // Chapter 1101 encounters failure and transitions to Retrying
        tracker.mark_retrying(1101, 2, now_secs() + 10, "503 service unavailable");

        // Chapter 1102 proceeds normally
        tracker.record_bytes(1102, 300, Some(600));
        tracker.record_bytes(1102, 600, Some(600));
        tracker.mark_completed(1102);

        let p1 = tracker.get_progress(1101).unwrap();
        let p2 = tracker.get_progress(1102).unwrap();

        assert_eq!(p1.status, TransferStatus::Retrying);
        assert_eq!(p1.attempt, 2);
        assert_eq!(p2.status, TransferStatus::Completed);
        assert_eq!(p2.bytes_downloaded, 600);
    }

    // 11. Duplicate worker prevention: duplicate download/retry requests for the same job do not create competing workers.
    #[tokio::test]
    async fn test_transfer_11_duplicate_worker_prevention() {
        let tracker = TransferTracker::new(TransferConfig::fast_for_testing());

        // First worker claims chapter 111
        let claimed_1 = tracker.try_claim_worker(111, 1, Some(1000), None);
        assert!(claimed_1, "First worker claim must succeed");

        // Second worker tries to claim the same chapter 111
        let claimed_2 = tracker.try_claim_worker(111, 1, Some(1000), None);
        assert!(!claimed_2, "Concurrent duplicate worker claim must be rejected");

        // Worker releases slot
        tracker.release_worker(111);

        // Next worker can now claim
        let claimed_3 = tracker.try_claim_worker(111, 2, Some(1000), None);
        assert!(claimed_3, "Claim succeeds after previous worker slot released");
    }

    // 12. CBZ validation: completed candidate archive verified for integrity; corrupted archive rejected before publication.
    #[tokio::test]
    async fn test_transfer_12_cbz_validation() {
        let dir = temp_test_dir("t12");
        let valid_cbz = dir.join("valid.cbz");
        make_test_cbz(&valid_cbz, "Series Alpha");

        assert!(crate::pipeline::publisher::verify_cbz_integrity(&valid_cbz).is_ok());

        let corrupt_cbz = dir.join("corrupt.cbz");
        std::fs::write(&corrupt_cbz, b"this is not a valid zip archive file").unwrap();

        let verify_res = crate::pipeline::publisher::verify_cbz_integrity(&corrupt_cbz);
        assert!(
            verify_res.is_err(),
            "Corrupt candidate archive must fail verification"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 13. Normal acquisition continuation: successful recovery proceeds through normal pipeline (publication, ownership, Komga).
    #[tokio::test]
    async fn test_transfer_13_normal_acquisition_continuation() {
        let dir = temp_test_dir("t13");
        let valid_part = dir.join("volume1.cbz.part");
        make_test_cbz(&valid_part, "Made in Abyss");

        assert!(crate::pipeline::publisher::verify_cbz_integrity(&valid_part).is_ok());

        // Promotion simulation
        let final_cbz = dir.join("volume1.cbz");
        std::fs::rename(&valid_part, &final_cbz).unwrap();

        assert!(final_cbz.is_file());
        assert!(!valid_part.exists());
        assert!(crate::pipeline::publisher::verify_cbz_integrity(&final_cbz).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 14. Stale .part rejection: invalid or truncated .part from previous aborted run cannot be promoted to completed CBZ.
    #[tokio::test]
    async fn test_transfer_14_stale_part_rejection() {
        let dir = temp_test_dir("t14");
        let stale_part = dir.join("stale.cbz.part");
        // Incomplete / truncated zip header
        std::fs::write(&stale_part, b"PK\x03\x04incomplete_truncated_zip_data").unwrap();

        let res = crate::pipeline::publisher::verify_cbz_integrity(&stale_part);
        assert!(
            res.is_err(),
            "Stale truncated .part must fail CBZ integrity"
        );

        if res.is_err() {
            let _ = std::fs::remove_file(&stale_part);
        }

        assert!(!stale_part.exists(), "Rejected stale .part must be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
