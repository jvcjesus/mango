//! Reading progress synchronization subsystem.
//!
//! Synchronizes authoritative Komga reading progress to external trackers
//! (AniList, MangaBaka) safely with dry-run support, never-decrease progress semantics,
//! and complete audit logging.

pub mod anilist;
pub mod audit;
pub mod catalog;
pub mod eligibility;
pub mod identity;
pub mod komga_reader;
pub mod mangabaka;
pub mod mapping;
pub mod model;
pub mod scheduler;
pub mod service;
pub mod totals;

#[cfg(test)]
mod tests;
