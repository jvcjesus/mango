//! Scheduled synchronization foundation.
//!
//! Provides background timer loop for automatic reading-progress synchronization.
//! Strictly disabled by default (`sync.enabled = false`) to guarantee zero unattended writes
//! until explicitly enabled by an administrator.

use super::service::SyncService;
use crate::{
    db::{now, Db},
    error::Result,
    settings,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

pub const SYNC_ENABLED: &str = "sync.enabled";
pub const SYNC_INTERVAL_HOURS: &str = "sync.interval_hours";
pub const SYNC_LAST_RUN: &str = "sync.last_run";
pub const DEFAULT_INTERVAL_HOURS: u32 = 24;

pub const RECONCILIATION_ENABLED: &str = "reconciliation.enabled";
pub const RECONCILIATION_INTERVAL_HOURS: &str = "reconciliation.interval_hours";
pub const RECONCILIATION_LAST_RUN: &str = "reconciliation.last_run";
pub const DEFAULT_RECONCILIATION_INTERVAL_HOURS: u32 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationScheduleConfig {
    pub enabled: bool,
    pub interval_hours: u32,
    pub last_run: Option<i64>,
}

impl Default for ReconciliationScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: DEFAULT_RECONCILIATION_INTERVAL_HOURS,
            last_run: None,
        }
    }
}

pub fn get_reconciliation_schedule_config(conn: &rusqlite::Connection) -> ReconciliationScheduleConfig {
    let enabled = settings::get::<bool>(conn, RECONCILIATION_ENABLED).unwrap_or(true);
    let interval_hours = settings::get::<u32>(conn, RECONCILIATION_INTERVAL_HOURS)
        .unwrap_or(DEFAULT_RECONCILIATION_INTERVAL_HOURS);
    let last_run = settings::get::<i64>(conn, RECONCILIATION_LAST_RUN);
    ReconciliationScheduleConfig {
        enabled,
        interval_hours: interval_hours.max(1).min(168),
        last_run,
    }
}

pub fn set_reconciliation_schedule_config(
    conn: &rusqlite::Connection,
    cfg: &ReconciliationScheduleConfig,
) -> Result<()> {
    settings::set(conn, RECONCILIATION_ENABLED, &json!(cfg.enabled))?;
    settings::set(
        conn,
        RECONCILIATION_INTERVAL_HOURS,
        &json!(cfg.interval_hours.max(1).min(168)),
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncScheduleConfig {
    pub enabled: bool,
    pub interval_hours: u32,
    pub last_run: Option<i64>,
}

impl Default for SyncScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Strictly disabled by default
            interval_hours: DEFAULT_INTERVAL_HOURS,
            last_run: None,
        }
    }
}

pub fn get_schedule_config(conn: &rusqlite::Connection) -> SyncScheduleConfig {
    let enabled = settings::get::<bool>(conn, SYNC_ENABLED).unwrap_or(false);
    let interval_hours = settings::get::<u32>(conn, SYNC_INTERVAL_HOURS).unwrap_or(DEFAULT_INTERVAL_HOURS);
    let last_run = settings::get::<i64>(conn, SYNC_LAST_RUN);
    SyncScheduleConfig {
        enabled,
        interval_hours: interval_hours.max(1).min(168),
        last_run,
    }
}

pub fn set_schedule_config(conn: &rusqlite::Connection, cfg: &SyncScheduleConfig) -> Result<()> {
    settings::set(conn, SYNC_ENABLED, &json!(cfg.enabled))?;
    settings::set(conn, SYNC_INTERVAL_HOURS, &json!(cfg.interval_hours.max(1).min(168)))?;
    Ok(())
}

pub async fn run_scheduler_forever(sync: SyncService, db: Db) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;

        let should_run = db
            .call(|conn| {
                let cfg = get_schedule_config(conn);
                if !cfg.enabled {
                    return Ok(false);
                }

                let interval_secs = (cfg.interval_hours as i64) * 3600;
                let current_time = now();

                match cfg.last_run {
                    Some(last) => {
                        if current_time >= last + interval_secs {
                            settings::set(conn, SYNC_LAST_RUN, &json!(current_time))?;
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    }
                    None => {
                        // First activation: wait one interval before executing
                        settings::set(conn, SYNC_LAST_RUN, &json!(current_time))?;
                        Ok(false)
                    }
                }
            })
            .await
            .unwrap_or(false);

        if should_run {
            tracing::info!("SYNC_SCHEDULER: Triggering scheduled reading progress synchronization");
            let _ = sync.run_sync(false, None, None).await;
        }

        let should_reconcile = db
            .call(|conn| {
                let cfg = get_reconciliation_schedule_config(conn);
                if !cfg.enabled {
                    return Ok(false);
                }

                let interval_secs = (cfg.interval_hours as i64) * 3600;
                let current_time = now();

                match cfg.last_run {
                    Some(last) => {
                        if current_time >= last + interval_secs {
                            settings::set(conn, RECONCILIATION_LAST_RUN, &json!(current_time))?;
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    }
                    None => {
                        // First run: trigger immediately and record last run
                        settings::set(conn, RECONCILIATION_LAST_RUN, &json!(current_time))?;
                        Ok(true)
                    }
                }
            })
            .await
            .unwrap_or(false);

        if should_reconcile {
            tracing::info!("SYNC_SCHEDULER: Triggering scheduled series reconciliation");
            let _ = sync.run_reconciliation_all_followed().await;
        }
    }
}
