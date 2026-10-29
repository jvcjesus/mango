pub mod config;
pub mod ntfy;
pub mod outbox;
pub mod service;
pub mod types;
pub mod worker;

#[cfg(test)]
pub mod tests;

#[allow(unused_imports)]
pub use config::{get_ntfy_config, set_ntfy_config};
#[allow(unused_imports)]
pub use ntfy::NtfyClient;
#[allow(unused_imports)]
pub use outbox::{build_payload, enqueue_from_event, list_recent_outbox};
pub use service::NotificationService;
#[allow(unused_imports)]
pub use types::{
    NtfyConfig, NtfyConfigDto, NotificationEventType, NotificationPayload, OutboxEntry,
    OutboxStatus, SendOutcome,
};
#[allow(unused_imports)]
pub use worker::run_worker_forever;
