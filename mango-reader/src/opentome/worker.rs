use super::service::OpenTomeService;
use std::sync::Arc;
use std::time::Duration;

pub async fn run_worker_forever(service: Arc<OpenTomeService>) {
    tracing::info!("Starting OpenTome release intelligence background worker");

    // Initial startup delay
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Check on startup
    if service.is_enabled().await {
        let auto_update = service.auto_update_enabled().await;
        let interval_secs = (service.update_interval_hours().await as i64) * 3600;

        if !service.updater().is_available() {
            tracing::info!("OpenTome catalogue not found on startup; initiating initial fetch");
            let _ = service.check_for_updates(false).await;
        } else if auto_update {
            let status = service.status().await;
            let last_checked = status.as_ref().ok().and_then(|s| s.last_checked_at).unwrap_or(0);
            let now = crate::db::now();

            if now - last_checked >= interval_secs {
                tracing::info!("OpenTome catalogue interval elapsed on startup; checking for updates");
                let _ = service.check_for_updates(false).await;
            }
        }
        let _ = service.evaluate_release_day_events().await;
    }

    loop {
        // Evaluate every 15 minutes so scheduled updates trigger promptly
        tokio::time::sleep(Duration::from_secs(900)).await;

        if !service.is_enabled().await {
            continue;
        }

        let auto_update = service.auto_update_enabled().await;
        if auto_update {
            let interval_secs = (service.update_interval_hours().await as i64) * 3600;
            let status = service.status().await;
            let last_checked = status.as_ref().ok().and_then(|s| s.last_checked_at).unwrap_or(0);
            let now = crate::db::now();

            if now - last_checked >= interval_secs {
                tracing::info!(
                    "OpenTome update interval reached ({}h); checking for catalogue updates",
                    interval_secs / 3600
                );
                let _ = service.check_for_updates(false).await;
            }
        }

        // Check release-day transitions
        if let Ok(count) = service.evaluate_release_day_events().await {
            if count > 0 {
                tracing::info!("OpenTome evaluated {count} volume release-day event(s)");
            }
        }
    }
}
