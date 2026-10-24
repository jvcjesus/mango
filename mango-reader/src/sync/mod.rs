//! Reading progress synchronization subsystem.
//!
//! Synchronizes authoritative Komga reading progress to external trackers
//! (AniList, MangaBaka) safely with dry-run support, never-decrease progress semantics,
//! and complete audit logging.

pub mod anilist;
pub mod komga_reader;
pub mod mangabaka;
pub mod mapping;
pub mod model;

#[cfg(test)]
mod tests;
