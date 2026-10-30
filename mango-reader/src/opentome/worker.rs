use super::service::OpenTomeService;
use std::sync::Arc;
use std::time::Duration;

pub async fn run_worker_forever(service: Arc<OpenTomeService>) {
    tracing::info!("Starting OpenTome release intelligence background worker");

    // Initial startup delay
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Check on startup if catalogue needs initial install
    if service.is_enabled().await {
        if !service.updater().is_available() {
            tracing::info!("OpenTome catalogue not found on startup; initiating initial fetch");
            let _ = service.check_for_updates(false).await;
        }
        let _ = service.evaluate_release_day_events().await;
    }

    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await; // hourly check

        if !service.is_enabled().await {
            continue;
        }

        // Daily catalogue check (every 24 hours)
        let status = service.status().await;
        let last_checked = status.as_ref().ok().and_then(|s| s.last_checked_at).unwrap_or(0);
        let now = crate::db::now();

        if now - last_checked >= 86_400 {
            tracing::info!("OpenTome 24h cadence reached; checking for catalogue updates");
            let _ = service.check_for_updates(false).await;
        }

        // Check release-day transitions
        if let Ok(count) = service.evaluate_release_day_events().await {
            if count > 0 {
                tracing::info!("OpenTome evaluated {count} volume release-day event(s)");
            }
        }
    }
}
