//! OpenTome Release Intelligence Subsystem
//!
//! Provides market-specific physical volume release intelligence for followed manga.
//! Queries an external, replaceable, read-only SQLite catalogue (`manga-metadata.sqlite`).
//!
//! Invariants:
//! - Optional and fail-open: OpenTome failure never breaks Mango boot, reading, MangaBaka, or acquisition.
//! - Canonical physical ownership remains authoritative (Komga).
//! - Strictly no auto-acquisition: OpenTome is an intelligence and notification provider only.

pub mod catalog;
pub mod service;
pub mod updater;
pub mod worker;

#[cfg(test)]
pub mod tests;
